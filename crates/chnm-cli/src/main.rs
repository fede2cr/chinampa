//! `chnm` — create and manage Chinampa plant tags stored as Markdown files.

use anyhow::{bail, Context, Result};
use chnm_core::{generate_id, is_valid_id, Tag, TagMeta};
use clap::{Parser, Subcommand};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const DEFAULT_BASE_URL: &str = "https://chnm.pa";
const DEFAULT_CURRENCY: &str = "CRC";

/// Minimal on-disk configuration (see `chnm.toml`).
#[derive(Debug, Default, Deserialize)]
struct Config {
    /// Host used to build the NFC tag URL, e.g. "chinampa.co.cr".
    domain: Option<String>,
    /// Currency code used to display prices in the web app, e.g. "CRC".
    currency: Option<String>,
}

/// Load the config file, treating a missing file as empty defaults.
fn load_config(path: &Path) -> Result<Config> {
    match fs::read_to_string(path) {
        Ok(src) => {
            toml::from_str(&src).with_context(|| format!("parsing {}", path.display()))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Resolve the base URL with precedence: `--base-url` > config `domain` >
/// built-in default. A bare domain is upgraded to an `https://` URL.
fn resolve_base_url(cli_base: Option<String>, cfg: &Config) -> String {
    if let Some(base) = cli_base {
        return base;
    }
    if let Some(domain) = &cfg.domain {
        let host = domain
            .trim()
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .trim_end_matches('/');
        return format!("https://{host}");
    }
    DEFAULT_BASE_URL.to_string()
}

/// Resolve the display currency: config `currency` > built-in default (`CRC`).
fn resolve_currency(cfg: &Config) -> String {
    cfg.currency
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .unwrap_or(DEFAULT_CURRENCY)
        .to_string()
}

#[derive(Parser)]
#[command(name = "chnm", version, about = "Create and manage Chinampa plant tags")]
struct Cli {
    /// Directory holding the tag Markdown files.
    #[arg(long, default_value = "tags", global = true)]
    tags_dir: PathBuf,

    /// Configuration file (TOML); missing file falls back to defaults.
    #[arg(long, default_value = "chnm.toml", global = true)]
    config: PathBuf,

    /// Base URL written onto the NFC tag (overrides the config `domain`).
    #[arg(long, global = true)]
    base_url: Option<String>,

    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new tag and print the URL to write onto the NFC chip.
    New {
        /// Human-readable species name.
        #[arg(long)]
        species_name: Option<String>,
        /// iNaturalist taxon ID for the species.
        #[arg(long)]
        species_inat_id: Option<u64>,
        /// Free-form description.
        #[arg(long)]
        description: Option<String>,
        /// Collection/grouping this tag belongs to.
        #[arg(long)]
        collection: Option<String>,
        /// Mark the plant as offered for sale.
        #[arg(long)]
        for_sale: bool,
        /// Sale price in the configured currency.
        #[arg(long)]
        price: Option<f64>,
    },
    /// Create a new tag linked back to an existing parent tag.
    Clone {
        /// The parent tag ID to link from.
        id: String,
    },
    /// List all tags (id, species, description).
    List,
    /// Print one tag's parsed contents.
    Show {
        /// The tag ID to show.
        id: String,
    },
    /// Print the NFC URL for a tag ID.
    Url {
        /// The tag ID.
        id: String,
    },
    /// Validate every tag file (ids unique/valid, linked tags resolve).
    Validate,
    /// Export every tag to JSON for the web app.
    Export {
        /// Output directory for `index.json` and `<id>.json`.
        #[arg(long, default_value = "dist/data")]
        out: PathBuf,
    },
}

fn tag_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.md"))
}

/// Generate an ID that does not already exist on disk (collision-checked).
fn fresh_id(dir: &Path) -> String {
    loop {
        let id = generate_id();
        if !tag_path(dir, &id).exists() {
            return id;
        }
    }
}

fn load_all(dir: &Path) -> Result<BTreeMap<String, Tag>> {
    let mut map = BTreeMap::new();
    if !dir.exists() {
        return Ok(map);
    }
    for entry in WalkDir::new(dir).into_iter().filter_map(Result::ok) {
        if entry.path().extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let src = fs::read_to_string(entry.path())?;
        let tag = Tag::from_markdown(&src)
            .with_context(|| format!("parsing {}", entry.path().display()))?;
        if let Some(prev) = map.insert(tag.meta.id.clone(), tag) {
            bail!("duplicate tag id: {}", prev.meta.id);
        }
    }
    Ok(map)
}

fn write_new(dir: &Path, meta: TagMeta) -> Result<Tag> {
    fs::create_dir_all(dir)?;
    let tag = Tag::new(meta);
    let path = tag_path(dir, &tag.meta.id);
    if path.exists() {
        bail!("collision: {} already exists", path.display());
    }
    fs::write(&path, tag.to_markdown()?)?;
    Ok(tag)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let dir = &cli.tags_dir;
    let config = load_config(&cli.config)?;
    let base_url = resolve_base_url(cli.base_url, &config);
    let currency = resolve_currency(&config);

    match cli.cmd {
        Command::New {
            species_name,
            species_inat_id,
            description,
            collection,
            for_sale,
            price,
        } => {
            let meta = TagMeta {
                id: fresh_id(dir),
                linked_tags: vec![],
                description,
                species_name,
                species_inat_id,
                observation_inat_id: None,
                collection,
                for_sale,
                price,
            };
            let tag = write_new(dir, meta)?;
            println!("{}", tag.url(&base_url));
        }
        Command::Clone { id } => {
            if !is_valid_id(&id) {
                bail!("invalid id: {id}");
            }
            if !tag_path(dir, &id).exists() {
                bail!("parent tag {id} not found");
            }
            let meta = TagMeta {
                id: fresh_id(dir),
                linked_tags: vec![id],
                description: None,
                species_name: None,
                species_inat_id: None,
                observation_inat_id: None,
                collection: None,
                for_sale: false,
                price: None,
            };
            let tag = write_new(dir, meta)?;
            println!("{}", tag.url(&base_url));
        }
        Command::List => {
            for (id, tag) in load_all(dir)? {
                println!(
                    "{id}\t{}\t{}",
                    tag.meta.species_name.unwrap_or_default(),
                    tag.meta.description.unwrap_or_default()
                );
            }
        }
        Command::Show { id } => {
            let src = fs::read_to_string(tag_path(dir, &id))
                .with_context(|| format!("tag {id} not found"))?;
            let tag = Tag::from_markdown(&src)?;
            println!("{tag:#?}");
        }
        Command::Url { id } => {
            if !is_valid_id(&id) {
                bail!("invalid id: {id}");
            }
            println!("{}/{id}", base_url.trim_end_matches('/'));
        }
        Command::Validate => {
            let all = load_all(dir)?;
            let mut errors = 0;
            for tag in all.values() {
                for link in &tag.meta.linked_tags {
                    if !all.contains_key(link) {
                        eprintln!("{}: linked tag {link} does not exist", tag.meta.id);
                        errors += 1;
                    }
                }
            }
            if errors > 0 {
                bail!("{errors} validation error(s)");
            }
            println!("ok: {} tags valid", all.len());
        }
        Command::Export { out } => {
            let all = load_all(dir)?;
            fs::create_dir_all(&out)?;
            // Lightweight index for list views.
            let index: Vec<_> = all
                .values()
                .map(|t| {
                    serde_json::json!({
                        "id": t.meta.id,
                        "species_name": t.meta.species_name,
                        "description": t.meta.description,
                        "collection": t.meta.collection,
                        "for_sale": t.meta.for_sale,
                        "price": t.meta.price,
                        "currency": currency,
                    })
                })
                .collect();
            fs::write(out.join("index.json"), serde_json::to_vec_pretty(&index)?)?;
            // One file per tag for detail views (currency is site-wide config).
            for tag in all.values() {
                let mut value = serde_json::to_value(tag)?;
                if let Some(obj) = value.as_object_mut() {
                    obj.insert("currency".into(), serde_json::json!(currency));
                }
                fs::write(
                    out.join(format!("{}.json", tag.meta.id)),
                    serde_json::to_vec_pretty(&value)?,
                )?;
            }
            println!("exported {} tags to {}", all.len(), out.display());
        }
    }
    Ok(())
}
