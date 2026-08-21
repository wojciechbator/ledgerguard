-- Category reads are not part of the current repository contract. Keeping this
-- index would add write amplification and disk/WAL churn to every accounting
-- import without serving a live query. Re-add a purpose-built index alongside
-- the first category-filtered read path.
DROP INDEX IF EXISTS ledger_entries_category_idx;
