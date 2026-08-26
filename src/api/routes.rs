use std::{str::FromStr, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use subtle::ConstantTimeEq;
use tower_http::{
    catch_panic::CatchPanicLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    set_header::SetResponseHeaderLayer,
    timeout::TimeoutLayer,
};
use tracing::error;

use super::dashboard::dashboard;
use crate::{
    application::{
        AccountingSource, LedgerRepository, ProviderDescriptor, SyncError, SyncReport, sync_month,
    },
    config::BudgetSettings,
    domain::{
        BudgetPolicy, Decision, EntryKind, LedgerEntry, Money, Month, MonthSummary, Planner,
        PlannerInput, PlannerPolicy, PlannerResult,
    },
};

const MAX_JSON_BODY_BYTES: usize = 64 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const RECENT_ENTRY_LIMIT: i64 = 10;
const MAX_MANUAL_CATEGORY_BYTES: usize = 256;
const MAX_MANUAL_COUNTERPARTY_BYTES: usize = 512;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub accounting: Arc<dyn AccountingSource>,
    pub ledger: Arc<dyn LedgerRepository>,
    pub live_sync_enabled: bool,
    pub budget: BudgetSettings,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Debug, Deserialize)]
pub struct EvaluateRequest {
    pub input: PlannerInput,
    pub policy: PlannerPolicy,
}

#[derive(Debug, Deserialize)]
pub struct SimulateRequest {
    pub input: PlannerInput,
    pub policy: PlannerPolicy,
    pub purchase_gross: Money,
}

#[derive(Debug, Serialize)]
pub struct PlannerResponse {
    pub result: PlannerResult,
}

#[derive(Debug, Deserialize)]
pub struct SyncRequest {
    pub year: i32,
    pub month: u32,
}

#[derive(Debug, Serialize)]
struct SystemStatusResponse {
    version: &'static str,
    provider: ProviderDescriptor,
    live_sync_requested: bool,
    live_sync_ready: bool,
}

#[derive(Debug, Serialize)]
struct ApiErrorBody {
    code: &'static str,
    message: String,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiErrorBody {
                code: self.code,
                message: self.message,
            }),
        )
            .into_response()
    }
}

/// Auth is decided once in `main` from validated config; the router then
/// cannot be built in a state where a required token is missing.
#[derive(Clone)]
pub enum ApiAuth {
    Bearer(Arc<str>),
    Disabled,
}

pub fn router(state: AppState, auth: ApiAuth) -> Router {
    let mut v1 = Router::new()
        .route("/accounting/provider", get(accounting_provider))
        .route("/accounting/sync", post(sync_accounting))
        .route("/system/status", get(system_status))
        .route("/planner/evaluate", post(evaluate))
        .route("/planner/simulate", post(simulate))
        .route("/ledger/month", get(ledger_month))
        .route("/ledger/manual", post(add_manual_entry))
        .route("/planner/affordability", get(affordability));

    if let ApiAuth::Bearer(expected) = auth {
        v1 = v1.route_layer(middleware::from_fn_with_state(expected, bearer_auth));
    }

    Router::new()
        .route("/", get(dashboard))
        .route("/dashboard", get(dashboard))
        .route("/healthz", get(health))
        .route("/readyz", get(ready))
        .nest("/v1", v1)
        .layer(DefaultBodyLimit::max(MAX_JSON_BODY_BYTES))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .layer(CatchPanicLayer::new())
        .with_state(state)
}

async fn bearer_auth(State(expected): State<Arc<str>>, request: Request, next: Next) -> Response {
    // Both sides are hashed before comparing: the digest has a fixed length,
    // so ct_eq never branches on a token-length mismatch and the request path
    // leaks nothing about the configured secret.
    let authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|provided| {
            let provided = Sha256::digest(provided.as_bytes());
            let expected = Sha256::digest(expected.as_bytes());
            bool::from(provided.as_slice().ct_eq(expected.as_slice()))
        });

    if authorized {
        next.run(request).await
    } else {
        ApiError::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "missing or invalid bearer token",
        )
        .into_response()
    }
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn ready(State(state): State<AppState>) -> Result<Json<HealthResponse>, ApiError> {
    sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.pool)
        .await
        .map_err(|_| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "database_unavailable",
                "database readiness check failed",
            )
        })?;
    Ok(Json(HealthResponse { status: "ready" }))
}

async fn accounting_provider(State(state): State<AppState>) -> Json<ProviderDescriptor> {
    Json(state.accounting.descriptor())
}

async fn system_status(State(state): State<AppState>) -> Json<SystemStatusResponse> {
    let provider = state.accounting.descriptor();
    Json(SystemStatusResponse {
        version: env!("CARGO_PKG_VERSION"),
        live_sync_requested: state.live_sync_enabled,
        live_sync_ready: provider.configured
            && provider.scope_configured
            && provider.read_only
            && provider.sync_enabled,
        provider,
    })
}

async fn sync_accounting(
    State(state): State<AppState>,
    Json(request): Json<SyncRequest>,
) -> Result<Json<SyncReport>, ApiError> {
    if !state.live_sync_enabled {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "live_sync_disabled",
            "live accounting synchronization is disabled by deployment policy",
        ));
    }

    let month = Month::new(request.year, request.month).map_err(|error| {
        ApiError::new(StatusCode::BAD_REQUEST, "invalid_month", error.to_string())
    })?;

    sync_month(state.accounting.as_ref(), state.ledger.as_ref(), month)
        .await
        .map(Json)
        .map_err(map_sync_error)
}

fn map_sync_error(error: SyncError) -> ApiError {
    match error {
        SyncError::ProviderNotConfigured(provider) => ApiError::new(
            StatusCode::CONFLICT,
            "provider_not_configured",
            format!("{provider} credentials are not configured"),
        ),
        SyncError::ProviderScopeNotConfigured(provider) => ApiError::new(
            StatusCode::CONFLICT,
            "provider_scope_not_configured",
            format!("{provider} company/account scope is not configured"),
        ),
        SyncError::WritableProviderRefused(provider) => ApiError::new(
            StatusCode::CONFLICT,
            "provider_not_read_only",
            format!("{provider} adapter is not read-only"),
        ),
        SyncError::ProviderNotVerified(provider) => ApiError::new(
            StatusCode::CONFLICT,
            "provider_not_verified",
            format!("{provider} normalization contract is not fixture-verified"),
        ),
        other => {
            error!(error = %other, "accounting synchronization failed");
            match other {
                SyncError::Repository(_) => ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "storage_error",
                    "normalized ledger persistence failed",
                ),
                SyncError::Source(_) | SyncError::InvalidBatch(_) => ApiError::new(
                    StatusCode::BAD_GATEWAY,
                    "accounting_source_error",
                    "accounting provider returned an unusable response",
                ),
                other => {
                    error!(error = %other, "unexpected sync policy failure");
                    ApiError::new(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "sync_policy_error",
                        "accounting synchronization was refused by policy",
                    )
                }
            }
        }
    }
}

async fn evaluate(Json(request): Json<EvaluateRequest>) -> Json<PlannerResponse> {
    Json(PlannerResponse {
        result: Planner::evaluate(request.input, request.policy),
    })
}

async fn simulate(Json(request): Json<SimulateRequest>) -> Json<PlannerResponse> {
    Json(PlannerResponse {
        result: Planner::simulate_purchase(request.input, request.policy, request.purchase_gross),
    })
}

#[derive(Debug, Deserialize)]
struct MonthQuery {
    year: Option<i32>,
    month: Option<u32>,
}

impl MonthQuery {
    fn month(self) -> Result<Month, ApiError> {
        let now = warsaw_now();
        let year = self.year.unwrap_or(now.year());
        let month = self.month.unwrap_or(now.month());
        Month::new(year, month).map_err(|error| {
            ApiError::new(StatusCode::BAD_REQUEST, "invalid_month", error.to_string())
        })
    }
}

/// Today's date in Europe/Warsaw, independent of the host or container clock
/// configuration. Deployment runs UTC; without this anchor, "current month"
/// defaults would flip at 22:00/23:00 UTC on Warsaw month boundaries.
fn warsaw_now() -> NaiveDate {
    chrono::Utc::now()
        .with_timezone(&chrono_tz::Europe::Warsaw)
        .date_naive()
}

fn map_repository_error(error: crate::application::RepositoryError) -> ApiError {
    tracing::error!(error = %error, "ledger read failed");
    ApiError::new(
        StatusCode::INTERNAL_SERVER_ERROR,
        "storage_error",
        "normalized ledger read failed",
    )
}

#[derive(Debug, Serialize)]
struct LedgerEntryView {
    booked_on: String,
    kind: &'static str,
    gross: String,
    category: Option<String>,
    counterparty: Option<String>,
}

#[derive(Debug, Serialize)]
struct LedgerMonthResponse {
    year: i32,
    month: u32,
    income: String,
    costs: String,
    net: String,
    entries: usize,
    recent: Vec<LedgerEntryView>,
}

async fn ledger_month(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<MonthQuery>,
) -> Result<Json<LedgerMonthResponse>, ApiError> {
    let month = query.month()?;
    let entries = state
        .ledger
        .entries_for_month(month)
        .await
        .map_err(map_repository_error)?;
    let summary = MonthSummary::from_entries(&entries);

    // The preview is bounded in SQL (ORDER BY booked_on DESC, id DESC LIMIT),
    // so a heavy month no longer pays for a full materialize-and-sort just to
    // render ten rows. The repository returns them newest-first already.
    let recent_rows = state
        .ledger
        .recent_entries_for_month(month, RECENT_ENTRY_LIMIT)
        .await
        .map_err(map_repository_error)?;
    let recent = recent_rows
        .iter()
        .map(|entry| LedgerEntryView {
            booked_on: entry.booked_on.to_string(),
            kind: match entry.kind {
                EntryKind::Revenue => "revenue",
                EntryKind::Expense => "expense",
            },
            gross: entry.gross.amount().to_string(),
            category: entry.category.clone(),
            counterparty: entry.counterparty.clone(),
        })
        .collect();

    Ok(Json(LedgerMonthResponse {
        year: month.year,
        month: month.month,
        income: summary.income.amount().to_string(),
        costs: summary.costs.amount().to_string(),
        net: summary.net.to_string(),
        entries: summary.entries,
        recent,
    }))
}

#[derive(Debug, Deserialize)]
struct AffordabilityQuery {
    amount: String,
    year: Option<i32>,
    month: Option<u32>,
}

#[derive(Debug, Serialize)]
struct AffordabilityResponse {
    year: i32,
    month: u32,
    income: String,
    costs: String,
    net: String,
    budget_configured: bool,
    planned: String,
    headroom: Option<String>,
    decision: Option<Decision>,
    message: String,
}

async fn affordability(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<AffordabilityQuery>,
) -> Result<Json<AffordabilityResponse>, ApiError> {
    let month = MonthQuery {
        year: query.year,
        month: query.month,
    }
    .month()?;
    let planned_decimal = rust_decimal::Decimal::from_str(query.amount.trim()).map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_amount",
            "amount must be a decimal like 1500 or 1500.00",
        )
    })?;
    let planned = Money::non_negative(planned_decimal).map_err(|error| {
        ApiError::new(StatusCode::BAD_REQUEST, "invalid_amount", error.to_string())
    })?;

    let entries = state
        .ledger
        .entries_for_month(month)
        .await
        .map_err(map_repository_error)?;
    let summary = MonthSummary::from_entries(&entries);

    let policy = BudgetPolicy {
        monthly_cost_budget: state.budget.monthly_cost_budget,
        tight_share_basis_points: state.budget.tight_share_basis_points,
    };
    let (headroom, decision, message, budget_configured) = match policy.afford(&summary, planned) {
        Some(verdict) => (
            Some(verdict.headroom.to_string()),
            Some(verdict.decision),
            match verdict.decision {
                Decision::Healthy => "Zaplanowany koszt miesci sie w budzecie miesiaca.".to_owned(),
                Decision::Tight => {
                    "Zostaje mniej niz prog ciasno — rozwaz przesuniecie.".to_owned()
                }
                Decision::Blocked => "To przekroczyloby budzet kosztow na ten miesiac.".to_owned(),
            },
            true,
        ),
        None => (
            None,
            None,
            "Ustaw LEDGERGUARD_MONTHLY_COST_BUDGET, aby dostawac werdykt.".to_owned(),
            false,
        ),
    };

    Ok(Json(AffordabilityResponse {
        year: month.year,
        month: month.month,
        income: summary.income.amount().to_string(),
        costs: summary.costs.amount().to_string(),
        net: summary.net.to_string(),
        budget_configured,
        planned: planned.amount().to_string(),
        headroom,
        decision,
        message,
    }))
}

#[derive(Debug, Deserialize)]
struct ManualEntryRequest {
    kind: EntryKind,
    /// Decimal string like "239.00". Gross amount, PLN.
    gross: String,
    /// ISO date YYYY-MM-DD; defaults to today in Europe/Warsaw, regardless of
    /// the server's own timezone configuration.
    booked_on: Option<String>,
    category: Option<String>,
    counterparty: Option<String>,
}

/// Upper bound for client-supplied `Idempotency-Key` values, matching the
/// external_id byte budget enforced on synced batches (`MAX_EXTERNAL_ID_BYTES`
/// in `application::sync`, which also allows the `manual-` prefix).
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 256;

/// Validates an optional `Idempotency-Key` header for reuse as the external_id
/// of a manual entry. Absent/empty means "generate one"; a present key must be
/// non-empty after trimming, printable ASCII, and within the byte cap — the
/// same shape `validate_batch` expects from provider ids. An unusable key is a
/// 400, never silently ignored: silently generating a fresh id is exactly the
/// duplicate-row bug retries are trying to avoid.
fn sanitized_idempotency_key(raw: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(trimmed) = raw.map(str::trim).filter(|key| !key.is_empty()) else {
        return Ok(None);
    };
    if trimmed.len() > MAX_IDEMPOTENCY_KEY_BYTES
        || !trimmed
            .chars()
            .all(|character| character.is_ascii_graphic())
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_idempotency_key",
            format!(
                "Idempotency-Key must be 1..={MAX_IDEMPOTENCY_KEY_BYTES} printable ASCII characters"
            ),
        ));
    }
    Ok(Some(trimmed.to_owned()))
}

/// Trims and validates an optional text field on a manual entry, matching the
/// normalization the sync path applies via `normalize_optional_text`. Keeps
/// manual and synced entries consistent: no leading/trailing whitespace, no
/// over-length values, empty becomes None.
fn normalize_manual_optional_text(
    field: &'static str,
    value: Option<&str>,
    max_bytes: usize,
) -> Result<Option<String>, ApiError> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if value.len() > max_bytes {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_field_length",
            format!("{field} exceeds {max_bytes} bytes"),
        ));
    }
    Ok(Some(value.to_owned()))
}

async fn add_manual_entry(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ManualEntryRequest>,
) -> Result<Json<LedgerEntryView>, ApiError> {
    let gross_decimal = rust_decimal::Decimal::from_str(request.gross.trim()).map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_amount",
            "gross must be a decimal like 239.00",
        )
    })?;
    let gross = Money::non_negative(gross_decimal).map_err(|error| {
        ApiError::new(StatusCode::BAD_REQUEST, "invalid_amount", error.to_string())
    })?;
    let booked_on = match request.booked_on.as_deref().map(str::trim) {
        None | Some("") => warsaw_now(),
        Some(raw) => raw.parse::<chrono::NaiveDate>().map_err(|_| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_date",
                "booked_on must be ISO YYYY-MM-DD",
            )
        })?,
    };

    // A retried POST (client timeout, lost response) must land on the same
    // row: the unique key is (source, external_id), so the idempotency key —
    // when the client supplies one — becomes the external_id basis and the
    // upsert collapses onto the original entry. Without a key the behavior
    // stays as before (fresh identity per call).
    let idempotency_key =
        sanitized_idempotency_key(headers.get("idempotency-key").and_then(|v| v.to_str().ok()))?;
    let external_id = match idempotency_key {
        Some(key) => format!("manual-{key}"),
        None => format!("manual-{}", uuid::Uuid::new_v4()),
    };

    let category = normalize_manual_optional_text(
        "category",
        request.category.as_deref(),
        MAX_MANUAL_CATEGORY_BYTES,
    )?;
    let counterparty = normalize_manual_optional_text(
        "counterparty",
        request.counterparty.as_deref(),
        MAX_MANUAL_COUNTERPARTY_BYTES,
    )?;

    let entry = LedgerEntry {
        id: uuid::Uuid::new_v4(),
        external_id,
        kind: request.kind,
        booked_on,
        gross,
        net: None,
        vat: None,
        category,
        counterparty,
        source: crate::domain::SourceSystem::manual(),
    };
    state
        .ledger
        .upsert_entries(std::slice::from_ref(&entry))
        .await
        .map_err(map_repository_error)?;

    Ok(Json(LedgerEntryView {
        booked_on: entry.booked_on.to_string(),
        kind: match entry.kind {
            EntryKind::Revenue => "revenue",
            EntryKind::Expense => "expense",
        },
        gross: entry.gross.amount().to_string(),
        category: entry.category,
        counterparty: entry.counterparty,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idempotency_key_absent_or_blank_generates_fresh_identity() {
        assert!(sanitized_idempotency_key(None).unwrap().is_none());
        assert!(sanitized_idempotency_key(Some("")).unwrap().is_none());
        assert!(sanitized_idempotency_key(Some("   ")).unwrap().is_none());
    }

    #[test]
    fn idempotency_key_is_trimmed_and_capped() {
        assert_eq!(
            sanitized_idempotency_key(Some("  retry-42 "))
                .unwrap()
                .as_deref(),
            Some("retry-42")
        );
        let max_key = "k".repeat(MAX_IDEMPOTENCY_KEY_BYTES);
        assert_eq!(
            sanitized_idempotency_key(Some(&max_key))
                .unwrap()
                .as_deref(),
            Some(max_key.as_str())
        );
        let too_long = "k".repeat(MAX_IDEMPOTENCY_KEY_BYTES + 1);
        assert!(sanitized_idempotency_key(Some(&too_long)).is_err());
    }

    #[test]
    fn idempotency_key_rejects_control_and_non_ascii() {
        assert!(sanitized_idempotency_key(Some("bad\nkey")).is_err());
        assert!(sanitized_idempotency_key(Some("Zażółć gęślą jaźń")).is_err());
    }

    #[test]
    fn warsaw_now_is_ahead_of_utc_at_the_day_boundary() {
        // 2026-03-29 22:30 UTC is already 2026-03-30 in Warsaw (CEST, +02:00):
        // a UTC-based "today" would attribute late-evening entries to the
        // wrong day. This pins the timezone anchor the handlers rely on.
        use chrono::TimeZone;
        let instant = chrono::Utc
            .with_ymd_and_hms(2026, 3, 29, 22, 30, 0)
            .single()
            .unwrap();
        let warsaw = chrono_tz::Europe::Warsaw.from_utc_datetime(&instant.naive_utc());
        assert_eq!(
            warsaw.date_naive(),
            NaiveDate::from_ymd_opt(2026, 3, 30).unwrap()
        );
    }
}
