use axum::response::Html;

const DASHBOARD_HTML: &str = include_str!("dashboard.html");

pub async fn dashboard() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
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
}
