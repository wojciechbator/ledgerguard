# LedgerGuard

**Cash-planning for small businesses. How much can I safely spend right now?**

LedgerGuard combines current cash, committed costs, tax/VAT/ZUS reserves, a minimum cash buffer, and planned purchases into a deterministic planning result. It answers one practical question that accounting platforms don't: given what I know today, what's my safe spending headroom?

## What it does

- **Normalized read model** — imports invoices, receipts, and commitments from accounting platforms into a neutral format. The accounting platform remains the source of truth; LedgerGuard stores a normalized copy plus planning policy.
- **Cash planning** — calculates safe spending headroom from current cash minus committed costs, reserves, buffer, and planned purchases. The result is deterministic: same inputs, same answer, every time.
- **Provider sync** — connects to SaldeoSMART, Fakturownia/InvoiceOcean, inFakt, and wFirma. Imports are idempotent: corrected provider data updates the normalized record rather than duplicating it.

## What it solves

Accounting platforms tell you what happened. They don't tell you what you can safely do next. A small business owner staring at their bank balance doesn't know if that number is safe to spend — there are pending invoices, tax reserves, ZUS payments, and planned purchases that haven't hit yet. LedgerGuard bridges that gap without becoming a second accounting system.

The key distinction: **accounting truth, imported evidence, and planning policy are three different concepts.** A convenience tool cannot quietly become a second accounting system. Money is handled as decimal strings end-to-end — no binary floating point, ever.

## Status

In production. The planner domain, provider-neutral accounting port, PostgreSQL persistence, sync validation, HTTP API, container deployment, and CI gates are all present. Four accounting adapters are wired.
