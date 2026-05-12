---
name: verify-tracker-runs
description: Build, launch, and end-to-end smoke-test the tracker. Use after non-trivial changes to confirm the binary still boots, opens its log/DB, and serves the API. Always cleans up its background process and test database.
---

# Verify tracker runs end-to-end

The tracker has many moving parts (log watcher, parser, state engine, axum,
SQLite migrations). Use this whenever you've changed anything in `src/` and
want a quick "does it actually still work" check, before claiming a task is
done.

## Recipe

Use a non-default port and a temp DB so we don't clobber the user's real
state. Always clean up.

```bash
cargo build --release 2>&1 | tail -3

# Pick a port that isn't 7843 (the user might have their own tracker
# running on the default), and a temp DB so we don't touch their data.
PORT=7848
TESTDB=/tmp/test-mtga-$$.sqlite
/home/waggins/projects/mtga-tracker/target/release/mtga-tracker \
  --no-card-db --bind 127.0.0.1:$PORT --db-path $TESTDB 2>&1 &
SP=$!
trap "kill \$SP 2>/dev/null; rm -f $TESTDB*" EXIT

# Wait for HTTP to come up (don't sleep an arbitrary amount).
until curl -sf http://127.0.0.1:$PORT/api/health >/dev/null 2>&1; do sleep 1; done
sleep 5  # give the log replay a moment to ingest

# Probe the endpoints you actually changed:
curl -s http://127.0.0.1:$PORT/api/health
curl -s http://127.0.0.1:$PORT/api/wallet
curl -s http://127.0.0.1:$PORT/api/decks | python3 -c "import sys,json;d=json.load(sys.stdin);print(f'{len(d)} decks')"
curl -s http://127.0.0.1:$PORT/api/matches | python3 -c "import sys,json;d=json.load(sys.stdin);print(f'{len(d)} matches')"

kill $SP; rm -f $TESTDB*
```

## What to look for

- **`web server listening` log line** — if it doesn't appear, axum failed
  to bind (port in use, syntax error in routes). Check the captured stderr.
- **Migrations succeed** (`sqlite ready`) — if not, a recent migration has
  bad SQL.
- **Each handler logs as it runs** at INFO level — `wallet updated`,
  `ingesting decks count=192`, `match started/ended`. Missing handlers
  here mean the event-name dispatch in `src/state/mod.rs` isn't matching.
- **API endpoints return non-empty arrays** if the underlying log has
  data. Empty `decks` despite a healthy log usually means a handler-side
  bug (extracted nothing from the payload).

## When to use `--no-card-db`

Always use `--no-card-db` for smoke tests. The Scryfall download is ~150
MB and irrelevant to verifying the tracker boots. The card lookup will
fall back to "Card #<id>" placeholders, which is fine for smoke checks.

## Don't do this

- Don't bind to `127.0.0.1:7843` (the default) — the user may already have
  their own instance running there.
- Don't reuse `~/.local/share/mtga-tracker/tracker.sqlite` — that's the
  user's real data. Always pass `--db-path /tmp/...`.
- Don't `rm` the user's real DB unless they explicitly asked. The auto-mode
  classifier will block it anyway.
- Don't run for longer than ~30s. If something hangs that's the bug;
  killing and inspecting is the right move.
