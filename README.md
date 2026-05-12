# mtga-tracker

A local **Magic: the Gathering Arena** tracker for Linux. Watches MTGA's
`Player.log`, parses game / deck / draft / collection events, and serves a
local web UI you open in your browser.

Inspired by [mtgatool-desktop](https://github.com/mtgatool/mtgatool-desktop),
[Untapped Companion](https://mtga.untapped.gg/companion), and [17lands](https://www.17lands.com/) —
but built native for Linux without Electron, Wine, or game-overlay shenanigans.

## Status

MVP, work-in-progress. The pieces in place:

- Live deck tracker (life totals, turn, opponent revealed)
- Match history
- Collection viewer (with Scryfall card images)
- Decks browser (mainboard / sideboard)
- Drafts browser (pack / pick history)
- Raw event inspector (for debugging the parser)

## Requirements

- Linux
- Rust 1.75+ (`cargo`, `rustc`)
- MTGA installed via Steam + Proton (the default-path heuristic targets the
  Snap Steam install at app id `2141910`; other layouts are auto-detected, and
  any path can be overridden with `--log-path`)
- ~150 MB disk space for the cached Scryfall card data (downloaded on first
  launch)

## ⚠️  Enable Detailed Logs in MTGA

**Before running the tracker**, you must enable MTGA's detailed log output.
Without it, `Player.log` only contains Unity boilerplate and the tracker will
have nothing to parse:

1. Launch MTG Arena
2. Click the gear icon → **Account** tab
3. Enable **"Detailed Logs (Plugin Support)"**
4. Restart MTGA so the new log format takes effect

If you forget, the **Events** tab in the tracker UI will be empty and the
console log will keep saying _"no events found in payload."_

## Build & run

```bash
cargo run --release
```

Then open http://127.0.0.1:7843 in your browser.

The first run downloads Scryfall's bulk card data (~150 MB) and filters it
down to Arena-only cards. Subsequent launches use the cached subset (refreshed
weekly).

### Configuration

All settings can be passed as flags or environment variables:

| Flag             | Env var          | Default                             |
| ---------------- | ---------------- | ----------------------------------- |
| `--log-path`     | `MTGA_LOG_PATH`  | Auto-detected Snap-Steam path       |
| `--bind`         | `MTGA_BIND`      | `127.0.0.1:7843`                    |
| `--db-path`      | `MTGA_DB_PATH`   | `~/.local/share/mtga-tracker/tracker.sqlite` |
| `--no-card-db`   | —                | Skip Scryfall download (names only) |

Increase log verbosity with `RUST_LOG=mtga_tracker=debug`.

### Default log paths checked

When `--log-path` is not set, the tracker tries these in order:

1. `~/snap/steam/common/.local/share/Steam/steamapps/compatdata/2141910/...` (Snap Steam, primary)
2. `~/.steam/steam/steamapps/compatdata/2141910/...`
3. `~/.local/share/Steam/steamapps/compatdata/2141910/...`
4. `~/.var/app/com.valvesoftware.Steam/data/Steam/steamapps/compatdata/2141910/...` (Flatpak)

The full file is `…/pfx/drive_c/users/steamuser/AppData/LocalLow/Wizards Of The Coast/MTGA/Player.log`.

If you use Lutris, Bottles, or a custom Wine prefix, pass `--log-path` directly.

## Architecture

```
            ┌─────────────────┐
Player.log →│  log_watcher    │  tail + rotation handling
            └────────┬────────┘
                     │ raw lines (mpsc)
            ┌────────▼────────┐
            │  parser         │  brace-counted JSON extractor
            └────────┬────────┘
                     │ ParsedEvent
            ┌────────▼────────┐
            │  state engine   │ ──→ SQLite (durable)
            │                 │ ──→ broadcast (SSE subscribers)
            └────────┬────────┘
                     │
            ┌────────▼────────┐
            │  axum web       │  REST + SSE + embedded static UI
            └─────────────────┘
```

Event handlers live in `src/state/handlers/` — one per MTGA event family
(`decks`, `collection`, `match_room`, `gre`, `draft`). They're best-effort:
unknown payload shapes are logged and dropped without aborting the tracker.

## Limitations / TODO

- The deck-tracker library view is wired through but not yet populated from
  GRE messages; it needs the deck list cross-referenced with `gameObjects`
  zone changes. **In progress.**
- Match win/loss attribution uses a heuristic (`teamId == 1` is the local
  player) — should read `systemSeatIds` from the room config.
- No draft pick recommendations yet (would need 17lands data).
- No graceful migration when MTGA changes log payload shapes.

## Development

```bash
cargo test          # unit tests, mostly in src/parser/extract.rs
cargo run -- --help # CLI flags
RUST_LOG=mtga_tracker=debug,sqlx=warn cargo run
```

The web UI lives in `web/static/` and is embedded into the binary via
`rust-embed`. Edit and rebuild — no separate frontend toolchain.
