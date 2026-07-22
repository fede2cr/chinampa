import { invoke } from "@tauri-apps/api/core";
import { isAvailable, write, uriRecord } from "@tauri-apps/plugin-nfc";
import { load, type Store } from "@tauri-apps/plugin-store";

/** Repository + display settings sent to the Rust commands. */
interface RepoSettings {
  owner: string;
  repo: string;
  branch: string;
  tagsPath: string;
  domain: string;
  token?: string;
}

/** A tag summary returned by the Rust commands. */
interface TagSummary {
  id: string;
  speciesName: string | null;
  description: string | null;
  collection: string | null;
  forSale: boolean;
  url: string;
}

/** Fields for creating a new tag (mirrors the Rust `NewTagInput`). */
interface NewTagInput {
  speciesName?: string;
  speciesInatId?: number;
  observationInatId?: number;
  description?: string;
  collection?: string;
  forSale: boolean;
  price?: number;
  count?: number;
}

/** The species resolved from an iNaturalist observation. */
interface ObservationSpecies {
  speciesInatId: number;
  speciesName: string;
}

/** Repository + display config read from the repo's `chnm.toml`. */
interface RemoteConfig {
  domain: string | null;
  owner: string | null;
  repo: string | null;
  branch: string | null;
  tagsPath: string | null;
}

/** A tag's full detail plus the blob sha needed to update it. */
interface TagDetail {
  id: string;
  linkedTags: string[];
  speciesName: string | null;
  speciesInatId: number | null;
  observationInatId: number | null;
  description: string | null;
  collection: string | null;
  forSale: boolean;
  price: number | null;
  count: number | null;
  body: string;
  sha: string;
  url: string;
}

/** Editable fields sent to the Rust `update_tag` command. */
interface TagEdit {
  linkedTags: string[];
  speciesName?: string;
  speciesInatId?: number;
  observationInatId?: number;
  description?: string;
  collection?: string;
  forSale: boolean;
  price?: number;
  count?: number;
  body: string;
}

/** Result of `update_tag`: either a saved summary or a stale-sha conflict. */
type UpdateOutcome =
  | { status: "ok"; tag: TagSummary }
  | { status: "conflict"; latest: TagDetail };

const STORE_FILE = "settings.json";
const SETTINGS_KEY = "settings";
// Last successfully loaded tag list, kept for offline display.
const CACHE_KEY = "cachedTags";

const DEFAULT_SETTINGS: RepoSettings = {
  owner: "fede2",
  repo: "chinampa",
  branch: "main",
  tagsPath: "tags",
  domain: "chinampa.co.cr",
};

const $ = <T extends HTMLElement>(id: string): T => {
  const el = document.getElementById(id);
  if (!el) throw new Error(`missing element #${id}`);
  return el as T;
};

const statusEl = $("status");
const tagsEl = $<HTMLUListElement>("tags");

// Settings persist to an app-private store file (sandboxed on Android), which
// keeps the GitHub token out of localStorage.
let store: Store;
let settings: RepoSettings = { ...DEFAULT_SETTINGS };

// The tag currently open in the editor (its id + blob sha), if any.
let editingId: string | null = null;
let editingSha: string | null = null;

function setStatus(message: string, kind: "info" | "error" | "ok" = "info"): void {
  statusEl.textContent = message;
  statusEl.dataset.kind = kind;
}

function fillForm(s: RepoSettings): void {
  $<HTMLInputElement>("owner").value = s.owner;
  $<HTMLInputElement>("repo").value = s.repo;
  $<HTMLInputElement>("branch").value = s.branch;
  $<HTMLInputElement>("domain").value = s.domain;
  $<HTMLInputElement>("token").value = s.token ?? "";
}

function readSettingsFromForm(): RepoSettings {
  const token = $<HTMLInputElement>("token").value.trim();
  return {
    owner: $<HTMLInputElement>("owner").value.trim() || DEFAULT_SETTINGS.owner,
    repo: $<HTMLInputElement>("repo").value.trim() || DEFAULT_SETTINGS.repo,
    branch: $<HTMLInputElement>("branch").value.trim() || DEFAULT_SETTINGS.branch,
    tagsPath: settings.tagsPath || DEFAULT_SETTINGS.tagsPath,
    domain: $<HTMLInputElement>("domain").value.trim() || DEFAULT_SETTINGS.domain,
    token: token || undefined,
  };
}

async function persistSettings(): Promise<void> {
  settings = readSettingsFromForm();
  await store.set(SETTINGS_KEY, settings);
  await store.save();
}

/** Apply non-empty fields from a repo `chnm.toml` onto the settings + form. */
function applyRemoteConfig(cfg: RemoteConfig): void {
  settings = readSettingsFromForm();
  if (cfg.owner) settings.owner = cfg.owner;
  if (cfg.repo) settings.repo = cfg.repo;
  if (cfg.branch) settings.branch = cfg.branch;
  if (cfg.domain) settings.domain = cfg.domain;
  if (cfg.tagsPath) settings.tagsPath = cfg.tagsPath;
  fillForm(settings);
}

/**
 * Fetch `chnm.toml` from the configured repo and seed the settings from it.
 * The token is never overwritten (it isn't stored in the repo).
 */
async function loadRemoteConfig(announce: boolean): Promise<void> {
  const current = readSettingsFromForm();
  if (announce) setStatus("Reading chnm.toml from GitHub…", "info");
  try {
    const cfg = await invoke<RemoteConfig>("fetch_config", {
      owner: current.owner,
      repo: current.repo,
      branch: current.branch,
    });
    applyRemoteConfig(cfg);
    await persistSettings();
    if (announce) setStatus("Loaded settings from chnm.toml.", "ok");
  } catch (err) {
    if (announce) setStatus(`Could not read chnm.toml: ${String(err)}`, "error");
  }
}

/** Read a numeric input, returning undefined when empty or invalid. */
function numberValue(id: string): number | undefined {
  const raw = $<HTMLInputElement>(id).value.trim();
  if (raw === "") return undefined;
  const n = Number(raw);
  return Number.isFinite(n) ? n : undefined;
}

function textValue(id: string): string | undefined {
  const raw = $<HTMLInputElement>(id).value.trim();
  return raw === "" ? undefined : raw;
}

function setTextInput(id: string, value: string | null): void {
  $<HTMLInputElement>(id).value = value ?? "";
}

function setNumberInput(id: string, value: number | null): void {
  $<HTMLInputElement>(id).value =
    value === null || value === undefined ? "" : String(value);
}

function parseLinked(value: string): string[] {
  return value
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

/** Write a tag's URL to a physical NFC chip as an NDEF URI record. */
async function writeTag(tag: TagSummary): Promise<void> {
  if (!(await isAvailable())) {
    setStatus("This device has no available NFC hardware.", "error");
    return;
  }
  setStatus(`Tap a blank tag to write ${tag.id}…`, "info");
  try {
    await write([uriRecord(tag.url)], {
      kind: { type: "ndef" },
      message: "Hold a blank NFC tag near the phone",
      successMessage: "Tag written",
    });
    setStatus(`Wrote ${tag.url} to the tag.`, "ok");
  } catch (err) {
    setStatus(`Failed to write tag: ${String(err)}`, "error");
  }
}

async function openEditor(tag: TagSummary): Promise<void> {
  await persistSettings();
  setStatus(`Loading ${tag.id}…`, "info");
  try {
    const detail = await invoke<TagDetail>("get_tag", { settings, id: tag.id });
    editingId = detail.id;
    editingSha = detail.sha;
    $("edit-id").textContent = detail.id;
    setTextInput("edit-species", detail.speciesName);
    setNumberInput("edit-species-inat", detail.speciesInatId);
    setNumberInput("edit-observation", detail.observationInatId);
    setTextInput("edit-collection", detail.collection);
    setNumberInput("edit-count", detail.count);
    setNumberInput("edit-price", detail.price);
    $<HTMLInputElement>("edit-for-sale").checked = detail.forSale;
    setTextInput("edit-description", detail.description);
    $<HTMLInputElement>("edit-linked").value = detail.linkedTags.join(", ");
    $<HTMLTextAreaElement>("edit-body").value = detail.body;
    $("editor").classList.remove("hidden");
    $("editor").scrollIntoView({ behavior: "smooth", block: "start" });
    setStatus(`Editing ${detail.id}.`, "info");
  } catch (err) {
    setStatus(`Failed to load tag: ${String(err)}`, "error");
  }
}

function closeEditor(): void {
  editingId = null;
  editingSha = null;
  $("editor").classList.add("hidden");
}

/** Overwrite the open editor's fields from a fresh server-side detail. */
function fillEditor(detail: TagDetail): void {
  editingId = detail.id;
  editingSha = detail.sha;
  $("edit-id").textContent = detail.id;
  setTextInput("edit-species", detail.speciesName);
  setNumberInput("edit-species-inat", detail.speciesInatId);
  setNumberInput("edit-observation", detail.observationInatId);
  setTextInput("edit-collection", detail.collection);
  setNumberInput("edit-count", detail.count);
  setNumberInput("edit-price", detail.price);
  $<HTMLInputElement>("edit-for-sale").checked = detail.forSale;
  setTextInput("edit-description", detail.description);
  $<HTMLInputElement>("edit-linked").value = detail.linkedTags.join(", ");
  $<HTMLTextAreaElement>("edit-body").value = detail.body;
}

async function saveEdit(): Promise<void> {
  if (!editingId || !editingSha) return;
  await persistSettings();
  if (!settings.token) {
    setStatus("Add a GitHub token in settings to save changes.", "error");
    return;
  }
  const edit: TagEdit = {
    linkedTags: parseLinked($<HTMLInputElement>("edit-linked").value),
    speciesName: textValue("edit-species"),
    speciesInatId: numberValue("edit-species-inat"),
    observationInatId: numberValue("edit-observation"),
    description: textValue("edit-description"),
    collection: textValue("edit-collection"),
    forSale: $<HTMLInputElement>("edit-for-sale").checked,
    price: numberValue("edit-price"),
    count: numberValue("edit-count"),
    body: $<HTMLTextAreaElement>("edit-body").value,
  };
  setStatus(`Saving ${editingId}…`, "info");
  try {
    const outcome = await invoke<UpdateOutcome>("update_tag", {
      settings,
      id: editingId,
      sha: editingSha,
      edit,
    });
    if (outcome.status === "conflict") {
      // Someone committed first: reload the editor with the latest server
      // state (and its new sha) so the user can reconcile and retry.
      fillEditor(outcome.latest);
      setStatus(
        `${outcome.latest.id} changed on GitHub since you opened it. ` +
          `The editor now shows the latest version — re-apply your changes and save again.`,
        "error",
      );
      return;
    }
    setStatus(`Updated ${outcome.tag.id}.`, "ok");
    closeEditor();
    await loadTags();
  } catch (err) {
    setStatus(`Failed to save: ${String(err)}`, "error");
  }
}

async function cloneTag(parent: TagSummary): Promise<void> {
  await persistSettings();
  if (!settings.token) {
    setStatus("Add a GitHub token in settings to clone tags.", "error");
    return;
  }
  setStatus(`Cloning ${parent.id}…`, "info");
  try {
    const tag = await invoke<TagSummary>("clone_tag", {
      settings,
      parentId: parent.id,
    });
    setStatus(`Created ${tag.id} linked to ${parent.id}.`, "ok");
    await writeTag(tag);
    await loadTags();
  } catch (err) {
    setStatus(`Failed to clone tag: ${String(err)}`, "error");
  }
}

/** Look up an iNaturalist observation and fill the species fields. */
async function lookupObservation(): Promise<void> {
  const obs = numberValue("new-observation");
  if (obs === undefined) {
    setStatus("Enter an observation iNat ID first.", "error");
    return;
  }
  setStatus(`Looking up observation ${obs}…`, "info");
  try {
    const res = await invoke<ObservationSpecies>("resolve_observation", {
      observationInatId: obs,
    });
    $<HTMLInputElement>("new-species").value = res.speciesName;
    $<HTMLInputElement>("new-species-inat").value = String(res.speciesInatId);
    setStatus(`Resolved species: ${res.speciesName}.`, "ok");
  } catch (err) {
    setStatus(`Lookup failed: ${String(err)}`, "error");
  }
}

async function createTag(): Promise<void> {
  await persistSettings();
  if (!settings.token) {
    setStatus("Add a GitHub token in settings to create tags.", "error");
    return;
  }
  const input: NewTagInput = {
    speciesName: textValue("new-species"),
    speciesInatId: numberValue("new-species-inat"),
    observationInatId: numberValue("new-observation"),
    description: textValue("new-description"),
    collection: textValue("new-collection"),
    forSale: $<HTMLInputElement>("new-for-sale").checked,
    price: numberValue("new-price"),
    count: numberValue("new-count"),
  };
  setStatus("Creating tag…", "info");
  try {
    const tag = await invoke<TagSummary>("create_tag", { settings, input });
    setStatus(`Created ${tag.id}.`, "ok");
    if ($<HTMLInputElement>("new-write-nfc").checked) {
      await writeTag(tag);
    }
    await loadTags();
  } catch (err) {
    setStatus(`Failed to create tag: ${String(err)}`, "error");
  }
}

function renderTags(tags: TagSummary[]): void {
  tagsEl.replaceChildren();
  if (tags.length === 0) {
    setStatus("No tags found in the repository.", "info");
    return;
  }
  for (const tag of tags) {
    const li = document.createElement("li");
    li.className = "tag";

    const info = document.createElement("div");
    info.className = "tag-info";

    const title = document.createElement("span");
    title.className = "tag-id";
    title.textContent = tag.id;

    const sub = document.createElement("span");
    sub.className = "tag-sub";
    sub.textContent = tag.speciesName ?? tag.description ?? tag.url;

    info.append(title, sub);

    const actions = document.createElement("div");
    actions.className = "tag-actions";

    const editBtn = document.createElement("button");
    editBtn.type = "button";
    editBtn.className = "ghost";
    editBtn.textContent = "Edit";
    editBtn.addEventListener("click", () => void openEditor(tag));

    const cloneBtn = document.createElement("button");
    cloneBtn.type = "button";
    cloneBtn.className = "ghost";
    cloneBtn.textContent = "Clone";
    cloneBtn.addEventListener("click", () => void cloneTag(tag));

    const writeBtn = document.createElement("button");
    writeBtn.type = "button";
    writeBtn.className = "write";
    writeBtn.textContent = "Write NFC";
    writeBtn.addEventListener("click", () => void writeTag(tag));

    actions.append(editBtn, cloneBtn, writeBtn);
    li.append(info, actions);
    tagsEl.append(li);
  }
}

async function loadTags(): Promise<void> {
  await persistSettings();
  setStatus("Loading tags from GitHub…", "info");
  try {
    const tags = await invoke<TagSummary[]>("list_tags", { settings });
    renderTags(tags);
    // Cache the last good result so the list still shows when offline.
    await store.set(CACHE_KEY, tags);
    await store.save();
    if (tags.length > 0) {
      setStatus(`Loaded ${tags.length} tag(s).`, "ok");
    }
  } catch (err) {
    // Network/API failure: fall back to the cached list if we have one.
    const cached = (await store.get<TagSummary[]>(CACHE_KEY)) ?? [];
    if (cached.length > 0) {
      renderTags(cached);
      setStatus(
        `Offline — showing ${cached.length} cached tag(s). (${String(err)})`,
        "error",
      );
    } else {
      setStatus(`Failed to load tags: ${String(err)}`, "error");
    }
  }
}

async function init(): Promise<void> {
  store = await load(STORE_FILE, { autoSave: false });
  const saved = await store.get<RepoSettings>(SETTINGS_KEY);
  settings = { ...DEFAULT_SETTINGS, ...(saved ?? {}) };
  fillForm(settings);
  $("reload").addEventListener("click", () => void loadTags());
  $("create").addEventListener("click", () => void createTag());
  $("lookup").addEventListener("click", () => void lookupObservation());
  $("edit-save").addEventListener("click", () => void saveEdit());
  $("edit-cancel").addEventListener("click", () => closeEditor());
  $("load-config").addEventListener("click", () => void loadRemoteConfig(true));
  // On first run (no saved settings) seed the config from the repo's chnm.toml.
  if (!saved) {
    await loadRemoteConfig(false);
  }
  void loadTags();
}

void init();
