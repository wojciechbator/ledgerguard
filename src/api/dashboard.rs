use axum::{
    http::{HeaderValue, header},
    response::{Html, IntoResponse, Response},
};

const DASHBOARD_HTML: &str = include_str!("dashboard.html");
const DASHBOARD_CACHE_CONTROL: &str = "private, max-age=60";

pub async fn dashboard() -> Response {
    (
        [(
            header::CACHE_CONTROL,
            HeaderValue::from_static(DASHBOARD_CACHE_CONTROL),
        )],
        Html(DASHBOARD_HTML),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashboard_is_embedded_and_has_no_external_assets() {
        assert!(DASHBOARD_HTML.contains("<title>LedgerGuard</title>"));
        assert!(DASHBOARD_HTML.contains("/v1/planner/evaluate"));
        assert!(DASHBOARD_HTML.contains("/v1/planner/simulate"));
        assert!(!DASHBOARD_HTML.contains("https://"));
        assert!(!DASHBOARD_HTML.contains("http://"));
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
}
