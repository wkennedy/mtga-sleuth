// MTGA Tracker frontend — single-page, no framework.
//
// Subscribes to /api/sse for live updates and lazily fetches each tab's data.

const tabs = ["live", "decks", "matches", "collection", "drafts", "events"];
const $ = (sel) => document.querySelector(sel);
const $$ = (sel) => Array.from(document.querySelectorAll(sel));

let activeTab = "live";

function showTab(name) {
  activeTab = name;
  for (const t of tabs) {
    $(`#tab-${t}`).hidden = t !== name;
    $(`button[data-tab="${t}"]`).classList.toggle("active", t === name);
  }
  switch (name) {
    case "live": refreshLive(); break;
    case "decks": loadDecks(); break;
    case "matches": loadMatches(); break;
    case "collection": loadCollection(); break;
    case "drafts": loadDrafts(); break;
    case "events": loadEvents(); break;
  }
}

$$(".tab").forEach((b) => b.addEventListener("click", () => showTab(b.dataset.tab)));

async function fetchJSON(url) {
  const r = await fetch(url);
  if (!r.ok) throw new Error(`${url} ${r.status}`);
  return r.json();
}

// ---- Live tab ----
async function refreshLive() {
  const live = await fetchJSON("/api/live").catch(() => null);
  const empty = $("#live-empty");
  const content = $("#live-content");
  if (!live) {
    empty.hidden = false;
    content.hidden = true;
    return;
  }
  empty.hidden = true;
  content.hidden = false;
  $("#m-id").textContent = live.match_id ?? "—";
  $("#m-opp").textContent = live.opponent_screen_name ?? "—";
  $("#m-turn").textContent = live.turn ?? "—";
  $("#m-life").textContent = `${live.player_life ?? "—"} / ${live.opponent_life ?? "—"}`;

  const lib = $("#library");
  lib.innerHTML = "";
  for (const c of live.library ?? []) {
    const li = document.createElement("li");
    li.innerHTML = `<span>${escapeHtml(c.name)}</span><span class="qty">${c.remaining}/${c.original}</span>`;
    lib.appendChild(li);
  }
  $("#lib-count").textContent = live.library?.length ? `(${live.library.length} unique)` : "";

  const rev = $("#revealed");
  rev.innerHTML = "";
  for (const c of live.opponent_revealed ?? []) {
    const li = document.createElement("li");
    li.innerHTML = `<span>${escapeHtml(c.name)}</span><span class="qty">×${c.remaining}</span>`;
    rev.appendChild(li);
  }
}

// ---- Decks tab ----
let decksCache = [];
async function loadDecks() {
  decksCache = await fetchJSON("/api/decks").catch(() => []);
  const ul = $("#decks-list");
  ul.innerHTML = "";
  if (decksCache.length === 0) {
    ul.innerHTML = '<li class="muted">No decks yet.</li>';
    return;
  }
  for (const d of decksCache) {
    const li = document.createElement("li");
    const manual = d.deck_id.startsWith("user-") ? '<span class="deck-tag">manual</span>' : "";
    li.innerHTML = `${escapeHtml(d.name)}${manual}<small>${d.format ?? "?"} · ${d.last_updated.slice(0, 10)}</small>`;
    li.addEventListener("click", () => loadDeckDetail(d.deck_id, li));
    ul.appendChild(li);
  }
}

async function loadDeckDetail(id, li) {
  $$("#decks-list li").forEach((x) => x.classList.remove("active"));
  li.classList.add("active");
  const d = await fetchJSON(`/api/decks/${id}`).catch(() => null);
  const det = $("#deck-detail");
  if (!d) { det.innerHTML = '<p class="muted">Failed to load.</p>'; return; }
  det.innerHTML = `
    <h3>${escapeHtml(d.name)}</h3>
    <p class="muted">${d.format ?? "Unknown format"}</p>
    ${renderDeckCharts(d.mainboard)}
    ${renderWildcardSummary(d)}
    <div class="deck-section">
      <h4>Mainboard (${d.mainboard.reduce((a, c) => a + c.quantity, 0)})</h4>
      ${d.mainboard.map(renderDeckCard).join("")}
    </div>
    ${d.sideboard.length ? `<div class="deck-section"><h4>Sideboard (${d.sideboard.reduce((a, c) => a + c.quantity, 0)})</h4>${d.sideboard.map(renderDeckCard).join("")}</div>` : ""}
  `;
}

function renderDeckCard(c) {
  const missingClass = c.missing > 0 ? " missing" : "";
  const ownedTxt = c.missing > 0
    ? `<span class="own bad">${c.owned}/${c.quantity}</span>`
    : `<span class="own good">${c.quantity}/${c.quantity}</span>`;
  return `<div class="deck-card${missingClass}">
    <span class="qty">${c.quantity}×</span>
    <span class="name">${escapeHtml(c.name)}</span>
    ${ownedTxt}
    <span class="cost">${renderManaCost(c.mana_cost)}</span>
  </div>`;
}

function renderWildcardSummary(d) {
  const wc = d.wildcards_needed || { common: 0, uncommon: 0, rare: 0, mythic: 0 };
  const total = (wc.common || 0) + (wc.uncommon || 0) + (wc.rare || 0) + (wc.mythic || 0);
  if (total === 0) {
    return `<div class="wc-summary complete">You own every non-basic card in this deck.</div>`;
  }
  const tile = (label, val, cls) => `<div class="wallet-tile ${cls}"><div class="label">${label}</div><div class="value">${val}</div></div>`;
  return `<div class="wc-summary">
    <div class="wc-summary-head">Missing ${d.total_missing} copies (${d.unique_missing} unique). Wildcards needed:</div>
    <div class="wallet-grid">
      ${tile("Common", wc.common || 0, "wc-common")}
      ${tile("Uncommon", wc.uncommon || 0, "wc-uncommon")}
      ${tile("Rare", wc.rare || 0, "wc-rare")}
      ${tile("Mythic", wc.mythic || 0, "wc-mythic")}
    </div>
  </div>`;
}

// ---- Decks tab: paste-to-analyze + save-as-deck ----
function syncSaveButton() {
  const hasText = $("#analyze-text").value.trim().length > 0;
  const hasName = $("#save-deck-name").value.trim().length > 0;
  $("#save-deck-btn").disabled = !(hasText && hasName);
}
$("#analyze-text").addEventListener("input", syncSaveButton);
$("#save-deck-name").addEventListener("input", syncSaveButton);

$("#analyze-btn").addEventListener("click", async () => {
  const text = $("#analyze-text").value;
  const btn = $("#analyze-btn"); const out = $("#analyze-result"); const detail = $("#analyze-detail");
  btn.disabled = true; out.className = "muted"; out.textContent = "Analyzing…"; detail.innerHTML = "";
  try {
    const r = await fetch("/api/decks/analyze", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ text }),
    });
    const data = await r.json();
    if (!r.ok) throw new Error(data.error || `HTTP ${r.status}`);
    const unmatchedBlock = data.unmatched_lines
      ? `<pre>Unmatched samples:\n${data.unmatched_samples.map(escapeHtml).join("\n")}</pre>`
      : "";
    out.className = "success";
    out.innerHTML = `Parsed ${data.matched_lines} lines, skipped ${data.unmatched_lines}.${unmatchedBlock}`;
    detail.innerHTML = `
      ${renderDeckCharts(data.mainboard)}
      ${renderWildcardSummary(data)}
      <div class="deck-section">
        <h4>Mainboard (${data.mainboard.reduce((a, c) => a + c.quantity, 0)})</h4>
        ${data.mainboard.map(renderDeckCard).join("")}
      </div>
      ${data.sideboard.length ? `<div class="deck-section"><h4>Sideboard (${data.sideboard.reduce((a, c) => a + c.quantity, 0)})</h4>${data.sideboard.map(renderDeckCard).join("")}</div>` : ""}
    `;
  } catch (e) {
    out.className = "error";
    out.textContent = `Analyze failed: ${e.message}`;
  } finally {
    btn.disabled = false;
  }
});

$("#save-deck-btn").addEventListener("click", async () => {
  const text = $("#analyze-text").value;
  const name = $("#save-deck-name").value.trim();
  const btn = $("#save-deck-btn"); const out = $("#analyze-result");
  btn.disabled = true; out.className = "muted"; out.textContent = "Saving…";
  try {
    const r = await fetch("/api/decks", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ name, text }),
    });
    const data = await r.json();
    if (!r.ok) throw new Error(data.error || `HTTP ${r.status}`);
    out.className = "success";
    out.textContent = `Saved as "${name}". Open it from the deck list below.`;
    await loadDecks();
  } catch (e) {
    out.className = "error";
    out.textContent = `Save failed: ${e.message}`;
  } finally {
    syncSaveButton();
  }
});

// ---- Mana curve + color pip charts ----
//
// Both charts read from the mainboard (lands excluded). The pip chart counts
// each {W}/{U}/… symbol in mana_cost weighted by card quantity; hybrid pips
// like {W/B} contribute 0.5 to each color so the visual stays honest.
const PIP_COLORS = { W: "#f5e9c5", U: "#7aa7d6", B: "#5c5c5c", R: "#d97a3c", G: "#7ab87a", C: "#bfbfbf" };

function isLand(typeLine) {
  return typeLine != null && /\bLand\b/.test(typeLine);
}

function buildCurve(cards) {
  const buckets = [0, 0, 0, 0, 0, 0, 0, 0]; // 0,1,2,3,4,5,6,7+
  for (const c of cards) {
    if (isLand(c.type_line)) continue;
    const cmc = Math.max(0, Math.round(c.cmc ?? 0));
    const idx = Math.min(7, cmc);
    buckets[idx] += c.quantity;
  }
  return buckets;
}

function buildPips(cards) {
  const pips = { W: 0, U: 0, B: 0, R: 0, G: 0, C: 0 };
  for (const c of cards) {
    if (isLand(c.type_line)) continue;
    if (!c.mana_cost) continue;
    for (const m of c.mana_cost.matchAll(/\{([^}]+)\}/g)) {
      const sym = m[1];
      // Bucket each symbol. Hybrid like W/B splits 0.5/0.5; phyrexian W/P → 1 W.
      const colors = sym.split("/").filter((s) => /^[WUBRG]$/.test(s));
      if (colors.length === 0) continue;
      const w = c.quantity / colors.length;
      for (const col of colors) pips[col] += w;
    }
  }
  return pips;
}

function renderDeckCharts(mainboard) {
  if (!mainboard || mainboard.length === 0) return "";
  const curve = buildCurve(mainboard);
  const pips = buildPips(mainboard);
  const curveMax = Math.max(1, ...curve);
  const curveBars = curve.map((v, i) => {
    const h = Math.round((v / curveMax) * 80);
    const label = i === 7 ? "7+" : String(i);
    return `<div class="curve-col">
      <div class="curve-bar" style="height:${h}px" title="CMC ${label}: ${v}"></div>
      <div class="curve-label">${label}</div>
      <div class="curve-count">${v}</div>
    </div>`;
  }).join("");
  const totalPips = Object.values(pips).reduce((a, b) => a + b, 0);
  const pipRows = ["W","U","B","R","G","C"].map((col) => {
    const v = pips[col];
    if (v === 0) return "";
    const pct = totalPips ? (v / totalPips) * 100 : 0;
    const sym = col === "C" ? "C" : col;
    return `<div class="pip-row">
      <img class="mana" src="/cdn/symbols/${sym}.svg" alt="{${sym}}">
      <div class="pip-bar"><div class="pip-fill" style="width:${pct}%; background:${PIP_COLORS[col]}"></div></div>
      <div class="pip-val">${v.toFixed(v % 1 ? 1 : 0)}</div>
    </div>`;
  }).join("");
  return `<div class="charts">
    <div class="chart-block">
      <h4>Mana curve <small>(non-land)</small></h4>
      <div class="curve">${curveBars}</div>
    </div>
    ${pipRows ? `<div class="chart-block">
      <h4>Color pips</h4>
      <div class="pips">${pipRows}</div>
    </div>` : ""}
  </div>`;
}

// Convert Scryfall-style mana cost text ("{2}{W/B}{X}") into inline SVG symbols
// from Scryfall's CDN. The slug for a symbol is whatever's between the braces
// with slashes removed: {W}→W, {W/B}→WB, {2/W}→2W, {W/P}→WP, {X}→X, {T}→T.
function renderManaCost(text) {
  if (!text) return "";
  return String(text).replace(/\{([^}]+)\}/g, (_, sym) => {
    const slug = sym.replace(/\//g, "");
    const safe = encodeURIComponent(slug);
    return `<img class="mana" src="/cdn/symbols/${safe}.svg" alt="{${escapeHtml(sym)}}" title="{${escapeHtml(sym)}}" loading="lazy">`;
  });
}

// ---- Matches tab ----
async function loadMatches() {
  const rows = await fetchJSON("/api/matches").catch(() => []);
  const tbody = $("#matches-table tbody");
  tbody.innerHTML = "";
  if (rows.length === 0) {
    tbody.innerHTML = '<tr><td colspan="4" class="muted">No matches recorded.</td></tr>';
    return;
  }
  for (const m of rows) {
    const tr = document.createElement("tr");
    const result = m.won === true ? '<span class="win">Win</span>'
                 : m.won === false ? '<span class="loss">Loss</span>'
                 : '<span class="muted">—</span>';
    tr.innerHTML = `
      <td>${escapeHtml(m.started_at.replace("T", " ").slice(0, 19))}</td>
      <td>${escapeHtml(m.opponent_screen_name ?? "")}</td>
      <td>${escapeHtml(m.event_name ?? "")}</td>
      <td>${result}</td>
    `;
    tbody.appendChild(tr);
  }
}

// ---- Collection tab ----
let collectionCache = [];
async function loadCollection() {
  // Wallet first — it's reliable; collection is a derived best-effort.
  const wallet = await fetchJSON("/api/wallet").catch(() => null);
  renderWallet(wallet);

  collectionCache = await fetchJSON("/api/collection").catch(() => []);
  if (collectionCache.length === 0) {
    // Derive a fallback from current decks: sum max-quantity-per-card across decks.
    collectionCache = await deriveFromDecks();
  }
  renderCollection("");
}

function renderWallet(w) {
  const grid = $("#wallet-grid");
  if (!w) { grid.innerHTML = '<p class="muted">No wallet data yet — launch MTGA at least once.</p>'; return; }
  const tile = (label, val, cls = "") => `<div class="wallet-tile ${cls}"><div class="label">${label}</div><div class="value">${val.toLocaleString()}</div></div>`;
  grid.innerHTML = [
    tile("Gold", w.gold),
    tile("Gems", w.gems),
    tile("Vault progress", `${(w.vault_progress / 10).toFixed(1)}%`),
    tile("Wildcards C", w.wc_common, "wc-common"),
    tile("Wildcards U", w.wc_uncommon, "wc-uncommon"),
    tile("Wildcards R", w.wc_rare, "wc-rare"),
    tile("Wildcards M", w.wc_mythic, "wc-mythic"),
    tile("WC track", `${w.wc_track_position}/6`),
  ].join("");
}

async function deriveFromDecks() {
  const decks = await fetchJSON("/api/decks").catch(() => []);
  const seen = new Map(); // card_id -> max quantity across decks
  for (const summary of decks) {
    const d = await fetchJSON(`/api/decks/${summary.deck_id}`).catch(() => null);
    if (!d) continue;
    for (const c of [...d.mainboard, ...d.sideboard]) {
      const prev = seen.get(c.arena_id);
      if (!prev || c.quantity > prev.quantity) {
        seen.set(c.arena_id, c);
      }
    }
  }
  return [...seen.values()].map((c) => ({
    arena_id: c.arena_id,
    quantity: c.quantity,
    name: c.name,
    set: null,
    rarity: c.rarity,
    type_line: c.type_line,
    mana_cost: c.mana_cost,
    cmc: c.cmc,
    colors: colorsFromManaCost(c.mana_cost),
    image_small: c.image_small,
    image_normal: null,
  }));
}

// Derive Scryfall-style colors from a mana cost like "{2}{W/B}{R}". Returns
// the unique set of WUBRG symbols present (ignoring generic, X, hybrid-with-2,
// and phyrexian costs that don't carry a color identity for filter purposes).
function colorsFromManaCost(cost) {
  if (!cost) return [];
  const found = new Set();
  for (const m of cost.matchAll(/\{([^}]+)\}/g)) {
    for (const part of m[1].split("/")) {
      if (/^[WUBRG]$/.test(part)) found.add(part);
    }
  }
  return [...found];
}

// Collection filter state: { name: "", color: Set, rarity: Set, type: Set }.
// Empty sets mean "no filter in this group"; otherwise it's union-within-group,
// AND-across-groups.
const collFilters = { name: "", color: new Set(), rarity: new Set(), type: new Set() };

$("#coll-filter").addEventListener("input", (e) => {
  collFilters.name = e.target.value.trim().toLowerCase();
  renderCollection();
});

$$(".filter-bar .filter-chip").forEach((btn) => {
  btn.addEventListener("click", () => {
    const group = btn.parentElement.dataset.group;
    const value = btn.dataset.value;
    const set = collFilters[group];
    if (set.has(value)) { set.delete(value); btn.classList.remove("active"); }
    else { set.add(value); btn.classList.add("active"); }
    renderCollection();
  });
});

$("#filter-clear").addEventListener("click", () => {
  collFilters.color.clear();
  collFilters.rarity.clear();
  collFilters.type.clear();
  collFilters.name = "";
  $("#coll-filter").value = "";
  $$(".filter-bar .filter-chip.active").forEach((b) => b.classList.remove("active"));
  renderCollection();
});

function matchesColorFilter(card, selected) {
  if (selected.size === 0) return true;
  const colors = card.colors || [];
  for (const c of selected) {
    if (c === "M") { if (colors.length > 1) return true; continue; }
    if (c === "C") { if (colors.length === 0) return true; continue; }
    if (colors.includes(c)) return true;
  }
  return false;
}

function matchesTypeFilter(card, selected) {
  if (selected.size === 0) return true;
  const t = (card.type_line || "").toLowerCase();
  for (const v of selected) if (t.includes(v)) return true;
  return false;
}

$("#import-btn").addEventListener("click", async () => {
  const text = $("#import-text").value;
  const replace = $("#import-replace").checked;
  const btn = $("#import-btn"); const out = $("#import-result");
  btn.disabled = true; out.className = "muted"; out.textContent = "Importing…";
  try {
    const r = await fetch("/api/collection/import", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ text, replace }),
    });
    const data = await r.json();
    if (!r.ok) throw new Error(data.error || `HTTP ${r.status}`);
    const unmatchedBlock = data.unmatched_lines
      ? `<pre>Unmatched samples:\n${data.unmatched_samples.map(escapeHtml).join("\n")}</pre>`
      : "";
    out.className = "success";
    out.innerHTML = `Imported ${data.unique_cards} unique cards (${data.total_cards} total) · matched ${data.matched_lines} lines, skipped ${data.unmatched_lines}.${unmatchedBlock}`;
    await loadCollection();
  } catch (e) {
    out.className = "error";
    out.textContent = `Import failed: ${e.message}`;
  } finally {
    btn.disabled = false;
  }
});

function renderCollection() {
  const grid = $("#collection-grid");
  grid.innerHTML = "";
  if (collectionCache.length === 0) {
    grid.innerHTML = '<p class="muted">No deck-derived cards yet — visit the Decks screen in MTGA so it sends your deck list.</p>';
    return;
  }
  let shown = 0;
  for (const c of collectionCache) {
    if (collFilters.name && !c.name.toLowerCase().includes(collFilters.name)) continue;
    if (!matchesColorFilter(c, collFilters.color)) continue;
    if (collFilters.rarity.size > 0 && !collFilters.rarity.has(c.rarity)) continue;
    if (!matchesTypeFilter(c, collFilters.type)) continue;
    const tile = document.createElement("div");
    tile.className = "card-tile";
    // Scryfall serves the same image at /small/, /normal/, /large/ — swap the
    // path segment to grab 672×936 for the popover instead of the 488×680 normal.
    const previewUrl = (c.image_normal || c.image_small || "").replace("/normal/", "/large/");
    tile.dataset.preview = previewUrl;
    tile.innerHTML = `
      ${c.image_small ? `<img src="${c.image_small}" alt="" loading="lazy">` : ""}
      <div class="info">
        <div class="name">${escapeHtml(c.name)}</div>
        <div class="meta">${c.set?.toUpperCase() ?? ""} · ${c.rarity ?? ""}</div>
      </div>
      <div class="qty">×${c.quantity}</div>
    `;
    grid.appendChild(tile);
    shown++;
  }
  if (shown === 0) {
    grid.innerHTML = '<p class="muted">No cards match the current filters.</p>';
  }
}

// ---- Hover preview (collection grid) ----
//
// One singleton popover anchored next to the cursor. Tracking mousemove keeps
// it pinned next to whatever the user is reading rather than fixed in a corner.
(function attachCardPreview() {
  const preview = $("#card-preview");
  const img = preview.querySelector("img");
  const grid = $("#collection-grid");
  let currentSrc = null;

  function show(url, ev) {
    if (url !== currentSrc) {
      img.src = url;
      currentSrc = url;
    }
    preview.hidden = false;
    position(ev);
  }
  function hide() {
    preview.hidden = true;
    currentSrc = null;
  }
  function position(ev) {
    // Place to the right of the cursor; flip left if it would overflow the viewport.
    const pad = 16;
    const w = 280; // matches CSS width
    const h = 390;
    let x = ev.clientX + pad;
    let y = ev.clientY + pad;
    if (x + w > window.innerWidth) x = ev.clientX - w - pad;
    if (y + h > window.innerHeight) y = window.innerHeight - h - pad;
    if (y < pad) y = pad;
    preview.style.left = x + "px";
    preview.style.top = y + "px";
  }

  grid.addEventListener("mouseover", (e) => {
    const tile = e.target.closest(".card-tile");
    if (!tile || !tile.dataset.preview) return;
    show(tile.dataset.preview, e);
  });
  grid.addEventListener("mousemove", (e) => {
    if (preview.hidden) return;
    position(e);
  });
  grid.addEventListener("mouseleave", hide);
  grid.addEventListener("mouseout", (e) => {
    if (!e.relatedTarget || !e.relatedTarget.closest(".card-tile")) hide();
  });
})();

// ---- Drafts tab ----
async function loadDrafts() {
  const drafts = await fetchJSON("/api/drafts").catch(() => []);
  const ul = $("#drafts-list");
  ul.innerHTML = "";
  if (drafts.length === 0) {
    ul.innerHTML = '<li class="muted">No drafts yet.</li>';
    return;
  }
  for (const d of drafts) {
    const li = document.createElement("li");
    li.innerHTML = `${(d.set_code ?? "?").toUpperCase()}<small>${d.picks} picks · ${d.started_at.slice(0, 10)}</small>`;
    li.addEventListener("click", () => loadDraftDetail(d.draft_id, li));
    ul.appendChild(li);
  }
}

async function loadDraftDetail(id, li) {
  $$("#drafts-list li").forEach((x) => x.classList.remove("active"));
  li.classList.add("active");
  const picks = await fetchJSON(`/api/drafts/${id}`).catch(() => []);
  const det = $("#draft-detail");
  if (picks.length === 0) { det.innerHTML = '<p class="muted">No picks recorded.</p>'; return; }
  det.innerHTML = picks.map((p) => `
    <div class="draft-pick">
      <div class="header">P${p.pack_number} P${p.pick_number} → <span class="pick">${escapeHtml(p.picked.name)}</span></div>
      <div class="pack-cards">From: ${p.pack.map((c) => escapeHtml(c.name)).join(", ")}</div>
    </div>
  `).join("");
}

// ---- Events tab ----
async function loadEvents() {
  const filter = $("#events-filter").value.trim();
  const url = filter ? `/api/events?kind=${encodeURIComponent(filter)}&limit=200` : "/api/events?limit=200";
  const events = await fetchJSON(url).catch(() => []);
  $("#events-pre").textContent = events.length
    ? events.map((e) => `[${e.timestamp}] ${e.direction.padEnd(8)} ${e.kind}\n${JSON.stringify(e.payload, null, 2)}`).join("\n\n---\n\n")
    : "(no events — make sure Detailed Logs are enabled in MTGA → Account)";
}

$("#events-filter").addEventListener("input", debounce(loadEvents, 300));

function debounce(fn, ms) {
  let t;
  return (...a) => { clearTimeout(t); t = setTimeout(() => fn(...a), ms); };
}

function escapeHtml(s) {
  if (s == null) return "";
  return String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

// ---- SSE live updates ----
function setConn(state) {
  const pill = $("#conn");
  pill.classList.remove("connected", "disconnected");
  if (state === "connected") { pill.classList.add("connected"); pill.textContent = "live"; }
  else if (state === "disconnected") { pill.classList.add("disconnected"); pill.textContent = "disconnected"; }
  else { pill.textContent = "connecting…"; }
}

function startSSE() {
  const es = new EventSource("/api/sse");
  es.onopen = () => setConn("connected");
  es.onerror = () => setConn("disconnected");
  es.onmessage = (e) => {
    let upd;
    try { upd = JSON.parse(e.data); } catch { return; }
    if (upd.type === "match" && activeTab === "live") refreshLive();
    if (upd.type === "decks_updated" && activeTab === "decks") loadDecks();
    if (upd.type === "collection_updated" && activeTab === "collection") loadCollection();
    if (upd.type === "event_tick" && activeTab === "events") loadEvents();
  };
}

showTab("live");
startSSE();
