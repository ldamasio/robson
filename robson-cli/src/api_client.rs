use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::commands::reconcile_close::EvidenceJson;

#[derive(Debug, Serialize)]
pub struct ReconcileCloseRequest {
    pub position_id: Uuid,
    pub evidence: EvidenceJson,
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

pub struct ApiClient {
    base_url: String,
    token: Option<String>,
    client: reqwest::Client,
}

impl ApiClient {
    pub fn new(base_url: &str, token: Option<&str>) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token: token.map(|t| t.to_string()),
            client: reqwest::Client::new(),
        }
    }

    pub async fn reconcile_close(
        &self,
        body: ReconcileCloseRequest,
    ) -> Result<ReconcileCloseResponse> {
        let url = format!("{}/reconcile-close", self.base_url);
        let mut req = self.client.post(&url);
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }
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
}
