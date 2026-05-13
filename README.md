# mtga-sleuth

[![Rust](https://github.com/wkennedy/mtga-sleuth/actions/workflows/rust.yml/badge.svg)](https://github.com/wkennedy/mtga-sleuth/actions/workflows/rust.yml)

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

The first run downloads Scryfall's bulk card data (~150 MB on the wire,
filtered to a ~8 MB on-disk JSON). Subsequent launches use the cached subset
(refreshed weekly).

### Pre-built binary (recommended for non-Rust users)

The [Releases](../../releases) page ships a single statically-linked
`x86_64-linux-musl` binary — no Rust toolchain, no runtime deps, works on any
glibc / musl Linux. Each release attaches:

- `mtga-sleuth-<version>-x86_64-linux-musl.tar.gz` — the binary + helper scripts
- `mtga-sleuth-<version>-x86_64-linux-musl.tar.gz.sha256` — checksum for verification

Download, verify, extract, run:

```bash
# 1. Pick a version. Either browse the Releases page or grab the latest tag:
VERSION=$(curl -fsSL https://api.github.com/repos/wkennedy/mtga-sleuth/releases/latest \
  | grep -oP '"tag_name":\s*"\K[^"]+')
ARCHIVE="mtga-sleuth-${VERSION}-x86_64-linux-musl.tar.gz"

# 2. Download the tarball and its checksum.
curl -fSLO "https://github.com/wkennedy/mtga-sleuth/releases/download/${VERSION}/${ARCHIVE}"
curl -fSLO "https://github.com/wkennedy/mtga-sleuth/releases/download/${VERSION}/${ARCHIVE}.sha256"

# 3. Verify the download wasn't tampered with or truncated.
sha256sum -c "${ARCHIVE}.sha256"

# 4. Extract. Creates a directory named after the archive.
tar xzf "${ARCHIVE}"
cd "mtga-sleuth-${VERSION}-x86_64-linux-musl"

# 5. Run it. The binary is already executable; this is just a safety net.
chmod +x mtga-sleuth
./mtga-sleuth
```

Then open <http://127.0.0.1:7843>. Make sure **Detailed Logs (Plugin Support)**
is enabled in MTGA first (see above) or the UI will be empty.

To install system-wide, drop the binary somewhere on your `PATH`:

```bash
sudo install -m 0755 mtga-sleuth /usr/local/bin/
mtga-sleuth   # now runnable from anywhere
```

Release binaries embed an Arena card snapshot, so the first launch works
immediately without contacting Scryfall.

### Build with bundled cards (for distributors)

```bash
cargo build --release --features bundled-cards
```

This makes `build.rs` fetch Scryfall's bulk data at compile time and embed a
filtered Arena-only snapshot into the binary. Useful when shipping releases —
end users get card names and images without an extra ~8 MB download on first
launch.

### Configuration

All settings can be passed as flags or environment variables:

| Flag             | Env var          | Default                                          |
| ---------------- | ---------------- | ------------------------------------------------ |
| `--log-path`     | `MTGA_LOG_PATH`  | Auto-detected from common Steam install paths    |
| `--data-dir`     | `MTGA_DATA_DIR`  | Auto-detected MTGA install (for localization)    |
| `--bind`         | `MTGA_BIND`      | `127.0.0.1:7843`                                 |
| `--db-path`      | `MTGA_DB_PATH`   | `~/.local/share/mtga-sleuth/tracker.sqlite`     |
| `--no-card-db`   | —                | Skip Scryfall download (names only)              |

Increase log verbosity with `RUST_LOG=mtga_tracker=debug`.

### Running with a non-Steam Wine prefix (Lutris / Bottles / standalone Wine)

Auto-detection only knows about Steam Proton's standard layouts. If you run
MTGA via Lutris, Bottles, or a custom Wine prefix, point the tracker at
`Player.log` (and the install dir, for localization) directly:

```bash
# Lutris (default magic-the-gathering-arena install):
mtga-sleuth \
  --log-path "$HOME/Games/magic-the-gathering-arena/drive_c/users/$USER/AppData/LocalLow/Wizards Of The Coast/MTGA/Player.log" \
  --data-dir "$HOME/Games/magic-the-gathering-arena/drive_c/Program Files/Wizards of the Coast/MTGA/MTGA_Data/Downloads/Raw"

# Bottles (substitute your bottle name):
mtga-sleuth \
  --log-path "$HOME/.var/app/com.usebottles.bottles/data/bottles/bottles/MTGA/drive_c/users/$USER/AppData/LocalLow/Wizards Of The Coast/MTGA/Player.log"
```

Without `--data-dir` the tracker still works, but deck-name strings using
MTGA's `?=?Loc/...` localization keys won't be translated to English.

### Offline assets

To run fully offline (no Scryfall round trips for card images or mana symbols):

```bash
scripts/download_assets.py            # symbols + small + normal images (~2.7 GB)
scripts/download_assets.py --sizes small   # just thumbnails (~640 MB)
scripts/download_assets.py --symbols-only  # mana SVGs only (~150 KB)
```

Files are stored under `~/.cache/mtga-sleuth/assets/`. The tracker's
`/cdn/{*}` route serves from this directory when files are present and
falls back to Scryfall otherwise — the UI works the same either way.

### Default paths checked

When `--log-path` / `--data-dir` are not set, the tracker tries these Steam
roots in order, then suffixes the per-feature path under each:

1. `~/snap/steam/common/.local/share/Steam/...` (Snap Steam, primary)
2. `~/.steam/steam/...`
3. `~/.local/share/Steam/...`
4. `~/.var/app/com.valvesoftware.Steam/data/Steam/...` (Flatpak)

| Feature        | Suffix appended under each Steam root                                                      |
| -------------- | ------------------------------------------------------------------------------------------ |
| `Player.log`   | `steamapps/compatdata/2141910/pfx/drive_c/users/steamuser/AppData/LocalLow/Wizards Of The Coast/MTGA/Player.log` |
| MTGA data dir  | `steamapps/common/MTGA/MTGA_Data/Downloads/Raw`                                            |

For non-Steam Wine prefixes (Lutris, Bottles, manual prefix) see the section
above for explicit `--log-path` / `--data-dir` examples.

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
