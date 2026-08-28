//! Thomann affiliate link converter and price crawler.
//!
//! Converts Thomann product URLs to affiliate links (offid/affid/subid
//! query params) and crawls each page to extract the product price.
//! The price is returned so the caller can sum a basket and check
//! affordability against the ledgerguard budget.

use regex::Regex;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use thiserror::Error;
use url::Url;

/// Affiliate parameters appended to every Thomann link.
const AFFILIATE_PARAMS: &[(&str, &str)] = &[
    ("offid", "1"),
    ("affid", "4979"),
    ("subid", "direct"),
    ("subid2", "referral"),
];

/// Keys that are stripped from the original URL before appending affiliate
/// params (case-insensitive match).
const AFFILIATE_KEYS: &[&str] = &["offid", "affid", "subid", "subid2"];

const REQUEST_TIMEOUT_MS: u64 = 8_000;
const MAX_RESPONSE_BYTES: usize = 512 * 1024;

#[derive(Debug, Error)]
pub enum ThomannError {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("not a Thomann domain: {0}")]
    NotThomann(String),
    #[error("fetch failed: {0}")]
    Fetch(String),
    #[error("price not found on page")]
    PriceNotFound,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThomannResolveRequest {
    pub urls: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ThomannItem {
    pub original_url: String,
    pub affiliate_url: String,
    pub title: Option<String>,
    pub price: Option<String>,
    pub currency: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ThomannResolveResponse {
    pub items: Vec<ThomannItem>,
    pub total: String,
    pub currency: Option<String>,
    pub resolvable_count: usize,
}

/// Converts a Thomann URL to an affiliate link by stripping existing
/// affiliate params and appending the canonical set.
pub fn to_affiliate_link(raw: &str) -> Result<String, ThomannError> {
    let mut url = Url::parse(raw).map_err(|e| ThomannError::InvalidUrl(e.to_string()))?;

    let host = url.host_str().unwrap_or("").to_lowercase();
    if !(host == "thomann.pl"
        || host.ends_with(".thomann.pl")
        || host == "thomann.de"
        || host.ends_with(".thomann.de"))
    {
        return Err(ThomannError::NotThomann(host));
    }

    // Force https.
    url.set_scheme("https").ok();

    // Strip existing affiliate params.
    let query = url.query().unwrap_or("");
    let pairs: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes())
        .filter(|(k, _)| !AFFILIATE_KEYS.contains(&k.to_lowercase().as_str()))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    // Append affiliate params.
    let mut pairs = pairs;
    for (key, value) in AFFILIATE_PARAMS {
        pairs.push(((*key).to_owned(), (*value).to_owned()));
    }

    let new_query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .finish();

    url.set_query(Some(&new_query));
    Ok(url.to_string())
}

/// Crawls a Thomann product page and extracts the price + title.
pub async fn resolve_product(
    client: &Client,
    raw_url: &str,
) -> Result<(Option<String>, Option<String>, Option<String>), ThomannError> {
    let affiliate_url = to_affiliate_link(raw_url)?;

    let response = client
        .get(&affiliate_url)
        .header("user-agent", "Mozilla/5.0 (compatible; LedgerGuard/1.0)")
        .timeout(std::time::Duration::from_millis(REQUEST_TIMEOUT_MS))
        .send()
        .await
        .map_err(|e| ThomannError::Fetch(e.to_string()))?;

    if !response.status().is_success() {
        return Err(ThomannError::Fetch(format!("HTTP {}", response.status())));
    }

    let body = response
        .text()
        .await
        .map_err(|e| ThomannError::Fetch(e.to_string()))?;

    // Truncate to avoid processing huge pages.
    let body = if body.len() > MAX_RESPONSE_BYTES {
        &body[..MAX_RESPONSE_BYTES]
    } else {
        &body[..]
    };

    let price = extract_price(body);
    let currency = extract_currency(body);
    let title = extract_title(body);

    Ok((price, currency, title))
}

/// Resolves a batch of Thomann URLs — converts to affiliate links, crawls
/// each page for the price, and returns the total.
pub async fn resolve_batch(client: &Client, urls: &[String]) -> ThomannResolveResponse {
    let mut items = Vec::with_capacity(urls.len());
    let mut total: f64 = 0.0;
    let mut currency: Option<String> = None;
    let mut resolvable_count = 0usize;

    for raw in urls {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }

        // Always try to convert to affiliate link first — this validates the
        // domain. If it fails, record the error but continue.
        let affiliate_url = match to_affiliate_link(trimmed) {
            Ok(url) => url,
            Err(error) => {
                items.push(ThomannItem {
                    original_url: trimmed.to_owned(),
                    affiliate_url: trimmed.to_owned(),
                    title: None,
                    price: None,
                    currency: None,
                    error: Some(error.to_string()),
                });
                continue;
            }
        };

        // Crawl the page for the price.
        match resolve_product(client, trimmed).await {
            Ok((price, curr, title)) => {
                let price_value = price.as_ref().and_then(|p| parse_price_value(p));

                if let Some(value) = price_value {
                    total += value;
                    resolvable_count += 1;
                    if currency.is_none() {
                        currency = curr.clone();
                    }
                }

                items.push(ThomannItem {
                    original_url: trimmed.to_owned(),
                    affiliate_url,
                    title,
                    price,
                    currency: curr,
                    error: None,
                });
            }
            Err(error) => {
                // We still return the affiliate link even if the crawl failed.
                items.push(ThomannItem {
                    original_url: trimmed.to_owned(),
                    affiliate_url,
                    title: None,
                    price: None,
                    currency: None,
                    error: Some(error.to_string()),
                });
            }
        }
    }

    ThomannResolveResponse {
        items,
        total: format!("{total:.2}"),
        currency,
        resolvable_count,
    }
}

// --- Price extraction ---

static JSON_LD_PRICE: OnceLock<Option<Regex>> = OnceLock::new();

fn json_ld_price() -> Option<&'static Regex> {
    JSON_LD_PRICE
        .get_or_init(|| Regex::new(r#""price"\s*:\s*"?(\d+[.,]?\d*)"?"#).ok())
        .as_ref()
}

static META_PRICE: OnceLock<Option<Regex>> = OnceLock::new();

fn meta_price() -> Option<&'static Regex> {
    META_PRICE
        .get_or_init(|| {
            Regex::new(r#"<meta\s+itemprop=["']price["']\s+content=["'](\d+[.,]?\d*)["']"#).ok()
        })
        .as_ref()
}

static THOMANN_PRICE: OnceLock<Option<Regex>> = OnceLock::new();

fn thomann_price() -> Option<&'static Regex> {
    THOMANN_PRICE
        .get_or_init(|| {
            Regex::new(r#"(?:class="[^"]*price[^"]*"[^>]*>|data-price=")(\d[\d.]*(?:,\d+)?)"#).ok()
        })
        .as_ref()
}

static TITLE_TAG: OnceLock<Option<Regex>> = OnceLock::new();

fn title_tag() -> Option<&'static Regex> {
    TITLE_TAG
        .get_or_init(|| Regex::new(r"<title>(.*?)</title>").ok())
        .as_ref()
}

static CURRENCY_SYMBOL: OnceLock<Option<Regex>> = OnceLock::new();

fn currency_symbol() -> Option<&'static Regex> {
    CURRENCY_SYMBOL
        .get_or_init(|| Regex::new(r"[€$£]|EUR|USD|GBP|PLN").ok())
        .as_ref()
}

fn extract_price(html: &str) -> Option<String> {
    // Try JSON-LD first — most reliable.
    if let Some(caps) = json_ld_price()?.captures(html) {
        return Some(normalize_price(&caps[1]));
    }

    // Try meta itemprop.
    if let Some(caps) = meta_price()?.captures(html) {
        return Some(normalize_price(&caps[1]));
    }

    // Try Thomann-specific price patterns.
    if let Some(caps) = thomann_price()?.captures(html) {
        return Some(normalize_price(&caps[1]));
    }

    None
}

fn extract_currency(html: &str) -> Option<String> {
    // Check for currency symbol in the page.
    if let Some(m) = currency_symbol()?.find(html) {
        let symbol = m.as_str();
        return Some(match symbol {
            "€" | "EUR" => "EUR".to_owned(),
            "$" | "USD" => "USD".to_owned(),
            "£" | "GBP" => "GBP".to_owned(),
            "PLN" => "PLN".to_owned(),
            other => other.to_owned(),
        });
    }
    None
}

fn extract_title(html: &str) -> Option<String> {
    let caps = title_tag()?.captures(html)?;
    let title = caps[1].trim();
    // Thomann titles often have " – Thomann ..." suffix; strip it.
    let title = title.split(" – ").next().unwrap_or(title).trim();
    if title.is_empty() {
        None
    } else {
        Some(title.to_owned())
    }
}

/// Normalizes a price string: "1.234,56" → "1234.56", "123,45" → "123.45".
fn normalize_price(raw: &str) -> String {
    let raw = raw.trim();
    if raw.contains('.') && raw.contains(',') {
        // European format: dots are thousands, comma is decimal.
        raw.replace('.', "").replace(',', ".")
    } else if raw.contains(',') {
        // Comma as decimal separator.
        raw.replace(',', ".")
    } else {
        raw.to_owned()
    }
}

/// Parses a normalized price string into a float for summation.
fn parse_price_value(raw: &str) -> Option<f64> {
    let normalized = normalize_price(raw);
    normalized.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_thomann_de_link_to_affiliate() {
        let result =
            to_affiliate_link("https://www.thomann.de/de/shure_sm7b_vocal_mikrofon.htm").unwrap();
        assert!(result.contains("offid=1"));
        assert!(result.contains("affid=4979"));
        assert!(result.contains("subid=direct"));
        assert!(result.contains("subid2=referral"));
        assert!(result.contains("/shure_sm7b_vocal_mikrofon.htm"));
    }

    #[test]
    fn strips_existing_affiliate_params() {
        let result =
            to_affiliate_link("https://www.thomann.de/de/test.htm?offid=999&affid=xxx&random=keep")
                .unwrap();
        assert!(!result.contains("999"));
        assert!(!result.contains("xxx"));
        assert!(result.contains("offid=1"));
        assert!(result.contains("affid=4979"));
        assert!(result.contains("random=keep"));
    }

    #[test]
    fn rejects_non_thomann_domain() {
        let result = to_affiliate_link("https://amazon.com/product/123");
        assert!(matches!(result, Err(ThomannError::NotThomann(_))));
    }

    #[test]
    fn accepts_thomann_pl() {
        let result = to_affiliate_link("https://www.thomann.pl/pl/test.htm").unwrap();
        assert!(result.contains("thomann.pl"));
        assert!(result.contains("offid=1"));
    }

    #[test]
    fn extracts_price_from_json_ld() {
        let html = r#"<script type="application/ld+json">{"@type":"Product","offers":{"price":"123.45","priceCurrency":"EUR"}}</script>"#;
        assert_eq!(extract_price(html), Some("123.45".to_owned()));
    }

    #[test]
    fn extracts_price_from_meta_itemprop() {
        let html = r#"<meta itemprop="price" content="89.99">"#;
        assert_eq!(extract_price(html), Some("89.99".to_owned()));
    }

    #[test]
    fn normalizes_european_price_format() {
        assert_eq!(normalize_price("1.234,56"), "1234.56");
        assert_eq!(normalize_price("123,45"), "123.45");
        assert_eq!(normalize_price("1234.56"), "1234.56");
    }

    #[test]
    fn extracts_title_stripping_thomann_suffix() {
        let html = "<title>Shure SM7B – Thomann Polska</title>";
        assert_eq!(extract_title(html), Some("Shure SM7B".to_owned()));
    }

    #[test]
    fn extracts_currency_from_symbol() {
        assert_eq!(extract_currency("€ 123,45"), Some("EUR".to_owned()));
        assert_eq!(extract_currency("123,45 zł PLN"), Some("PLN".to_owned()));
    }
}
