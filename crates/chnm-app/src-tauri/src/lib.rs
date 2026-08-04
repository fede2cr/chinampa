//! Chinampa mobile app backend.
//!
//! Exposes Tauri commands to the WebView frontend:
//! - [`list_tags`] reads the repository's `tags/` directory through the GitHub
//!   REST API and parses each Markdown file with [`chnm_core`].
//! - [`create_tag`] / [`clone_tag`] write a new tag file back to the repository
//!   through the GitHub Contents API (create commit). These require a token.
//! - [`tag_url`] validates an ID and builds the URL written to an NFC chip.
//!
//! The GitHub repository remains the source of truth. Reading is unauthenticated
//! for public repositories; writing (create/clone) needs a token with `contents`
//! write access to the repo.

use std::collections::HashSet;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use chnm_core::{generate_id, is_valid_id, BookReference, Tag, TagMeta};
use serde::{Deserialize, Serialize};

/// Repository + display settings supplied by the frontend.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoSettings {
    /// Repository owner (user or org), e.g. `fede2`.
    pub owner: String,
    /// Repository name, e.g. `chinampa`.
    pub repo: String,
    /// Git branch/ref to read from.
    #[serde(default = "default_branch")]
    pub branch: String,
    /// Directory holding the tag Markdown files.
    #[serde(default = "default_tags_path")]
    pub tags_path: String,
    /// Host used to build the NFC URL (`https://<domain>/<id>`).
    pub domain: String,
    /// GitHub token. Optional for reading public repos; required for writing.
    #[serde(default)]
    pub token: Option<String>,
}

fn default_branch() -> String {
    "main".to_string()
}

fn default_tags_path() -> String {
    "tags".to_string()
}

/// Fields the frontend can set when creating a new tag (mirrors `chnm new`).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewTagInput {
    #[serde(default)]
    pub species_name: Option<String>,
    #[serde(default)]
    pub species_inat_id: Option<u64>,
    #[serde(default)]
    pub observation_inat_id: Option<u64>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub collection: Option<String>,
    #[serde(default)]
    pub for_sale: bool,
    #[serde(default)]
    pub price: Option<f64>,
    #[serde(default)]
    pub count: Option<u32>,
    /// Book references cited by this tag (book name, authors, ISBN, page).
    #[serde(default)]
    pub book_references: Vec<BookReference>,
    /// Parent tag IDs to link back to (used by `clone_tag`).
    #[serde(default)]
    pub linked_tags: Vec<String>,
}

/// A tag summary returned to the frontend for listing and NFC writing.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagSummary {
    pub id: String,
    pub species_name: Option<String>,
    pub description: Option<String>,
    pub collection: Option<String>,
    pub for_sale: bool,
    /// The URL written to the NFC tag.
    pub url: String,
}

/// A tag's full detail plus the GitHub blob SHA needed to update it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagDetail {
    pub id: String,
    pub linked_tags: Vec<String>,
    pub species_name: Option<String>,
    pub species_inat_id: Option<u64>,
    pub observation_inat_id: Option<u64>,
    pub description: Option<String>,
    pub collection: Option<String>,
    pub for_sale: bool,
    pub price: Option<f64>,
    pub count: Option<u32>,
    pub book_references: Vec<BookReference>,
    pub body: String,
    pub sha: String,
    pub url: String,
}

/// Editable fields for an existing tag (mirrors `chnm_core::Tag`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagEdit {
    #[serde(default)]
    pub linked_tags: Vec<String>,
    #[serde(default)]
    pub species_name: Option<String>,
    #[serde(default)]
    pub species_inat_id: Option<u64>,
    #[serde(default)]
    pub observation_inat_id: Option<u64>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub collection: Option<String>,
    #[serde(default)]
    pub for_sale: bool,
    #[serde(default)]
    pub price: Option<f64>,
    #[serde(default)]
    pub count: Option<u32>,
    #[serde(default)]
    pub book_references: Vec<BookReference>,
    pub body: String,
}

/// One entry in a GitHub Contents API directory listing.
#[derive(Debug, Deserialize)]
struct ContentEntry {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    download_url: Option<String>,
}

/// A single file fetched from the GitHub Contents API (base64 content + sha).
#[derive(Debug, Deserialize)]
struct ContentFile {
    content: String,
    encoding: String,
    sha: String,
}

/// Build `https://<host>/<id>`, tolerating a `domain` that already carries a
/// scheme and/or trailing slash.
fn build_url(domain: &str, id: &str) -> String {
    let host = domain
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    format!("https://{host}/{id}")
}

/// A shared HTTP client with the User-Agent GitHub requires.
fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("chinampa-app")
        .build()
        .map_err(|e| e.to_string())
}

/// The GitHub Contents API URL for the tags directory.
fn contents_dir_url(settings: &RepoSettings) -> String {
    format!(
        "https://api.github.com/repos/{}/{}/contents/{}?ref={}",
        settings.owner,
        settings.repo,
        settings.tags_path.trim_matches('/'),
        settings.branch
    )
}

fn summary_from_tag(settings: &RepoSettings, tag: Tag) -> TagSummary {
    let url = build_url(&settings.domain, &tag.meta.id);
    TagSummary {
        id: tag.meta.id,
        species_name: tag.meta.species_name,
        description: tag.meta.description,
        collection: tag.meta.collection,
        for_sale: tag.meta.for_sale,
        url,
    }
}

/// Collect the set of existing tag IDs (file stems) in the repo's tags dir.
async fn existing_ids(
    client: &reqwest::Client,
    settings: &RepoSettings,
) -> Result<HashSet<String>, String> {
    let mut req = client
        .get(contents_dir_url(settings))
        .header("Accept", "application/vnd.github+json");
    if let Some(token) = settings.token.as_deref().filter(|t| !t.is_empty()) {
        req = req.bearer_auth(token);
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    // A missing tags directory just means there are no tags yet.
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(HashSet::new());
    }
    let entries: Vec<ContentEntry> = resp
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    Ok(entries
        .into_iter()
        .filter(|e| e.kind == "file")
        .filter_map(|e| e.name.strip_suffix(".md").map(str::to_string))
        .collect())
}

/// Outcome of a Contents API write. A `Conflict` means the supplied `sha` was
/// stale (someone else committed to the file first).
enum PutResult {
    Written,
    Conflict,
}

/// Create or update a `tags/<id>.md` file via the GitHub Contents API. Passing
/// `sha` updates the existing file; `None` creates a new one.
async fn put_tag_file(
    client: &reqwest::Client,
    settings: &RepoSettings,
    token: &str,
    id: &str,
    markdown: &str,
    sha: Option<&str>,
) -> Result<PutResult, String> {
    let path = format!("{}/{}.md", settings.tags_path.trim_matches('/'), id);
    let url = format!(
        "https://api.github.com/repos/{}/{}/contents/{}",
        settings.owner, settings.repo, path
    );
    let action = if sha.is_some() { "update" } else { "add" };
    let mut body = serde_json::json!({
        "message": format!("chnm-app: {action} tag {id}"),
        "content": STANDARD.encode(markdown.as_bytes()),
        "branch": settings.branch,
    });
    if let Some(sha) = sha {
        body["sha"] = serde_json::json!(sha);
    }
    let resp = client
        .put(&url)
        .header("Accept", "application/vnd.github+json")
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status().is_success() {
        return Ok(PutResult::Written);
    }
    let status = resp.status();
    // GitHub answers 409 Conflict (and sometimes 422) when the sha is stale.
    if sha.is_some()
        && matches!(
            status,
            reqwest::StatusCode::CONFLICT | reqwest::StatusCode::UNPROCESSABLE_ENTITY
        )
    {
        return Ok(PutResult::Conflict);
    }
    let text = resp.text().await.unwrap_or_default();
    Err(format!("GitHub write failed ({status}): {text}"))
}

/// Shared create path used by both `create_tag` and `clone_tag`.
async fn create_and_commit(
    settings: RepoSettings,
    mut input: NewTagInput,
) -> Result<TagSummary, String> {
    let token = settings
        .token
        .clone()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| "a GitHub token is required to create tags".to_string())?;

    let client = build_client()?;

    // Mirror `chnm new`: when an observation is given but the species wasn't set
    // directly, fill the species fields from iNaturalist.
    if let Some(obs) = input.observation_inat_id {
        if input.species_inat_id.is_none() || input.species_name.is_none() {
            let (species_inat_id, species_name) =
                fetch_observation_species(&client, obs).await?;
            input.species_inat_id.get_or_insert(species_inat_id);
            input.species_name.get_or_insert(species_name);
        }
    }

    let existing = existing_ids(&client, &settings).await?;

    // Allocate a fresh, collision-free ID.
    let id = (0..1000)
        .map(|_| generate_id())
        .find(|candidate| !existing.contains(candidate))
        .ok_or_else(|| "could not allocate a unique id".to_string())?;

    let meta = TagMeta {
        id: id.clone(),
        linked_tags: input.linked_tags,
        description: input.description,
        species_inat_id: input.species_inat_id,
        species_name: input.species_name,
        observation_inat_id: input.observation_inat_id,
        collection: input.collection,
        for_sale: input.for_sale,
        price: input.price,
        count: input.count,
        book_references: input.book_references,
    };
    let tag = Tag::new(meta);
    let markdown = tag.to_markdown().map_err(|e| e.to_string())?;

    match put_tag_file(&client, &settings, &token, &id, &markdown, None).await? {
        PutResult::Written => Ok(summary_from_tag(&settings, tag)),
        // A create should never collide on sha; surface it as an error.
        PutResult::Conflict => Err(format!("tag {id} already exists")),
    }
}

/// The species resolved from an iNaturalist observation.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservationSpecies {
    pub species_inat_id: u64,
    pub species_name: String,
}

#[derive(Deserialize)]
struct InatWrapper {
    results: Vec<InatObservation>,
}

#[derive(Deserialize)]
struct InatObservation {
    taxon: Option<InatTaxon>,
}

#[derive(Deserialize)]
struct InatTaxon {
    id: u64,
    /// Scientific name (e.g. "Cattleya trianae").
    name: String,
}

/// Look up an iNaturalist observation and return its associated species.
async fn fetch_observation_species(
    client: &reqwest::Client,
    observation_id: u64,
) -> Result<(u64, String), String> {
    let url = format!("https://api.inaturalist.org/v1/observations/{observation_id}");
    let wrapper: InatWrapper = client
        .get(&url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let taxon = wrapper
        .results
        .into_iter()
        .next()
        .ok_or_else(|| format!("iNaturalist observation {observation_id} not found"))?
        .taxon
        .ok_or_else(|| {
            format!("iNaturalist observation {observation_id} has no identified species")
        })?;
    Ok((taxon.id, taxon.name))
}

/// A displayable iNaturalist photo (mirrors the web front-end's gallery).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Photo {
    /// A medium-sized image URL suitable for display.
    pub url: String,
    /// Attribution string required by iNaturalist's terms.
    pub attribution: String,
    /// Link to the iNaturalist page this photo came from (observation or taxon).
    pub link: String,
}

#[derive(Deserialize)]
struct InatResults<T> {
    results: Vec<T>,
}

#[derive(Deserialize)]
struct InatObsPhotos {
    #[serde(default)]
    photos: Vec<InatObsPhoto>,
}

#[derive(Deserialize)]
struct InatObsPhoto {
    url: String,
    #[serde(default)]
    attribution: String,
}

#[derive(Deserialize)]
struct InatTaxonPhotos {
    default_photo: Option<InatTaxonPhoto>,
}

#[derive(Deserialize)]
struct InatTaxonPhoto {
    medium_url: String,
    #[serde(default)]
    attribution: String,
}

/// iNaturalist returns square thumbnails by default; swap the size token so we
/// display a larger image.
fn to_medium(url: &str) -> String {
    url.replace("/square.", "/medium.")
}

/// Fetch all photos for an observation. Returns an empty list on any error.
async fn observation_photos(client: &reqwest::Client, id: u64) -> Vec<Photo> {
    let url = format!("https://api.inaturalist.org/v1/observations/{id}");
    let link = format!("https://www.inaturalist.org/observations/{id}");
    let Ok(resp) = client.get(&url).send().await else {
        return Vec::new();
    };
    match resp.json::<InatResults<InatObsPhotos>>().await {
        Ok(w) => w
            .results
            .into_iter()
            .flat_map(|r| r.photos)
            .map(|p| Photo {
                url: to_medium(&p.url),
                attribution: p.attribution,
                link: link.clone(),
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Fall back to the species' default photo. Returns `None` on any error.
async fn species_photo(client: &reqwest::Client, taxon_id: u64) -> Option<Photo> {
    let url = format!("https://api.inaturalist.org/v1/taxa/{taxon_id}");
    let resp = client.get(&url).send().await.ok()?;
    let w = resp.json::<InatResults<InatTaxonPhotos>>().await.ok()?;
    let p = w.results.into_iter().next()?.default_photo?;
    Some(Photo {
        url: p.medium_url,
        attribution: p.attribution,
        link: format!("https://www.inaturalist.org/taxa/{taxon_id}"),
    })
}

/// Resolve a tag's photos: observation photos first, species photo as the
/// fallback. Errors degrade to an empty list so the detail screen still shows.
#[tauri::command]
async fn tag_photos(
    observation_inat_id: Option<u64>,
    species_inat_id: Option<u64>,
) -> Result<Vec<Photo>, String> {
    let client = build_client()?;
    if let Some(obs) = observation_inat_id {
        let photos = observation_photos(&client, obs).await;
        if !photos.is_empty() {
            return Ok(photos);
        }
    }
    if let Some(taxon) = species_inat_id {
        if let Some(p) = species_photo(&client, taxon).await {
            return Ok(vec![p]);
        }
    }
    Ok(Vec::new())
}

/// List every tag in the repository's `tags/` directory via the GitHub API.
#[tauri::command]
async fn list_tags(settings: RepoSettings) -> Result<Vec<TagSummary>, String> {
    let client = build_client()?;

    let mut req = client
        .get(contents_dir_url(&settings))
        .header("Accept", "application/vnd.github+json");
    if let Some(token) = settings.token.as_deref().filter(|t| !t.is_empty()) {
        req = req.bearer_auth(token);
    }

    let resp = req.send().await.map_err(|e| e.to_string())?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(Vec::new());
    }
    let entries: Vec<ContentEntry> = resp
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for entry in entries {
        if entry.kind != "file" || !entry.name.ends_with(".md") {
            continue;
        }
        let Some(download_url) = entry.download_url else {
            continue;
        };

        // Skip files that fail to fetch or parse rather than failing the whole list.
        let src = match client.get(&download_url).send().await {
            Ok(resp) => match resp.error_for_status() {
                Ok(resp) => match resp.text().await {
                    Ok(text) => text,
                    Err(_) => continue,
                },
                Err(_) => continue,
            },
            Err(_) => continue,
        };

        let Ok(tag) = Tag::from_markdown(&src) else {
            continue;
        };
        out.push(summary_from_tag(&settings, tag));
    }

    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Create a new tag file in the repository and return its summary.
#[tauri::command]
async fn create_tag(settings: RepoSettings, input: NewTagInput) -> Result<TagSummary, String> {
    create_and_commit(settings, input).await
}

/// Create a new tag linked back to an existing parent tag.
#[tauri::command]
async fn clone_tag(settings: RepoSettings, parent_id: String) -> Result<TagSummary, String> {
    if !is_valid_id(&parent_id) {
        return Err(format!("invalid parent id: {parent_id}"));
    }
    let input = NewTagInput {
        linked_tags: vec![parent_id],
        ..NewTagInput::default()
    };
    create_and_commit(settings, input).await
}

/// Fetch one tag's full detail plus the blob SHA required to update it. Shared
/// by the `get_tag` command and the conflict-recovery path in `update_tag`.
async fn fetch_tag_detail(
    client: &reqwest::Client,
    settings: &RepoSettings,
    id: &str,
) -> Result<TagDetail, String> {
    let path = format!("{}/{}.md", settings.tags_path.trim_matches('/'), id);
    let url = format!(
        "https://api.github.com/repos/{}/{}/contents/{}?ref={}",
        settings.owner, settings.repo, path, settings.branch
    );
    let mut req = client
        .get(&url)
        .header("Accept", "application/vnd.github+json");
    if let Some(token) = settings.token.as_deref().filter(|t| !t.is_empty()) {
        req = req.bearer_auth(token);
    }
    let file: ContentFile = req
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    if file.encoding != "base64" {
        return Err(format!("unexpected content encoding: {}", file.encoding));
    }
    // GitHub wraps the base64 payload; drop whitespace before decoding.
    let cleaned: String = file.content.split_whitespace().collect();
    let bytes = STANDARD.decode(cleaned).map_err(|e| e.to_string())?;
    let src = String::from_utf8(bytes).map_err(|e| e.to_string())?;
    let tag = Tag::from_markdown(&src).map_err(|e| e.to_string())?;
    let url = build_url(&settings.domain, &tag.meta.id);
    Ok(TagDetail {
        id: tag.meta.id,
        linked_tags: tag.meta.linked_tags,
        species_name: tag.meta.species_name,
        species_inat_id: tag.meta.species_inat_id,
        observation_inat_id: tag.meta.observation_inat_id,
        description: tag.meta.description,
        collection: tag.meta.collection,
        for_sale: tag.meta.for_sale,
        price: tag.meta.price,
        count: tag.meta.count,
        book_references: tag.meta.book_references,
        body: tag.body,
        sha: file.sha,
        url,
    })
}

/// Fetch one tag's full contents plus the blob SHA required to update it.
#[tauri::command]
async fn get_tag(settings: RepoSettings, id: String) -> Result<TagDetail, String> {
    if !is_valid_id(&id) {
        return Err(format!("invalid id: {id}"));
    }
    let client = build_client()?;
    fetch_tag_detail(&client, &settings, &id).await
}

/// Result of an `update_tag` call. `Conflict` carries the current server-side
/// tag so the frontend can show the latest state and let the user retry.
#[derive(Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum UpdateOutcome {
    Ok { tag: TagSummary },
    Conflict { latest: Box<TagDetail> },
}

/// Update an existing tag file via the GitHub Contents API (requires its sha).
#[tauri::command]
async fn update_tag(
    settings: RepoSettings,
    id: String,
    sha: String,
    edit: TagEdit,
) -> Result<UpdateOutcome, String> {
    if !is_valid_id(&id) {
        return Err(format!("invalid id: {id}"));
    }
    let token = settings
        .token
        .clone()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| "a GitHub token is required to update tags".to_string())?;
    let meta = TagMeta {
        id: id.clone(),
        linked_tags: edit.linked_tags,
        description: edit.description,
        species_inat_id: edit.species_inat_id,
        species_name: edit.species_name,
        observation_inat_id: edit.observation_inat_id,
        collection: edit.collection,
        for_sale: edit.for_sale,
        price: edit.price,
        count: edit.count,
        book_references: edit.book_references,
    };
    let tag = Tag {
        meta,
        body: edit.body,
    };
    let markdown = tag.to_markdown().map_err(|e| e.to_string())?;
    let client = build_client()?;
    match put_tag_file(&client, &settings, &token, &id, &markdown, Some(&sha)).await? {
        PutResult::Written => Ok(UpdateOutcome::Ok {
            tag: summary_from_tag(&settings, tag),
        }),
        // Stale sha: refetch the current server-side tag so the user can review.
        PutResult::Conflict => {
            let latest = fetch_tag_detail(&client, &settings, &id).await?;
            Ok(UpdateOutcome::Conflict {
                latest: Box::new(latest),
            })
        }
    }
}

/// Repository + display config parsed from the repo's `chnm.toml`. Every field
/// is optional so a partial or older config still deserializes cleanly.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteConfig {
    pub domain: Option<String>,
    pub owner: Option<String>,
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub tags_path: Option<String>,
}

/// Shape of `chnm.toml` (the fields the app cares about).
#[derive(Deserialize)]
struct ChnmToml {
    domain: Option<String>,
    github: Option<GithubSection>,
}

#[derive(Deserialize)]
struct GithubSection {
    owner: Option<String>,
    repo: Option<String>,
    branch: Option<String>,
    tags_path: Option<String>,
}

/// Read `chnm.toml` from the given repo and return the domain + github settings.
/// Used to seed the app's configuration from the repository's single source of
/// truth (the CLI reads the same file).
#[tauri::command]
async fn fetch_config(
    owner: String,
    repo: String,
    branch: String,
) -> Result<RemoteConfig, String> {
    let client = build_client()?;
    let url = format!(
        "https://raw.githubusercontent.com/{owner}/{repo}/{branch}/chnm.toml"
    );
    let src = client
        .get(&url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    let parsed: ChnmToml = toml::from_str(&src).map_err(|e| e.to_string())?;
    let github = parsed.github.unwrap_or(GithubSection {
        owner: None,
        repo: None,
        branch: None,
        tags_path: None,
    });
    Ok(RemoteConfig {
        domain: parsed.domain,
        owner: github.owner,
        repo: github.repo,
        branch: github.branch,
        tags_path: github.tags_path,
    })
}

/// Resolve an iNaturalist observation to its species (ID + scientific name).
#[tauri::command]
async fn resolve_observation(observation_inat_id: u64) -> Result<ObservationSpecies, String> {
    let client = build_client()?;
    let (species_inat_id, species_name) =
        fetch_observation_species(&client, observation_inat_id).await?;
    Ok(ObservationSpecies {
        species_inat_id,
        species_name,
    })
}

/// Validate an ID and return the URL that should be written to its NFC tag.
#[tauri::command]
fn tag_url(id: String, domain: String) -> Result<String, String> {
    if !is_valid_id(&id) {
        return Err(format!("invalid id: {id}"));
    }
    Ok(build_url(&domain, &id))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_nfc::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .invoke_handler(tauri::generate_handler![
            list_tags,
            get_tag,
            create_tag,
            clone_tag,
            update_tag,
            resolve_observation,
            tag_photos,
            fetch_config,
            tag_url
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
