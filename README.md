# Chinampa

Manage your plant in-vitro lifecyle easily.

## What is Chinampa

Chinampa is a software for managing an in-vitro plant nursery, and uses NFC tags follow-up the entire lifecycle of the plant: from the preparation of it's in-vitro media, through it propagation, growing, and acclimation, as well as some optional steps like explant introduction or seed germination.

## Role of NFC

NFC tags are used instead of traditional garden tags, with advantages like the speed of writing each tag, the reusability, and the "permanence" of the data in the tag (as opposed to the ink of a garden tag being bleached or covered by algae).

## Operations on NFC

- Create: A new small unique 8-char alphanumeric ID `[a-zA-Z0-9]`, non-sequential, similar to a YouTube video id. Combined with a short internet domain like `https://chnm.pa/`, the ID is appended to create a URL that is written to the tag. When read by a phone, it takes the user or nursery operator to a web page with the history of that particular plant.

- Clone: When plants come out of in-vitro multiplication (one flask with many plants) and into individual pots, the tags are cloned with NFC equipment (such as a cellphone with third-party software).

## chnm.pa website design

The content is authored as Markdown files (one per tag) and published via CI to GitHub Pages, allowing the nursery to have a complete website without server infrastructure. The site itself is a Leptos (Rust/WASM) app that renders each tag's history and pulls images from iNaturalist.

The `chnm` CLI tool takes care of creating new tags and writing a skeleton Markdown file; a third-party editor is then used to flesh the skeleton out into the plant's history.

### Skelethon fields

These are fields inside of the markdown

| Field | Description |
|-------|-------------|
| ID    | ID of the tag |
| Linked tags | Previous tag IDs associated with this tag. For example, the in-vitro mediums used during the plant propagation, or the mother-plant used as explant |
| Description | If this is a young or mature plant, and in-vitro individual, an in-vitro container with multiple plants for seeding, multiplication, rooting, aclimation, etc |
| Species | The species would be linked to Inaturalist species, so that if there is no observation in the next field, we can use pictures from the species to render the tag |
| Inaturalist observation | The link to a inaturalist observation with this plant. The photos from this ID would be used by the website to render the image links directly to inaturalist, so that it's easy to see the flower, fruits, plant size, etc |
| Log | Date-coded events that include the history from linked tags, but also the dates of when it was seeded, multiplied, etc., including the in-vitro media formulation, grow-regulators, concentrations, etc. |

## Architecture

- **`chnm` CLI** (Rust) — creates/clones tags, validates them, and exports them to JSON.
- **Leptos web app** (Rust/WASM) — renders each tag as a static, server-free page, fetching iNaturalist photos at runtime (observation photos when available, otherwise the species' default photo).
- **GitHub Actions** — on every push to `main`, processes the tag files, builds the site, and publishes it to GitHub Pages.

See [Development.md](Development.md) for the full build guide (project layout, code, and CI recipes).

## Suggestions / open questions

A few points worth deciding before/while implementing:

- **Tag file format:** the skeleton fields are stored as YAML frontmatter (machine-readable) plus a Markdown body (the human-edited Log). This keeps third-party editors working while making fields trivial to parse.
- **Species & observation linking:** store the numeric iNaturalist taxon ID and observation ID, not just URLs, so the web app can call the iNat API directly.
- **NFC URL vs. static hosting:** GitHub Pages can't rewrite `/<id>` server-side. Either use a custom domain (`chnm.pa`) with a redirect, write the hash form `…/#/t/<id>` directly to the tag, or use a `404.html` redirect shim. Pick one and document it.
- **ID scheme:** prefer a standard over a bespoke format. The tag stores only a short URL, so the NFC chip's capacity (e.g. 504 bytes on an NTAG215) is never the limit — the namespace is. An 8-char `[a-zA-Z0-9]` ID gives 62⁸ ≈ 2.18×10¹⁴ combinations, far beyond any nursery's needs. Recommended: **NanoID** (a de-facto standard for short, URL-safe IDs) with an unambiguous Crockford-style alphabet, or **ULID** if you want a formal spec with time-sortable IDs. See [Development.md](Development.md#9-nfc-tag-capacity--id-scheme-analysis) for the full capacity and collision analysis. The CLI still checks for an existing file before writing.
- **Clone semantics:** confirm whether a clone is an exact duplicate of the source URL/ID (as some NFC cloning apps do) or a brand-new ID that references the parent via `linked_tags`. The CLI currently assumes the latter.
