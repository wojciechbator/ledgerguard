-- Audit trail for the email-OCR cost ingestion pipeline.
-- Each PDF attachment processed from the sent-email inbox gets one row,
-- regardless of whether it was classified as an invoice or skipped as a
-- bank confirmation. The SHA-256 content hash is the dedup key: re-running
-- the pipeline never re-processes a PDF already seen.
CREATE TABLE ingested_documents (
    id UUID PRIMARY KEY,
    content_hash TEXT NOT NULL UNIQUE,
    filename TEXT NOT NULL,
    email_date TIMESTAMPTZ NOT NULL,
    email_subject TEXT NOT NULL,
    email_recipient TEXT NOT NULL,
    classification TEXT NOT NULL CHECK (classification IN ('invoice', 'bank_confirmation', 'unparseable')),
    extracted_text TEXT,
    vendor TEXT,
    invoice_date DATE,
    gross NUMERIC(20, 2),
    net NUMERIC(20, 2),
    vat NUMERIC(20, 2),
    vat_rate NUMERIC(5, 2),
    ledger_entry_id UUID REFERENCES ledger_entries(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX ingested_documents_email_date_idx ON ingested_documents (email_date DESC);
CREATE INDEX ingested_documents_classification_idx ON ingested_documents (classification);
