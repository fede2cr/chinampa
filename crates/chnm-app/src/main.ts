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
  bookReferences: BookReference[];
  body: string;
  sha: string;
  url: string;
}

/** A bibliographic reference (book) cited by a tag. */
interface BookReference {
  book: string;
  authors: string[];
  isbn: string | null;
  page: number | null;
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
  bookReferences: BookReference[];
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

const tagsEl = $<HTMLUListElement>("tags");

// Settings persist to an app-private store file (sandboxed on Android), which
// keeps the GitHub token out of localStorage.
let store: Store;
let settings: RepoSettings = { ...DEFAULT_SETTINGS };

// The tag currently open in the editor (its id + blob sha), if any.
let editingId: string | null = null;
let editingSha: string | null = null;

// The single-window app swaps between these screens. Each has a matching
// `#screen-<id>` section in index.html and a `#status-<id>` status line; the
// write overlay ("write") floats above whichever screen is active.
type ScreenId = "list" | "detail" | "edit" | "create" | "config" | "write";
const SCREENS: Exclude<ScreenId, "write">[] = [
  "list",
  "detail",
  "edit",
  "create",
  "config",
];
let currentScreen: ScreenId = "list";

/** Swap the visible screen (the write overlay is toggled separately). */
function showScreen(id: Exclude<ScreenId, "write">): void {
  for (const s of SCREENS) {
    $(`screen-${s}`).classList.toggle("hidden", s !== id);
  }
  currentScreen = id;
  window.scrollTo(0, 0);
}

/**
 * Write a status message to a screen's status line. Defaults to the active
 * screen; pass `screen` explicitly for the floating write overlay.
 */
function setStatus(
  message: string,
  kind: "info" | "error" | "ok" = "info",
  screen: ScreenId = currentScreen,
): void {
  const el = document.getElementById(`status-${screen}`);
  if (!el) return;
  el.textContent = message;
  el.dataset.kind = kind;
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

/**
 * Serialize book references to one line per entry for the editor textarea:
 * `Book title | Author 1, Author 2 | ISBN | Page`.
 */
function formatReferences(refs: BookReference[]): string {
  return refs
    .map((r) =>
      [r.book, r.authors.join(", "), r.isbn ?? "", r.page ?? ""]
        .map((s) => String(s).trim())
        .join(" | "),
    )
    .join("\n");
}

/** Parse the editor textarea back into structured book references. */
function parseReferences(value: string): BookReference[] {
  return value
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0)
    .map((line) => {
      const [book = "", authors = "", isbn = "", page = ""] = line
        .split("|")
        .map((s) => s.trim());
      const pageNum = Number(page);
      return {
        book,
        authors: authors
          .split(",")
          .map((s) => s.trim())
          .filter((s) => s.length > 0),
        isbn: isbn === "" ? null : isbn,
        page: page !== "" && Number.isFinite(pageNum) ? pageNum : null,
      };
    })
    .filter((r) => r.book.length > 0);
}

// ---------------------------------------------------------------------------
// NFC writing
//
// Android only routes a scanned tag to this app while a scan/write session is
// active (foreground dispatch). If no session is live when a tag is tapped, the
// OS hands it to the default tag handler — which is why another app can pop up
// instead. The batch loop below keeps a `write()` session alive continuously so
// tag after tag can be written until the user presses Done.
// ---------------------------------------------------------------------------

let batchWriting = false;
let batchCount = 0;
// Resolver that lets the Done button interrupt the pending `write()` wait.
let cancelBatch: (() => void) | null = null;

function updateWriteCount(): void {
  $("write-count").textContent = String(batchCount);
}

/**
 * Open the write overlay and keep writing the same URL to any tag presented,
 * counting successes, until the user presses Done.
 */
async function startBatchWrite(url: string): Promise<void> {
  if (batchWriting) return;
  batchCount = 0;
  updateWriteCount();
  $("write-url").textContent = url;
  $("write-overlay").classList.remove("hidden");
  setStatus("Checking NFC…", "info", "write");

  let available: boolean;
  try {
    available = await isAvailable();
  } catch (err) {
    console.error("[nfc] isAvailable() threw:", err);
    setStatus(`NFC check failed: ${String(err)}`, "error", "write");
    return;
  }
  console.info("[nfc] isAvailable() =>", available);
  if (!available) {
    setStatus(
      "NFC is off or unsupported. Turn NFC on in Android settings, then reopen this dialog.",
      "error",
      "write",
    );
    return;
  }

  batchWriting = true;
  setStatus("Hold a blank NFC tag against the phone…", "info", "write");

  while (batchWriting) {
    const cancelled = new Promise<"cancel">((resolve) => {
      cancelBatch = () => resolve("cancel");
    });
    const started = Date.now();
    const writePromise = write([uriRecord(url)], {
      kind: { type: "ndef" },
      message: "Hold a blank NFC tag near the phone",
      successMessage: "Tag written",
    });
    // If the user cancels mid-wait we stop awaiting this promise; swallow any
    // later rejection so it doesn't surface as an unhandled rejection.
    writePromise.catch(() => {});
    try {
      const outcome = await Promise.race([
        writePromise.then(() => "ok" as const),
        cancelled,
      ]);
      if (outcome === "cancel") break;
      batchCount += 1;
      updateWriteCount();
      console.info(`[nfc] wrote tag #${batchCount}`);
      setStatus(
        `Written ${batchCount} tag(s). Present another tag, or press Done.`,
        "ok",
        "write",
      );
    } catch (err) {
      console.error("[nfc] write() failed:", err);
      if (!batchWriting) break;
      // A near-instant rejection is a hard failure (NFC disabled, permission,
      // unsupported tag) rather than "no tag was tapped" — stop to avoid a tight
      // retry loop and let the user read the error.
      if (Date.now() - started < 750) {
        setStatus(`Write failed: ${String(err)}`, "error", "write");
        batchWriting = false;
        break;
      }
      setStatus(
        `That tag failed: ${String(err)}. Try another tag, or press Done.`,
        "error",
        "write",
      );
    } finally {
      cancelBatch = null;
    }
  }
}

/** Close the write overlay and interrupt any pending write wait. */
function stopBatchWrite(): void {
  batchWriting = false;
  if (cancelBatch) cancelBatch();
  $("write-overlay").classList.add("hidden");
}

// The tag currently shown on the detail screen. `currentDetail` is null when we
// could only load the list summary (e.g. offline): writing needs just the URL,
// but editing needs the full detail (for its blob sha).
let currentTag: TagSummary | null = null;
let currentDetail: TagDetail | null = null;

/** Append a term/description row to a detail list, skipping empty values. */
function addDetailRow(
  dl: HTMLElement,
  label: string,
  value: string | null | undefined,
): void {
  if (value === null || value === undefined || value === "") return;
  const dt = document.createElement("dt");
  dt.textContent = label;
  const dd = document.createElement("dd");
  dd.textContent = value;
  dl.append(dt, dd);
}

/** Render a tag's full detail into the detail screen. */
function renderDetail(detail: TagDetail): void {
  const dl = $("detail-body");
  dl.replaceChildren();
  addDetailRow(dl, "Species", detail.speciesName);
  addDetailRow(
    dl,
    "Species iNat ID",
    detail.speciesInatId != null ? String(detail.speciesInatId) : null,
  );
  addDetailRow(
    dl,
    "Observation iNat ID",
    detail.observationInatId != null ? String(detail.observationInatId) : null,
  );
  addDetailRow(dl, "Collection", detail.collection);
  addDetailRow(dl, "Count", detail.count != null ? String(detail.count) : null);
  addDetailRow(dl, "Price", detail.price != null ? String(detail.price) : null);
  addDetailRow(dl, "For sale", detail.forSale ? "Yes" : null);
  addDetailRow(dl, "Description", detail.description);
  addDetailRow(
    dl,
    "Linked tags",
    detail.linkedTags.length ? detail.linkedTags.join(", ") : null,
  );
  addDetailRow(
    dl,
    "References",
    detail.bookReferences.length
      ? detail.bookReferences.map((r) => r.book).join("; ")
      : null,
  );
  addDetailRow(dl, "URL", detail.url);
  if (detail.body.trim().length > 0) {
    const dt = document.createElement("dt");
    dt.textContent = "Body";
    const dd = document.createElement("dd");
    const pre = document.createElement("pre");
    pre.className = "detail-body-text";
    pre.textContent = detail.body;
    dd.append(pre);
    dl.append(dt, dd);
  }
}

/** Open the detail screen for a tag, loading its full contents from GitHub. */
async function openDetail(tag: TagSummary): Promise<void> {
  await persistSettings();
  currentTag = tag;
  currentDetail = null;
  $("detail-id").textContent = tag.id;
  $("detail-body").replaceChildren();
  showScreen("detail");
  setStatus(`Loading ${tag.id}…`, "info", "detail");
  try {
    const detail = await invoke<TagDetail>("get_tag", { settings, id: tag.id });
    currentDetail = detail;
    renderDetail(detail);
    setStatus("", "info", "detail");
  } catch (err) {
    // Fall back to the list summary so Write to NFC still works offline; only
    // editing is unavailable without the full detail (and its sha).
    const dl = $("detail-body");
    addDetailRow(dl, "Species", tag.speciesName);
    addDetailRow(dl, "Description", tag.description);
    addDetailRow(dl, "Collection", tag.collection);
    addDetailRow(dl, "URL", tag.url);
    setStatus(`Couldn't load full details: ${String(err)}`, "error", "detail");
  }
}

/** Fill the edit form from a tag detail (also stashing its id + sha). */
function fillEditForm(detail: TagDetail): void {
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
  $<HTMLTextAreaElement>("edit-references").value = formatReferences(
    detail.bookReferences,
  );
  $<HTMLTextAreaElement>("edit-body").value = detail.body;
}

/** Move from the detail screen into the editor for the loaded tag. */
function openEdit(): void {
  if (!currentDetail) {
    setStatus(
      "Full details aren't loaded yet — reconnect and reopen the tag to edit.",
      "error",
      "detail",
    );
    return;
  }
  fillEditForm(currentDetail);
  showScreen("edit");
}

async function saveEdit(): Promise<void> {
  if (!editingId || !editingSha) return;
  await persistSettings();
  if (!settings.token) {
    setStatus("Add a GitHub token in Config to save changes.", "error", "edit");
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
    bookReferences: parseReferences(
      $<HTMLTextAreaElement>("edit-references").value,
    ),
    body: $<HTMLTextAreaElement>("edit-body").value,
  };
  setStatus(`Saving ${editingId}…`, "info", "edit");
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
      fillEditForm(outcome.latest);
      setStatus(
        `${outcome.latest.id} changed on GitHub since you opened it. ` +
          `The editor now shows the latest version — re-apply your changes and save again.`,
        "error",
        "edit",
      );
      return;
    }
    setStatus(`Updated ${outcome.tag.id}.`, "ok", "list");
    showScreen("list");
    await loadTags();
  } catch (err) {
    setStatus(`Failed to save: ${String(err)}`, "error", "edit");
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
    setStatus("Add a GitHub token in Config to create tags.", "error");
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
    const writeAfter = $<HTMLInputElement>("new-write-nfc").checked;
    setStatus(`Created ${tag.id}.`, "ok", "list");
    showScreen("list");
    await loadTags();
    if (writeAfter) {
      void startBatchWrite(tag.url);
    }
  } catch (err) {
    setStatus(`Failed to create tag: ${String(err)}`, "error");
  }
}

function renderTags(tags: TagSummary[]): void {
  tagsEl.replaceChildren();
  if (tags.length === 0) {
    setStatus("No tags found in the repository.", "info", "list");
    return;
  }
  for (const tag of tags) {
    const li = document.createElement("li");

    // The whole row is a button that opens the tag's detail screen.
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "tag-button";

    const title = document.createElement("span");
    title.className = "tag-id";
    title.textContent = tag.id;

    const sub = document.createElement("span");
    sub.className = "tag-sub";
    sub.textContent = tag.speciesName ?? tag.description ?? tag.url;

    btn.append(title, sub);
    btn.addEventListener("click", () => void openDetail(tag));
    li.append(btn);
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

/** Reset the create form to its defaults before showing the create screen. */
function clearCreateForm(): void {
  setTextInput("new-species", "");
  setTextInput("new-species-inat", "");
  setTextInput("new-collection", "");
  setTextInput("new-observation", "");
  setTextInput("new-description", "");
  setTextInput("new-count", "");
  setTextInput("new-price", "");
  $<HTMLInputElement>("new-for-sale").checked = false;
  $<HTMLInputElement>("new-write-nfc").checked = true;
  setStatus("", "info", "create");
}

/** Persist the Config screen's settings and return to the list. */
async function saveConfig(): Promise<void> {
  await persistSettings();
  setStatus("Settings saved.", "ok", "list");
  showScreen("list");
  await loadTags();
}

async function init(): Promise<void> {
  store = await load(STORE_FILE, { autoSave: false });
  const saved = await store.get<RepoSettings>(SETTINGS_KEY);
  settings = { ...DEFAULT_SETTINGS, ...(saved ?? {}) };
  fillForm(settings);

  // List screen.
  $("reload").addEventListener("click", () => void loadTags());
  $("open-config").addEventListener("click", () => {
    fillForm(settings);
    showScreen("config");
  });
  $("open-create").addEventListener("click", () => {
    clearCreateForm();
    showScreen("create");
  });

  // Detail screen.
  $("detail-back").addEventListener("click", () => showScreen("list"));
  $("detail-edit").addEventListener("click", () => openEdit());
  $("detail-write").addEventListener("click", () => {
    const url = currentDetail?.url ?? currentTag?.url;
    if (url) void startBatchWrite(url);
  });

  // Edit screen.
  $("edit-save").addEventListener("click", () => void saveEdit());
  $("edit-cancel").addEventListener("click", () => showScreen("detail"));

  // Create screen.
  $("create").addEventListener("click", () => void createTag());
  $("lookup").addEventListener("click", () => void lookupObservation());
  $("create-cancel").addEventListener("click", () => showScreen("list"));

  // Config screen.
  $("config-save").addEventListener("click", () => void saveConfig());
  $("config-back").addEventListener("click", () => showScreen("list"));
  $("load-config").addEventListener("click", () => void loadRemoteConfig(true));

  // Write overlay.
  $("write-done").addEventListener("click", () => stopBatchWrite());

  // On first run (no saved settings) seed the config from the repo's chnm.toml.
  if (!saved) {
    await loadRemoteConfig(false);
  }
  void loadTags();
}

void init();
