// MTGA Sleuth frontend — single-page, no framework.
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
let walletCache = null;

async function loadDecks() {
  [decksCache, walletCache] = await Promise.all([
    fetchJSON("/api/decks").catch(() => []),
    fetchJSON("/api/wallet").catch(() => null),
  ]);
  renderDeckList();
}

// Per-rarity wildcard shortfall after spending wallet balances; null when the
// wallet is unknown (can't judge affordability without it).
function wcShortfall(wc) {
  if (!walletCache) return null;
  return {
    common: Math.max(0, (wc.common || 0) - walletCache.wc_common),
    uncommon: Math.max(0, (wc.uncommon || 0) - walletCache.wc_uncommon),
    rare: Math.max(0, (wc.rare || 0) - walletCache.wc_rare),
    mythic: Math.max(0, (wc.mythic || 0) - walletCache.wc_mythic),
  };
}

// Rarity-weighted "distance from buildable" for sorting. Rare/mythic wildcards
// are far scarcer than commons, so a deck short 1 mythic ranks farther than
// one short 4 commons.
const WC_WEIGHT = { common: 1, uncommon: 2, rare: 8, mythic: 16 };
function buildDistance(d) {
  if (d.total_missing === 0) return -1; // already built
  const short = wcShortfall(d.wildcards_needed);
  if (!short) return Number.MAX_SAFE_INTEGER;
  return Object.entries(WC_WEIGHT).reduce((a, [r, w]) => a + short[r] * w, 0);
}

function renderDeckBadge(d) {
  if (d.total_missing === 0) return '<span class="deck-badge complete">✓ complete</span>';
  const short = wcShortfall(d.wildcards_needed);
  if (!short) return "";
  const parts = [["common", "C"], ["uncommon", "U"], ["rare", "R"], ["mythic", "M"]]
    .filter(([r]) => short[r] > 0)
    .map(([r, abbr]) => `<span class="wc-abbr wc-${r}">${short[r]}${abbr}</span>`);
  if (parts.length === 0) return '<span class="deck-badge craftable">craftable</span>';
  return `<span class="deck-badge short">short ${parts.join(" ")}</span>`;
}

function renderDeckList() {
  const ul = $("#decks-list");
  ul.innerHTML = "";
  if (decksCache.length === 0) {
    ul.innerHTML = '<li class="muted">No decks yet.</li>';
    return;
  }
  const rows = [...decksCache];
  if ($("#deck-sort").value === "buildable") {
    // Complete first, then closest to buildable; recency breaks ties.
    rows.sort((a, b) =>
      buildDistance(a) - buildDistance(b) ||
      a.total_missing - b.total_missing ||
      b.last_updated.localeCompare(a.last_updated));
  }
  for (const d of rows) {
    const li = document.createElement("li");
    const manual = d.deck_id.startsWith("user-") ? '<span class="deck-tag">manual</span>' : "";
    li.innerHTML = `${escapeHtml(d.name)}${manual}${renderDeckBadge(d)}<small>${d.format ?? "?"} · ${d.last_updated.slice(0, 10)}</small>`;
    li.addEventListener("click", () => loadDeckDetail(d.deck_id, li));
    ul.appendChild(li);
  }
}

$("#deck-sort").addEventListener("change", renderDeckList);
$("#new-deck-btn").addEventListener("click", () => openEditor(null));

async function loadDeckDetail(id, li) {
  $$("#decks-list li").forEach((x) => x.classList.remove("active"));
  if (li) li.classList.add("active");
  const d = await fetchJSON(`/api/decks/${id}`).catch(() => null);
  const det = $("#deck-detail");
  if (!d) { det.innerHTML = '<p class="muted">Failed to load.</p>'; return; }
  const editLabel = d.deck_id.startsWith("user-") ? "Edit" : "Edit a copy";
  det.innerHTML = `
    <div class="deck-detail-head">
      <h3>${escapeHtml(d.name)}</h3>
      <span class="deck-detail-actions">
        <button id="edit-deck-btn">${editLabel}</button>
        <button id="export-deck-btn" title="Copy this deck as Arena-format text">Copy for Arena</button>
      </span>
    </div>
    <p class="muted">${d.format ?? "Unknown format"}</p>
    ${renderDeckCharts(d.mainboard)}
    ${renderWildcardSummary(d)}
    ${renderLegality(d.mainboard, d.sideboard, d.format)}
    <div class="deck-section">
      <h4>Mainboard (${d.mainboard.reduce((a, c) => a + c.quantity, 0)})</h4>
      ${d.mainboard.map((c) => renderDeckCard(c, legalityFormatKey(d.format))).join("")}
    </div>
    ${d.sideboard.length ? `<div class="deck-section"><h4>Sideboard (${d.sideboard.reduce((a, c) => a + c.quantity, 0)})</h4>${d.sideboard.map((c) => renderDeckCard(c, legalityFormatKey(d.format))).join("")}</div>` : ""}
  `;
  $("#edit-deck-btn").addEventListener("click", () => openEditor(d));
  $("#export-deck-btn").addEventListener("click", async (e) => {
    const btn = e.target;
    try {
      const r = await fetch(`/api/decks/${id}/export`);
      if (!r.ok) throw new Error(`HTTP ${r.status}`);
      await navigator.clipboard.writeText(await r.text());
      btn.textContent = "Copied ✓";
    } catch (err) {
      btn.textContent = "Copy failed";
      console.error("export failed:", err);
    }
    setTimeout(() => { btn.textContent = "Copy for Arena"; }, 1600);
  });
}

function renderDeckCard(c, fmtKey = null) {
  const missingClass = c.missing > 0 ? " missing" : "";
  const ownedTxt = c.missing > 0
    ? `<span class="own bad">${c.owned}/${c.quantity}</span>`
    : `<span class="own good">${c.quantity}/${c.quantity}</span>`;
  const issue = cardLegalityIssue(c, fmtKey);
  const chip = issue ? `<span class="legal-chip">${escapeHtml(issue)}</span>` : "";
  return `<div class="deck-card${missingClass}">
    <span class="qty">${c.quantity}×</span>
    <span class="name">${escapeHtml(c.name)}${chip}</span>
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
  const have = walletCache
    ? { common: walletCache.wc_common, uncommon: walletCache.wc_uncommon, rare: walletCache.wc_rare, mythic: walletCache.wc_mythic }
    : null;
  const tile = (label, rarity, cls) => {
    const need = wc[rarity] || 0;
    const haveLine = have
      ? `<div class="have ${need <= have[rarity] ? "good" : "bad"}">have ${have[rarity]}</div>`
      : "";
    return `<div class="wallet-tile ${cls}"><div class="label">${label}</div><div class="value">${need}</div>${haveLine}</div>`;
  };
  let verdict = "";
  if (have) {
    const short = wcShortfall(wc);
    const totalShort = short.common + short.uncommon + short.rare + short.mythic;
    const missing = ["common", "uncommon", "rare", "mythic"]
      .filter((r) => short[r] > 0)
      .map((r) => `${short[r]} ${r}`);
    verdict = totalShort === 0
      ? '<div class="wc-verdict good">You have enough wildcards to craft everything missing.</div>'
      : `<div class="wc-verdict bad">Short ${missing.join(", ")} wildcard${totalShort === 1 ? "" : "s"}.</div>`;
  }
  return `<div class="wc-summary">
    <div class="wc-summary-head">Missing ${d.total_missing} copies (${d.unique_missing} unique). Wildcards needed:</div>
    <div class="wallet-grid">
      ${tile("Common", "common", "wc-common")}
      ${tile("Uncommon", "uncommon", "wc-uncommon")}
      ${tile("Rare", "rare", "wc-rare")}
      ${tile("Mythic", "mythic", "wc-mythic")}
    </div>
    ${verdict}
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
    const fmt = $("#analyze-format").value || null;
    detail.innerHTML = `
      ${renderDeckCharts(data.mainboard)}
      ${renderWildcardSummary(data)}
      ${fmt ? renderLegality(data.mainboard, data.sideboard, fmt) : ""}
      <div class="deck-section">
        <h4>Mainboard (${data.mainboard.reduce((a, c) => a + c.quantity, 0)})</h4>
        ${data.mainboard.map((c) => renderDeckCard(c, legalityFormatKey(fmt))).join("")}
      </div>
      ${data.sideboard.length ? `<div class="deck-section"><h4>Sideboard (${data.sideboard.reduce((a, c) => a + c.quantity, 0)})</h4>${data.sideboard.map((c) => renderDeckCard(c, legalityFormatKey(fmt))).join("")}</div>` : ""}
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

// ---- Deck editor ----
//
// State lives in two Maps (arena_id → {name, qty}); every mutation re-runs the
// server-side analyze endpoint (as `arena_id,quantity` lines, which the
// importer accepts) so owned/missing/wildcard/legality data stays live without
// duplicating collection logic client-side.
const editor = {
  active: false,
  deckId: null, // null = creating a new deck
  main: new Map(),
  side: new Map(),
};

function openEditor(d) {
  editor.active = true;
  editor.main = new Map();
  editor.side = new Map();
  let name = "";
  let format = "";
  if (d) {
    const isUser = d.deck_id.startsWith("user-");
    editor.deckId = isUser ? d.deck_id : null;
    name = isUser ? d.name : `Copy of ${d.name}`;
    format = d.format ?? "";
    for (const c of d.mainboard) editor.main.set(c.arena_id, { name: c.name, qty: c.quantity });
    for (const c of d.sideboard) editor.side.set(c.arena_id, { name: c.name, qty: c.quantity });
  } else {
    editor.deckId = null;
  }
  const det = $("#deck-detail");
  det.innerHTML = `
    <div class="editor">
      <div class="editor-head">
        <input id="editor-name" type="text" placeholder="Deck name" value="${escapeHtml(name)}">
        <select id="editor-format">
          <option value="">No format</option>
          <option>Standard</option>
          <option>Alchemy</option>
          <option>Historic</option>
          <option>Timeless</option>
          <option>Explorer</option>
          <option value="Brawl">Standard Brawl</option>
          <option value="HistoricBrawl">Brawl (100)</option>
        </select>
        <button id="editor-save" class="primary">Save</button>
        <button id="editor-cancel">Cancel</button>
        ${editor.deckId ? '<button id="editor-delete" class="danger">Delete</button>' : ""}
      </div>
      <div class="editor-search">
        <input id="editor-search" type="search" placeholder="Search cards to add (min 2 letters)…" autocomplete="off">
        <div class="board-toggle">
          <button id="editor-board-main" class="active">Main</button>
          <button id="editor-board-side">Side</button>
        </div>
        <div id="editor-results" class="search-results" hidden></div>
      </div>
      <div id="editor-body"><p class="muted">Add cards via search, or adjust quantities below.</p></div>
    </div>
  `;
  const fmtSel = $("#editor-format");
  if ([...fmtSel.options].some((o) => o.value === format)) fmtSel.value = format;
  setEditorBoard("main");
  $("#editor-name").addEventListener("input", syncEditorSave);

  $("#editor-cancel").addEventListener("click", () => {
    editor.active = false;
    if (d) loadDeckDetail(d.deck_id, null);
    else $("#deck-detail").innerHTML = '<p class="muted">Pick a deck to see its contents.</p>';
  });
  $("#editor-save").addEventListener("click", saveEditor);
  if (editor.deckId) {
    $("#editor-delete").addEventListener("click", async () => {
      if (!confirm("Delete this deck? This cannot be undone.")) return;
      const r = await fetch(`/api/decks/${editor.deckId}`, { method: "DELETE" });
      if (r.ok) {
        editor.active = false;
        $("#deck-detail").innerHTML = '<p class="muted">Deck deleted.</p>';
        await loadDecks();
      }
    });
  }

  $("#editor-board-main").addEventListener("click", () => setEditorBoard("main"));
  $("#editor-board-side").addEventListener("click", () => setEditorBoard("side"));
  $("#editor-search").addEventListener("input", debounce(runCardSearch, 250));
  $("#editor-format").addEventListener("change", refreshEditor);

  $("#editor-body").addEventListener("click", (e) => {
    const btn = e.target.closest("button[data-act]");
    if (!btn) return;
    const board = btn.dataset.board === "side" ? editor.side : editor.main;
    const id = Number(btn.dataset.id);
    const entry = board.get(id);
    if (!entry) return;
    if (btn.dataset.act === "inc") entry.qty += 1;
    if (btn.dataset.act === "dec") entry.qty -= 1;
    if (btn.dataset.act === "rm" || entry.qty <= 0) board.delete(id);
    refreshEditor();
  });

  refreshEditor();
}

let editorBoard = "main";
function setEditorBoard(which) {
  editorBoard = which;
  $("#editor-board-main").classList.toggle("active", which === "main");
  $("#editor-board-side").classList.toggle("active", which === "side");
}

async function runCardSearch() {
  const q = $("#editor-search").value.trim();
  const box = $("#editor-results");
  if (q.length < 2) { box.hidden = true; box.innerHTML = ""; return; }
  const results = await fetchJSON(`/api/cards?q=${encodeURIComponent(q)}`).catch(() => []);
  if (results.length === 0) {
    box.innerHTML = '<div class="muted result-empty">No matches.</div>';
    box.hidden = false;
    return;
  }
  box.innerHTML = results.map((c) => `
    <button class="search-result" data-id="${c.arena_id}" data-name="${escapeHtml(c.name)}">
      <span class="name">${escapeHtml(c.name)}</span>
      <span class="cost">${renderManaCost(c.mana_cost)}</span>
      <span class="meta">${c.set?.toUpperCase() ?? ""} · own ${c.owned}</span>
    </button>
  `).join("");
  box.hidden = false;
  $$("#editor-results .search-result").forEach((b) => b.addEventListener("click", () => {
    const board = editorBoard === "side" ? editor.side : editor.main;
    const id = Number(b.dataset.id);
    const entry = board.get(id);
    if (entry) entry.qty += 1;
    else board.set(id, { name: b.dataset.name, qty: 1 });
    refreshEditor();
  }));
}

function editorAsText() {
  const lines = [];
  for (const [id, e] of editor.main) lines.push(`${id},${e.qty}`);
  if (editor.side.size) {
    lines.push("", "Sideboard");
    for (const [id, e] of editor.side) lines.push(`${id},${e.qty}`);
  }
  return lines.join("\n");
}

function renderEditorRow(c, board) {
  const ownBad = c.missing > 0 ? "bad" : "good";
  const fmtKey = legalityFormatKey($("#editor-format").value || null);
  const issue = cardLegalityIssue(c, fmtKey);
  const chip = issue ? `<span class="legal-chip">${escapeHtml(issue)}</span>` : "";
  return `<div class="deck-card editor-row">
    <span class="stepper">
      <button data-act="dec" data-id="${c.arena_id}" data-board="${board}">−</button>
      <span class="qty">${c.quantity}</span>
      <button data-act="inc" data-id="${c.arena_id}" data-board="${board}">+</button>
    </span>
    <span class="name">${escapeHtml(c.name)}${chip}</span>
    <span class="own ${ownBad}">${c.owned}/${c.quantity}</span>
    <span class="cost">${renderManaCost(c.mana_cost)}</span>
    <button class="rm" data-act="rm" data-id="${c.arena_id}" data-board="${board}">×</button>
  </div>`;
}

const refreshEditor = debounce(async () => {
  if (!editor.active) return;
  const body = $("#editor-body");
  if (editor.main.size === 0 && editor.side.size === 0) {
    body.innerHTML = '<p class="muted">Deck is empty — add cards via search.</p>';
    syncEditorSave();
    return;
  }
  try {
    const r = await fetch("/api/decks/analyze", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ text: editorAsText() }),
    });
    const data = await r.json();
    if (!r.ok) throw new Error(data.error || `HTTP ${r.status}`);
    const fmt = $("#editor-format").value || null;
    const mainCount = data.mainboard.reduce((a, c) => a + c.quantity, 0);
    const sideCount = data.sideboard.reduce((a, c) => a + c.quantity, 0);
    body.innerHTML = `
      ${renderDeckCharts(data.mainboard)}
      ${renderWildcardSummary(data)}
      ${fmt ? renderLegality(data.mainboard, data.sideboard, fmt) : ""}
      <div class="deck-section">
        <h4>Mainboard (${mainCount})</h4>
        ${data.mainboard.map((c) => renderEditorRow(c, "main")).join("")}
      </div>
      <div class="deck-section">
        <h4>Sideboard (${sideCount})</h4>
        ${data.sideboard.length ? data.sideboard.map((c) => renderEditorRow(c, "side")).join("") : '<p class="muted">Empty — use the Side toggle to add here.</p>'}
      </div>
    `;
  } catch (e) {
    body.innerHTML = `<p class="error">Analysis failed: ${escapeHtml(e.message)}</p>`;
  }
  syncEditorSave();
}, 250);

function syncEditorSave() {
  const hasName = $("#editor-name").value.trim().length > 0;
  const hasCards = editor.main.size > 0 || editor.side.size > 0;
  $("#editor-save").disabled = !(hasName && hasCards);
}

async function saveEditor() {
  const name = $("#editor-name").value.trim();
  const format = $("#editor-format").value || null;
  const payload = {
    name,
    format,
    mainboard: [...editor.main.entries()].map(([id, e]) => [id, e.qty]),
    sideboard: [...editor.side.entries()].map(([id, e]) => [id, e.qty]),
  };
  const [url, method] = editor.deckId
    ? [`/api/decks/${editor.deckId}`, "PUT"]
    : ["/api/decks", "POST"];
  const btn = $("#editor-save");
  btn.disabled = true;
  try {
    const r = await fetch(url, {
      method,
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    });
    const data = await r.json();
    if (!r.ok) throw new Error(data.error || `HTTP ${r.status}`);
    editor.active = false;
    await loadDecks();
    loadDeckDetail(data.deck_id, null);
  } catch (e) {
    btn.disabled = false;
    alert(`Save failed: ${e.message}`);
  }
}

// ---- Format legality ----
//
// Maps MTGA's deck-format strings onto the Scryfall legality keys the backend
// keeps (cards::ARENA_FORMATS). MTGA's "Brawl" is the 60-card variant
// (Scryfall "standardbrawl"); "HistoricBrawl" is the 100-card one (Scryfall
// "brawl"). Traditional* variants share their base format's card pool.
const MTGA_TO_SCRYFALL_FORMAT = {
  standard: "standard", alchemy: "alchemy", historic: "historic",
  timeless: "timeless", explorer: "explorer", pauper: "pauper",
  brawl: "standardbrawl", standardbrawl: "standardbrawl", historicbrawl: "brawl",
};
function legalityFormatKey(format) {
  if (!format) return null;
  return MTGA_TO_SCRYFALL_FORMAT[format.toLowerCase().replace(/^traditional/, "")] ?? null;
}

const SINGLETON_FORMATS = new Set(["brawl", "standardbrawl"]);

// null = fine (or unknowable); otherwise a short problem word for the chip.
function cardLegalityIssue(c, fmtKey) {
  if (!fmtKey || !c.legalities) return null;
  const status = c.legalities[fmtKey];
  if (status === "legal") return null;
  if (status === "banned" || status === "suspended" || status === "restricted") return status;
  return "not legal";
}

function validateDeck(mainboard, sideboard, fmtKey, formatLabel) {
  const issues = [];
  const flagged = new Map(); // issue kind -> [card names]
  for (const c of [...mainboard, ...sideboard]) {
    const issue = cardLegalityIssue(c, fmtKey);
    if (!issue) continue;
    if (!flagged.has(issue)) flagged.set(issue, []);
    flagged.get(issue).push(c.name);
  }
  for (const [issue, names] of flagged) {
    const list = names.slice(0, 6).join(", ") + (names.length > 6 ? "…" : "");
    issues.push(`${names.length} card${names.length > 1 ? "s" : ""} ${issue} in ${formatLabel}: ${list}`);
  }
  // Copy limit counts mainboard + sideboard together; basic lands are exempt.
  const isBasic = (c) => (c.type_line || "").includes("Basic Land");
  const maxCopies = SINGLETON_FORMATS.has(fmtKey) ? 1 : 4;
  const perCard = new Map();
  for (const c of [...mainboard, ...sideboard]) {
    if (isBasic(c)) continue;
    perCard.set(c.arena_id, { name: c.name, qty: (perCard.get(c.arena_id)?.qty || 0) + c.quantity });
  }
  for (const { name, qty } of perCard.values()) {
    if (qty > maxCopies) issues.push(`${qty}× ${name} exceeds the ${maxCopies}-copy limit`);
  }
  const mainCount = mainboard.reduce((a, c) => a + c.quantity, 0);
  if (!SINGLETON_FORMATS.has(fmtKey) && mainCount < 60) {
    issues.push(`Mainboard has ${mainCount} cards (minimum 60)`);
  }
  return issues;
}

function renderLegality(mainboard, sideboard, format) {
  const fmtKey = legalityFormatKey(format);
  if (!fmtKey) return "";
  // Old card caches carry no legality data — stay quiet rather than calling
  // every card illegal.
  if (![...mainboard, ...sideboard].some((c) => c.legalities)) return "";
  const issues = validateDeck(mainboard, sideboard, fmtKey, format);
  if (issues.length === 0) return `<div class="legality ok">✓ Legal in ${escapeHtml(format)}.</div>`;
  return `<div class="legality bad"><strong>${escapeHtml(format)} issues</strong><ul>${issues.map((i) => `<li>${escapeHtml(i)}</li>`).join("")}</ul></div>`;
}

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
  collPage = 1;
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

const COLLECTION_PAGE_SIZE = 30;
let collPage = 1;

$("#coll-filter").addEventListener("input", (e) => {
  collFilters.name = e.target.value.trim().toLowerCase();
  collPage = 1;
  renderCollection();
});

$$(".filter-bar .filter-chip").forEach((btn) => {
  btn.addEventListener("click", () => {
    const group = btn.parentElement.dataset.group;
    const value = btn.dataset.value;
    const set = collFilters[group];
    if (set.has(value)) { set.delete(value); btn.classList.remove("active"); }
    else { set.add(value); btn.classList.add("active"); }
    collPage = 1;
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
  collPage = 1;
  renderCollection();
});

$("#pager-prev").addEventListener("click", () => {
  if (collPage > 1) { collPage--; renderCollection(); }
});
$("#pager-next").addEventListener("click", () => {
  collPage++;
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
  const pager = $("#collection-pager");
  grid.innerHTML = "";
  pager.hidden = true;
  if (collectionCache.length === 0) {
    grid.innerHTML = '<p class="muted">No deck-derived cards yet — visit the Decks screen in MTGA so it sends your deck list.</p>';
    return;
  }
  const matches = collectionCache.filter((c) =>
    (!collFilters.name || c.name.toLowerCase().includes(collFilters.name)) &&
    matchesColorFilter(c, collFilters.color) &&
    (collFilters.rarity.size === 0 || collFilters.rarity.has(c.rarity)) &&
    matchesTypeFilter(c, collFilters.type)
  );
  if (matches.length === 0) {
    grid.innerHTML = '<p class="muted">No cards match the current filters.</p>';
    return;
  }
  const totalPages = Math.max(1, Math.ceil(matches.length / COLLECTION_PAGE_SIZE));
  if (collPage > totalPages) collPage = totalPages;
  const start = (collPage - 1) * COLLECTION_PAGE_SIZE;
  const slice = matches.slice(start, start + COLLECTION_PAGE_SIZE);
  for (const c of slice) {
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
  }
  pager.hidden = false;
  $("#pager-prev").disabled = collPage <= 1;
  $("#pager-next").disabled = collPage >= totalPages;
  const rangeEnd = Math.min(start + COLLECTION_PAGE_SIZE, matches.length);
  $("#pager-status").textContent =
    `${start + 1}–${rangeEnd} of ${matches.length} · page ${collPage}/${totalPages}`;
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
