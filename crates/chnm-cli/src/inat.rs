//! Minimal blocking iNaturalist API v1 client used by the CLI to resolve a
//! species from an observation at tag-creation time.

use anyhow::{Context, Result};
use serde::Deserialize;

const API: &str = "https://api.inaturalist.org/v1";

#[derive(Deserialize)]
struct Wrapper<T> {
    results: Vec<T>,
}

#[derive(Deserialize)]
struct ObsResult {
    taxon: Option<Taxon>,
}

#[derive(Deserialize)]
struct Taxon {
    id: u64,
    /// Scientific name (e.g. "Cattleya trianae").
    name: String,
}

/// The species details resolved from an iNaturalist observation.
pub struct ObservationSpecies {
    /// iNaturalist taxon ID for the observation's species.
    pub species_inat_id: u64,
    /// Scientific species name.
    pub species_name: String,
}

/// Look up an iNaturalist observation and return its associated species.
pub fn observation_species(id: u64) -> Result<ObservationSpecies> {
    let url = format!("{API}/observations/{id}");
    let wrapper: Wrapper<ObsResult> = ureq::get(&url)
        .call()
        .with_context(|| format!("fetching iNaturalist observation {id}"))?
        .into_json()
        .with_context(|| format!("parsing iNaturalist observation {id}"))?;
    let taxon = wrapper
        .results
        .into_iter()
        .next()
        .with_context(|| format!("iNaturalist observation {id} not found"))?
        .taxon
        .with_context(|| format!("iNaturalist observation {id} has no identified species"))?;
    Ok(ObservationSpecies {
        species_inat_id: taxon.id,
        species_name: taxon.name,
    })
}
