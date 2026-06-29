# Chinampa — Development Guide

This document describes how to build the Chinampa system end to end:

1. A **Rust CLI tool** (`chnm`) to create and manage plant tags.
2. A **Leptos web application** that renders each tag's history, pulling
   images from iNaturalist (observation photos when available, otherwise the
   linked species' default photo).
3. **CI recipes** that process every tag file committed to the repository,
   build the web app, and publish it to **GitHub Pages** — with no dedicated
   server.

The whole system is *data-as-files*: each NFC tag is a Markdown file in the
repository. The CLI writes those files, CI turns them into JSON the web app can
consume, and the Leptos app renders them as a static site.

---

## 1. Architecture overview

```
┌──────────────┐      writes        ┌──────────────────┐
│  chnm (CLI)  │ ─────────────────▶ │  tags/<id>.md    │  (source of truth, git-tracked)
└──────────────┘                    └──────────────────┘
                                              │
                                       CI build step
                                              │  (chnm export)
                                              ▼
                                     ┌──────────────────┐
                                     │ dist/data/*.json │  (generated, not committed)
                                     └──────────────────┘
                                              │
                                       trunk build
                                              ▼
                                     ┌──────────────────┐
                                     │  GitHub Pages    │  (static Leptos CSR app)
                                     └──────────────────┘
                                              ▲
                                     fetch() at runtime
                                              │
                                     ┌──────────────────┐
                                     │ iNaturalist API  │  (observation / taxa photos)
                                     └──────────────────┘
```

Key decisions:

- **Tags are Markdown with YAML frontmatter.** The frontmatter holds the
  machine-readable fields (ID, linked tags, species, observation); the Markdown
  body holds the free-form, human-edited log. This keeps the README's
  "editable in any third-party editor" promise while making fields trivial to
  parse.
- **The web app is Leptos in CSR (client-side rendering) mode**, built with
  [Trunk](https://trunkrs.dev/) to a fully static bundle. GitHub Pages serves
  it; there is no server runtime.
- **Routing uses path routes served from the domain root** (`https://chinampa.co.cr/<id>`);
  deep links work on GitHub Pages because the deploy workflow copies
  `index.html` to `404.html`, which re-serves the SPA for any unmatched path.
- **iNaturalist images are resolved in the browser** at runtime via the public
  iNat API v1, so the repository never stores binary images.

---

## 2. Repository layout

```
chinampa/
├── Cargo.toml                  # Cargo workspace
├── rust-toolchain.toml         # pin toolchain + wasm target
├── crates/
│   ├── chnm-core/              # shared library: model, ID gen, parse/serialize
│   │   ├── Cargo.toml
│   │   └── src/lib.rs
│   ├── chnm-cli/               # the `chnm` binary
│   │   ├── Cargo.toml
│   │   └── src/main.rs
│   └── chnm-web/               # Leptos CSR app
│       ├── Cargo.toml
│       ├── Trunk.toml
│       ├── index.html
│       ├── public/             # static assets (favicon, css)
│       └── src/
│           ├── main.rs
│           ├── app.rs
│           ├── inat.rs         # iNaturalist API client
│           └── components/
├── tags/                       # ← NFC tag Markdown files (the data)
│   └── 7Gk2pQ8x.md             # example
├── .github/workflows/
│   ├── ci.yml                  # validate tags + build on every PR/push
│   └── deploy.yml              # build data + web, publish to Pages
├── Development.md
└── README.md
```

---

## 3. Prerequisites

```bash
# Rust + the wasm target used by the web app
rustup toolchain install stable
rustup target add wasm32-unknown-unknown

# Trunk builds/serves the Leptos CSR app
cargo install trunk --locked

# (optional) wasm-bindgen / wasm-opt are pulled in by trunk automatically
```

Pin the toolchain so CI and local builds match — `rust-toolchain.toml`:

```toml
[toolchain]
channel = "stable"
targets = ["wasm32-unknown-unknown"]
```

---

## 4. The Cargo workspace

`Cargo.toml` (repo root):

```toml
[workspace]
resolver = "2"
members = ["crates/chnm-core", "crates/chnm-cli", "crates/chnm-web"]

[workspace.package]
edition = "2021"
license = "MIT OR Apache-2.0"
repository = "https://github.com/<owner>/chinampa"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
serde_yaml = "0.9"
```

---

## 5. `chnm-core` — shared model

This crate is shared by the CLI and (optionally) the web app, so the tag model
and parsing logic live in exactly one place.

`crates/chnm-core/Cargo.toml`:

```toml
[package]
name = "chnm-core"
version = "0.1.0"
edition.workspace = true

[dependencies]
serde = { workspace = true }
serde_yaml = { workspace = true }
serde_json = { workspace = true }
nanoid = "0.4"
time = { version = "0.3", features = ["formatting", "parsing", "macros"] }
thiserror = "1"
```

### 5.1 The tag model

`crates/chnm-core/src/lib.rs`:

```rust
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Allowed characters for a tag ID: [a-zA-Z0-9].
const ID_ALPHABET: [char; 62] = [
    'a','b','c','d','e','f','g','h','i','j','k','l','m','n','o','p','q','r','s','t',
    'u','v','w','x','y','z','A','B','C','D','E','F','G','H','I','J','K','L','M','N',
    'O','P','Q','R','S','T','U','V','W','X','Y','Z','0','1','2','3','4','5','6','7','8','9',
];

pub const ID_LEN: usize = 8;

/// The machine-readable frontmatter of a tag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagMeta {
    pub id: String,
    #[serde(default)]
    pub linked_tags: Vec<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// iNaturalist taxon ID (numeric) for the species.
    #[serde(default)]
    pub species_inat_id: Option<u64>,
    /// Human-readable species name (scientific or common).
    #[serde(default)]
    pub species_name: Option<String>,
    /// iNaturalist observation ID (numeric), if one exists for this plant.
    #[serde(default)]
    pub observation_inat_id: Option<u64>,
}

/// A full tag: frontmatter + the free-form Markdown log body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tag {
    #[serde(flatten)]
    pub meta: TagMeta,
    /// The Markdown body (the "Log" section the operator edits by hand).
    pub body: String,
}

#[derive(Debug, Error)]
pub enum TagError {
    #[error("missing or malformed YAML frontmatter")]
    Frontmatter,
    #[error("yaml error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("invalid id: {0}")]
    InvalidId(String),
}

/// Generate a non-sequential 8-char [a-zA-Z0-9] ID (YouTube-style).
pub fn generate_id() -> String {
    nanoid::nanoid!(ID_LEN, &ID_ALPHABET)
}

pub fn is_valid_id(id: &str) -> bool {
    id.len() == ID_LEN && id.chars().all(|c| ID_ALPHABET.contains(&c))
}

impl Tag {
    /// Build the URL written to the NFC tag, e.g. https://chnm.pa/7Gk2pQ8x
    pub fn url(&self, base: &str) -> String {
        format!("{}/{}", base.trim_end_matches('/'), self.meta.id)
    }

    /// Parse a tag from its Markdown-with-frontmatter representation.
    pub fn from_markdown(src: &str) -> Result<Tag, TagError> {
        let rest = src.strip_prefix("---").ok_or(TagError::Frontmatter)?;
        let end = rest.find("\n---").ok_or(TagError::Frontmatter)?;
        let yaml = &rest[..end];
        let body = rest[end + 4..].trim_start_matches('\n').to_string();
        let meta: TagMeta = serde_yaml::from_str(yaml)?;
        if !is_valid_id(&meta.id) {
            return Err(TagError::InvalidId(meta.id));
        }
        Ok(Tag { meta, body })
    }

    /// Serialize back to Markdown-with-frontmatter.
    pub fn to_markdown(&self) -> Result<String, TagError> {
        let yaml = serde_yaml::to_string(&self.meta)?;
        Ok(format!("---\n{yaml}---\n\n{}", self.body))
    }
}
```

### 5.2 The skeleton body

A helper that produces the README's "skeleton" body for a newly created tag:

```rust
pub fn skeleton_body() -> String {
    "\
## Description

<!-- young/mature plant, in-vitro individual, multi-plant container, etc. -->

## Log

- YYYY-MM-DD — created
"
    .to_string()
}
```

---

## 6. `chnm-cli` — the CLI tool

`crates/chnm-cli/Cargo.toml`:

```toml
[package]
name = "chnm-cli"
version = "0.1.0"
edition.workspace = true

[[bin]]
name = "chnm"
path = "src/main.rs"

[dependencies]
chnm-core = { path = "../chnm-core" }
serde = { workspace = true }
serde_json = { workspace = true }
clap = { version = "4", features = ["derive"] }
anyhow = "1"
walkdir = "2"
```

### 6.1 Commands

| Command | Purpose |
|---------|---------|
| `chnm new` | Generate a unique ID and write `tags/<id>.md` with the skeleton. Prints the tag URL to write onto the NFC tag. |
| `chnm clone <id>` | Create a new tag that lists `<id>` in its `linked_tags` (used when splitting one flask into many pots). |
| `chnm list` | List all tags (id, species, description). |
| `chnm show <id>` | Print one tag's parsed contents. |
| `chnm url <id>` | Print the NFC URL for a tag. |
| `chnm validate` | Parse every file under `tags/`, check IDs are valid/unique and `linked_tags` resolve. Exit non-zero on error (used by CI). |
| `chnm export --out <dir>` | Emit `<dir>/index.json` and `<dir>/<id>.json` for the web app (used by CI). |

`crates/chnm-cli/src/main.rs`:

```rust
use anyhow::{bail, Context, Result};
use chnm_core::{generate_id, is_valid_id, skeleton_body, Tag, TagMeta};
use clap::{Parser, Subcommand};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const DEFAULT_BASE_URL: &str = "https://chnm.pa";

#[derive(Parser)]
#[command(name = "chnm", about = "Create and manage Chinampa plant tags")]
struct Cli {
    /// Directory holding the tag Markdown files.
    #[arg(long, default_value = "tags", global = true)]
    tags_dir: PathBuf,
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    New {
        #[arg(long)]
        species_name: Option<String>,
        #[arg(long)]
        species_inat_id: Option<u64>,
    },
    Clone { id: String },
    List,
    Show { id: String },
    Url { id: String },
    Validate,
    Export {
        #[arg(long, default_value = "dist/data")]
        out: PathBuf,
    },
}

fn tag_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.md"))
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
    let tag = Tag { meta, body: skeleton_body() };
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
        Command::New { species_name, species_inat_id } => {
            let meta = TagMeta {
                id: generate_id(),
                linked_tags: vec![],
                description: None,
                species_name,
                species_inat_id,
                observation_inat_id: None,
            };
            let tag = write_new(dir, meta)?;
            println!("{}", tag.url(DEFAULT_BASE_URL));
        }
        Command::Clone { id } => {
            if !is_valid_id(&id) {
                bail!("invalid id: {id}");
            }
            if !tag_path(dir, &id).exists() {
                bail!("parent tag {id} not found");
            }
            let meta = TagMeta {
                id: generate_id(),
                linked_tags: vec![id],
                description: None,
                species_name: None,
                species_inat_id: None,
                observation_inat_id: None,
            };
            let tag = write_new(dir, meta)?;
            println!("{}", tag.url(DEFAULT_BASE_URL));
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
            let src = fs::read_to_string(tag_path(dir, &id))?;
            let tag = Tag::from_markdown(&src)?;
            println!("{tag:#?}");
        }
        Command::Url { id } => {
            if !is_valid_id(&id) {
                bail!("invalid id: {id}");
            }
            println!("{DEFAULT_BASE_URL}/{id}");
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
```

Build & try it:

```bash
cargo run -p chnm-cli -- new --species-name "Cattleya trianae" --species-inat-id 50310
cargo run -p chnm-cli -- list
cargo run -p chnm-cli -- validate
cargo run -p chnm-cli -- export --out dist/data
```

---

## 7. `chnm-web` — the Leptos app

### 7.1 Crate setup

`crates/chnm-web/Cargo.toml`:

```toml
[package]
name = "chnm-web"
version = "0.1.0"
edition.workspace = true

[dependencies]
leptos = { version = "0.7", features = ["csr"] }
leptos_router = { version = "0.7" }
serde = { workspace = true }
serde_json = { workspace = true }
gloo-net = { version = "0.6", features = ["http"] }
wasm-bindgen = "0.2"
console_error_panic_hook = "0.1"
```

`crates/chnm-web/Trunk.toml`:

```toml
[build]
target = "index.html"
# IMPORTANT for GitHub Pages project sites served from /<repo>/:
public_url = "/chinampa/"
```

> If you publish to a user/organization page (`<owner>.github.io`) or a custom
> domain like `chnm.pa`, set `public_url = "/"` instead.

`crates/chnm-web/index.html`:

```html
<!DOCTYPE html>
<html>
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Chinampa</title>
    <link data-trunk rel="rust" data-bin="chnm-web" />
    <link data-trunk rel="copy-dir" href="public" />
  </head>
  <body></body>
</html>
```

### 7.2 iNaturalist client

`crates/chnm-web/src/inat.rs`:

```rust
use serde::Deserialize;

const API: &str = "https://api.inaturalist.org/v1";

#[derive(Debug, Clone)]
pub struct Photo {
    /// A medium-sized image URL suitable for display.
    pub url: String,
    pub attribution: String,
}

#[derive(Deserialize)]
struct Wrapper<T> {
    results: Vec<T>,
}

#[derive(Deserialize)]
struct ObsResult {
    photos: Vec<ObsPhoto>,
}

#[derive(Deserialize)]
struct ObsPhoto {
    url: String,
    #[serde(default)]
    attribution: String,
}

#[derive(Deserialize)]
struct TaxonResult {
    default_photo: Option<TaxonPhoto>,
}

#[derive(Deserialize)]
struct TaxonPhoto {
    medium_url: String,
    #[serde(default)]
    attribution: String,
}

/// iNaturalist returns square thumbnails by default; swap the size token.
fn to_medium(url: &str) -> String {
    url.replace("/square.", "/medium.")
}

/// Fetch all photos for an observation.
pub async fn observation_photos(id: u64) -> Vec<Photo> {
    let url = format!("{API}/observations/{id}");
    match gloo_net::http::Request::get(&url).send().await {
        Ok(resp) => match resp.json::<Wrapper<ObsResult>>().await {
            Ok(w) => w
                .results
                .into_iter()
                .flat_map(|r| r.photos)
                .map(|p| Photo { url: to_medium(&p.url), attribution: p.attribution })
                .collect(),
            Err(_) => vec![],
        },
        Err(_) => vec![],
    }
}

/// Fall back to the species' default photo.
pub async fn species_photo(taxon_id: u64) -> Option<Photo> {
    let url = format!("{API}/taxa/{taxon_id}");
    let resp = gloo_net::http::Request::get(&url).send().await.ok()?;
    let w = resp.json::<Wrapper<TaxonResult>>().await.ok()?;
    let p = w.results.into_iter().next()?.default_photo?;
    Some(Photo { url: p.medium_url, attribution: p.attribution })
}
```

### 7.3 App, routing & detail view

`crates/chnm-web/src/app.rs` (abridged — shows the data flow):

```rust
use crate::inat::{observation_photos, species_photo, Photo};
use leptos::prelude::*;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::hooks::use_params_map;
use leptos_router::path;
use serde::Deserialize;

#[derive(Clone, Deserialize)]
struct Tag {
    id: String,
    species_name: Option<String>,
    species_inat_id: Option<u64>,
    observation_inat_id: Option<u64>,
    description: Option<String>,
    body: String,
}

// Base path must match Trunk's public_url so fetches resolve on Pages.
const DATA_BASE: &str = "/chinampa/data";

async fn load_tag(id: String) -> Option<Tag> {
    let url = format!("{DATA_BASE}/{id}.json");
    let resp = gloo_net::http::Request::get(&url).send().await.ok()?;
    resp.json::<Tag>().await.ok()
}

/// Observation photos first; fall back to the species photo.
async fn load_photos(tag: Tag) -> Vec<Photo> {
    if let Some(obs) = tag.observation_inat_id {
        let photos = observation_photos(obs).await;
        if !photos.is_empty() {
            return photos;
        }
    }
    if let Some(taxon) = tag.species_inat_id {
        if let Some(p) = species_photo(taxon).await {
            return vec![p];
        }
    }
    vec![]
}

#[component]
pub fn App() -> impl IntoView {
    view! {
        <Router>
            <Routes fallback=|| view! { <p>"Not found"</p> }>
                <Route path=path!("/:id") view=TagView/>
                <Route path=path!("/") view=Home/>
            </Routes>
        </Router>
    }
}

#[component]
fn Home() -> impl IntoView {
    view! { <h1>"Chinampa"</h1> <p>"Scan a tag to view a plant's history."</p> }
}

#[component]
fn TagView() -> impl IntoView {
    let params = use_params_map();
    let id = move || params.with(|p| p.get("id").unwrap_or_default());

    // Load the tag JSON, then its photos.
    let tag = LocalResource::new(move || load_tag(id()));
    let photos = LocalResource::new(move || async move {
        match tag.await {
            Some(t) => load_photos(t).await,
            None => vec![],
        }
    });

    view! {
        <Suspense fallback=|| view! { <p>"Loading…"</p> }>
            {move || tag.get().map(|maybe| match maybe {
                None => view! { <p>"Tag not found"</p> }.into_any(),
                Some(t) => view! {
                    <article>
                        <h1>{t.species_name.clone().unwrap_or(t.id.clone())}</h1>
                        <p>{t.description.clone()}</p>
                        <div class="gallery">
                            {move || photos.get().map(|ps| ps.into_iter().map(|p| view! {
                                <figure>
                                    <img src=p.url alt="iNaturalist photo"/>
                                    <figcaption>{p.attribution}</figcaption>
                                </figure>
                            }).collect_view())}
                        </div>
                        // Render the Markdown log body (use a Markdown crate such
                        // as `pulldown-cmark` -> HTML, then set inner_html).
                        <pre class="log">{t.body.clone()}</pre>
                    </article>
                }.into_any(),
            })}
        </Suspense>
    }
}
```

`crates/chnm-web/src/main.rs`:

```rust
mod app;
mod inat;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}
```

> **Rendering the Markdown body:** for a richer log, convert `t.body` to HTML
> with [`pulldown-cmark`](https://crates.io/crates/pulldown-cmark) and render it
> via `inner_html`. Sanitize the output (e.g. with `ammonia`) since the body is
> human-edited.

### 7.4 Run locally

```bash
cargo run -p chnm-cli -- export --out crates/chnm-web/data   # generate data
cd crates/chnm-web && trunk serve --open
# open http://localhost:8080/  (deep link: http://localhost:8080/<id>)
```

---

## 8. NFC tag content

The CLI prints `https://chinampa.co.cr/<id>` (the domain comes from `chnm.toml`,
falling back to the `DEFAULT_BASE_URL` constant). That URL lands directly on the
static app's `/:id` route:

- The site is served from the **domain root** via a custom domain, with
  `public_url = "/"` in `Trunk.toml` and an empty router `base`.
- GitHub Pages has no server-side rewrites, so the deploy workflow copies
  `index.html` to **`404.html`**. A scan of `/<id>` 404s into that copy, the
  WASM bundle boots, and `leptos_router` matches the `/:id` route client-side.

No hash fragment and no redirect shim are required.

---

## 9. NFC tag capacity & ID scheme analysis

### 9.1 What actually gets written to the tag

A Chinampa tag does **not** store the plant's history — it stores a single URL
(`https://chinampa.co.cr/<id>`). The history lives on the website. So the only
thing that competes for the tag's bytes is one NDEF URI record.

The **504-byte** capacity corresponds to an **NTAG215** chip (for reference:
NTAG213 = 144 B, NTAG215 = 504 B, NTAG216 = 888 B — all of them hold the URL
with room to spare).

Byte budget for `https://chinampa.co.cr/7GK2PQ8X` encoded as an NDEF URI record:

| Part | Bytes |
|------|-------|
| NDEF message TLV wrapper + terminator | ~3 |
| NDEF record header + type length + payload length + type `U` | 4 |
| URI prefix code `0x04` (`https://`) | 1 |
| `chinampa.co.cr/7GK2PQ8X` (23 chars) | 23 |
| **Total** | **~31** |

Even a longer custom domain URL lands well under **~50 bytes**. Either way you
use **~5–10 %** of an NTAG215; roughly **460–480 bytes stay free**.

**Conclusion:** the 504-byte capacity is *not* a constraint and does not limit
how many tags you can manage. The number of manageable tags is determined by
the **size of the ID namespace**, not by the bytes on any single tag.

### 9.2 How many distinct tags the ID space allows

With the current scheme — 8 characters from a 62-symbol alphabet (`[a-zA-Z0-9]`)
— the namespace is:

$$62^8 = 218{,}340{,}105{,}584{,}896 \approx 2.18 \times 10^{14}$$

For comparison, namespaces at other lengths:

| ID length | Namespace ($62^L$) | Approx. |
|-----------|--------------------|---------|
| 4 | 14,776,336 | 1.5 × 10⁷ |
| 5 | 916,132,832 | 9.2 × 10⁸ |
| 6 | 56,800,235,584 | 5.7 × 10¹⁰ |
| 7 | 3,521,614,606,208 | 3.5 × 10¹² |
| **8** | **218,340,105,584,896** | **2.2 × 10¹⁴** |

### 9.3 Collision analysis (birthday bound)

IDs are random and non-sequential, so the relevant risk is the *birthday
problem*: the chance that two independently generated IDs collide. For $n$
random IDs in a namespace of size $N$:

$$P(\text{any collision}) \approx \frac{n^2}{2N}$$

Crucially, `chnm new`/`clone` **check for an existing file before writing**, and
`chnm validate` enforces uniqueness across the whole repository in CI. A
collision is therefore *detected and the ID is regenerated* — it can never
silently produce a duplicate. So the only practical question is *how often a
regeneration is needed*, which per new tag is `current_count / N`.

Assume a generous upper bound of **1,000,000 tags** over the nursery's entire
lifetime:

| ID length | Expected collisions over 1M tags ($n^2/2N$) | Per-insert retry chance at 1M tags |
|-----------|---------------------------------------------|------------------------------------|
| 6 | ~8.8 | 0.0018 % |
| 7 | ~0.14 | 0.00003 % |
| **8** | **~0.002** | **0.0000005 %** |

Even at a million plants, an 8-char ID is effectively collision-free, and a
6-char ID would still only need a handful of automatic retries over the entire
history of the nursery.

### 9.4 Should the ID be shortened?

**No meaningful benefit.** Shortening trades away large amounts of collision
headroom to save 1–2 bytes on a tag that already has ~470 free bytes, and it
barely shortens the URL (the domain dominates the length, not the ID). The
recommendation is to **keep 8 characters**. The spare capacity could optionally
hold a small fallback (e.g. a cached species name), but the design deliberately
stores only the URL so tags remain trivially cloneable and rewritable.

### 9.5 Use a standard instead of a bespoke scheme

Good news: the implementation in §5 already uses **NanoID** (the `nanoid`
crate), which *is* a widely adopted, specified standard for short, URL-safe
IDs — so you are not really inventing your own algorithm. The only bespoke
choices are the length (8) and the base62 alphabet.

Standards comparable to a “short UUID”:

| Scheme | Typical length | Alphabet | Time-sortable | Notes |
|--------|----------------|----------|---------------|-------|
| **NanoID** | configurable (8 here; 21 default) | configurable, URL-safe | no | De-facto standard; already used; published collision calculator. |
| **ULID** | 26 chars | Crockford Base32 | yes | 128-bit; canonical spec; lexicographically sortable by creation time; avoids ambiguous chars (no `I L O U`). |
| **UUIDv7** | 36 (or 22 base64) | hex | yes | RFC 9562; time-ordered; long. |
| **short-uuid** | ~22 chars | base57 | depends | Re-encodes a UUIDv4 — literally the “short UUID” idea, but longer than NanoID. |
| **Crockford Base32** | encoding only | `0-9 A-Z` minus `I L O U` | — | Case-insensitive, unambiguous; good if an ID is ever read aloud or typed. |
| **Base58** | encoding only | minus `0 O I l` | — | Bitcoin/Flickr style; unambiguous. |

**Recommendations**

1. **Stay with NanoID (default choice).** It is the standard that best fits this
   use case — short, URL-friendly, length-tunable — and it is already wired in.
   Keeping length 8 is ample (§9.2–9.3).
2. **Harden the alphabet for human fallback.** If a tag is ever damaged and an
   operator must read/key the ID, switch NanoID's alphabet to a Crockford-style
   set that drops confusable characters. This keeps the “standard” while making
   IDs unambiguous:

   ```rust
   // chnm-core: unambiguous, case-insensitive-friendly alphabet (32 symbols).
   // Excludes I, L, O, U and the digits 0/1 that look like letters.
   const ID_ALPHABET: [char; 32] = [
       '2','3','4','5','6','7','8','9',
       'A','B','C','D','E','F','G','H','J','K','M','N',
       'P','Q','R','S','T','V','W','X','Y','Z',
   ];
   // 32^8 = 1.1 × 10¹² namespace — still far beyond any nursery's needs.
   ```

3. **Choose ULID instead if you want a formal spec + sortability.** ULIDs are
   26 chars (fits comfortably in 504 bytes) and let tags sort by creation time,
   at the cost of a longer URL and exposing the creation timestamp. Swap the
   generator:

   ```toml
   # chnm-core/Cargo.toml
   ulid = "1"
   ```

   ```rust
   pub fn generate_id() -> String {
       ulid::Ulid::new().to_string() // 26-char Crockford Base32, time-ordered
   }
   ```

For Chinampa, **NanoID with the hardened alphabet (option 1 + 2)** is the
recommended balance: a standard scheme, short URLs, unambiguous characters, and
a namespace orders of magnitude larger than required.

---

## 10. CI recipes (GitHub Actions → GitHub Pages)

Two workflows. The first guards every change; the second publishes.

### 10.1 `.github/workflows/ci.yml` — validate & build on PRs

```yaml
name: ci

on:
  pull_request:
  push:
    branches: [main]

jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: wasm32-unknown-unknown
      - uses: Swatinem/rust-cache@v2

      # Validate every tag file (ids unique/valid, linked tags resolve).
      - name: Validate tags
        run: cargo run -p chnm-cli -- validate

      - name: Unit tests
        run: cargo test --workspace

      # Smoke-build the web app so broken UI never reaches main.
      - name: Install trunk
        run: cargo install trunk --locked
      - name: Build web
        run: |
          cargo run -p chnm-cli -- export --out crates/chnm-web/dist/data
          trunk build crates/chnm-web/index.html
```

### 10.2 `.github/workflows/deploy.yml` — process tags & publish

This is the recipe that "adds each new tag file": on every push to `main` it
re-exports **all** tag Markdown files to JSON, builds the static app, and
deploys it to Pages. Adding a new `tags/<id>.md` and merging it is all that's
required — no server, no manual step.

```yaml
name: deploy

on:
  push:
    branches: [main]
  workflow_dispatch:

permissions:
  contents: read
  pages: write
  id-token: write

concurrency:
  group: pages
  cancel-in-progress: true

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: wasm32-unknown-unknown
      - uses: Swatinem/rust-cache@v2
      - name: Install trunk
        run: cargo install trunk --locked

      # 1. Turn every tags/*.md into dist/data/*.json
      - name: Export tag data
        run: cargo run -p chnm-cli -- export --out crates/chnm-web/dist/data

      # 2. Build the static Leptos bundle (data is copied into dist/).
      - name: Build site
        run: trunk build --release crates/chnm-web/index.html

      # 3. SPA fallback so deep links work on Pages.
      - name: SPA 404 fallback
        run: cp crates/chnm-web/dist/index.html crates/chnm-web/dist/404.html

      - uses: actions/upload-pages-artifact@v3
        with:
          path: crates/chnm-web/dist

  deploy:
    needs: build
    runs-on: ubuntu-latest
    environment:
      name: github-pages
      url: ${{ steps.deployment.outputs.page_url }}
    steps:
      - id: deployment
        uses: actions/deploy-pages@v4
```

Enable **Settings → Pages → Build and deployment → Source: GitHub Actions** once.

> **Note on `public_url`:** Trunk rewrites asset paths using `public_url`. The
> `dist/data` directory is copied verbatim by Trunk's `copy-dir`, so the web
> app's `DATA_BASE` (`/chinampa/data`) must match `public_url` + `data`.

---

## 11. Suggested test coverage

Add unit tests in `chnm-core` (run by `cargo test` in CI):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_valid_shape() {
        let id = generate_id();
        assert!(is_valid_id(&id));
        assert_eq!(id.len(), ID_LEN);
    }

    #[test]
    fn roundtrip_markdown() {
        let tag = Tag {
            meta: TagMeta {
                id: generate_id(),
                linked_tags: vec![],
                description: Some("in-vitro individual".into()),
                species_name: Some("Cattleya trianae".into()),
                species_inat_id: Some(50310),
                observation_inat_id: None,
            },
            body: "## Log\n- 2026-01-01 — created\n".into(),
        };
        let md = tag.to_markdown().unwrap();
        let back = Tag::from_markdown(&md).unwrap();
        assert_eq!(back.meta.id, tag.meta.id);
        assert_eq!(back.meta.species_inat_id, Some(50310));
    }
}
```

---

## 12. Build order checklist

1. [x] Create the workspace `Cargo.toml` and `rust-toolchain.toml`.
2. [x] Implement `chnm-core` (model, ID gen, parse/serialize) + tests.
3. [x] Implement `chnm-cli` (`new`, `clone`, `list`, `show`, `url`, `validate`, `export`).
4. [x] Create a couple of sample tags under `tags/` with `chnm new`.
5. [x] Scaffold `chnm-web` (Trunk + Leptos CSR), iNat client, routing, detail view.
6. [x] Wire `chnm export` → `crates/chnm-web/data` (copied to `dist/data`) and confirm `trunk serve` renders a tag.
7. [x] Add `ci.yml` (validate + smoke build) and `deploy.yml` (export + build + Pages).
8. [ ] Enable GitHub Pages (Source: GitHub Actions) and pick the NFC URL scheme.
9. [ ] Push to `main`, confirm the site deploys, scan a real tag.

---

## 13. Example tag file

`tags/7Gk2pQ8x.md`:

```markdown
---
id: 7Gk2pQ8x
linked_tags: []
description: in-vitro individual
species_name: Cattleya trianae
species_inat_id: 50310
observation_inat_id: null
---

## Description

In-vitro individual transferred from multiplication flask.

## Log

- 2026-01-15 — created
- 2026-01-15 — explant from mother plant (linked tag pending)
```
