#[starknet::interface]
pub trait IFossilVerifier<TContractState> {
    fn verify_mmr_proof(
        ref self: TContractState, proof: Span<felt252>, ipfs_hash: ByteArray,
    ) -> bool;
    fn get_verifier_address(self: @TContractState) -> starknet::ContractAddress;
    fn get_fossil_store_address(self: @TContractState) -> starknet::ContractAddress;
}

#[starknet::contract]
mod FossilVerifier {
    use fossil_store::{IFossilStoreDispatcher, IFossilStoreDispatcherTrait};
    use verifier::decode_journal;
    use verifier::groth16_verifier::{
        IRisc0Groth16VerifierBN254Dispatcher, IRisc0Groth16VerifierBN254DispatcherTrait,
    };

    #[storage]
    struct Storage {
        bn254_verifier: IRisc0Groth16VerifierBN254Dispatcher,
        fossil_store: IFossilStoreDispatcher,
    }

    #[event]
    #[derive(Drop, starknet::Event)]
    enum Event {
        MmrProofVerified: MmrProofVerified,
    }

    #[derive(Drop, starknet::Event)]
    struct MmrProofVerified {
        batch_index: u64,
        latest_mmr_block: u64,
        new_leaves_count: u64,
        new_mmr_root: u256,
    }

    #[constructor]
    fn constructor(
        ref self: ContractState,
        verifier_address: starknet::ContractAddress,
        fossil_store_address: starknet::ContractAddress,
    ) {
        self
            .bn254_verifier
            .write(IRisc0Groth16VerifierBN254Dispatcher { contract_address: verifier_address });
        self.fossil_store.write(IFossilStoreDispatcher { contract_address: fossil_store_address });
    }

    #[external(v0)]
    fn verify_mmr_proof(
        ref self: ContractState, mut proof: Span<felt252>, ipfs_hash: ByteArray,
    ) -> bool {
        let _ = proof.pop_front();
        let journal = self
            .bn254_verifier
            .read()
            .verify_groth16_proof_bn254(proof)
            .expect('Failed to verify proof');

        let journal = decode_journal(journal);

        self.fossil_store.read().update_mmr_state(journal, ipfs_hash);

        self
            .emit(
                MmrProofVerified {
                    batch_index: journal.batch_index,
                    latest_mmr_block: journal.latest_mmr_block,
                    new_leaves_count: journal.leaves_count,
                    new_mmr_root: journal.root_hash,
                },
            );

        true
    }
}
