-- Distinguish the player's own decks from precon/reference decks. MTGA mixes
-- both into StartHook's Decks map (precons carry ?=?Loc/Decks/Precon/* name
-- keys), and DeckGetAllPreconDecksV3 is the full in-game precon catalog.
ALTER TABLE decks ADD COLUMN origin TEXT NOT NULL DEFAULT 'personal';

-- Backfill existing rows: precons are identifiable by their unlocalized name
-- keys; everything else stays 'personal' until re-ingested with real
-- classification on the next StartHook.
UPDATE decks SET origin = 'precon' WHERE name LIKE '?=?Loc/Decks/Precon%';
