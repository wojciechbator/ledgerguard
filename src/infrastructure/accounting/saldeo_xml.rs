//! SaldeoSMART `document.list` XML normalization.
//!
//! The provider speaks XML; the domain speaks typed records. This module is
//! the only place that translation lives, and it is intentionally tolerant of
//! unknown extra elements while strict about the fields planning depends on:
//! id, kind, issue date and gross amount. A document missing any of those is
//! an error, not a silently dropped row — silent drops would make the ledger
//! look complete while quietly losing costs.

use chrono::Datelike;
use rust_decimal::Decimal;
use std::str::FromStr;

use crate::application::{AccountingProvider, AccountingRecord, AccountingSourceError};
use crate::domain::{EntryKind, Money, Month};

/// Normalizes a `document.list` XML response into accounting records.
///
/// Only documents whose issue date falls inside `month` are returned:
/// Saldeo returns a wider window than requested, and importing outside rows
/// would silently mutate other months' planning.
pub fn normalize_document_list(
    xml: &str,
    month: Month,
) -> Result<Vec<AccountingRecord>, AccountingSourceError> {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_str(xml);
    let mut text = String::new();
    let mut in_document = false;
    let mut current = DocumentFields::default();
    let mut documents: Vec<AccountingRecord> = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => {
                let name = local_name(element.name().as_ref());
                if name == "document" {
                    in_document = true;
                    current = DocumentFields::default();
                }
            }
            Ok(Event::Text(text_event)) => {
                if !in_document {
                    continue;
                }
                // Entities (&amp;, &lt;, numeric refs) travel inline inside
                // Text events, so they must be decoded here or a contractor
                // like "R&amp;D" would be stored literally corrupted. A value
                // that fails to decode (e.g. a bare "&" in non-conformant
                // XML) keeps its raw bytes: tolerance beats dropping data.
                let decoded = text_event
                    .unescape()
                    .unwrap_or_else(|_| String::from_utf8_lossy(text_event.as_ref()));
                text.push_str(&decoded);
            }
            Ok(Event::CData(cdata)) => {
                // CDATA content is raw by definition (no entity encoding);
                // absorbing it as plain text is the whole point of the fix.
                if in_document {
                    text.push_str(&String::from_utf8_lossy(cdata.as_ref()));
                }
            }
            Ok(Event::End(element)) => {
                let name = local_name(element.name().as_ref());
                if !in_document {
                    continue;
                }
                if name == "document" {
                    // `?` propagates real defects; the Option filters
                    // out-of-month documents without treating them as errors.
                    match current.to_record(&month) {
                        Some(Ok(record)) => documents.push(record),
                        Some(Err(defect)) => return Err(defect),
                        None => {}
                    }
                    in_document = false;
                } else {
                    current.absorb(&name, text.trim());
                }
                text.clear();
            }
            Ok(Event::Eof) => break,
            Err(error) => {
                return Err(AccountingSourceError::InvalidData {
                    provider: AccountingProvider::Saldeo,
                    reason: format!("XML parse failure: {error}"),
                });
            }
            _ => {}
        }
    }
    Ok(documents)
}

fn local_name(name: &[u8]) -> String {
    String::from_utf8_lossy(name).into_owned()
}

#[derive(Default)]
struct DocumentFields {
    id: String,
    kind_text: String,
    issue_date: String,
    gross: Option<Decimal>,
    net: Option<Decimal>,
    vat: Option<Decimal>,
    counterparty: String,
    category: String,
}

impl DocumentFields {
    fn absorb(&mut self, leaf_element: &str, raw_text: &str) {
        let value = raw_text.trim();
        match leaf_element {
            // Repeated scalar elements are last-write-wins across the board
            // (amounts always were; ids join them), so a provider correction
            // later in the document is the one that lands.
            "document_id" | "id" => self.id = value.to_owned(),
            "document_type" | "type" => self.kind_text = value.to_owned(),
            "date_issued" | "date_issue" => self.issue_date = value.to_owned(),
            "price_gross" | "total_gross" => self.gross = parse_decimal(value),
            "price_net" | "total_net" => self.net = parse_decimal(value),
            "price_vat" | "total_vat" => self.vat = parse_decimal(value),
            "contractor_name" | "counterparty" => self.counterparty = value.to_owned(),
            "category_name" | "category" => self.category = value.to_owned(),
            _ => {}
        }
    }

    /// Converts accumulated fields into a record. Returns:
    /// * `Some(Ok(record))` — inside the requested month;
    /// * `Some(Err(..))` — a real defect worth surfacing;
    /// * `None` — outside the requested month.
    fn to_record(&self, month: &Month) -> Option<Result<AccountingRecord, AccountingSourceError>> {
        let invalid = |reason: String| AccountingSourceError::InvalidData {
            provider: AccountingProvider::Saldeo,
            reason,
        };
        if self.id.trim().is_empty() {
            return Some(Err(invalid("document without id".into())));
        }
        let booked_on = match chrono::NaiveDate::parse_from_str(self.issue_date.trim(), "%Y-%m-%d")
        {
            Ok(date) => date,
            Err(_) => {
                return Some(Err(invalid(format!(
                    "document {}: unparseable issue date {:?}",
                    self.id, self.issue_date
                ))));
            }
        };
        let Ok(record_month) = Month::new(booked_on.year(), booked_on.month()) else {
            return Some(Err(invalid(format!(
                "document {}: year out of range",
                self.id
            ))));
        };
        if record_month != *month {
            // Outside the requested window: filter outcome, not an error.
            return None;
        }
        let Some(gross_value) = self.gross else {
            return Some(Err(invalid(format!(
                "document {}: missing gross amount",
                self.id
            ))));
        };
        let lower_kind = self.kind_text.to_ascii_lowercase();
        let kind = if lower_kind.contains("cost")
            || lower_kind.contains("purchase")
            || lower_kind.contains("expense")
        {
            EntryKind::Expense
        } else if lower_kind.contains("sale")
            || lower_kind.contains("income")
            || lower_kind.contains("revenue")
            || lower_kind.contains("invoice")
        {
            EntryKind::Revenue
        } else {
            return Some(Err(invalid(format!(
                "document {}: unknown type {:?}",
                self.id, self.kind_text
            ))));
        };
        let money = |value: Decimal, field: &str| {
            Money::non_negative(value)
                .map_err(|error| invalid(format!("document {}: {field} {error}", self.id)))
        };
        Some(Ok(AccountingRecord {
            external_id: self.id.trim().to_owned(),
            kind,
            booked_on,
            gross: match money(gross_value, "gross") {
                Ok(value) => value,
                Err(defect) => return Some(Err(defect)),
            },
            net: match self.net.map(|value| money(value, "net")) {
                Some(Ok(value)) => Some(value),
                Some(Err(defect)) => return Some(Err(defect)),
                None => None,
            },
            vat: match self.vat.map(|value| money(value, "vat")) {
                Some(Ok(value)) => Some(value),
                Some(Err(defect)) => return Some(Err(defect)),
                None => None,
            },
            category: non_empty(&self.category),
            counterparty: non_empty(&self.counterparty),
        }))
    }
}

fn parse_decimal(text: &str) -> Option<Decimal> {
    Decimal::from_str(text.trim()).ok()
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
    <response>
      <status>OK</status>
      <document_list>
        <document>
          <document_id>90001</document_id>
          <document_type>cost</document_type>
          <date_issued>2026-08-03</date_issued>
          <price_net>813.01</price_net>
          <price_vat>186.99</price_vat>
          <price_gross>1000.00</price_gross>
          <contractor_name>Dostawca Czesci Sp. z o.o.</contractor_name>
          <category_name>Czesci samochodowe</category_name>
        </document>
        <document>
          <id>90002</id>
          <type>sales_invoice</type>
          <date_issue>2026-08-14</date_issue>
          <total_gross>2460.00</total_gross>
          <total_net>2000.00</total_net>
          <total_vat>460.00</total_vat>
          <counterparty>Klient Kontrakt Sp. j.</counterparty>
        </document>
        <document>
          <document_id>90003</document_id>
          <document_type>cost</document_type>
          <date_issued>2026-07-28</date_issued>
          <price_gross>50.00</price_gross>
        </document>
      </document_list>
    </response>"#;

    #[test]
    fn fixture_normalizes_filters_month_and_types() {
        let records =
            normalize_document_list(FIXTURE, Month::new(2026, 8).unwrap()).expect("normalizes");
        assert_eq!(records.len(), 2, "July document filtered out");
        assert_eq!(records[0].external_id, "90001");
        assert!(matches!(records[0].kind, EntryKind::Expense));
        assert_eq!(records[0].gross.amount().normalize().to_string(), "1000");
        assert_eq!(records[1].external_id, "90002");
        assert!(matches!(records[1].kind, EntryKind::Revenue));
    }

    #[test]
    fn unknown_type_is_an_error_not_a_guess() {
        let xml = r#"<response><document_list><document>
            <document_id>1</document_id><document_type>mystery</document_type>
            <date_issued>2026-08-01</date_issued><price_gross>10</price_gross>
        </document></document_list></response>"#;
        let result = normalize_document_list(xml, Month::new(2026, 8).unwrap());
        let error = result.unwrap_err().to_string();
        assert!(error.contains("unknown type"), "{error}");
    }

    #[test]
    fn missing_gross_is_an_error_not_a_zero() {
        let xml = r#"<response><document_list><document>
            <document_id>2</document_id><document_type>cost</document_type>
            <date_issued>2026-08-01</date_issued>
        </document></document_list></response>"#;
        let result = normalize_document_list(xml, Month::new(2026, 8).unwrap());
        assert!(result.unwrap_err().to_string().contains("missing gross"));
    }

    #[test]
    fn money_values_survive_as_exact_decimals() {
        let records =
            normalize_document_list(FIXTURE, Month::new(2026, 8).unwrap()).expect("normalizes");
        assert_eq!(
            records[0]
                .net
                .as_ref()
                .map(|money| money.amount().to_string()),
            Some("813.01".into())
        );
    }

    #[test]
    fn xml_entities_are_decoded_not_stored_literally() {
        let xml = r#"<response><document_list><document>
            <document_id>7</document_id>
            <document_type>cost</document_type>
            <date_issued>2026-08-05</date_issued>
            <price_gross>10.00</price_gross>
            <contractor_name>R&amp;D &lt;Lab&gt; Sp. z &#322; o.o.</contractor_name>
        </document></document_list></response>"#;
        let records =
            normalize_document_list(xml, Month::new(2026, 8).unwrap()).expect("normalizes");
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].counterparty.as_deref(),
            Some("R&D <Lab> Sp. z ł o.o.")
        );
    }

    #[test]
    fn double_escaped_entity_is_unescaped_exactly_one_level() {
        // XML has no recursive entity resolution: "&amp;amp;" is the literal
        // text "&amp;" after one decode pass, not "&".
        let xml = r#"<response><document_list><document>
            <document_id>8</document_id>
            <document_type>cost</document_type>
            <date_issued>2026-08-06</date_issued>
            <price_gross>10.00</price_gross>
            <contractor_name>A &amp;amp; B</contractor_name>
        </document></document_list></response>"#;
        let records =
            normalize_document_list(xml, Month::new(2026, 8).unwrap()).expect("normalizes");
        assert_eq!(records[0].counterparty.as_deref(), Some("A &amp; B"));
    }

    #[test]
    fn cdata_sections_are_absorbed_as_text() {
        let xml = "<response><document_list><document>
            <document_id>9</document_id>
            <document_type>cost</document_type>
            <date_issued>2026-08-07</date_issued>
            <price_gross>10.00</price_gross>
            <contractor_name><![CDATA[Firma <> Sp. z o.o. & Wspolnicy]]></contractor_name>
        </document></document_list></response>";
        let records =
            normalize_document_list(xml, Month::new(2026, 8).unwrap()).expect("normalizes");
        assert_eq!(
            records[0].counterparty.as_deref(),
            Some("Firma <> Sp. z o.o. & Wspolnicy")
        );
    }

    #[test]
    fn repeated_scalar_elements_are_last_write_wins() {
        let xml = r#"<response><document_list><document>
            <document_id>stale</document_id>
            <document_type>cost</document_type>
            <date_issued>2026-08-08</date_issued>
            <price_gross>1.00</price_gross>
            <id>10</id>
            <total_gross>42.50</total_gross>
        </document></document_list></response>"#;
        let records =
            normalize_document_list(xml, Month::new(2026, 8).unwrap()).expect("normalizes");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].external_id, "10", "later id element wins");
        // Money::non_negative normalizes scale, hence 42.50 → 42.5.
        assert_eq!(
            records[0].gross.amount().to_string(),
            "42.5",
            "later amount element wins"
        );
    }
}
