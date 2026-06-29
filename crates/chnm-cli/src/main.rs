//! `chnm` — create and manage Chinampa plant tags stored as Markdown files.

use anyhow::{bail, Context, Result};
use chnm_core::{generate_id, is_valid_id, Tag, TagMeta};
use clap::{Parser, Subcommand};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const DEFAULT_BASE_URL: &str = "https://chnm.pa";

#[derive(Parser)]
#[command(name = "chnm", version, about = "Create and manage Chinampa plant tags")]
struct Cli {
    /// Directory holding the tag Markdown files.
    #[arg(long, default_value = "tags", global = true)]
    tags_dir: PathBuf,

    /// Base URL written onto the NFC tag (the tag ID is appended).
    #[arg(long, default_value = DEFAULT_BASE_URL, global = true)]
    base_url: String,

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

    match cli.cmd {
        Command::New {
            species_name,
            species_inat_id,
            description,
        } => {
            let meta = TagMeta {
                id: fresh_id(dir),
                linked_tags: vec![],
                description,
                species_name,
                species_inat_id,
                observation_inat_id: None,
            };
            let tag = write_new(dir, meta)?;
            println!("{}", tag.url(&cli.base_url));
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
            };
            let tag = write_new(dir, meta)?;
            println!("{}", tag.url(&cli.base_url));
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
            println!("{}/{id}", cli.base_url.trim_end_matches('/'));
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
                    })
                })
                .collect();
            fs::write(out.join("index.json"), serde_json::to_vec_pretty(&index)?)?;
            // One file per tag for detail views.
            for tag in all.values() {
                fs::write(
                    out.join(format!("{}.json", tag.meta.id)),
                    serde_json::to_vec_pretty(tag)?,
                )?;
            }
            println!("exported {} tags to {}", all.len(), out.display());
        }
    }
    Ok(())
}
