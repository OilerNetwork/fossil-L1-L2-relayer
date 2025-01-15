// main.rs
use eth_rlp_verify::are_blocks_valid;
use risc0_zkvm::guest::env;
use guest_mmr::core::GuestMMR;
use guest_types::BlocksValidityInput;

fn main() {
    // Read combined input
    let input: BlocksValidityInput = env::read();

    // Verify block headers
    if !are_blocks_valid(&input.headers(), input.chain_id()) {
        env::commit(&false);
    }
    // Initialize MMR with previous state
    let mmr = GuestMMR::new(
        input.mmr_input().initial_peaks(),
        input.mmr_input().elements_count(),
        input.mmr_input().leaves_count(),
    );
    // Append block hashes to MMR
    for (i, header) in input.headers().iter().enumerate() {
        let block_hash = header.block_hash.clone();
        let proof = input.proofs()[i].clone();
        
        if !mmr.verify_proof(proof, block_hash, None).unwrap() {
            env::commit(&false);
        }
    }

    // Collect base_fee_per_gas values
    let base_fees: Vec<String> = input.headers()
        .iter()
        .map(|header| header.base_fee_per_gas.clone().unwrap_or_default())
        .collect();

    env::commit(&base_fees);
}
