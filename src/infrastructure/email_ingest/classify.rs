/// Classifies extracted PDF text as either a cost invoice or a bank
/// transfer confirmation. Bank confirmations are skipped entirely — they
/// document a payment that was already made, not a new cost.
///
/// Polish bank transfer confirmations have very distinctive markers:
/// - "Potwierdzenie przelewu" / "Potwierdzenie operacji" in the title
/// - Bank names: mBank, PKO, PKO BP, Santander, ING, Alior, Millennium, Pekao
/// - IBAN numbers (PL + 26 digits)
/// - "Nadawca" / "Odbiorca" sections
/// - "Kwota przelewu" / "Kwota operacji"
/// - "Tytuł przelewu"
///
/// Cost invoices have different markers:
/// - "Faktura" / "Faktura VAT" / "Rachunek"
/// - NIP numbers (10 digits)
/// - "Netto" / "VAT" / "Brutto" breakdown
/// - Invoice numbers (FV/2024/01/123)
/// - Vendor names and addresses

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentClass {
    Invoice,
    BankConfirmation,
    Unparseable,
}

/// Bank confirmation indicators — if any of these appear, the document is
/// classified as a bank confirmation. These are highly specific phrases that
/// do not appear on cost invoices.
const BANK_CONFIRMATION_MARKERS: &[&str] = &[
    "potwierdzenie przelewu",
    "potwierdzenie operacji",
    "potwierdzenie wykonania operacji",
    "potwierdzenie zlecenia",
    "dowód księgowania",
    "potwierdzenie transakcji",
];

/// Bank names that appear on Polish bank transfer confirmations. These are
/// checked case-insensitively and only reinforce the classification — a
/// bank name alone without other markers is not enough.
const BANK_NAMES: &[&str] = &[
    "mbank",
    "pko bp",
    "pko bank",
    "santander",
    "ing bank",
    "alior bank",
    "bank millennium",
    "bank pekao",
    "bnp paribas",
    "bank pocztowy",
    "credit agricole",
    "nest bank",
    "velo bank",
];

/// Invoice indicators — if any of these appear, the document is likely an
/// invoice. Combined with the absence of bank confirmation markers, this
/// is a strong signal.
const INVOICE_MARKERS: &[&str] = &[
    "faktura",
    "faktura vat",
    "rachunek",
    "f-vat",
    "fv/",
    "fv ",
    "nr faktury",
    "numer faktury",
    "nip:",
    "nip ",
    "regon",
    "netto",
    "brutto",
    "vat%",
    "stawka vat",
    "kwota netto",
    "kwota brutto",
    "kwota vat",
    "sprzedawca",
    "nabywca",
    "termin płatności",
    "sposób płatności",
];

pub fn classify(text: &str) -> DocumentClass {
    let lower = text.to_lowercase();

    // Bank confirmations are checked first — they are the most distinctive.
    let bank_marker_hits = BANK_CONFIRMATION_MARKERS
        .iter()
        .filter(|marker| lower.contains(*marker))
        .count();

    if bank_marker_hits > 0 {
        return DocumentClass::BankConfirmation;
    }

    // A bank name + IBAN pattern + "nadawca"/"odbiorca" is also a strong
    // bank confirmation signal, even without the exact "potwierdzenie" phrase.
    let has_bank_name = BANK_NAMES.iter().any(|name| lower.contains(name));
    let has_iban = lower.contains("pl") && iban_pattern_match(&lower);
    let has_sender_receiver = lower.contains("nadawca") || lower.contains("odbiorca");

    if has_bank_name && has_iban && has_sender_receiver {
        return DocumentClass::BankConfirmation;
    }

    // Invoice markers.
    let invoice_hits = INVOICE_MARKERS
        .iter()
        .filter(|marker| lower.contains(*marker))
        .count();

    if invoice_hits >= 2 {
        return DocumentClass::Invoice;
    }

    // If we have at least some invoice-like text but not enough markers,
    // still try to parse it — better to attempt and fail than to skip a
    // real invoice.
    if invoice_hits >= 1 {
        return DocumentClass::Invoice;
    }

    DocumentClass::Unparseable
}

fn iban_pattern_match(text: &str) -> bool {
    // Polish IBAN: PL + 26 digits, possibly with spaces. Text is already
    // lowercased by the caller, so we check for "pl" followed by digits.
    let stripped = text.replace(' ', "");
    stripped.contains("pl") && {
        let after_pl = stripped
            .find("pl")
            .map(|pos| &stripped[pos + 2..])
            .unwrap_or("");
        after_pl
            .chars()
            .take(26)
            .filter(|c| c.is_ascii_digit())
            .count()
            >= 24
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_bank_confirmation_by_marker() {
        let text = "Potwierdzenie przelewu\nNadawca: Wojciech Bator\nKwota: 1234,56 PLN";
        assert_eq!(classify(text), DocumentClass::BankConfirmation);
    }

    #[test]
    fn classifies_bank_confirmation_by_bank_name_and_iban() {
        let text = "mBank\nOdbiorca: Thomann\nPL 11 1140 2004 0000 3502 7654 3210\nKwota operacji: 500,00 zł";
        assert_eq!(classify(text), DocumentClass::BankConfirmation);
    }

    #[test]
    fn classifies_invoice_by_markers() {
        let text = "Faktura VAT nr FV/2024/01/123\nSprzedawca: Thomann GmbH\nNIP: DE123456789\nNetto: 100,00\nVAT 23%: 23,00\nBrutto: 123,00";
        assert_eq!(classify(text), DocumentClass::Invoice);
    }

    #[test]
    fn classifies_minimal_invoice() {
        let text = "Rachunek za paliwo\nOrlen\nBrutto: 250,00 zł";
        assert_eq!(classify(text), DocumentClass::Invoice);
    }

    #[test]
    fn unparseable_for_empty_text() {
        assert_eq!(classify(""), DocumentClass::Unparseable);
        assert_eq!(
            classify("random text without invoice markers"),
            DocumentClass::Unparseable
        );
    }
}
