//! Core model for Chinampa plant tags: the tag data structure, ID generation,
//! and Markdown (YAML-frontmatter) parsing/serialization shared by the CLI and
//! the web app.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Unambiguous, Crockford-style ID alphabet (30 symbols).
///
/// Excludes characters that are easy to confuse when a tag is read or keyed by
/// hand: `I`, `L`, `O`, `U`, and the digits `0`/`1`. This keeps the IDs
/// URL-safe and human-friendly while still giving a `30^8 ≈ 6.6 × 10¹¹`
/// namespace. The random generation itself is done by the `nanoid` crate; this
/// is just the custom charset handed to it (nanoid has no built-in
/// unambiguous alphabet).
const ID_ALPHABET: &str = "23456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Length of a generated tag ID.
pub const ID_LEN: usize = 8;

/// The machine-readable frontmatter of a tag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TagMeta {
    /// Unique 8-char ID (see [`generate_id`]).
    pub id: String,
    /// Previous tag IDs associated with this tag (e.g. the in-vitro media used,
    /// or the mother plant used as explant).
    #[serde(default)]
    pub linked_tags: Vec<String>,
    /// Free-form description (young/mature plant, in-vitro individual,
    /// multi-plant container, etc.).
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
    /// Optional collection/grouping this tag belongs to.
    #[serde(default)]
    pub collection: Option<String>,
    /// Whether the plant is currently offered for sale.
    #[serde(default)]
    pub for_sale: bool,
    /// Sale price, expressed in the currency configured in `chnm.toml`
    /// (`currency`, default `CRC`).
    #[serde(default)]
    pub price: Option<f64>,
}

/// A full tag: frontmatter + the free-form Markdown log body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tag {
    #[serde(flatten)]
    pub meta: TagMeta,
    /// The Markdown body (the "Log" section the operator edits by hand).
    pub body: String,
}

/// Errors produced while parsing or validating a tag.
#[derive(Debug, Error)]
pub enum TagError {
    #[error("missing or malformed YAML frontmatter")]
    Frontmatter,
    #[error("yaml error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("invalid id: {0}")]
    InvalidId(String),
}

/// Generate a non-sequential 8-char ID from the unambiguous alphabet
/// (YouTube/NanoID-style).
pub fn generate_id() -> String {
    let alphabet: Vec<char> = ID_ALPHABET.chars().collect();
    nanoid::nanoid!(ID_LEN, &alphabet)
}

/// Return `true` if `id` has the expected length and only uses the allowed
/// alphabet.
pub fn is_valid_id(id: &str) -> bool {
    id.len() == ID_LEN && id.chars().all(|c| ID_ALPHABET.contains(c))
}

/// The skeleton Markdown body written for a newly created tag.
pub fn skeleton_body() -> String {
    "\
## Description

<!-- young/mature plant, in-vitro individual, multi-plant container, etc. -->

## Log

- YYYY-MM-DD — created
"
    .to_string()
}

impl Tag {
    /// Create a new tag with the given metadata and the default skeleton body.
    pub fn new(meta: TagMeta) -> Tag {
        Tag {
            meta,
            body: skeleton_body(),
        }
    }

    /// Build the URL written to the NFC tag, e.g. `https://chnm.pa/7Gk2PQ8X`.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_valid_shape() {
        let id = generate_id();
        assert!(is_valid_id(&id), "generated id {id} should be valid");
        assert_eq!(id.len(), ID_LEN);
    }

    #[test]
    fn rejects_ambiguous_and_wrong_length() {
        assert!(!is_valid_id("short"));
        assert!(!is_valid_id("ABCDEFGI")); // contains excluded 'I'
        assert!(!is_valid_id("ABCDEFG0")); // contains excluded '0'
        assert!(!is_valid_id("abcdefgh")); // lowercase not in alphabet
    }

    #[test]
    fn url_is_well_formed() {
        let tag = Tag::new(TagMeta {
            id: "23456789".to_string(),
            linked_tags: vec![],
            description: None,
            species_inat_id: None,
            species_name: None,
            observation_inat_id: None,
            collection: None,
            for_sale: false,
            price: None,
        });
        assert_eq!(tag.url("https://chnm.pa/"), "https://chnm.pa/23456789");
        assert_eq!(tag.url("https://chnm.pa"), "https://chnm.pa/23456789");
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
                collection: None,
                for_sale: false,
                price: None,
            },
            body: "## Log\n- 2026-01-01 — created\n".into(),
        };
        let md = tag.to_markdown().unwrap();
        let back = Tag::from_markdown(&md).unwrap();
        assert_eq!(back, tag);
        assert_eq!(back.meta.species_inat_id, Some(50310));
    }

    #[test]
    fn rejects_missing_frontmatter() {
        assert!(matches!(
            Tag::from_markdown("no frontmatter here"),
            Err(TagError::Frontmatter)
        ));
    }
}
