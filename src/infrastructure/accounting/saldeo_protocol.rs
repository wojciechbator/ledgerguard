use std::collections::BTreeMap;

use md5::{Digest, Md5};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaldeoHttpMethod {
    Get,
    Post,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SaldeoProtocolError {
    #[error("Saldeo request parameter {0} must not be empty")]
    EmptyParameter(String),
    #[error("Saldeo request parameter occurs more than once: {0}")]
    DuplicateParameter(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaldeoRequest {
    pub method: SaldeoHttpMethod,
    pub path: &'static str,
    /// Sorted request parameters including req_sig. The API token is never present here.
    pub parameters: Vec<(String, String)>,
}

pub fn signed_request(
    method: SaldeoHttpMethod,
    path: &'static str,
    username: &str,
    api_token: &str,
    req_id: &str,
    extra: &[(&str, &str)],
) -> Result<SaldeoRequest, SaldeoProtocolError> {
    let mut params = BTreeMap::<String, String>::new();
    insert_parameter(&mut params, "username", username)?;
    insert_parameter(&mut params, "req_id", req_id)?;
    for (key, value) in extra {
        insert_parameter(&mut params, key, value)?;
    }

    let signature_base = params
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<String>();
    let encoded = saldeo_url_encode(&signature_base);
    let mut signer = Md5::new();
    signer.update(encoded.as_bytes());
    signer.update(api_token.as_bytes());
    let req_sig = format!("{:x}", signer.finalize());

    let mut parameters = params.into_iter().collect::<Vec<_>>();
    parameters.push(("req_sig".to_owned(), req_sig));

    Ok(SaldeoRequest {
        method,
        path,
        parameters,
    })
}

fn insert_parameter(
    params: &mut BTreeMap<String, String>,
    key: &str,
    value: &str,
) -> Result<(), SaldeoProtocolError> {
    if value.is_empty() {
        return Err(SaldeoProtocolError::EmptyParameter(key.to_owned()));
    }
    if params.insert(key.to_owned(), value.to_owned()).is_some() {
        return Err(SaldeoProtocolError::DuplicateParameter(key.to_owned()));
    }
    Ok(())
}

/// SaldeoSMART API XML uses a Java-URLEncoder-like encoding for the signature:
/// spaces become `+`, `*` remains unescaped, `~` is escaped and percent hex is uppercase.
fn saldeo_url_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len());
    for byte in input.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => {
                encoded.push(char::from(*byte));
            }
            b' ' => encoded.push('+'),
            other => {
                use std::fmt::Write as _;
                write!(&mut encoded, "%{other:02X}").expect("writing to String cannot fail");
            }
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signing_matches_official_saldeo_example() {
        let request = signed_request(
            SaldeoHttpMethod::Get,
            "/api/xml/1.0/company/list",
            "user",
            "token",
            "request-id",
            &[],
        )
        .unwrap();

        assert_eq!(request.method, SaldeoHttpMethod::Get);
        assert_eq!(request.path, "/api/xml/1.0/company/list");
        assert_eq!(
            request
                .parameters
                .iter()
                .find(|(key, _)| key == "req_sig")
                .map(|(_, value)| value.as_str()),
            Some("d73710fdff6acc96361f5b9cb3425cee")
        );
        assert!(!request.parameters.iter().any(|(_, value)| value == "token"));
    }

    #[test]
    fn signature_encoding_matches_saldeo_rules() {
        assert_eq!(saldeo_url_encode("a b*~Ł"), "a+b*%7E%C5%81");
    }

    #[test]
    fn empty_and_duplicate_parameters_are_rejected_before_signing() {
        assert_eq!(
            signed_request(SaldeoHttpMethod::Get, "/x", "user", "token", "", &[]).unwrap_err(),
            SaldeoProtocolError::EmptyParameter("req_id".to_owned())
        );
        assert_eq!(
            signed_request(
                SaldeoHttpMethod::Get,
                "/x",
                "user",
                "token",
                "id",
                &[("username", "other")]
            )
            .unwrap_err(),
            SaldeoProtocolError::DuplicateParameter("username".to_owned())
        );
    }
}
