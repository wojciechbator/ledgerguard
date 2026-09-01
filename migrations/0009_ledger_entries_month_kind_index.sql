-- Composite index for month-range + kind queries (dashboard month view,
-- affordability checks). The existing index on booked_on alone forces a
-- filter on kind after the range scan; this composite lets Postgres do
-- an index-only scan for the common "expenses in this month" query.
CREATE INDEX ledger_entries_booked_on_kind_idx
    ON ledger_entries (booked_on DESC, kind);
