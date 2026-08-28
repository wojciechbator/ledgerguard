//! Integration tests for the email-OCR ingestion pipeline.
//!
//! These tests exercise the classify → parse → vendor-classify chain
//! without requiring IMAP or PostgreSQL. They use realistic invoice
//! text extracted from Polish and German cost invoices.

use ledgerguard::infrastructure::email_ingest::{classify, parse, vendor};

#[test]
fn full_pipeline_polish_invoice_with_vat_breakdown() {
    let text = r#"
        Faktura VAT nr FV/2024/03/042
        Sprzedawca: Thomann GmbH
        Data wystawienia: 2024-03-15
        NIP: DE123456789

        Netto: 1 000,00 zł
        VAT 23%: 230,00 zł
        Brutto: 1 230,00 zł
    "#;

    // Classify
    let class = classify::classify(text);
    assert_eq!(class, classify::DocumentClass::Invoice);

    // Parse
    let parsed = parse::parse_invoice(text);
    assert_eq!(parsed.gross, Some(rust_decimal::Decimal::new(123000, 2)));
    assert_eq!(parsed.net, Some(rust_decimal::Decimal::new(100000, 2)));
    assert_eq!(parsed.vat, Some(rust_decimal::Decimal::new(23000, 2)));
    assert_eq!(
        parsed.invoice_date,
        chrono::NaiveDate::from_ymd_opt(2024, 3, 15)
    );

    // Vendor classification
    let vc = vendor::classify_vendor(parsed.vendor.as_deref(), text);
    assert_eq!(vc.vendor.as_deref(), Some("Thomann"));
    assert_eq!(vc.category.as_deref(), Some("equipment"));
}

#[test]
fn full_pipeline_gas_station_receipt_gross_only() {
    let text = r#"
        Orlen Polska Sp. z o.o.
        Rachunek za paliwo
        Data: 15.03.2024
        Stacja: Warszawa, ul. Złota 44

        PB95 40L x 6,45 zł = 258,00 zł
        Brutto: 258,00 zł
        VAT 23%
    "#;

    let class = classify::classify(text);
    assert_eq!(class, classify::DocumentClass::Invoice);

    let parsed = parse::parse_invoice(text);
    assert!(parsed.gross.is_some());
    assert_eq!(parsed.vat_rate, Some(rust_decimal::Decimal::new(23, 0)));

    // Net and VAT should be derived from gross + rate
    assert!(parsed.net.is_some());
    assert!(parsed.vat.is_some());

    let vc = vendor::classify_vendor(parsed.vendor.as_deref(), text);
    assert_eq!(vc.vendor.as_deref(), Some("Orlen"));
    assert_eq!(vc.category.as_deref(), Some("fuel"));
}

#[test]
fn full_pipeline_bank_confirmation_is_skipped() {
    let text = r#"
        Potwierdzenie przelewu
        Nadawca: Wojciech Bator
        Odbiorca: Thomann GmbH
        mBank
        PL 11 1140 2004 0000 3502 7654 3210
        Kwota przelewu: 1 230,00 PLN
        Tytuł: Faktura FV/2024/03/042
    "#;

    let class = classify::classify(text);
    assert_eq!(class, classify::DocumentClass::BankConfirmation);

    // Even though this text has invoice-like markers, the bank confirmation
    // classification takes priority and the document is skipped.
}

#[test]
fn full_pipeline_german_thomann_invoice() {
    let text = r#"
        Thomann GmbH
        Burgeberg 4
        96148 Baunach
        Germany

        Rechnungsdatum: 15.03.2024
        Rechnungsnummer: TH-2024-12345

        Netto-Betrag: 100,00 EUR
        zzgl. 19% USt: 19,00 EUR
        Gesamtbetrag: 119,00 EUR
    "#;

    let class = classify::classify(text);
    assert_eq!(class, classify::DocumentClass::Invoice);

    let parsed = parse::parse_invoice(text);
    assert_eq!(parsed.gross, Some(rust_decimal::Decimal::new(11900, 2)));
    assert_eq!(parsed.net, Some(rust_decimal::Decimal::new(10000, 2)));
    assert_eq!(parsed.vat, Some(rust_decimal::Decimal::new(1900, 2)));
    assert_eq!(parsed.vat_rate, Some(rust_decimal::Decimal::new(19, 0)));

    let vc = vendor::classify_vendor(parsed.vendor.as_deref(), text);
    assert_eq!(vc.vendor.as_deref(), Some("Thomann"));
    assert_eq!(vc.category.as_deref(), Some("equipment"));
}

#[test]
fn full_pipeline_software_subscription() {
    let text = r#"
        JetBrains s.r.o.
        Kavci Hory 2161/6
        Praha 4

        Faktura: JetBrains-2024-03-001
        Data: 2024-03-01

        Subscription renewal: All Products Pack
        Netto: 239,00 EUR
        VAT 21%: 50,19 EUR
        Brutto: 289,19 EUR
    "#;

    let class = classify::classify(text);
    assert_eq!(class, classify::DocumentClass::Invoice);

    let parsed = parse::parse_invoice(text);
    assert!(parsed.gross.is_some());

    let vc = vendor::classify_vendor(parsed.vendor.as_deref(), text);
    assert_eq!(vc.vendor.as_deref(), Some("JetBrains"));
    assert_eq!(vc.category.as_deref(), Some("software"));
}

#[test]
fn full_pipeline_utility_bill() {
    let text = r#"
        PGE Dystrybucja S.A.
        Faktura nr PGE/2024/02/789
        Data wystawienia: 2024-02-20

        Zużycie prądu: 350 kWh
        Netto: 180,00 zł
        VAT 23%: 41,40 zł
        Brutto: 221,40 zł
    "#;

    let class = classify::classify(text);
    assert_eq!(class, classify::DocumentClass::Invoice);

    let parsed = parse::parse_invoice(text);
    assert_eq!(parsed.gross, Some(rust_decimal::Decimal::new(22140, 2)));

    let vc = vendor::classify_vendor(parsed.vendor.as_deref(), text);
    assert_eq!(vc.vendor.as_deref(), Some("PGE"));
    assert_eq!(vc.category.as_deref(), Some("utilities"));
}

#[test]
fn bank_confirmation_with_iban_but_no_marker_phrase() {
    // Some bank confirmations don't say "Potwierdzenie przelewu" but still
    // have the bank name + IBAN + sender/receiver structure.
    let text = r#"
        mBank S.A.
        Odbiorca: Thomann GmbH
        PL 11 1140 2004 0000 3502 7654 3210
        Kwota operacji: 119,00 EUR
        Data operacji: 2024-03-16
    "#;

    let class = classify::classify(text);
    assert_eq!(class, classify::DocumentClass::BankConfirmation);
}

#[test]
fn duplicate_invoice_same_amount_different_vendor_is_not_confused() {
    // Two invoices with the same gross amount but different vendors should
    // parse independently — the vendor classification distinguishes them.
    let text1 = "Orlen\nBrutto: 250,00 zł\nVAT 23%";
    let text2 = "Shell\nBrutto: 250,00 zł\nVAT 23%";

    let vc1 = vendor::classify_vendor(parse::parse_invoice(text1).vendor.as_deref(), text1);
    let vc2 = vendor::classify_vendor(parse::parse_invoice(text2).vendor.as_deref(), text2);

    assert_eq!(vc1.vendor.as_deref(), Some("Orlen"));
    assert_eq!(vc2.vendor.as_deref(), Some("Shell"));
    assert_eq!(vc1.category, vc2.category); // both "fuel"
}
