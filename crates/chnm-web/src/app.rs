//! Leptos CSR app: a home index and a per-tag detail view that resolves
//! iNaturalist photos at runtime.
//!
//! Routing uses path routes served from the domain root
//! (https://chinampa.co.cr/); deep links work thanks to the `404.html` SPA
//! fallback created by the deploy workflow. To host under a subpath instead
//! (e.g. a GitHub Pages project site at `/chinampa/`), set `BASE`/`DATA_BASE`
//! below and `public_url` in `Trunk.toml` to that subpath.

use crate::inat::{observation_photos, species_photo, Photo};
use leptos::prelude::*;
use leptos_router::components::{Route, Router, Routes, A};
use leptos_router::hooks::use_params_map;
use leptos_router::path;
use serde::Deserialize;
use std::collections::HashSet;

/// Router base path. Empty = served from the domain root. For a subpath
/// deployment set this to e.g. `/chinampa` and `public_url` in `Trunk.toml`
/// to `/chinampa/`.
const BASE: &str = "";
/// Where `chnm export` writes the generated JSON (served at the site root/data).
const DATA_BASE: &str = "/data";

/// One tag's full detail document (mirrors `chnm-core::Tag` flattened).
#[derive(Clone, Deserialize)]
struct Tag {
    id: String,
    #[serde(default)]
    species_name: Option<String>,
    #[serde(default)]
    species_inat_id: Option<u64>,
    #[serde(default)]
    observation_inat_id: Option<u64>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    collection: Option<String>,
    #[serde(default)]
    for_sale: bool,
    #[serde(default)]
    price: Option<f64>,
    #[serde(default)]
    currency: Option<String>,
    body: String,
}

/// A lightweight index entry used by the home list view.
#[derive(Clone, Deserialize)]
struct TagSummary {
    id: String,
    #[serde(default)]
    species_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    for_sale: bool,
    #[serde(default)]
    price: Option<f64>,
    #[serde(default)]
    currency: Option<String>,
}

async fn load_tag(id: String) -> Option<Tag> {
    let url = format!("{DATA_BASE}/{id}.json");
    let resp = gloo_net::http::Request::get(&url).send().await.ok()?;
    resp.json::<Tag>().await.ok()
}

async fn load_index() -> Vec<TagSummary> {
    let url = format!("{DATA_BASE}/index.json");
    match gloo_net::http::Request::get(&url).send().await {
        Ok(resp) => resp.json::<Vec<TagSummary>>().await.unwrap_or_default(),
        Err(_) => vec![],
    }
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

/// Group an amount's integer digits in threes with a (non-breaking) space,
/// e.g. `15000.0` -> `15 000`, keeping any fractional part intact.
fn group_digits(price: f64) -> String {
    let s = format!("{price}");
    let (int_part, frac_part) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s.as_str(), None),
    };
    let neg = int_part.starts_with('-');
    let digits = int_part.trim_start_matches('-');
    let len = digits.len();
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            out.push('\u{00a0}'); // non-breaking space
        }
        out.push(ch);
    }
    if let Some(f) = frac_part {
        out.push('.');
        out.push_str(f);
    }
    out
}

/// Format a price using a small currency-symbol map, falling back to the code.
fn format_price(price: f64, currency: &str) -> String {
    let symbol = match currency {
        "CRC" => Some("\u{20a1}"), // ₡
        "USD" => Some("$"),
        "EUR" => Some("\u{20ac}"), // €
        "GBP" => Some("\u{a3}"),   // £
        _ => None,
    };
    let amount = group_digits(price);
    match symbol {
        Some(sym) => format!("{sym}{amount}"),
        None => format!("{amount} {currency}"),
    }
}

/// Render the human-edited Markdown log body to HTML, turning any bare
/// reference to a known tag id into a link to that tag.
///
/// The body comes from tag files committed to this repository (a trusted
/// source reviewed via pull request), so the rendered HTML is embedded
/// directly. If untrusted bodies ever become possible, sanitize here (e.g.
/// with `ammonia`) before display. Ids inside code spans/blocks are left
/// untouched so verbatim text stays verbatim.
fn markdown_to_html(src: &str, known: &HashSet<String>, current: &str) -> String {
    use pulldown_cmark::{html, CowStr, Event, LinkType, Options, Parser, Tag, TagEnd};

    // Emit a finished alphanumeric run as either a link (known id) or text.
    let flush_run = |run: &mut String, plain: &mut String, events: &mut Vec<Event>| {
        if !run.is_empty() && run.as_str() != current && known.contains(run.as_str()) {
            if !plain.is_empty() {
                events.push(Event::Text(CowStr::from(std::mem::take(plain))));
            }
            let id = std::mem::take(run);
            events.push(Event::Start(Tag::Link {
                link_type: LinkType::Inline,
                dest_url: CowStr::from(format!("/{id}")),
                title: CowStr::from(""),
                id: CowStr::from(""),
            }));
            events.push(Event::Text(CowStr::from(id)));
            events.push(Event::End(TagEnd::Link));
        } else {
            plain.push_str(run);
            run.clear();
        }
    };

    let mut events: Vec<Event> = Vec::new();
    for ev in Parser::new_ext(src, Options::all()) {
        match ev {
            Event::Text(text) => {
                let mut plain = String::new();
                let mut run = String::new();
                for ch in text.chars() {
                    if ch.is_ascii_alphanumeric() {
                        run.push(ch);
                    } else {
                        flush_run(&mut run, &mut plain, &mut events);
                        plain.push(ch);
                    }
                }
                flush_run(&mut run, &mut plain, &mut events);
                if !plain.is_empty() {
                    events.push(Event::Text(CowStr::from(plain)));
                }
            }
            other => events.push(other),
        }
    }
    let mut out = String::new();
    html::push_html(&mut out, events.into_iter());
    out
}

#[component]
pub fn App() -> impl IntoView {
    view! {
        <Router base=BASE>
            <main>
                <Routes fallback=|| view! { <p>"Not found"</p> }>
                    <Route path=path!("/") view=Home/>
                    <Route path=path!("/:id") view=TagView/>
                </Routes>
            </main>
        </Router>
    }
}

#[component]
fn Home() -> impl IntoView {
    let index = LocalResource::new(load_index);
    view! {
        <h1>"Chinampa"</h1>
        <p>"Scan a tag to view a plant's history, or pick one below."</p>
        <Suspense fallback=|| view! { <p>"Loading…"</p> }>
            {move || index.get().map(|entries| {
                let entries = entries.take();
                if entries.is_empty() {
                    view! { <p>"No tags yet."</p> }.into_any()
                } else {
                    view! {
                        <ul class="tag-list">
                            {entries.into_iter().map(|t| {
                                let title = t.species_name.clone().unwrap_or_else(|| t.id.clone());
                                let desc = t.description.clone().unwrap_or_default();
                                let currency = t.currency.clone().unwrap_or_default();
                                let price_tag = (t.for_sale && t.price.is_some()).then(|| {
                                    format_price(t.price.unwrap(), &currency)
                                });
                                view! {
                                    <li>
                                        <A href=format!("/{}", t.id)>{title}</A>
                                        <span class="meta">" "{desc}</span>
                                        {price_tag.map(|p| view! {
                                            <span class="price">{p}</span>
                                        })}
                                    </li>
                                }
                            }).collect_view()}
                        </ul>
                    }.into_any()
                }
            })}
        </Suspense>
    }
}

#[component]
fn TagView() -> impl IntoView {
    let params = use_params_map();
    let id = move || params.with(|p| p.get("id").unwrap_or_default());

    // Load the tag JSON, then its photos (which depend on the tag).
    let tag = LocalResource::new(move || load_tag(id()));
    let photos = LocalResource::new(move || async move {
        match tag.await {
            Some(t) => load_photos(t).await,
            None => vec![],
        }
    });
    // The index gives us the set of known ids so we can link references in the
    // body. It may resolve after the tag; the body re-renders when it does.
    let index = LocalResource::new(load_index);

    view! {
        <p><A href="/">"← All tags"</A></p>
        <Suspense fallback=|| view! { <p>"Loading…"</p> }>
            {move || tag.get().map(|maybe| match maybe.take() {
                None => view! { <p>"Tag not found"</p> }.into_any(),
                Some(t) => {
                    let title = t.species_name.clone().unwrap_or_else(|| t.id.clone());
                    let currency = t.currency.clone().unwrap_or_default();
                    let price_tag = (t.for_sale && t.price.is_some())
                        .then(|| format_price(t.price.unwrap(), &currency));
                    let collection = t.collection.clone();
                    let known: HashSet<String> = index
                        .get()
                        .map(|s| s.take().into_iter().map(|e| e.id).collect())
                        .unwrap_or_default();
                    let body_html = markdown_to_html(&t.body, &known, &t.id);
                    view! {
                        <article>
                            <h1>{title}</h1>
                            {t.description.clone().map(|d| view! { <p>{d}</p> })}
                            <dl class="meta">
                                {collection.map(|c| view! {
                                    <div><dt>"Collection"</dt><dd>{c}</dd></div>
                                })}
                                {price_tag.map(|p| view! {
                                    <div><dt>"For sale"</dt><dd><span class="price">{p}</span></dd></div>
                                })}
                            </dl>
                            <div class="gallery">
                                {move || photos.get().map(|ps| ps.take().into_iter().map(|p| view! {
                                    <figure>
                                        <img src=p.url alt="iNaturalist photo"/>
                                        <figcaption inner_html=p.attribution></figcaption>
                                    </figure>
                                }).collect_view())}
                            </div>
                            <div class="log" inner_html=body_html></div>
                        </article>
                    }.into_any()
                }
            })}
        </Suspense>
    }
}
