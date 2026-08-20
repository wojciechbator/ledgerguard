use std::{sync::Arc, time::Duration};

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tower_http::{
    catch_panic::CatchPanicLayer,
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    set_header::SetResponseHeaderLayer,
    timeout::TimeoutLayer,
    validate_request::ValidateRequestHeaderLayer,
};
use tracing::error;

use crate::{
    application::{
        AccountingSource, LedgerRepository, ProviderDescriptor, SyncError, SyncReport, sync_month,
    },
    domain::{Money, Month, Planner, PlannerInput, PlannerPolicy, PlannerResult},
};

const MAX_JSON_BODY_BYTES: usize = 64 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub accounting: Arc<dyn AccountingSource>,
    pub ledger: Arc<dyn LedgerRepository>,
    pub live_sync_enabled: bool,
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

pub fn router(state: AppState, api_token: Option<&str>, auth_disabled: bool) -> Router {
    let mut v1 = Router::new()
        .route("/accounting/provider", get(accounting_provider))
        .route("/accounting/sync", post(sync_accounting))
        .route("/system/status", get(system_status))
        .route("/planner/evaluate", post(evaluate))
        .route("/planner/simulate", post(simulate));

    if !auth_disabled {
        let api_token = api_token.expect("configuration validates API token when auth is enabled");
        v1 = v1.route_layer(ValidateRequestHeaderLayer::bearer(api_token));
    }

    Router::new()
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
        .layer(TimeoutLayer::new(REQUEST_TIMEOUT))
        .layer(CatchPanicLayer::new())
        .with_state(state)
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
                _ => unreachable!("policy errors are handled above"),
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
