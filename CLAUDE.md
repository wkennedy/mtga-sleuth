# CLAUDE.md

Context for Claude Code working on this repo. Anything derivable from `cargo`,
the source tree, or `git log` is omitted — this file captures the
non-obvious stuff.

## What this is

A Linux-native MTG Arena tracker. Watches MTGA's `Player.log`, parses game/
deck/draft/collection events, and serves a local web UI on `127.0.0.1:7843`.

Reference apps: mtgatool-desktop, Untapped Companion, 17lands.

## Stack

- **Backend**: Rust 2021. axum 0.8, tokio, sqlx (sqlite, runtime-tokio-rustls).
- **Persistence**: SQLite at `~/.local/share/mtga-sleuth/tracker.sqlite` (WAL).
- **Frontend**: Vanilla HTML/CSS/JS in `web/static/`, embedded into the binary
  via `rust-embed` at build time. **No JS toolchain** — edit the file, rebuild.
- **Card data**: Scryfall bulk download cached at `~/.cache/mtga-sleuth/scryfall-arena.json`,
  filtered to entries with an `arena_id`.

## Architecture

```
Player.log  →  log_watcher  →  parser  →  state engine  →  SQLite + SSE broadcast
                                                  ↓
                                              web (axum: REST + SSE + embedded UI)
```

Module layout (`src/`):
- `log_watcher/` — polling tail with rotation/truncation handling
- `parser/extract.rs` — brace-counted JSON extractor (state machine across lines)
- `state/handlers/` — one file per MTGA event family (`decks`, `collection`,
  `match_room`, `gre`, `draft`, `starthook`, `inventory_changes`, `wallet`)
- `cards/` — Scryfall fetch + `(set, num) → arena_id` index for paste import
- `localization/` — loads MTGA's `Raw_ClientLocalization_*.mtga` SQLite for
  translating `?=?Loc/...` deck-name keys
- `web/` — axum routes, SSE, paste-import (`web/import.rs`), embedded assets

## Critical gotchas

1. **Detailed Logs MUST be on in MTGA** (Account → Detailed Logs (Plugin
   Support)) or `Player.log` only contains Unity boot noise. The UI's Events
   tab will be empty. Always remind users.

2. **MTGA's modern detailed-log format** (verified empirically 2026-05-11):
   - Event names are **CamelCase no dots** (`DeckUpsertDeckV3`, not
     `Deck.UpsertDeckV3`). Old third-party docs are stale.
   - Most lines have a `M/D/YYYY H:MM:SS [AM|PM]:` timestamp prefix between
     `[UnityCrossThreadLogger]` and the marker.
   - Response (`<==`) lines arrive on their own line WITHOUT the
     `[UnityCrossThreadLogger]` prefix, after a separate timestamp-only
     preamble line. The parser handles this; if you change parsing, don't
     break it.
   - Match-flow markers are `Match to <playerId>:` and `<playerId> to Match:`
     — NOT the older `GRE to Client` / `Match to GRE` strings.

3. **`StartHook.Decks` mixes personal and precon decks** (verified 2026-08-13):
   its `Decks` map contains the player's real decks (human-typed `Name`) AND
   ~120 precon/reference decks (`Name` starts with `?=?Loc/Decks/Precon`,
   empty `Attributes`). `decks::classify_origin` splits them into the
   `decks.origin` column; keep that in mind when querying decks.

4. **Per-card collection counts are NOT in detailed logs**. Confirmed:
   no `Inventory*` events emit a card-id→count map. We work around this with:
   - Incremental tracking from `BoosterOpen|Reward|Grant|...` events and
     `Changes`/`delta.cardsAdded` arrays (handler: `inventory_changes.rs`)
   - Paste-import via `POST /api/collection/import` accepting Arena format
   - Deck-derived lower-bound shown when collection is empty (frontend only)

5. **Default install path** is Snap Steam at app id `2141910`. Other paths
   (Lutris, Bottles, Flatpak Steam) are auto-detected as fallbacks.
   Localization SQLite lives next to the game install at
   `MTGA_Data/Downloads/Raw/Raw_ClientLocalization_<hash>.mtga`.

6. **Static assets are embedded at build time**. Editing `web/static/*`
   requires `cargo build` to take effect — there's no live-reload.

7. **rust-embed in debug mode reads from disk dynamically; release embeds.**
   So `cargo run` picks up frontend edits without rebuild; `cargo run --release`
   does not.

## Commands

```bash
cargo run --release                          # production: embed assets, fast
cargo run                                    # dev: live frontend reload
cargo test                                   # all tests (parser + handlers + loc)
RUST_LOG=mtga_tracker=debug cargo run        # verbose logging
```

CLI flags / env (see `src/main.rs`):
- `--log-path` / `MTGA_LOG_PATH`
- `--db-path` / `MTGA_DB_PATH`
- `--bind` / `MTGA_BIND` (default `127.0.0.1:7843`)
- `--no-card-db` (skip Scryfall download; names will be missing)

## Adding a new event handler

1. Find the actual MTGA event name and payload shape by greping the live
   `Player.log` (the `inspect-mtga-events` skill exists for this).
2. Create `src/state/handlers/<name>.rs` with a `handle(state, event)` fn.
3. Add a `pub mod` entry in `src/state/handlers/mod.rs`.
4. Add a dispatch arm in `src/state/mod.rs::ingest`. Prefer prefix-matching
   over exact strings — MTGA renames events between releases.
5. If durable persistence is needed, add a migration under `migrations/`
   (timestamp-prefixed); sqlx runs them automatically on startup.
6. Write a unit test with a realistic payload snippet. The parser/handler
   tests are the canonical examples.

## Conventions

- Handlers are best-effort: log + return Ok on unexpected payloads, never
  panic. The raw event is already saved in `raw_events` so debugging stays
  possible.
- Don't read MTGA's name strings as truth — pass them through
  `state.loc.translate()` first (handles `?=?Loc/` localization keys).
- Frontend has zero build deps. Don't introduce a bundler. If a feature ever
  genuinely needs componentized state, the sanctioned path is vendoring
  Preact + htm as static files (still no build step) — never a compile step.
  Fonts/images are vendored into `web/static/` too; no CDN requests at runtime.
