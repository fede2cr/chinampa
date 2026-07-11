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

## Usage

### Create a new tag

The `chnm new` command generates a fresh 8-char ID, writes a skeleton Markdown
file into the tags directory, and prints the NFC URL to write onto the chip:

```sh
chnm new \
  --species-name "Cattleya skinneri" \
  --species-inat-id 123456 \
  --description "In-vitro flask, multiplication stage" \
  --collection "Orquídeas" \
  --for-sale \
  --price 15000
```

All flags are optional — `chnm new` on its own creates an empty skeleton you
can flesh out later in any Markdown editor:

```sh
chnm new
```

The command prints the URL (e.g. `https://chinampa.co.cr/4MNP9RST`) that you
then write to the NFC tag.

Useful options:

- `--tags-dir <DIR>` — directory holding the tag files (default `tags`).
- `--config <FILE>` — config file (default `chnm.toml`); sets the `domain` and `currency`.
- `--base-url <URL>` — override the base URL written onto the tag.

To create a tag that links back to an existing parent (e.g. a plant moved from
an in-vitro flask into its own pot), use `clone`:

```sh
chnm clone 7GK2PQ8X
```
