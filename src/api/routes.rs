use std::sync::Arc;

use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::{
    application::{AccountingSource, ProviderDescriptor},
    domain::{Money, Planner, PlannerInput, PlannerPolicy, PlannerResult},
};

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub accounting: Arc<dyn AccountingSource>,
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

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/readyz", get(ready))
        .route("/v1/accounting/provider", get(accounting_provider))
        .route("/v1/planner/evaluate", post(evaluate))
        .route("/v1/planner/simulate", post(simulate))
        .with_state(state)
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn ready(State(state): State<AppState>) -> Result<Json<HealthResponse>, StatusCode> {
    sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.pool)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(Json(HealthResponse { status: "ready" }))
}

async fn accounting_provider(State(state): State<AppState>) -> Json<ProviderDescriptor> {
    Json(state.accounting.descriptor())
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
