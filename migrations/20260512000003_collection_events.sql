-- Audit trail of every collection delta (booster open, draft pick, reward,
-- paste import). The `collection` table holds running totals; this table
-- captures provenance so we can debug or rebuild totals if needed.
CREATE TABLE collection_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    card_id INTEGER NOT NULL,
    delta INTEGER NOT NULL,
    source TEXT NOT NULL,           -- 'booster' | 'draft' | 'reward' | 'import' | 'inventory_change' | ...
    context TEXT                    -- free-form: booster set code, draft id, etc.
);
CREATE INDEX collection_events_card_idx ON collection_events(card_id);
CREATE INDEX collection_events_ts_idx ON collection_events(timestamp DESC);
CREATE INDEX collection_events_source_idx ON collection_events(source);
