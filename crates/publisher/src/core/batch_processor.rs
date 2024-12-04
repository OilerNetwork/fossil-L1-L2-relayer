use crate::core::{MMRStateManager, ProofGenerator};
use crate::db::DbConnection;
use crate::errors::AccumulatorError;
use crate::utils::BatchResult;
use common::get_or_create_db_path;
use guest_types::{CombinedInput, GuestOutput, MMRInput};
use mmr::PeaksOptions;
use mmr_utils::initialize_mmr;
use tracing::{debug, error, info, span, Level};

pub struct BatchProcessor {
    batch_size: u64,
    proof_generator: ProofGenerator<CombinedInput>,
    skip_proof_verification: bool,
}

impl BatchProcessor {
    pub fn new(
        batch_size: u64,
        proof_generator: ProofGenerator<CombinedInput>,
        skip_proof_verification: bool,
    ) -> Self {
        Self {
            batch_size,
            proof_generator,
            skip_proof_verification,
        }
    }

    pub fn batch_size(&self) -> u64 {
        self.batch_size
    }

    pub fn skip_proof_verification(&self) -> bool {
        self.skip_proof_verification
    }

    pub async fn process_batch(
        &self,
        start_block: u64,
        end_block: u64,
    ) -> Result<Option<BatchResult>, AccumulatorError> {
        let span = span!(Level::INFO, "process_batch", start_block, end_block);
        let _enter = span.enter();

        let batch_index = start_block / self.batch_size;
        let (_, batch_end) = self.calculate_batch_bounds(batch_index);
        let adjusted_end_block = std::cmp::min(end_block, batch_end);

        info!(
            "Processing batch {} (blocks {} to {})",
            batch_index, start_block, adjusted_end_block
        );

        let batch_file_name =
            get_or_create_db_path(&format!("batch_{}.db", batch_index)).map_err(|e| {
                error!(error = %e, "Failed to get or create DB path");
                e
            })?;
        debug!("Using batch file: {}", batch_file_name);

        let (store_manager, mut mmr, pool) =
            initialize_mmr(&batch_file_name).await.map_err(|e| {
                error!(error = %e, "Failed to initialize MMR");
                e
            })?;

        let current_leaves_count = mmr.leaves_count.get().await.map_err(|e| {
            error!(error = %e, "Failed to get current leaves count");
            e
        })?;
        if current_leaves_count as u64 >= self.batch_size {
            debug!("Batch {} is already complete", batch_index);
            return Ok(None);
        }

        let db_connection = DbConnection::new().await.map_err(|e| {
            error!(error = %e, "Failed to create DB connection");
            e
        })?;
        let headers = db_connection
            .get_block_headers_by_block_range(start_block, adjusted_end_block)
            .await
            .map_err(|e| {
                error!(error = %e, "Failed to fetch block headers");
                e
            })?;

        if headers.is_empty() {
            error!(
                "No headers found for block range {} to {}",
                start_block, adjusted_end_block
            );
            return Err(AccumulatorError::EmptyHeaders {
                start_block,
                end_block: adjusted_end_block,
            });
        }

        let current_peaks = mmr.get_peaks(PeaksOptions::default()).await.map_err(|e| {
            error!(error = %e, "Failed to get current peaks");
            e
        })?;
        let current_elements_count = mmr.elements_count.get().await.map_err(|e| {
            error!(error = %e, "Failed to get current elements count");
            e
        })?;
        let current_leaves_count = mmr.leaves_count.get().await.map_err(|e| {
            error!(error = %e, "Failed to get current leaves count");
            e
        })?;

        let new_headers: Vec<String> = headers.iter().map(|h| h.block_hash.clone()).collect();

        let mmr_input = MMRInput::new(
            current_peaks,
            current_elements_count,
            current_leaves_count,
            new_headers.clone(),
        );

        let combined_input =
            CombinedInput::new(headers.clone(), mmr_input, self.skip_proof_verification);

        let proof = self
            .proof_generator
            .generate_groth16_proof(combined_input)
            .await
            .map_err(|e| {
                error!(error = %e, "Failed to generate proof");
                e
            })?;

        debug!("Generated proof with {} elements", proof.calldata().len());

        let guest_output: GuestOutput =
            self.proof_generator.decode_journal(&proof).map_err(|e| {
                error!(error = %e, "Failed to decode guest output");
                e
            })?;
        debug!(
            "Guest output - root_hash: {}, leaves_count: {}",
            guest_output.root_hash(),
            guest_output.leaves_count()
        );

        let new_mmr_state = MMRStateManager::update_state(
            store_manager,
            &mut mmr,
            &pool,
            adjusted_end_block,
            &guest_output,
            &new_headers,
        )
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to update MMR state");
            e
        })?;

        let new_leaves_count = mmr.leaves_count.get().await.map_err(|e| {
            error!(error = %e, "Failed to get new leaves count");
            e
        })?;
        let batch_is_complete = new_leaves_count as u64 >= self.batch_size;

        if batch_is_complete {
            info!("Batch {} is now complete", batch_index);
        }

        Ok(Some(BatchResult::new(
            start_block,
            adjusted_end_block,
            new_mmr_state,
            proof,
        )))
    }

    pub fn calculate_batch_bounds(&self, batch_index: u64) -> (u64, u64) {
        let batch_start = batch_index * self.batch_size;
        let batch_end = batch_start + self.batch_size - 1;
        (batch_start, batch_end)
    }

    pub fn calculate_start_block(&self, current_end: u64) -> u64 {
        current_end.saturating_sub(current_end % self.batch_size)
    }

    pub fn calculate_batch_range(&self, current_end: u64, start_block: u64) -> BatchRange {
        let batch_start = current_end - (current_end % self.batch_size);
        let effective_start = batch_start.max(start_block);
        let effective_end = std::cmp::min(current_end, batch_start + self.batch_size - 1);

        BatchRange {
            start: effective_start,
            end: effective_end,
        }
    }
}

pub struct BatchRange {
    pub start: u64,
    pub end: u64,
}
