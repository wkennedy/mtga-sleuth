-- Arena card id of the deck's tile/box art, from StartHook's
-- DeckSummaries[].DeckTileId. NULL for decks ingested before this migration
-- (heals on next StartHook) and for API-created decks (UI falls back to the
-- deck's best card).
ALTER TABLE decks ADD COLUMN tile_card_id INTEGER;
