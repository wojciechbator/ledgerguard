CREATE TABLE ledger_entries (
    id UUID PRIMARY KEY,
    external_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('expense', 'revenue')),
    booked_on DATE NOT NULL,
    gross NUMERIC(20, 2) NOT NULL CHECK (gross >= 0),
    net NUMERIC(20, 2) CHECK (net IS NULL OR net >= 0),
    vat NUMERIC(20, 2) CHECK (vat IS NULL OR vat >= 0),
    category TEXT,
    counterparty TEXT,
    source TEXT NOT NULL CHECK (source IN ('saldeo', 'manual')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (source, external_id)
);

CREATE INDEX ledger_entries_booked_on_idx ON ledger_entries (booked_on);
CREATE INDEX ledger_entries_category_idx ON ledger_entries (category) WHERE category IS NOT NULL;

CREATE TABLE monthly_budgets (
    month_start DATE NOT NULL,
    category TEXT NOT NULL,
    limit_amount NUMERIC(20, 2) NOT NULL CHECK (limit_amount >= 0),
    PRIMARY KEY (month_start, category),
    CHECK (date_part('day', month_start) = 1)
);

CREATE TABLE cash_snapshots (
    id UUID PRIMARY KEY,
    observed_at TIMESTAMPTZ NOT NULL,
    balance NUMERIC(20, 2) NOT NULL,
    source TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE sync_runs (
    id UUID PRIMARY KEY,
    source TEXT NOT NULL,
    started_at TIMESTAMPTZ NOT NULL,
    finished_at TIMESTAMPTZ,
    status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed')),
    imported_count INTEGER NOT NULL DEFAULT 0 CHECK (imported_count >= 0),
    error_message TEXT
);
