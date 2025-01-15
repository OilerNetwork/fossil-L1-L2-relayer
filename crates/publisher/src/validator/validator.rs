use crate::db::DbConnection;
use crate::errors::ValidatorError;
use crate::{core::ProofGenerator, utils::Stark};
use common::get_or_create_db_path;
use guest_types::{BlocksValidityInput, GuestProof, MMRInput};
use ipfs_utils::IpfsManager;
use methods::{VALIDATE_BLOCKS_AND_EXTRACT_FEES_ELF, VALIDATE_BLOCKS_AND_EXTRACT_FEES_ID};
use mmr::{PeaksOptions, MMR};
use mmr_utils::{initialize_mmr, StoreManager};
use starknet::core::types::U256;
use starknet_handler::provider::StarknetProvider;
use starknet_handler::u256_from_hex;
use std::collections::HashMap;
use std::path::Path;
use store::SqlitePool;
use tracing::{error, info, warn};

pub struct ValidatorBuilder<'a> {
    rpc_url: &'a str,
    l2_store_address: &'a str,
    chain_id: u64,
    proof_generator: ProofGenerator<BlocksValidityInput>,
    batch_size: u64,
    skip_proof: bool,
}

impl<'a> ValidatorBuilder<'a> {
    pub async fn new(
        rpc_url: &'a str,
        l2_store_address: &'a str,
        chain_id: u64,
        batch_size: u64,
        skip_proof: bool,
    ) -> Result<Self, ValidatorError> {
        if batch_size == 0 {
            return Err(ValidatorError::InvalidInput(
                "Batch size must be greater than 0",
            ));
        }

        let proof_generator = ProofGenerator::new(
            VALIDATE_BLOCKS_AND_EXTRACT_FEES_ELF,
            VALIDATE_BLOCKS_AND_EXTRACT_FEES_ID,
        )?;

        Ok(Self {
            rpc_url,
            l2_store_address,
            chain_id,
            proof_generator,
            batch_size,
            skip_proof,
        })
    }

    pub async fn validate_blocks_and_extract_fees(
        &self,
        start_block: u64,
        end_block: u64,
    ) -> Result<Vec<Stark>, ValidatorError> {
        let db_connection = DbConnection::new().await.map_err(|e| {
            error!(error = %e, "Failed to create DB connection");
            e
        })?;
        let headers: Vec<eth_rlp_types::BlockHeader> = db_connection
            .get_hourly_block_headers_in_range(start_block, end_block)
            .await
            .map_err(|e| {
                error!(error = %e, "Failed to fetch block headers");
                e
            })?;

        self.validate_headers(&headers)?;

        let mmrs = self.initialize_mmrs_for_headers(&headers).await?;

        if self.skip_proof {
            tracing::info!("Skipping MMR root verification as skip_proof is enabled");
        } else {
            tracing::info!("Verifying MMR roots against onchain state...");
            self.verify_mmr_roots(&mmrs).await?;
        }

        let block_indexes = self.collect_block_indexes(&headers, &mmrs).await?;
        self.generate_proofs_for_batches(&headers, &mmrs, &block_indexes)
            .await
    }

    fn validate_headers(
        &self,
        headers: &Vec<eth_rlp_types::BlockHeader>,
    ) -> Result<(), ValidatorError> {
        if headers.is_empty() {
            return Err(ValidatorError::InvalidInput("Headers list cannot be empty"));
        }
        Ok(())
    }

    async fn verify_mmr_roots(
        &self,
        mmrs: &HashMap<u64, (StoreManager, MMR, SqlitePool)>,
    ) -> Result<(), ValidatorError> {
        let batch_indexes: Vec<u64> = mmrs.keys().cloned().collect();
        let onchain_mmr_roots = self.get_onchain_mmr_root(&batch_indexes).await?;

        let onchain_roots_map: HashMap<u64, U256> = batch_indexes
            .iter()
            .zip(onchain_mmr_roots.iter())
            .map(|(&index, root)| (index, root.clone()))
            .collect();

        for (batch_index, (_, mmr, _)) in mmrs.iter() {
            self.verify_single_mmr_root(batch_index, mmr, &onchain_roots_map)
                .await?;
        }

        Ok(())
    }

    async fn verify_single_mmr_root(
        &self,
        batch_index: &u64,
        mmr: &MMR,
        onchain_roots_map: &HashMap<u64, U256>,
    ) -> Result<(), ValidatorError> {
        let mmr_elements_count = mmr.elements_count.get().await?;

        let bag = mmr.bag_the_peaks(Some(mmr_elements_count)).await?;

        let mmr_root_hex = mmr
            .calculate_root_hash(&bag, mmr_elements_count)?
            .to_string();

        let mmr_root = u256_from_hex(&mmr_root_hex)?;

        let onchain_root = onchain_roots_map
            .get(batch_index)
            .ok_or_else(|| ValidatorError::InvalidInput("Missing onchain MMR root for batch"))?;

        if onchain_root.clone() != mmr_root {
            return Err(ValidatorError::InvalidMmrRoot {
                expected: onchain_root.clone(),
                actual: mmr_root,
            });
        }

        Ok(())
    }

    async fn generate_proofs_for_batches(
        &self,
        headers: &[eth_rlp_types::BlockHeader],
        mmrs: &HashMap<u64, (StoreManager, MMR, SqlitePool)>,
        block_indexes: &[(usize, u64)],
    ) -> Result<Vec<Stark>, ValidatorError> {
        let mut proofs = Vec::new();
        for (batch_index, (_, mmr, _)) in mmrs {
            let proof = self
                .generate_batch_proof(headers, mmr, block_indexes, *batch_index)
                .await?;
            proofs.push(proof);
        }

        Ok(proofs)
    }

    async fn generate_batch_proof(
        &self,
        headers: &[eth_rlp_types::BlockHeader],
        mmr: &MMR,
        block_indexes: &[(usize, u64)],
        batch_index: u64,
    ) -> Result<Stark, ValidatorError> {
        let batch_block_indexes = self.get_batch_block_indexes(block_indexes, batch_index);
        let batch_headers = self.get_batch_headers(headers, batch_index);

        let batch_proofs = self
            .get_batch_proofs(mmr, &batch_block_indexes, batch_index)
            .await?;
        let guest_proofs = self.convert_to_guest_proofs(batch_proofs);

        self.validate_proofs_count(&batch_headers, &guest_proofs, batch_index)?;

        let mmr_input = self.prepare_mmr_input(mmr).await?;
        let blocks_validity_input =
            BlocksValidityInput::new(self.chain_id, batch_headers, mmr_input, guest_proofs);

        Ok(self
            .proof_generator
            .generate_stark_proof(blocks_validity_input)
            .await?)
    }

    async fn get_batch_proofs(
        &self,
        mmr: &MMR,
        batch_block_indexes: &Vec<usize>,
        batch_index: u64,
    ) -> Result<Vec<mmr::Proof>, ValidatorError> {
        mmr.get_proofs(batch_block_indexes, None)
            .await
            .map_err(|e| {
                error!(error = %e, "Failed to get proofs for batch index: {}", batch_index);
                ValidatorError::Store(store::StoreError::GetError)
            })
    }

    fn validate_proofs_count(
        &self,
        batch_headers: &[eth_rlp_types::BlockHeader],
        guest_proofs: &[GuestProof],
        batch_index: u64,
    ) -> Result<(), ValidatorError> {
        if batch_headers.len() != guest_proofs.len() {
            error!(
                "Proofs count mismatch for batch index: {}. Expected: {}, Actual: {}",
                batch_index,
                batch_headers.len(),
                guest_proofs.len()
            );
            return Err(ValidatorError::InvalidProofsCount {
                expected: batch_headers.len(),
                actual: guest_proofs.len(),
            });
        }
        Ok(())
    }

    async fn get_onchain_mmr_root(
        &self,
        batch_indexs: &Vec<u64>,
    ) -> Result<Vec<starknet::core::types::U256>, ValidatorError> {
        let provider = StarknetProvider::new(&self.rpc_url)?;

        let mut mmr_roots = Vec::new();

        for batch_index in batch_indexs {
            let mmr_state = provider
                .get_mmr_state(&self.l2_store_address, *batch_index)
                .await?;
            mmr_roots.push(mmr_state.root_hash());
        }

        Ok(mmr_roots)
    }

    async fn initialize_mmrs_for_headers(
        &self,
        headers: &[eth_rlp_types::BlockHeader],
    ) -> Result<HashMap<u64, (StoreManager, MMR, SqlitePool)>, ValidatorError> {
        let mut mmrs = HashMap::new();
        let provider = StarknetProvider::new(&self.rpc_url)?;
        let ipfs_manager = IpfsManager::new();

        for header in headers {
            let batch_index = header.number as u64 / self.batch_size;

            if !mmrs.contains_key(&batch_index) {
                let mmr_state = provider
                    .get_mmr_state(&self.l2_store_address, batch_index)
                    .await?;

                let batch_file_name = get_or_create_db_path(&format!("batch_{}.db", batch_index))
                    .map_err(|e| {
                    error!(error = %e, "Failed to get or create DB path");
                    ValidatorError::Store(store::StoreError::GetError)
                })?;

                let ipfs_hash = mmr_state.ipfs_hash();
                let ipfs_hash_str = String::try_from(ipfs_hash)
                    .map_err(|_| ValidatorError::Store(store::StoreError::GetError))?;
                match ipfs_manager
                    .fetch_db(&ipfs_hash_str, Path::new(&batch_file_name))
                    .await
                {
                    Ok(_) => {
                        info!(
                            "Successfully downloaded DB from IPFS for batch {}",
                            batch_index
                        );
                    }
                    Err(e) => {
                        warn!(
                            error = %e,
                            batch_index = batch_index,
                            "Failed to fetch DB from IPFS, falling back to local file"
                        );
                        if !std::path::Path::new(&batch_file_name).exists() {
                            return Err(ValidatorError::Store(store::StoreError::GetError));
                        }
                    }
                }

                let mmr_components = initialize_mmr(&batch_file_name).await.map_err(|e| {
                    error!(error = %e, "Failed to initialize MMR");
                    ValidatorError::Store(store::StoreError::GetError)
                })?;
                mmrs.insert(batch_index, mmr_components);
            }
        }

        Ok(mmrs)
    }

    async fn collect_block_indexes(
        &self,
        headers: &[eth_rlp_types::BlockHeader],
        mmrs: &HashMap<u64, (StoreManager, MMR, SqlitePool)>,
    ) -> Result<Vec<(usize, u64)>, ValidatorError> {
        let mut block_indexes = Vec::new();

        for header in headers {
            let batch_index = header.number as u64 / self.batch_size;
            let (store_manager, _, pool) = mmrs.get(&batch_index).ok_or_else(|| {
                error!("MMR not found for batch index: {}", batch_index);
                ValidatorError::Store(store::StoreError::GetError)
            })?;

            let index = store_manager
                .get_element_index_for_value(pool, &header.block_hash)
                .await?
                .ok_or_else(|| {
                    error!(
                        "Element index not found for block hash: {}",
                        header.block_hash
                    );
                    ValidatorError::Store(store::StoreError::GetError)
                })?;

            block_indexes.push((index, batch_index));
        }

        Ok(block_indexes)
    }

    fn get_batch_block_indexes(
        &self,
        block_indexes: &[(usize, u64)],
        batch_index: u64,
    ) -> Vec<usize> {
        block_indexes
            .iter()
            .filter(|(_, idx)| *idx == batch_index)
            .map(|(index, _)| *index)
            .collect()
    }

    fn get_batch_headers(
        &self,
        headers: &[eth_rlp_types::BlockHeader],
        batch_index: u64,
    ) -> Vec<eth_rlp_types::BlockHeader> {
        headers
            .iter()
            .filter(|header| header.number as u64 / self.batch_size == batch_index)
            .cloned()
            .collect()
    }

    fn convert_to_guest_proofs(&self, batch_proofs: Vec<mmr::Proof>) -> Vec<GuestProof> {
        batch_proofs
            .into_iter()
            .map(|proof| LocalGuestProof::from(proof).into())
            .collect()
    }

    async fn prepare_mmr_input(&self, mmr: &MMR) -> Result<MMRInput, ValidatorError> {
        let current_peaks = mmr.get_peaks(PeaksOptions::default()).await?;
        let current_elements_count = mmr.elements_count.get().await?;
        let current_leaves_count = mmr.leaves_count.get().await?;

        Ok(MMRInput::new(
            current_peaks,
            current_elements_count,
            current_leaves_count,
            vec![],
        ))
    }
}

// Add this wrapper struct
pub struct LocalGuestProof {
    pub element_index: usize,
    pub element_hash: String,
    pub siblings_hashes: Vec<String>,
    pub peaks_hashes: Vec<String>,
    pub elements_count: usize,
}

// Implement From for the local wrapper type
impl From<mmr::Proof> for LocalGuestProof {
    fn from(proof: mmr::Proof) -> Self {
        Self {
            element_index: proof.element_index,
            element_hash: proof.element_hash,
            siblings_hashes: proof.siblings_hashes,
            peaks_hashes: proof.peaks_hashes,
            elements_count: proof.elements_count,
        }
    }
}

// Add conversion from LocalGuestProof to GuestProof
impl From<LocalGuestProof> for GuestProof {
    fn from(local: LocalGuestProof) -> Self {
        Self {
            element_index: local.element_index,
            element_hash: local.element_hash,
            siblings_hashes: local.siblings_hashes,
            peaks_hashes: local.peaks_hashes,
            elements_count: local.elements_count,
        }
    }
}
