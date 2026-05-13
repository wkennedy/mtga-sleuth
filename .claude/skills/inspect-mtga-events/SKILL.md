---
name: inspect-mtga-events
description: Survey what events MTGA is currently emitting in Player.log and identify the payload shape of a specific event. Use when a tracker handler isn't firing, when adding support for a new event type, when MTGA's log format may have changed, or when the user asks why a particular feature has no data.
---

# Inspect MTGA events

A repeatable workflow for surveying the live Player.log when the tracker
isn't picking up an expected feature.

## Step 1 — confirm Detailed Logs are enabled

```bash
PLOG="$HOME/snap/steam/common/.local/share/Steam/steamapps/compatdata/2141910/pfx/drive_c/users/steamuser/AppData/LocalLow/Wizards Of The Coast/MTGA/Player.log"
grep "DETAILED LOGS" "$PLOG"
```

If you see `DETAILED LOGS: DISABLED`, stop. The user must enable
**Account → Detailed Logs (Plugin Support)** in MTGA and restart the game.
No event handlers can fire without this. Tell them and stop.

## Step 2 — enumerate every event name

```bash
grep -oE "(==>|<==) [A-Za-z0-9_]+" "$PLOG" | sort -u
```

Cross-reference this list with the dispatch arms in `src/state/mod.rs`. If
the event you expect to handle isn't in the output, MTGA isn't emitting it
in this session — usually because the user hasn't navigated to the
relevant in-game screen (Decks/Collection/Drafts) or hasn't performed the
triggering action (open a booster, finish a quest).

Also check `Player-prev.log` next to it — some events fire once per
session at startup.

## Step 3 — drill into a specific event's payload

Find the response line and the JSON body that follows it:

```bash
LINE=$(grep -n "^<== EventName" "$PLOG" | head -1 | cut -d: -f1)
sed -n "$((LINE+1))p" "$PLOG" | python3 -c "
import sys, json
d = json.loads(sys.stdin.read())
print('top-level keys:', list(d.keys())[:30])
# For each, print type and size
for k, v in d.items():
    if isinstance(v, (list, dict)):
        print(f'  {k}: {type(v).__name__} len={len(v)}')
    else:
        print(f'  {k}: {type(v).__name__} = {str(v)[:80]}')
"
```

For nested structures, drill further by extracting one entry and recursing.
**Don't dump the whole payload into the conversation** — MTGA payloads can
be hundreds of KB. Always slice and summarize.

## Step 4 — check the tracker's DB for what got ingested

```bash
python3 -c "
import sqlite3
con = sqlite3.connect('$HOME/.local/share/mtga-sleuth/tracker.sqlite')
for row in con.execute('SELECT kind, COUNT(*) FROM raw_events GROUP BY kind ORDER BY 2 DESC LIMIT 20'):
    print(row)
"
```

If the event name appears in `raw_events` but the corresponding typed
table (decks/matches/collection_events/etc.) is empty, the parser is
catching it but the handler isn't extracting from the payload — focus
debugging on `src/state/handlers/<file>.rs`.

## Step 5 — quirks to watch for

- Event names changed from dotted (`Deck.UpsertDeckV3`) to CamelCase
  (`DeckUpsertDeckV3`) at some point. Old tracker docs are stale.
- Response lines are bare `<== Foo(uuid)` on their own line, preceded by
  a `[UnityCrossThreadLogger]<timestamp>` preamble. They do NOT carry the
  `[UnityCrossThreadLogger]` prefix themselves. The parser handles this.
- Match flow uses `Match to <playerId>:` markers, not the older
  `Match to GRE` / `GRE to Client` strings.
- Per-card collection counts are NOT emitted by detailed logs. Don't
  spend time hunting for an `InventoryGetCards`-style event — there isn't
  one. Use `inventory_changes.rs`'s incremental approach or paste-import.
