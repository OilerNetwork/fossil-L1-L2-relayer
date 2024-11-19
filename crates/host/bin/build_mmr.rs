use clap::Parser;
use common::get_env_var;
use eyre::{eyre, Result};
use host::{get_store_path, AccumulatorBuilder, ProofGenerator, ProofType};
use methods::{MMR_GUEST_ELF, MMR_GUEST_ID};
use starknet_handler::provider::StarknetProvider;
use tracing::info;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Batch size for processing blocks
    #[arg(short, long, default_value_t = 1024)]
    batch_size: u64,

    /// Path to the SQLite database file. If not specified, a new one will be created.
    #[arg(short, long)]
    db_file: Option<String>,

    /// Number of batches to process. If not specified, processes until block #0.
    #[arg(short, long)]
    num_batches: Option<u64>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // initialize_logger_and_env()?;

    let rpc_url = get_env_var("STARKNET_RPC_URL")?;
    let verifier_address = get_env_var("STARKNET_VERIFIER")?;

    info!("Starting Publisher...");

    // Parse CLI arguments
    let args = Args::parse();

    let store_path = get_store_path(args.db_file).map_err(|e| eyre!(e))?;

    info!("Initializing proof generator...");
    // Initialize proof generator
    let proof_generator = ProofGenerator::new(MMR_GUEST_ELF, MMR_GUEST_ID);

    info!("Initializing accumulator builder...");
    // Initialize accumulator builder with the batch size
    let mut builder =
        AccumulatorBuilder::new(&store_path, proof_generator, args.batch_size).await?;

    info!("Building MMR...");
    // Build MMR from finalized block to block #0 or up to the specified number of batches
    let results = if let Some(num_batches) = args.num_batches {
        builder.build_with_num_batches(num_batches).await?
    } else {
        builder.build_from_finalized().await?
    };

    info!("Processing results...");
    // Print results
    for result in &results {
        info!(
            "Processed blocks {} to {}",
            result.start_block(),
            result.end_block()
        );
        match result.proof() {
            Some(ProofType::Stark { .. }) => info!("Generated STARK proof"),
            Some(ProofType::Groth16 { calldata, .. }) => {
                info!("Generated Groth16 proof");
                let provider = StarknetProvider::new(&rpc_url)?;
                let result = provider.verify_groth16_proof_onchain(&verifier_address, &calldata);
                info!(
                    "Proof verification result: {:?}",
                    result.await.map_err(|e| eyre!(e))?
                );
            }
            None => info!("No proof generated"),
        }
    }
    info!("Host finished");

    Ok(())
}
