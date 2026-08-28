//! Vendor normalization and category auto-detection.
//!
//! OCR text often has variant spellings, casing, and partial names. This
//! module maps raw vendor strings to canonical names and assigns a cost
//! category based on vendor identity or invoice text patterns.

/// Canonical vendor name and category for a parsed invoice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VendorClassification {
    /// Canonical vendor name (e.g. "Thomann", "Orlen", "Shell").
    pub vendor: Option<String>,
    /// Cost category (e.g. "fuel", "equipment", "software").
    pub category: Option<String>,
}

/// Known vendor patterns: (regex fragment, canonical name, category).
/// The regex is matched case-insensitively against the raw vendor string
/// and the full invoice text.
const VENDOR_PATTERNS: &[(&str, &str, &str)] = &[
    // Fuel / gas stations
    (r"orlen", "Orlen", "fuel"),
    (r"shell", "Shell", "fuel"),
    (r"\bbp\b", "BP", "fuel"),
    (r"circle\s*k", "Circle K", "fuel"),
    (r"moya", "Moya", "fuel"),
    (r"energy\s*and\s*transport", "Energy and Transport", "fuel"),
    // Music / audio equipment
    (r"thomann", "Thomann", "equipment"),
    (r"bax-shop|bax\s*shop", "Bax Shop", "equipment"),
    (r"thomann\.de", "Thomann", "equipment"),
    // Electronics / hardware
    (r"media\s*expert", "Media Expert", "electronics"),
    (r"rtv\s*euro\s*agd", "RTV Euro AGD", "electronics"),
    (r"x-kom|xkom", "X-Kom", "electronics"),
    (r"morele\.net", "Morele", "electronics"),
    (r"amazon", "Amazon", "electronics"),
    (r"allegro", "Allegro", "marketplace"),
    // Software / subscriptions
    (r"jetbrains", "JetBrains", "software"),
    (r"github", "GitHub", "software"),
    (r"openai", "OpenAI", "software"),
    (r"anthropic", "Anthropic", "software"),
    (r"google\s*cloud|gcp", "Google Cloud", "software"),
    (r"aws|amazon\s*web\s*services", "AWS", "software"),
    (r"hetzner", "Hetzner", "software"),
    (r"digittal\s*ocean|digitalocean", "DigitalOcean", "software"),
    (r"netlify", "Netlify", "software"),
    (r"stripe", "Stripe", "software"),
    (r"notion", "Notion", "software"),
    (r"linear", "Linear", "software"),
    (r"vercel", "Vercel", "software"),
    // Home / building
    (r"castorama", "Castorama", "home"),
    (r"leroy\s*merlin", "Leroy Merlin", "home"),
    (r"\bob\b\s*centrum|ob\s*projekt", "OB", "home"),
    (r"ikea", "IKEA", "home"),
    (r"psb\s*komfort|psb", "PSB", "home"),
    // Groceries / convenience
    (r"żabka|zabka", "Żabka", "groceries"),
    (r"biedronka", "Biedronka", "groceries"),
    (r"lidl", "Lidl", "groceries"),
    (r"carrefour", "Carrefour", "groceries"),
    (r"mcdonald", "McDonald's", "food"),
    (r"kfc", "KFC", "food"),
    // Shipping / logistics
    (r"inpost|paczkomat", "InPost", "shipping"),
    (r"dhl", "DHL", "shipping"),
    (r"fedex", "FedEx", "shipping"),
    (r"ups\b", "UPS", "shipping"),
    (r"poczta\s*polska", "Poczta Polska", "shipping"),
    // Telecom
    (r"orange", "Orange", "telecom"),
    (r"play\b", "Play", "telecom"),
    (r"plus\b", "Plus", "telecom"),
    (r"t-mobile|tmobile", "T-Mobile", "telecom"),
    (r"netia", "Netia", "telecom"),
    // Utilities
    (r"pge\b|polska\s*energetyka", "PGE", "utilities"),
    (r"pgnig|pgn\s*ig|polski\s*gaz", "PGNiG", "utilities"),
    (r"tauron", "Tauron", "utilities"),
    // Accounting / legal
    (r"saldeo", "SaldeoSMART", "accounting"),
    (r"infakt", "inFakt", "accounting"),
    (r"fakturownia", "Fakturownia", "accounting"),
];

/// Category patterns based on invoice text content, used as a fallback when
/// the vendor is unknown but the invoice text contains category keywords.
const CATEGORY_TEXT_PATTERNS: &[(&str, &str)] = &[
    (
        r"paliwo|benzyna|diesel|on\s*\d+|pb95|pb98|autogaz|lpg",
        "fuel",
    ),
    (
        r"sprzęt|mikrofon|słuchawki|kabel|instrument|gitara|keyboard|monitor",
        "equipment",
    ),
    (r"subscription|renewal|licens|subskrypcja", "software"),
    (r"hosting|server|serwer|vps|cloud|chmura", "software"),
    (
        r"dom\b|materiały\s*budowlane|narzędzia|farba|płytki",
        "home",
    ),
    (r"żywność|spożywcze|piekarnia|warzywniak", "groceries"),
    (r"przesyłka|paczka|kurier|wysyłka", "shipping"),
    (r"telefon|internet|komórkowy|stacjonarny", "telecom"),
    (r"prąd|energia|gaz|woda|ciepło", "utilities"),
    (r"księgow|podatki|doradztwo|prawne|radca", "accounting"),
];

/// Classifies a vendor and assigns a category based on the raw vendor string
/// and the full invoice text. Vendor patterns take priority; category text
/// patterns are a fallback for unknown vendors.
pub fn classify_vendor(raw_vendor: Option<&str>, invoice_text: &str) -> VendorClassification {
    // Try vendor patterns against the raw vendor string first, then the
    // full invoice text as a fallback.
    if let Some(raw) = raw_vendor
        && let Some(result) = match_vendor_pattern(raw)
    {
        return result;
    }

    if let Some(result) = match_vendor_pattern(invoice_text) {
        return result;
    }

    // Unknown vendor — try category detection from text patterns.
    let category = match_category_from_text(invoice_text);

    VendorClassification {
        vendor: raw_vendor.map(|v| v.to_owned()),
        category,
    }
}

fn match_vendor_pattern(text: &str) -> Option<VendorClassification> {
    let lower = text.to_lowercase();
    for (pattern, name, category) in VENDOR_PATTERNS {
        // Use a simple substring match for patterns that are literal names.
        // For patterns with regex metacharacters, fall back to regex.
        if pattern
            .chars()
            .all(|c| c.is_alphanumeric() || c == ' ' || c == '.')
        {
            if lower.contains(pattern) {
                return Some(VendorClassification {
                    vendor: Some((*name).to_owned()),
                    category: Some((*category).to_owned()),
                });
            }
        } else {
            if let Ok(re) = regex::Regex::new(&format!("(?i){pattern}"))
                && re.is_match(text)
            {
                return Some(VendorClassification {
                    vendor: Some((*name).to_owned()),
                    category: Some((*category).to_owned()),
                });
            }
        }
    }
    None
}

fn match_category_from_text(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    for (pattern, category) in CATEGORY_TEXT_PATTERNS {
        if let Ok(re) = regex::Regex::new(&format!("(?i){pattern}"))
            && re.is_match(text)
        {
            return Some((*category).to_owned());
        }
        // Simple substring check for patterns without regex metacharacters.
        if pattern.chars().all(|c| c.is_alphanumeric() || c == ' ') && lower.contains(pattern) {
            return Some((*category).to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_thomann_as_equipment() {
        let result = classify_vendor(Some("Thomann GmbH"), "Thomann GmbH");
        assert_eq!(result.vendor.as_deref(), Some("Thomann"));
        assert_eq!(result.category.as_deref(), Some("equipment"));
    }

    #[test]
    fn classifies_orlen_as_fuel() {
        let result = classify_vendor(Some("Orlen Polska"), "Orlen");
        assert_eq!(result.vendor.as_deref(), Some("Orlen"));
        assert_eq!(result.category.as_deref(), Some("fuel"));
    }

    #[test]
    fn classifies_unknown_vendor_by_text_pattern() {
        let result = classify_vendor(None, "Rachunek za paliwo PB95 50L");
        assert_eq!(result.category.as_deref(), Some("fuel"));
    }

    #[test]
    fn classifies_gas_station_from_text_only() {
        let result = classify_vendor(None, "Shell Polska\nPaliwo\nBrutto: 150,00 zł");
        assert_eq!(result.vendor.as_deref(), Some("Shell"));
        assert_eq!(result.category.as_deref(), Some("fuel"));
    }

    #[test]
    fn classifies_software_subscription() {
        let result = classify_vendor(Some("JetBrains s.r.o."), "JetBrains subscription renewal");
        assert_eq!(result.vendor.as_deref(), Some("JetBrains"));
        assert_eq!(result.category.as_deref(), Some("software"));
    }

    #[test]
    fn unknown_vendor_no_category_returns_none() {
        let result = classify_vendor(Some("Unknown Vendor XYZ"), "Unknown Vendor XYZ");
        assert_eq!(result.vendor.as_deref(), Some("Unknown Vendor XYZ"));
        assert!(result.category.is_none());
    }

    #[test]
    fn classifies_utilities_from_text() {
        let result = classify_vendor(None, "PGE Dystrybucja\nPrąd: 250,00 zł");
        assert_eq!(result.vendor.as_deref(), Some("PGE"));
        assert_eq!(result.category.as_deref(), Some("utilities"));
    }
}
