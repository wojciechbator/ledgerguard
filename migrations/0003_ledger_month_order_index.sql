DROP INDEX IF EXISTS ledger_entries_booked_on_idx;
CREATE INDEX ledger_entries_booked_on_id_idx ON ledger_entries (booked_on, id);
