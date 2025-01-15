use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use publisher::extract_fees;
use serde::{Deserialize, Serialize};
use tracing::{error, info};

#[derive(Deserialize)]
pub struct BlockRangeParams {
    from_block: u64,
    to_block: u64,
    /// Optional override for skip_proof_verification from CLI
    skip_proof_verification: Option<bool>,
}

#[derive(Clone)]
pub struct AppState {
    pub rpc_url: String,
    pub l2_store_address: String,
    pub chain_id: u64,
    pub skip_proof_verification: bool,
    pub batch_size: u64,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

pub async fn verify_blocks(
    State(state): State<Arc<AppState>>,
    Query(params): Query<BlockRangeParams>,
) -> impl IntoResponse {
    // Log request details
    info!(
        "Processing block range request: from_block={}, to_block={}, total_blocks={}",
        params.from_block,
        params.to_block,
        params.to_block.saturating_sub(params.from_block) + 1,
    );

    // Use query parameter if provided, otherwise use CLI default
    let skip_proof = params
        .skip_proof_verification
        .unwrap_or(state.skip_proof_verification);

    let result = match extract_fees(
        &state.rpc_url,
        &state.l2_store_address,
        state.chain_id,
        state.batch_size,
        params.from_block,
        params.to_block,
        Some(skip_proof),
    )
    .await
    {
        Ok(result) => match bincode::serialize(&result) {
            Ok(bytes) => {
                let mut headers = HeaderMap::new();
                headers.insert(
                    header::CONTENT_TYPE,
                    "application/octet-stream".parse().unwrap(),
                );
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "application/octet-stream")
                    .body(Body::from(bytes))
                    .unwrap()
            }
            Err(e) => {
                error!("Failed to serialize response: {}", e);
                let error_json = serde_json::to_vec(&ErrorResponse {
                    error: format!("Failed to serialize response: {}", e),
                })
                .unwrap_or_default();
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(error_json.into())
                    .unwrap()
            }
        },
        Err(e) => {
            error!("Error verifying blocks: {}", e);
            let error_json = serde_json::to_vec(&ErrorResponse {
                error: e.to_string(),
            })
            .unwrap_or_default();
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header(header::CONTENT_TYPE, "application/json")
                .body(error_json.into())
                .unwrap()
        }
    };

    result
}
