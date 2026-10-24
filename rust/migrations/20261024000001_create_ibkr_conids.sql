-- IBKR catalogue identity (Ibkr plan R1, D-IBKR-CONIDS): each conid the catalogue sync has imported, and the asset it was
-- imported for. Append-only: a conid never moves to another asset; a feed row that tries is skipped and counted.
CREATE TABLE ibkr_conids (
  conid INTEGER PRIMARY KEY,
  asset_id INTEGER NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TRIGGER ibkr_conids_no_update BEFORE UPDATE ON ibkr_conids BEGIN SELECT RAISE(ABORT, 'ibkr_conids is append-only'); END;
CREATE TRIGGER ibkr_conids_no_delete BEFORE DELETE ON ibkr_conids BEGIN SELECT RAISE(ABORT, 'ibkr_conids is append-only'); END;
