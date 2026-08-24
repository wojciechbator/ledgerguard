use std::{str::FromStr, sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Request, State},
    http::{HeaderName, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::Datelike;
use serde::{Deserialize, Serialize};
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
    let authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|provided| bool::from(provided.as_bytes().ct_eq(expected.as_bytes())));

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
        let now = chrono::Local::now().date_naive();
        let year = self.year.unwrap_or(now.year());
        let month = self.month.unwrap_or(now.month());
        Month::new(year, month).map_err(|error| {
            ApiError::new(StatusCode::BAD_REQUEST, "invalid_month", error.to_string())
        })
    }
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

    let mut sorted = entries;
    sorted.sort_by_key(|entry| std::cmp::Reverse(entry.booked_on));
    let recent = sorted
        .iter()
        .take(10)
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
    /// ISO date YYYY-MM-DD; defaults to today (Europe/Warsaw host clock).
    booked_on: Option<String>,
    category: Option<String>,
    counterparty: Option<String>,
}

async fn add_manual_entry(
    State(state): State<AppState>,
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
        None | Some("") => chrono::Local::now().date_naive(),
        Some(raw) => raw.parse::<chrono::NaiveDate>().map_err(|_| {
            ApiError::new(
                StatusCode::BAD_REQUEST,
                "invalid_date",
                "booked_on must be ISO YYYY-MM-DD",
            )
        })?,
    };

    let entry = LedgerEntry {
        id: uuid::Uuid::new_v4(),
        external_id: format!("manual-{}", uuid::Uuid::new_v4()),
        kind: request.kind,
        booked_on,
        gross,
        net: None,
        vat: None,
        category: request.category.filter(|c| !c.trim().is_empty()),
        counterparty: request.counterparty.filter(|c| !c.trim().is_empty()),
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
