ALTER TABLE ledger_entries
    DROP CONSTRAINT IF EXISTS ledger_entries_source_check;

ALTER TABLE ledger_entries
    ADD CONSTRAINT ledger_entries_source_check
    CHECK (source IN ('saldeo', 'fakturownia', 'infakt', 'wfirma', 'manual'));
