use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::commands::reconcile_close::EvidenceJson;

#[derive(Debug, Serialize)]
pub struct ReconcileCloseRequest {
    pub position_id: Uuid,
    pub evidence: EvidenceJson,
}

#[derive(Debug, Serialize)]
pub struct IncomeAckRequest {
    pub reason: String,
    pub actor: String,
}

#[derive(Debug, Deserialize)]
pub struct IncomeAckSuccessResponse {
    pub status: String,
    pub exchange_income_id: String,
    pub acked_at: chrono::DateTime<chrono::Utc>,
    pub ack_reason: String,
    pub acked_by: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct SuccessResponse {
    pub status: String,
    pub position_id: Uuid,
    pub realized_pnl: String,
    pub exit_price: String,
    pub closure_evidence: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct NotFoundResponse {
    pub error: String,
    pub position_id: Uuid,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct NotActiveResponse {
    pub error: String,
    pub details: String,
    pub current_state: String,
}

#[derive(Debug, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
    #[allow(dead_code)]
    pub details: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UnauthorizedResponse {
    pub error: String,
}

#[derive(Debug)]
pub enum ReconcileCloseResponse {
    Success(SuccessResponse),
    NotFound(NotFoundResponse),
    NotActive(NotActiveResponse),
    Inconsistent(ErrorResponse),
    Unsupported(ErrorResponse),
    Unauthorized(UnauthorizedResponse),
}

#[derive(Debug)]
pub enum IncomeAckApiResponse {
    Success(IncomeAckSuccessResponse),
    Invalid(ErrorResponse),
    NotFound(ErrorResponse),
    Conflict(ErrorResponse),
    Unauthorized(UnauthorizedResponse),
    Unavailable(ErrorResponse),
}

pub struct ApiClient {
    base_url: String,
    /// Google ID token (ADR-0054) — always present. Callers obtain it via
    /// `crate::auth::current_id_token()`, which transparently refreshes it
    /// and errors out (telling the operator to run `robson auth login`)
    /// when there is no usable cached credential, so by the time an
    /// `ApiClient` exists there is always a token to send.
    token: String,
    client: reqwest::Client,
}

impl ApiClient {
    pub fn new(base_url: &str, token: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token: token.to_string(),
            client: reqwest::Client::new(),
        }
    }

    pub async fn reconcile_close(
        &self,
        body: ReconcileCloseRequest,
    ) -> Result<ReconcileCloseResponse> {
        let url = format!("{}/reconcile-close", self.base_url);
        let mut req = self.client.post(&url);
        req = req.bearer_auth(&self.token);
        let resp = req.json(&body).send().await.context("failed to connect to robsond")?;

        match resp.status().as_u16() {
            200 => {
                let success: SuccessResponse =
                    resp.json().await.context("failed to parse success response")?;
                Ok(ReconcileCloseResponse::Success(success))
            },
            400 => {
                let err: ErrorResponse =
                    resp.json().await.context("failed to parse 400 response")?;
                if err.error == "unsupported_evidence" {
                    Ok(ReconcileCloseResponse::Unsupported(err))
                } else {
                    Ok(ReconcileCloseResponse::Inconsistent(err))
                }
            },
            401 => {
                let err: UnauthorizedResponse =
                    resp.json().await.context("failed to parse 401 response")?;
                Ok(ReconcileCloseResponse::Unauthorized(err))
            },
            404 => {
                let err: NotFoundResponse =
                    resp.json().await.context("failed to parse 404 response")?;
                Ok(ReconcileCloseResponse::NotFound(err))
            },
            409 => {
                let err: NotActiveResponse =
                    resp.json().await.context("failed to parse 409 response")?;
                Ok(ReconcileCloseResponse::NotActive(err))
            },
            other => {
                let text = resp.text().await.unwrap_or_default();
                anyhow::bail!("unexpected HTTP {} from robsond: {}", other, text)
            },
        }
    }

    pub async fn acknowledge_income(
        &self,
        exchange_income_id: &str,
        body: IncomeAckRequest,
    ) -> Result<IncomeAckApiResponse> {
        let mut url =
            reqwest::Url::parse(&format!("{}/", self.base_url)).context("invalid robsond URL")?;
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("robsond URL cannot be used as a base URL"))?
            .extend(["income", exchange_income_id, "ack"]);

        let mut req = self.client.post(url);
        req = req.bearer_auth(&self.token);
        let resp = req.json(&body).send().await.context("failed to connect to robsond")?;

        match resp.status().as_u16() {
            200 => {
                let success: IncomeAckSuccessResponse =
                    resp.json().await.context("failed to parse income ack response")?;
                Ok(IncomeAckApiResponse::Success(success))
            },
            400 => {
                let error = resp.json().await.context("failed to parse 400 response")?;
                Ok(IncomeAckApiResponse::Invalid(error))
            },
            401 => {
                let error = resp.json().await.context("failed to parse 401 response")?;
                Ok(IncomeAckApiResponse::Unauthorized(error))
            },
            404 => {
                let error = resp.json().await.context("failed to parse 404 response")?;
                Ok(IncomeAckApiResponse::NotFound(error))
            },
            409 => {
                let error = resp.json().await.context("failed to parse 409 response")?;
                Ok(IncomeAckApiResponse::Conflict(error))
            },
            503 => {
                let error = resp.json().await.context("failed to parse 503 response")?;
                Ok(IncomeAckApiResponse::Unavailable(error))
            },
            other => {
                let text = resp.text().await.unwrap_or_default();
                anyhow::bail!("unexpected HTTP {} from robsond: {}", other, text)
            },
        }
    }
}
