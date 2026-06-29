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

/// Render the human-edited Markdown log body to HTML.
///
/// The body comes from tag files committed to this repository (a trusted
/// source reviewed via pull request), so the rendered HTML is embedded
/// directly. If untrusted bodies ever become possible, sanitize here (e.g.
/// with `ammonia`) before display.
fn markdown_to_html(src: &str) -> String {
    use pulldown_cmark::{html, Options, Parser};
    let parser = Parser::new_ext(src, Options::all());
    let mut out = String::new();
    html::push_html(&mut out, parser);
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
                                view! {
                                    <li>
                                        <A href=format!("/{}", t.id)>{title}</A>
                                        <span class="meta">" "{desc}</span>
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

    view! {
        <p><A href="/">"← All tags"</A></p>
        <Suspense fallback=|| view! { <p>"Loading…"</p> }>
            {move || tag.get().map(|maybe| match maybe.take() {
                None => view! { <p>"Tag not found"</p> }.into_any(),
                Some(t) => {
                    let title = t.species_name.clone().unwrap_or_else(|| t.id.clone());
                    let body_html = markdown_to_html(&t.body);
                    view! {
                        <article>
                            <h1>{title}</h1>
                            {t.description.clone().map(|d| view! { <p>{d}</p> })}
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
