use axum::{
    http::{HeaderName, HeaderValue, header},
    response::{Html, IntoResponse, Response},
};

const DASHBOARD_HTML: &str = include_str!("dashboard.html");
const THOMANN_HTML: &str = include_str!("thomann.html");
const DASHBOARD_CACHE_CONTROL: &str = "private, max-age=60";
const DASHBOARD_CSP: &str = "default-src 'self'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self' https://*.thomann.de https://*.thomann.pl; img-src 'self' data:; base-uri 'none'; frame-ancestors 'none'; form-action 'self'; object-src 'none'";
const CONTENT_SECURITY_POLICY: HeaderName = HeaderName::from_static("content-security-policy");
const X_CONTENT_TYPE_OPTIONS: HeaderName = HeaderName::from_static("x-content-type-options");
const X_FRAME_OPTIONS: HeaderName = HeaderName::from_static("x-frame-options");
const REFERRER_POLICY: HeaderName = HeaderName::from_static("referrer-policy");

fn html_response(html: &'static str) -> Response {
    (
        [
            (
                header::CACHE_CONTROL,
                HeaderValue::from_static(DASHBOARD_CACHE_CONTROL),
            ),
            (
                CONTENT_SECURITY_POLICY,
                HeaderValue::from_static(DASHBOARD_CSP),
            ),
            (X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
            (X_FRAME_OPTIONS, HeaderValue::from_static("DENY")),
            (REFERRER_POLICY, HeaderValue::from_static("no-referrer")),
        ],
        Html(html),
    )
        .into_response()
}

pub async fn dashboard() -> Response {
    html_response(DASHBOARD_HTML)
}

pub async fn thomann_page() -> Response {
    html_response(THOMANN_HTML)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashboard_is_embedded_and_has_no_external_assets() {
        assert!(DASHBOARD_HTML.contains("<title>LedgerGuard</title>"));
        assert!(DASHBOARD_HTML.contains("/v1/planner/affordability"));
        assert!(DASHBOARD_HTML.contains("/v1/ingest/email"));
        assert!(DASHBOARD_HTML.contains("/v1/costs/summary"));
        assert!(DASHBOARD_HTML.contains("/v1/thomann/resolve"));
        // No external script/style/link/img tags — only example URLs in a
        // textarea placeholder are allowed (same rule as thomann_page test).
        assert!(!DASHBOARD_HTML.contains("<script src="));
        assert!(!DASHBOARD_HTML.contains("<link href="));
        assert!(!DASHBOARD_HTML.contains("<img src="));
    }

    #[test]
    fn thomann_page_is_embedded_and_uses_api_endpoints() {
        assert!(THOMANN_HTML.contains("<title>Thomann"));
        assert!(THOMANN_HTML.contains("/v1/thomann/resolve"));
        assert!(THOMANN_HTML.contains("/v1/planner/affordability"));
        assert!(THOMANN_HTML.contains("affiliate"));
        // No external script/style/link tags — only example URLs in a
        // textarea placeholder are allowed.
        assert!(!THOMANN_HTML.contains("<script src="));
        assert!(!THOMANN_HTML.contains("<link href="));
        assert!(!THOMANN_HTML.contains("<img src="));
    }

    #[tokio::test]
    async fn dashboard_allows_short_private_browser_caching() {
        let response = dashboard().await;

        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .expect("dashboard cache-control header")
                .to_str()
                .unwrap(),
            DASHBOARD_CACHE_CONTROL
        );
    }

    #[tokio::test]
    async fn dashboard_sends_security_policy_as_response_headers() {
        let response = dashboard().await;
        let headers = response.headers();

        assert_eq!(
            headers
                .get(&CONTENT_SECURITY_POLICY)
                .expect("dashboard CSP header")
                .to_str()
                .unwrap(),
            DASHBOARD_CSP
        );
        assert_eq!(
            headers
                .get(&X_CONTENT_TYPE_OPTIONS)
                .expect("nosniff header"),
            "nosniff"
        );
        assert_eq!(
            headers.get(&X_FRAME_OPTIONS).expect("frame options header"),
            "DENY"
        );
        assert_eq!(
            headers
                .get(&REFERRER_POLICY)
                .expect("referrer policy header"),
            "no-referrer"
        );
        assert!(DASHBOARD_CSP.contains("frame-ancestors 'none'"));
        assert!(DASHBOARD_CSP.contains("object-src 'none'"));
    }
}
