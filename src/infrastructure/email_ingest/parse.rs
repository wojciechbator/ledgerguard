use chrono::NaiveDate;
use regex::Regex;
use rust_decimal::Decimal;
use std::sync::OnceLock;

/// Parsed invoice fields extracted from OCR/text. All fields are optional —
/// the parser extracts what it can find. `gross` is the minimum required for
/// a usable ledger entry.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedInvoice {
    pub vendor: Option<String>,
    pub invoice_date: Option<NaiveDate>,
    pub gross: Option<Decimal>,
    pub net: Option<Decimal>,
    pub vat: Option<Decimal>,
    pub vat_rate: Option<Decimal>,
}

/// Parses extracted text into structured invoice fields. The parser uses
/// regex patterns tuned for Polish and German invoice formats:
///
/// - Polish invoices: "Netto: 100,00 zł", "VAT 23%: 23,00", "Brutto: 123,00"
/// - German invoices (Thomann): "Netto-Betrag: 100,00 EUR", "zzgl. 19% USt"
/// - Gas station receipts: "Brutto: 250,00 zł", "VAT 23%", "PKO"
/// - Amounts use comma as decimal separator (Polish/European convention)
pub fn parse_invoice(text: &str) -> ParsedInvoice {
    // Pre-process OCR artifacts in the full text before regex matching.
    // Tesseract on scanned receipts often misreads '?' for ',' in numbers,
    // 'O' for '0', etc. Fixing these in the raw text ensures the regex
    // patterns can match amounts that contain OCR errors.
    let cleaned_text = fix_ocr_artifacts(text);

    let vendor = extract_vendor(&cleaned_text);
    let invoice_date = extract_date(&cleaned_text);
    let gross = extract_gross(&cleaned_text);
    let net = extract_net(&cleaned_text);
    let vat = extract_vat(&cleaned_text);
    let vat_rate = extract_vat_rate(&cleaned_text);

    let mut result = ParsedInvoice {
        vendor,
        invoice_date,
        gross,
        net,
        vat,
        vat_rate,
    };

    // If we have net + vat but no gross, compute it.
    if result.gross.is_none()
        && let (Some(net), Some(vat)) = (result.net, result.vat)
    {
        result.gross = Some(net + vat);
    }

    // If we have net + vat_rate but no gross and no vat, compute gross
    // from net * (1 + rate/100). Handles gas station receipts where we
    // can read "Netto: 173,35" and "Kwota A: 23,00%" but not the VAT amount.
    if result.gross.is_none()
        && result.vat.is_none()
        && let (Some(net), Some(rate)) = (result.net, result.vat_rate)
        && rate > Decimal::ZERO
    {
        let gross = net * (Decimal::ONE + rate / Decimal::from(100));
        let vat = gross - net;
        result.gross = Some(round2(gross));
        result.vat = Some(round2(vat));
    }

    // If we still have no gross but have net, use net as gross. This is
    // a fallback for 0% VAT invoices (intra-EU reverse charge) where
    // net = gross, e.g. Thomann PLN invoices with no VAT breakdown.
    if result.gross.is_none()
        && let Some(net) = result.net
    {
        result.gross = Some(net);
    }

    // If we have gross + net but no vat, compute it.
    if result.vat.is_none()
        && let (Some(gross), Some(net)) = (result.gross, result.net)
        && gross >= net
    {
        result.vat = Some(gross - net);
    }

    // If we have gross + vat_rate but no net/vat, derive them.
    if result.net.is_none()
        && result.vat.is_none()
        && let (Some(gross), Some(rate)) = (result.gross, result.vat_rate)
        && rate > Decimal::ZERO
    {
        let net = gross / (Decimal::ONE + rate / Decimal::from(100));
        let vat = gross - net;
        result.net = Some(round2(net));
        result.vat = Some(round2(vat));
    }

    result
}

// --- Amount parsing ---

/// Fixes common OCR character confusions in numeric strings. Tesseract on
/// scanned receipts often misreads: '?' → ',', 'O' → '0', 'l' → '1', '|'
/// → '1'. Only applies to characters that appear adjacent to digits, so
/// text labels are not corrupted.
fn fix_ocr_artifacts(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    for (i, &c) in chars.iter().enumerate() {
        let prev = if i > 0 { Some(chars[i - 1]) } else { None };
        let next = chars.get(i + 1).copied();
        let adjacent_digit =
            prev.is_some_and(|p| p.is_ascii_digit()) || next.is_some_and(|n| n.is_ascii_digit());
        if !adjacent_digit {
            out.push(c);
            continue;
        }
        match c {
            '?' => out.push(','),
            'O' => out.push('0'),
            '|' => out.push('1'),
            _ => out.push(c),
        }
    }
    out
}

/// Parses a Polish/European decimal amount: "1.234,56" or "1234,56" or "1234.56".
/// Handles thousand separators (dots or spaces) and comma/slash decimal separators.
fn parse_amount(raw: &str) -> Option<Decimal> {
    let cleaned = raw.trim().replace('\u{a0}', " "); // non-breaking space

    // Remove currency symbols and units.
    let cleaned = cleaned
        .replace("zł", "")
        .replace("PLN", "")
        .replace("EUR", "")
        .replace("USD", "")
        .replace("€", "")
        .replace("$", "")
        .replace(" brutto", "")
        .replace(" netto", "");

    let cleaned = cleaned.trim();

    // Handle European format: dots as thousand separators, comma as decimal.
    // e.g. "1.234,56" -> "1234.56"
    // But also handle "1234.56" (US format) and "1234,56" (European without thousands).
    let normalized = if cleaned.contains('.') && cleaned.contains(',') {
        // Both present: the last one is the decimal separator.
        if cleaned.rfind(',') > cleaned.rfind('.') {
            // European: dots are thousands, comma is decimal.
            cleaned.replace('.', "").replace(',', ".")
        } else {
            // US: commas are thousands, dot is decimal.
            cleaned.replace(',', "")
        }
    } else if cleaned.contains(',') {
        // Only comma — could be decimal or thousands.
        // If there's exactly one comma followed by 1-2 digits, it's a decimal.
        let parts: Vec<&str> = cleaned.split(',').collect();
        if parts.len() == 2 && parts[1].len() <= 2 {
            cleaned.replace(',', ".")
        } else {
            // Multiple commas or long suffix — thousands separators.
            cleaned.replace(',', "")
        }
    } else {
        cleaned.to_owned()
    };

    // Remove any remaining spaces (thousand separators).
    let normalized = normalized.replace(' ', "");

    Decimal::from_str_exact(&normalized).ok()
}

fn round2(value: Decimal) -> Decimal {
    value.round_dp(2)
}

// --- Regex patterns ---

// Amount capture group inlined in each pattern: handles European
// (1.234,56 / 1234,56 / 418,7) and US (1234.56) decimal formats, plus
// bare integers (300). Uses 1-2 decimal digits for OCR text like "418,7".

static GROSS_PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();

fn gross_patterns() -> &'static [Regex] {
    GROSS_PATTERNS.get_or_init(|| {
        [
            // Polish: "Brutto: 123,00 zł" / "Wartość brutto: 123,00"
            // Uses [ \t] (not \s) to prevent matching across newlines —
            // a product code on the next line must not be captured.
            // Allows optional currency between label and amount for
            // scanned receipts: "SUMA PLN 213,22".
            r"(?i)(?:warto[śs][ćc][ \t]+)?brutto[ \t]*[:\-]?[ \t]*(?:z[łl]|pln|eur|usd|€|\$)?[ \t]*(\d{1,3}(?:[. ]\d{3})*,\d{1,2}|\d{1,6},\d{1,2}|\d{1,6}\.\d{1,2}|\d{1,6})[ \t]*(?:z[łl]|pln|eur|usd|€|\$)?",
            // Polish: "Do zapłaty: 123,00 zł" / "Razem: 123,00" / "SUMA PLN 213,22"
            r"(?i)(?:do[ \t]+zap[łl]aty|razem|suma)[ \t]*[:\-]?[ \t]*(?:z[łl]|pln|eur|usd|€|\$)?[ \t]*(\d{1,3}(?:[. ]\d{3})*,\d{1,2}|\d{1,6},\d{1,2}|\d{1,6}\.\d{1,2}|\d{1,6})[ \t]*(?:z[łl]|pln|eur|usd|€|\$)?",
            // Polish: "Całkowita cena: 439,12 zł" (Audio Partner)
            r"(?i)całkowita[ \t]+cena[ \t]*[:\-]?[ \t]*(?:z[łl]|pln|eur|usd|€|\$)?[ \t]*(\d{1,3}(?:[. ]\d{3})*,\d{1,2}|\d{1,6},\d{1,2}|\d{1,6}\.\d{1,2}|\d{1,6})[ \t]*(?:z[łl]|pln|eur|usd|€|\$)?",
            // Czech: "Celkem: 439,12 zł" (Audio Partner)
            r"(?i)celkem[ \t]*[:\-]?[ \t]*(?:z[łl]|pln|eur|usd|€|\$)?[ \t]*(\d{1,3}(?:[. ]\d{3})*,\d{1,2}|\d{1,6},\d{1,2}|\d{1,6}\.\d{1,2}|\d{1,6})[ \t]*(?:z[łl]|pln|eur|usd|€|\$)?",
            // English: "Sub-total: 7.231,62 PLN" (Thomann)
            r"(?i)sub-?total[ \t]*[:\-]?[ \t]*(?:z[łl]|pln|eur|usd|€|\$)?[ \t]*(\d{1,3}(?:[. ]\d{3})*,\d{1,2}|\d{1,6},\d{1,2}|\d{1,6}\.\d{1,2}|\d{1,6})[ \t]*(?:z[łl]|pln|eur|usd|€|\$)?",
            // English: "Bank transfer  1.001,62 PLN" / "Cash on delivery  1.201,63 PLN"
            // (Thomann invoices without Sub-total — payment method line is the total)
            r"(?i)(?:bank[ \t]+transfer|cash[ \t]+on[ \t]+delivery)[ \t]*(\d{1,3}(?:[. ]\d{3})*,\d{1,2}|\d{1,6},\d{1,2}|\d{1,6}\.\d{1,2}|\d{1,6})[ \t]*(?:z[łl]|pln|eur|usd|€|\$)?",
            // German: "Gesamtbetrag: 123,00 EUR" / "Endbetrag: 123,00"
            r"(?i)(?:gesamtbetrag|endbetrag|summe)[ \t]*[:\-]?[ \t]*(?:z[łl]|pln|eur|usd|€|\$)?[ \t]*(\d{1,3}(?:[. ]\d{3})*,\d{1,2}|\d{1,6},\d{1,2}|\d{1,6}\.\d{1,2}|\d{1,6})[ \t]*(?:eur|€)?",
            // English: "Total EUR 155,91" / "Total: 123.45" — allows optional
            // currency label between "Total" and the amount (MUSIC STORE).
            r"(?i)total[ \t]*(?:eur|pln|zł)?[ \t]*[:\-]?[ \t]*(\d{1,3}(?:[. ]\d{3})*,\d{1,2}|\d{1,6},\d{1,2}|\d{1,6}\.\d{1,2}|\d{1,6})",
        ]
        .into_iter()
        .filter_map(|p| Regex::new(p).ok())
        .collect()
    })
}

static NET_PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();

fn net_patterns() -> &'static [Regex] {
    NET_PATTERNS.get_or_init(|| {
        [
            r"(?i)(?:warto[śs][ćc]\s+)?netto\s*[:\-]?\s*([\d.\s,]+)\s*(?:z[łl]|pln|eur|usd|€|\$)?",
            r"(?i)netto-betrag\s*[:\-]?\s*([\d.\s,]+)\s*(?:eur|€)?",
            r"(?i)net\s*amount\s*[:\-]?\s*([\d.\s,]+)",
            // English: "Value of goods: 7.455,28 PLN" (Thomann)
            r"(?i)value\s+of\s+goods\s*[:\-]?\s*(\d{1,3}(?:[. ]\d{3})*,\d{1,2}|\d{1,6},\d{1,2}|\d{1,6}\.\d{1,2}|\d{1,6})\s*(?:z[łl]|pln|eur|usd|€|\$)?",
            // Polish/Czech: "Razem bez VAT: 418,7 zł" (Muziker)
            r"(?i)razem\s+bez\s+vat\s*[:\-]?\s*(\d{1,3}(?:[. ]\d{3})*,\d{1,2}|\d{1,6},\d{1,2}|\d{1,6}\.\d{1,2}|\d{1,6})\s*(?:z[łl]|pln|eur|usd|€|\$)?",
        ]
        .into_iter()
        .filter_map(|p| Regex::new(p).ok())
        .collect()
    })
}

static VAT_PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();

fn vat_patterns() -> &'static [Regex] {
    VAT_PATTERNS.get_or_init(|| {
        [
            // Polish: "VAT 23%: 23,00" / "Kwota VAT: 23,00" — requires a colon/dash
            // separator before the amount to avoid backtracking into the rate.
            r"(?i)(?:kwota\s+)?vat\s*(?:\d+%)?\s*[:\-]\s*([\d.\s,]+)\s*(?:z[łl]|pln|eur|usd|€|\$)?",
            // Polish: "VAT: 23,00" — colon required
            r"(?i)vat\s*[:\-]\s*([\d.\s,]+)\s*(?:z[łl]|pln|eur|usd|€|\$)?",
            // Polish gas station: "Kwota A: 23,00% 48,25" — the VAT amount
            // follows the rate on the same line (Circle K, Orlen receipts).
            r"(?i)kwota[ \t]+[a-z][ \t]*[:\-]?[ \t]*\d+[.,]?\d*%[ \t]*([\d.\s,]+)",
            // German: "zzgl. 19% USt: 23,00 EUR" / "USt: 23,00"
            r"(?i)(?:zzgl\.?\s*)?\d+%?\s*ust\s*[:\-]?\s*([\d.\s,]+)\s*(?:eur|€)?",
        ]
        .into_iter()
        .filter_map(|p| Regex::new(p).ok())
        .collect()
    })
}

static VAT_RATE_PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();

fn vat_rate_patterns() -> &'static [Regex] {
    VAT_RATE_PATTERNS.get_or_init(|| {
        [
            // "VAT 23%" / "VAT: 23%" / "stawka VAT: 23%"
            r"(?i)vat\s*[:\-]?\s*(\d+(?:[.,]\d+)?)\s*%",
            // "19% USt" / "ust 19%"
            r"(?i)(\d+(?:[.,]\d+)?)\s*%\s*ust",
            // "Steuersatz: 19%"
            r"(?i)steuersatz\s*[:\-]?\s*(\d+(?:[.,]\d+)?)\s*%",
            // Polish gas station: "Kwota A: 23,00%" (Circle K, Orlen)
            r"(?i)kwota[ \t]+[a-z][ \t]*[:\-]?[ \t]*(\d+(?:[.,]\d+)?)\s*%",
        ]
        .into_iter()
        .filter_map(|p| Regex::new(p).ok())
        .collect()
    })
}

static DATE_PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();

fn date_patterns() -> &'static [Regex] {
    DATE_PATTERNS.get_or_init(|| {
        [
            // Polish: "Data wystawienia: 2024-01-15" / "Data: 15.01.2024"
            r"(?i)data\s+(?:wystawienia|sprzedaży|faktury)\s*[:\-]?\s*(\d{4}-\d{2}-\d{2}|\d{1,2}[.\-/]\d{1,2}[.\-/]\d{4})",
            // Generic: "Data: 2024-01-15"
            r"(?i)data\s*[:\-]\s*(\d{4}-\d{2}-\d{2}|\d{1,2}[.\-/]\d{1,2}[.\-/]\d{4})",
            // German: "Datum: 15.01.2024" / "Rechnungsdatum: 2024-01-15"
            r"(?i)(?:rechnungs)?datum\s*[:\-]?\s*(\d{1,2}[.\-/]\d{1,2}[.\-/]\d{4}|\d{4}-\d{2}-\d{2})",
            // English: "Invoice Date: 12.08.2026" / "Invoice Date 12.02.2026"
            r"(?i)(?:invoice\s+)?date\s*[:\-]?\s*(\d{4}-\d{2}-\d{2}|\d{1,2}[.\-/]\d{1,2}[.\-/]\d{4})",
        ]
        .into_iter()
        .filter_map(|p| Regex::new(p).ok())
        .collect()
    })
}

static VENDOR_PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();

fn vendor_patterns() -> &'static [Regex] {
    VENDOR_PATTERNS.get_or_init(|| {
        [
            // Polish: "Sprzedawca: Thomann GmbH"
            r"(?i)sprzedawca\s*[:\-]\s*(.+)",
            // German: "Verkäufer: Thomann GmbH" / "Rechnung von: Thomann"
            r"(?i)(?:verkäufer|rechnung\s+von)\s*[:\-]\s*(.+)",
            // English: "Seller: Thomann GmbH" / "Vendor: Thomann"
            r"(?i)(?:seller|vendor|from)\s*[:\-]\s*(.+)",
        ]
        .into_iter()
        .filter_map(|p| Regex::new(p).ok())
        .collect()
    })
}

// Known vendor names that appear in invoice text — used as a fallback when
// the "Sprzedawca:" label is not found.
static KNOWN_VENDORS: &[&str] = &[
    "thomann",
    "orlen",
    "shell",
    "bp ",
    "circle k",
    "mcdonald",
    "zabka",
    "żabka",
    "carrefour",
    "lidl",
    "biedronka",
    "allegro",
    "amazon",
    "media expert",
    "rtv euro agd",
    "ikea",
    "castorama",
    "leroy merlin",
    "ob",
    "psb",
    "music store",
    "musicstore",
    "audio partner",
    "muziker",
    "mol polska",
    "somacare",
];

fn extract_gross(text: &str) -> Option<Decimal> {
    for pattern in gross_patterns() {
        if let Some(caps) = pattern.captures(text)
            && let Some(amount) = parse_amount(&caps[1])
            && amount > Decimal::ZERO
        {
            return Some(amount);
        }
    }
    None
}

fn extract_net(text: &str) -> Option<Decimal> {
    for pattern in net_patterns() {
        if let Some(caps) = pattern.captures(text)
            && let Some(amount) = parse_amount(&caps[1])
            && amount > Decimal::ZERO
        {
            return Some(amount);
        }
    }
    None
}

fn extract_vat(text: &str) -> Option<Decimal> {
    for pattern in vat_patterns() {
        if let Some(caps) = pattern.captures(text)
            && let Some(amount) = parse_amount(&caps[1])
            && amount > Decimal::ZERO
        {
            return Some(amount);
        }
    }
    None
}

fn extract_vat_rate(text: &str) -> Option<Decimal> {
    for pattern in vat_rate_patterns() {
        if let Some(caps) = pattern.captures(text) {
            let raw = &caps[1];
            let normalized = raw.replace(',', ".");
            if let Ok(rate) = Decimal::from_str_exact(&normalized)
                && rate > Decimal::ZERO
                && rate <= Decimal::from(100)
            {
                return Some(rate);
            }
        }
    }
    None
}

fn extract_date(text: &str) -> Option<NaiveDate> {
    for pattern in date_patterns() {
        if let Some(caps) = pattern.captures(text) {
            let raw = &caps[1];
            if let Some(date) = parse_date(raw) {
                return Some(date);
            }
        }
    }
    None
}

fn parse_date(raw: &str) -> Option<NaiveDate> {
    let raw = raw.trim();

    // ISO format: 2024-01-15
    if let Ok(date) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        return Some(date);
    }

    // European: 15.01.2024 or 15/01/2024
    for sep in &['.', '/'] {
        let fmt = format!("%d{}%m{}%Y", sep, sep);
        if let Ok(date) = NaiveDate::parse_from_str(raw, &fmt) {
            return Some(date);
        }
    }

    // US: 01/15/2024
    if let Ok(date) = NaiveDate::parse_from_str(raw, "%m/%d/%Y") {
        return Some(date);
    }

    None
}

fn extract_vendor(text: &str) -> Option<String> {
    // Try labeled patterns first.
    for pattern in vendor_patterns() {
        if let Some(caps) = pattern.captures(text) {
            let raw = caps[1].trim();
            // Take the first line only — the vendor name is usually on one line.
            let first_line = raw.lines().next()?.trim();
            if !first_line.is_empty() && first_line.len() <= 200 {
                return Some(first_line.to_owned());
            }
        }
    }

    // Fallback: scan for known vendor names in the text.
    let lower = text.to_lowercase();
    for vendor in KNOWN_VENDORS {
        if lower.contains(*vendor) {
            return Some((*vendor).to_owned());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_polish_invoice_with_vat_breakdown() {
        let text = "Faktura VAT nr FV/2024/01/123\nSprzedawca: TechSp. z o.o.\nData wystawienia: 2024-01-15\nNetto: 1000,00 zł\nVAT 23%: 230,00 zł\nBrutto: 1230,00 zł";
        let parsed = parse_invoice(text);

        assert_eq!(parsed.vendor.as_deref(), Some("TechSp. z o.o."));
        assert_eq!(
            parsed.invoice_date,
            Some(NaiveDate::from_ymd_opt(2024, 1, 15).unwrap())
        );
        assert_eq!(parsed.gross, Some(Decimal::new(123000, 2)));
        assert_eq!(parsed.net, Some(Decimal::new(100000, 2)));
        assert_eq!(parsed.vat, Some(Decimal::new(23000, 2)));
        assert_eq!(parsed.vat_rate, Some(Decimal::new(23, 0)));
    }

    #[test]
    fn parses_german_thomann_invoice() {
        let text = "Thomann GmbH\nRechnungsdatum: 15.01.2024\nNetto-Betrag: 100,00 EUR\nzzgl. 19% USt: 19,00 EUR\nGesamtbetrag: 119,00 EUR";
        let parsed = parse_invoice(text);

        assert_eq!(parsed.gross, Some(Decimal::new(11900, 2)));
        assert_eq!(parsed.net, Some(Decimal::new(10000, 2)));
        assert_eq!(parsed.vat, Some(Decimal::new(1900, 2)));
        assert_eq!(parsed.vat_rate, Some(Decimal::new(19, 0)));
        assert_eq!(
            parsed.invoice_date,
            Some(NaiveDate::from_ymd_opt(2024, 1, 15).unwrap())
        );
    }

    #[test]
    fn parses_gas_station_receipt_with_only_gross() {
        let text = "Orlen\nRachunek\nData: 15.01.2024\nBrutto: 250,00 zł\nVAT 23%";
        let parsed = parse_invoice(text);

        assert_eq!(parsed.gross, Some(Decimal::new(25000, 2)));
        assert_eq!(parsed.vat_rate, Some(Decimal::new(23, 0)));
        // Net and VAT should be derived from gross + rate.
        assert!(parsed.net.is_some());
        assert!(parsed.vat.is_some());
    }

    #[test]
    fn parses_thousand_separators() {
        let text = "Brutto: 1.234,56 zł";
        let parsed = parse_invoice(text);
        assert_eq!(parsed.gross, Some(Decimal::new(123456, 2)));
    }

    #[test]
    fn handles_us_decimal_format() {
        let text = "Total: 1234.56";
        let parsed = parse_invoice(text);
        assert_eq!(parsed.gross, Some(Decimal::new(123456, 2)));
    }

    #[test]
    fn computes_vat_from_gross_and_net() {
        let text = "Netto: 100,00\nBrutto: 123,00";
        let parsed = parse_invoice(text);
        assert_eq!(parsed.vat, Some(Decimal::new(2300, 2)));
    }

    #[test]
    fn extracts_vendor_from_known_names() {
        let text = "Shell Polska\nPaliwo\nBrutto: 150,00 zł";
        let parsed = parse_invoice(text);
        assert_eq!(parsed.vendor.as_deref(), Some("shell"));
    }

    #[test]
    fn parses_english_thomann_invoice_with_pln() {
        // Real Thomann invoice format: English labels, European decimals, PLN.
        let text = "Invoice Nr.: 91356438\nDate: 12.08.2026\nThomann GmbH\nValue of goods: 7.455,28 PLN\n3,00 % Discount: -223,66 PLN\nSub-total: 7.231,62 PLN\nBank transfer 7.231,62 PLN";
        let parsed = parse_invoice(text);

        assert_eq!(parsed.gross, Some(Decimal::new(723162, 2)));
        assert_eq!(parsed.net, Some(Decimal::new(745528, 2)));
        assert_eq!(
            parsed.invoice_date,
            Some(NaiveDate::from_ymd_opt(2026, 8, 12).unwrap())
        );
        assert_eq!(parsed.vendor.as_deref(), Some("thomann"));
    }

    #[test]
    fn parses_music_store_invoice_total_eur() {
        // MUSIC STORE format: "Total EUR 155,91" with currency label.
        let text = "MUSIC STORE professional GmbH\nInvoice Date 12.02.2026\nTotal EUR 155,91\nTotal VAT 0,00";
        let parsed = parse_invoice(text);

        assert_eq!(parsed.gross, Some(Decimal::new(15591, 2)));
        assert_eq!(
            parsed.invoice_date,
            Some(NaiveDate::from_ymd_opt(2026, 2, 12).unwrap())
        );
    }

    #[test]
    fn parses_audio_partner_czech_polish_labels() {
        // Audio Partner: Czech "Celkem" and Polish "Całkowita cena".
        let text =
            "Faktura: FV25172048\nNabywca: WB Soft\nCelkem: 439,12 zł\nCałkowita cena: 439,12 zł";
        let parsed = parse_invoice(text);

        assert_eq!(parsed.gross, Some(Decimal::new(43912, 2)));
    }

    #[test]
    fn parses_muziker_one_decimal_place() {
        // Muziker: "Razem: 418,7 zł" — only 1 decimal digit.
        let text = "Faktura n.: 165673830\nRazem bez VAT: 418,7 zł\nRazem: 418,7 zł";
        let parsed = parse_invoice(text);

        assert_eq!(parsed.gross, Some(Decimal::new(41870, 2)));
        assert_eq!(parsed.net, Some(Decimal::new(41870, 2)));
    }

    #[test]
    fn parses_somacare_integer_amount() {
        // Somacare: "Razem do zapłaty: 300 zł" — integer, no decimal separator.
        let text =
            "Faktura nr FVB/2026/2946\nSprzedawca: Somacare Sp. z o.o.\nRazem do zapłaty: 300 zł";
        let parsed = parse_invoice(text);

        assert_eq!(parsed.gross, Some(Decimal::new(30000, 2)));
    }

    #[test]
    fn does_not_match_product_code_across_newline() {
        // Muziker: "brutto" in column header, product code "1208024" on next
        // line. Must NOT match 120802 as gross — real total is "Razem: 418,7".
        let text = "Kod     Nazwa     Ilość    Cena bez VAT    Razem brutto\n1208024  Paradise   1.0      418,7 zł        418,7 zł\nRazem bez VAT: 418,7 zł\nRazem: 418,7 zł";
        let parsed = parse_invoice(text);

        assert_eq!(parsed.gross, Some(Decimal::new(41870, 2)));
    }

    #[test]
    fn parses_thomann_cash_on_delivery() {
        // Thomann COD invoice: no Sub-total, total is on "Cash on delivery" line.
        let text = "Invoice Nr.: 91346010\nDate: 12.08.2026\nThomann GmbH\nValue of goods: 1.201,63 PLN\nCash on delivery 1.201,63 PLN";
        let parsed = parse_invoice(text);

        assert_eq!(parsed.gross, Some(Decimal::new(120163, 2)));
        assert_eq!(parsed.net, Some(Decimal::new(120163, 2)));
    }

    #[test]
    fn parses_thomann_bank_transfer_without_subtotal() {
        // Thomann invoice with Bank transfer but no Sub-total line.
        let text = "Invoice Nr.: 83615429\nDate: 15.10.2025\nThomann GmbH\nValue of goods: 1.001,62 PLN\nBank transfer 1.001,62 PLN";
        let parsed = parse_invoice(text);

        assert_eq!(parsed.gross, Some(Decimal::new(100162, 2)));
    }

    #[test]
    fn parses_scanned_circle_k_receipt() {
        // Circle K gas station receipt (OCR text from Tesseract):
        // "SUMA PLN 213,22" with currency between label and amount.
        // "Kwota A: 23,00%" for VAT rate, "Netto: 173,35" for net.
        let text = "Circle K Polska Sp. z o.o.\nNIP 7790001083\nFAKTURA NR:\nMILES 95 (P2)\n37,94 litry * 5,62 zł 213,22\nKwota A: 23,00% 39,87\nNetto: 173,35\nSUMA PLN 213,22";
        let parsed = parse_invoice(text);

        assert_eq!(parsed.gross, Some(Decimal::new(21322, 2)));
        assert_eq!(parsed.net, Some(Decimal::new(17335, 2)));
        assert_eq!(parsed.vat_rate, Some(Decimal::new(23, 0)));
    }

    #[test]
    fn parses_ocr_artifact_question_mark_as_comma() {
        // OCR reads "2091?9" instead of "2091,9" — ? adjacent to digits.
        let text = "Circle K\nNetto: 2091?9\nSUMA PLN 2592?94\nKwota A: 23,00%";
        let parsed = parse_invoice(text);

        // SUMA should be parsed with ? → , fix: 2592,94
        assert!(parsed.gross.is_some());
        assert_eq!(parsed.gross, Some(Decimal::new(259294, 2)));
    }

    #[test]
    fn computes_gross_from_net_and_vat_rate() {
        // Gas station receipt: net=173,35, vat_rate=23%, no gross label found.
        // Parser should compute gross = 173,35 * 1.23 = 213,22.
        let text = "Circle K\nNetto: 173,35\nKwota A: 23,00%";
        let parsed = parse_invoice(text);

        assert_eq!(parsed.gross, Some(Decimal::new(21322, 2)));
        assert_eq!(parsed.vat, Some(Decimal::new(3987, 2)));
    }
}
