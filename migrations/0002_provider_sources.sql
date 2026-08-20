ALTER TABLE ledger_entries
    DROP CONSTRAINT IF EXISTS ledger_entries_source_check;

ALTER TABLE ledger_entries
    ADD CONSTRAINT ledger_entries_source_check
    CHECK (
        char_length(source) BETWEEN 1 AND 32
        AND source ~ '^[a-z0-9_-]+$'
    );
