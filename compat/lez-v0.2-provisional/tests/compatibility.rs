use std::{path::PathBuf, time::Duration};

use bytesize::ByteSize;
use common::transaction::LeeTransaction;
use nssa_core::program::{PdaSeed, ProgramId};
use sequencer_service::{BedrockConfig, SequencerConfig};
use spel_framework_core::pda::{compute_pda, seed_from_str};

fn isolated_config(home: PathBuf) -> SequencerConfig {
    SequencerConfig {
        home,
        max_num_tx_in_block: 20,
        max_block_size: ByteSize::mib(4),
        mempool_max_size: 1_000,
        block_create_timeout: Duration::from_secs(60),
        retry_pending_blocks_timeout: Duration::from_secs(1),
        signing_key: [37; 32],
        bedrock_config: BedrockConfig {
            channel_id: [0; 32].into(),
            node_url: "http://127.0.0.1:1".parse().expect("static URL"),
            auth: None,
        },
        genesis: vec![],
    }
}

#[test]
fn exact_pr_head_compiles_with_v0_2_standalone_config_and_lee_pdas() {
    let home = tempfile::tempdir().expect("isolated sequencer home");
    let config = isolated_config(home.path().to_path_buf());

    let encoded = serde_json::to_value(&config).expect("serialize exact v0.2 config");
    assert_eq!(encoded["home"], home.path().to_string_lossy().as_ref());
    assert_eq!(encoded["genesis"], serde_json::json!([]));
    for removed in [
        "genesis_id",
        "is_genesis_random",
        "indexer_rpc_url",
        "initial_public_accounts",
        "initial_private_accounts",
    ] {
        assert!(
            encoded.get(removed).is_none(),
            "stale v0.1.2 field {removed}"
        );
    }

    // Constructing (but deliberately not polling) this future proves the exact
    // standalone entry point compiles without binding a port or starting tasks.
    let standalone = sequencer_service::run(config, 0);
    drop(standalone);

    // Keep the renamed transaction envelope visible in this compatibility seam.
    let no_transaction: Option<LeeTransaction> = None;
    assert!(no_transaction.is_none());

    let program_id: ProgramId = [1; 8];
    let seed = seed_from_str("zec-v0.2-compat");
    let through_spel = compute_pda(&program_id, &[&seed]);
    let through_lez =
        nssa_core::account::AccountId::for_public_pda(&program_id, &PdaSeed::new(seed));

    assert_eq!(through_spel, through_lez);
    assert_eq!(
        through_spel.to_string(),
        "Hc6erQu6uNFNvniSH1Gk8NCJFPtspi5VCQkEDbY3hLgo"
    );
}
