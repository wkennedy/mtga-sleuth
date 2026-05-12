CREATE TABLE matches (
    match_id TEXT PRIMARY KEY,
    started_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    ended_at TEXT,
    player_seat INTEGER,
    opponent_screen_name TEXT,
    opponent_rank TEXT,
    deck_id TEXT,
    event_name TEXT,
    won INTEGER
);

CREATE INDEX matches_started_at_idx ON matches(started_at DESC);

CREATE TABLE decks (
    deck_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    format TEXT,
    last_updated TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE deck_cards (
    deck_id TEXT NOT NULL,
    card_id INTEGER NOT NULL,
    quantity INTEGER NOT NULL,
    sideboard INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (deck_id, card_id, sideboard),
    FOREIGN KEY (deck_id) REFERENCES decks(deck_id) ON DELETE CASCADE
);

CREATE TABLE collection (
    card_id INTEGER PRIMARY KEY,
    quantity INTEGER NOT NULL
);

CREATE TABLE drafts (
    draft_id TEXT PRIMARY KEY,
    set_code TEXT,
    started_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    completed_at TEXT
);

CREATE TABLE draft_picks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    draft_id TEXT NOT NULL,
    pack_number INTEGER NOT NULL,
    pick_number INTEGER NOT NULL,
    picked_card_id INTEGER NOT NULL,
    pack_card_ids TEXT NOT NULL,
    picked_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (draft_id) REFERENCES drafts(draft_id) ON DELETE CASCADE
);

CREATE INDEX draft_picks_draft_idx ON draft_picks(draft_id, pack_number, pick_number);

CREATE TABLE raw_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp TEXT NOT NULL,
    kind TEXT NOT NULL,
    direction TEXT NOT NULL,
    payload TEXT NOT NULL
);

CREATE INDEX raw_events_timestamp_idx ON raw_events(timestamp DESC);
CREATE INDEX raw_events_kind_idx ON raw_events(kind);
