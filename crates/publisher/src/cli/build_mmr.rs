use crate::core::AccumulatorBuilder;
use clap::Parser;
use common::{get_env_var, initialize_logger_and_env};
use starknet_handler::{account::StarknetAccount, provider::StarknetProvider};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    /// Batch size for processing blocks
    #[arg(short, long, default_value_t = 1024)]
    pub batch_size: u64,

    /// Number of batches to process. If not specified, processes until block #0.
    #[arg(short, long)]
    pub num_batches: Option<u64>,

    /// Skip proof verification
    #[arg(short = 'p', long, default_value_t = false)]
    pub skip_proof: bool,

    /// Path to environment file (optional)
    #[arg(short = 'e', long, default_value = ".env")]
    pub env_file: String,

    /// Start building from this block number. If not specified, starts from the latest finalized block.
    #[arg(short = 's', long)]
    pub start_block: Option<u64>,

    /// Start building from the latest MMR block
    #[arg(short = 'l', long, default_value_t = false)]
    pub from_latest: bool,
}

pub async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    // Initialize environment with specified file
    dotenv::from_path(&args.env_file)?;
    initialize_logger_and_env()?;

    let chain_id = get_env_var("CHAIN_ID")?.parse::<u64>()?;
    let rpc_url = get_env_var("STARKNET_RPC_URL")?;
    let verifier_address = get_env_var("FOSSIL_VERIFIER")?;
    let store_address = get_env_var("FOSSIL_STORE")?;
    let private_key = get_env_var("STARKNET_PRIVATE_KEY")?;
    let account_address = get_env_var("STARKNET_ACCOUNT_ADDRESS")?;

    let starknet_provider = StarknetProvider::new(&rpc_url)?;
    let starknet_account =
        StarknetAccount::new(starknet_provider.provider(), &private_key, &account_address)?;

    let mut builder = AccumulatorBuilder::new(
        &rpc_url,
        chain_id,
        &verifier_address,
        &store_address,
        starknet_account,
        args.batch_size,
        args.skip_proof,
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Failed to create AccumulatorBuilder");
        e
    })?;

    // Build MMR from specified start block or finalized block
    match (args.from_latest, args.start_block, args.num_batches) {
        (true, Some(_), _) => {
            return Err("Cannot specify both --from-latest and --start-block".into());
        }
        (true, None, Some(num_batches)) => {
            builder.build_from_latest_with_batches(num_batches).await?
        }
        (true, None, None) => builder.build_from_latest().await?,
        (false, Some(start_block), Some(num_batches)) => {
            builder
                .build_from_block_with_batches(start_block, num_batches)
                .await?
        }
        (false, Some(start_block), None) => builder.build_from_block(start_block).await?,
        (false, None, Some(num_batches)) => builder.build_with_num_batches(num_batches).await?,
        (false, None, None) => builder.build_from_finalized().await?,
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn create_test_env_file() -> NamedTempFile {
        let mut temp = NamedTempFile::new().unwrap();
        writeln!(
            temp,
            r#"
CHAIN_ID=1
STARKNET_RPC_URL=http://localhost:5050
FOSSIL_VERIFIER=0x1234
FOSSIL_STORE=0x5678
STARKNET_PRIVATE_KEY=0x123456789
STARKNET_ACCOUNT_ADDRESS=0x987654321
"#
        )
        .unwrap();
        temp
    }

    #[test]
    fn test_args_default_values() {
        let args = Args::parse_from(&["test"]);
        assert_eq!(args.batch_size, 1024);
        assert_eq!(args.skip_proof, false);
        assert_eq!(args.env_file, ".env");
        assert_eq!(args.from_latest, false);
        assert!(args.num_batches.is_none());
        assert!(args.start_block.is_none());
    }

    #[test]
    fn test_custom_batch_size() {
        let args = Args::parse_from(&["test", "--batch-size", "2048"]);
        assert_eq!(args.batch_size, 2048);
    }

    #[test]
    fn test_invalid_batch_size() {
        let result = Args::try_parse_from(&["test", "--batch-size", "invalid"]);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("invalid digit found in string"));
    }

    #[tokio::test]
    async fn test_conflicting_args() {
        let env_file = create_test_env_file();
        let args = Args {
            batch_size: 1024,
            num_batches: None,
            skip_proof: false,
            env_file: env_file.path().to_str().unwrap().to_string(),
            start_block: Some(100),
            from_latest: true,
        };

        let result = run(args).await;
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "Cannot specify both --from-latest and --start-block"
        );
    }
}
