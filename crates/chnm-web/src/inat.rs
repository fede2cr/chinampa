//! Minimal iNaturalist API v1 client used to resolve photos at runtime.
//!
//! Observation photos are preferred; the species' default photo is the
//! fallback. Network/parse errors degrade gracefully to "no photos".

use serde::Deserialize;

const API: &str = "https://api.inaturalist.org/v1";

/// A displayable photo resolved from iNaturalist.
#[derive(Debug, Clone)]
pub struct Photo {
    /// A medium-sized image URL suitable for display.
    pub url: String,
    /// Attribution string required by iNaturalist's terms.
    pub attribution: String,
}

#[derive(Deserialize)]
struct Wrapper<T> {
    results: Vec<T>,
}

#[derive(Deserialize)]
struct ObsResult {
    #[serde(default)]
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

/// iNaturalist returns square thumbnails by default; swap the size token so we
/// display a larger image.
fn to_medium(url: &str) -> String {
    url.replace("/square.", "/medium.")
}

/// Fetch all photos for an observation. Returns an empty list on any error.
pub async fn observation_photos(id: u64) -> Vec<Photo> {
    let url = format!("{API}/observations/{id}");
    match gloo_net::http::Request::get(&url).send().await {
        Ok(resp) => match resp.json::<Wrapper<ObsResult>>().await {
            Ok(w) => w
                .results
                .into_iter()
                .flat_map(|r| r.photos)
                .map(|p| Photo {
                    url: to_medium(&p.url),
                    attribution: p.attribution,
                })
                .collect(),
            Err(_) => vec![],
        },
        Err(_) => vec![],
    }
}

/// Fall back to the species' default photo. Returns `None` on any error.
pub async fn species_photo(taxon_id: u64) -> Option<Photo> {
    let url = format!("{API}/taxa/{taxon_id}");
    let resp = gloo_net::http::Request::get(&url).send().await.ok()?;
    let w = resp.json::<Wrapper<TaxonResult>>().await.ok()?;
    let p = w.results.into_iter().next()?.default_photo?;
    Some(Photo {
        url: p.medium_url,
        attribution: p.attribution,
    })
}
