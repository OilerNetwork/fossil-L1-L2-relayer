use crate::errors::{DbError, PublisherError};
use common::get_env_var;
use eth_rlp_types::BlockHeader;
use mmr_utils::{create_database_file, ensure_directory_exists};
use sqlx::{postgres::PgPoolOptions, Pool, Postgres};
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tracing::{error, info};

#[derive(Debug)]
pub struct DbConnection {
    pub pool: Pool<Postgres>,
}

// Use Arc to allow thread-safe cloning
impl DbConnection {
    const MAX_RETRIES: u32 = 3;
    const RETRY_DELAY: Duration = Duration::from_secs(5);

    /// Creates a new database connection with retries
    pub async fn new() -> Result<Arc<Self>, DbError> {
        let mut attempt = 0;

        while attempt < Self::MAX_RETRIES {
            match Self::try_connect().await {
                Ok(db) => {
                    if attempt > 0 {
                        info!(
                            "Successfully connected to database after {} attempts",
                            attempt + 1
                        );
                    }
                    return Ok(db);
                }
                Err(e) => {
                    attempt += 1;
                    if attempt < Self::MAX_RETRIES {
                        error!(
                            error = %e,
                            attempt,
                            "Database connection failed, retrying in {} seconds...",
                            Self::RETRY_DELAY.as_secs()
                        );
                        sleep(Self::RETRY_DELAY).await;
                    } else {
                        return Err(DbError::Connection(format!(
                            "Failed to connect after {} attempts: {}",
                            Self::MAX_RETRIES,
                            e
                        )));
                    }
                }
            }
        }

        unreachable!()
    }

    /// Internal method to attempt a database connection
    async fn try_connect() -> Result<Arc<Self>, DbError> {
        let database_url = get_env_var("DATABASE_URL")?;

        let pool = PgPoolOptions::new()
            .max_connections(20)
            .min_connections(5)
            .max_lifetime(std::time::Duration::from_secs(30 * 60))
            .idle_timeout(std::time::Duration::from_secs(10 * 60))
            .acquire_timeout(std::time::Duration::from_secs(30))
            .connect(&database_url)
            .await
            .map_err(DbError::Database)?;

        Ok(Arc::new(Self { pool }))
    }

    pub async fn get_block_headers_by_block_range(
        &self,
        start_block: u64,
        end_block: u64,
    ) -> Result<Vec<BlockHeader>, DbError> {
        if start_block > end_block {
            return Err(DbError::InvalidBlockRange {
                start_block,
                end_block,
            });
        }
        let temp_headers = sqlx::query_as!(
            TempBlockHeader,
            r#"
            SELECT block_hash, number, gas_limit, gas_used, nonce, 
                   transaction_root, receipts_root, state_root, 
                   base_fee_per_gas, parent_hash, miner, logs_bloom, 
                   difficulty, totaldifficulty, sha3_uncles, "timestamp", 
                   extra_data, mix_hash, withdrawals_root, 
                   blob_gas_used, excess_blob_gas, parent_beacon_block_root
            FROM blockheaders
            WHERE number BETWEEN $1 AND $2
            ORDER BY number ASC
            "#,
            start_block as i64,
            end_block as i64
        )
        .fetch_all(&self.pool)
        .await?;

        // Convert TempBlockHeader to BlockHeader
        let headers: Vec<BlockHeader> =
            temp_headers.into_iter().map(temp_to_block_header).collect();

        Ok(headers)
    }

    pub async fn get_block_header_by_number(
        &self,
        block_number: u64,
    ) -> Result<Option<BlockHeader>, DbError> {
        let temp_header = sqlx::query_as!(
            TempBlockHeader,
            r#"
            SELECT block_hash, number, gas_limit, gas_used, nonce, 
                   transaction_root, receipts_root, state_root, 
                   base_fee_per_gas, parent_hash, miner, logs_bloom, 
                   difficulty, totaldifficulty, sha3_uncles, "timestamp", 
                   extra_data, mix_hash, withdrawals_root, 
                   blob_gas_used, excess_blob_gas, parent_beacon_block_root
            FROM blockheaders
            WHERE number = $1
            "#,
            block_number as i64
        )
        .fetch_optional(&self.pool)
        .await?;

        // Convert TempBlockHeader to BlockHeader if found
        let header = temp_header.map(temp_to_block_header);

        Ok(header)
    }

    pub async fn get_hourly_block_headers_in_range(
        &self,
        start_block: u64,
        end_block: u64,
    ) -> Result<Vec<BlockHeader>, DbError> {
        if start_block > end_block {
            return Err(DbError::InvalidBlockRange {
                start_block,
                end_block,
            });
        }

        let temp_headers = sqlx::query_as!(
            TempBlockHeader,
            r#"
            WITH hourly_blocks AS (
                SELECT DISTINCT ON (date_trunc('hour', to_timestamp("timestamp")))
                    block_hash, number, gas_limit, gas_used, nonce, 
                    transaction_root, receipts_root, state_root, 
                    base_fee_per_gas, parent_hash, miner, logs_bloom, 
                    difficulty, totaldifficulty, sha3_uncles, "timestamp", 
                    extra_data, mix_hash, withdrawals_root, 
                    blob_gas_used, excess_blob_gas, parent_beacon_block_root
                FROM blockheaders
                WHERE number BETWEEN $1 AND $2
                ORDER BY date_trunc('hour', to_timestamp("timestamp")), number ASC
            )
            SELECT * FROM hourly_blocks
            ORDER BY number ASC
            "#,
            start_block as i64,
            end_block as i64
        )
        .fetch_all(&self.pool)
        .await?;

        let headers: Vec<BlockHeader> =
            temp_headers.into_iter().map(temp_to_block_header).collect();

        Ok(headers)
    }
}

#[derive(sqlx::FromRow, Debug)]
pub struct DbBlockHeader {
    pub block_hash: Option<String>,
    pub number: i64,
    pub gas_limit: Option<i64>,
    pub gas_used: Option<i64>,
    pub base_fee_per_gas: Option<String>,
    pub nonce: Option<String>,
    pub transaction_root: Option<String>,
    pub receipts_root: Option<String>,
    pub state_root: Option<String>,
    pub timestamp: Option<i64>,
}

#[derive(Debug, sqlx::FromRow)]
struct TempBlockHeader {
    pub block_hash: String,
    pub number: i64,
    pub gas_limit: i64,
    pub gas_used: i64,
    pub nonce: String,
    pub transaction_root: Option<String>,
    pub receipts_root: Option<String>,
    pub state_root: Option<String>,
    pub base_fee_per_gas: Option<String>,
    pub parent_hash: Option<String>,
    pub miner: Option<String>,
    pub logs_bloom: Option<String>,
    pub difficulty: Option<String>,
    pub totaldifficulty: Option<String>,
    pub sha3_uncles: Option<String>,
    pub timestamp: Option<i64>, // Assuming this is stored as bigint
    pub extra_data: Option<String>,
    pub mix_hash: Option<String>,
    pub withdrawals_root: Option<String>,
    pub blob_gas_used: Option<String>,
    pub excess_blob_gas: Option<String>,
    pub parent_beacon_block_root: Option<String>,
}

fn temp_to_block_header(temp: TempBlockHeader) -> BlockHeader {
    BlockHeader {
        block_hash: temp.block_hash,             // String (not Option<String>)
        number: temp.number,                     // i64 (not Option<i64>)
        gas_limit: temp.gas_limit,               // i64 (not Option<i64>)
        gas_used: temp.gas_used,                 // i64 (not Option<i64>)
        nonce: temp.nonce,                       // String (not Option<String>)
        transaction_root: temp.transaction_root, // Option<String>
        receipts_root: temp.receipts_root,       // Option<String>
        state_root: temp.state_root,             // Option<String>
        base_fee_per_gas: temp.base_fee_per_gas, // Option<String>

        // Only assign fields that exist in EthBlockHeader
        parent_hash: temp.parent_hash, // Option<String> (if exists)
        ommers_hash: temp.sha3_uncles.clone(), // Option<String> (if exists)
        miner: temp.miner,             // Option<String> (if exists)

        // For the following, use Option<String> correctly
        logs_bloom: Some(temp.logs_bloom.unwrap_or_default()),
        difficulty: Some(temp.difficulty.unwrap_or_else(|| "0x0".to_string())),
        totaldifficulty: Some(temp.totaldifficulty.unwrap_or_else(|| "0x0".to_string())),
        sha3_uncles: temp.sha3_uncles, // Option<String> (if exists)

        // Convert timestamp from Option<i64> to Option<String>
        timestamp: temp.timestamp.map(|ts| format!("0x{:x}", ts)), // Convert i64 to hex string
        extra_data: Some(temp.extra_data.unwrap_or_default()),
        mix_hash: Some(temp.mix_hash.unwrap_or_default()),
        withdrawals_root: Some(temp.withdrawals_root.unwrap_or_default()),
        blob_gas_used: Some(temp.blob_gas_used.unwrap_or_default()),
        excess_blob_gas: Some(temp.excess_blob_gas.unwrap_or_default()),
        parent_beacon_block_root: Some(temp.parent_beacon_block_root.unwrap_or_default()),
    }
}

pub fn get_store_path(db_file: Option<String>) -> Result<String, PublisherError> {
    // Load the database file path from the environment or use the provided argument
    let store_path = if let Some(db_file) = db_file {
        db_file
    } else {
        // Otherwise, create a new database file
        let current_dir = ensure_directory_exists("db-instances")?;
        create_database_file(&current_dir, 0)?
    };

    Ok(store_path)
}
