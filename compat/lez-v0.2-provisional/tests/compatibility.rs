use std::{path::PathBuf, time::Duration};

use bytesize::ByteSize;
use common::transaction::LeeTransaction;
#[path = "../../../crates/zec-swap-sdk/src/lez_derivation.rs"]
mod sdk_lez_derivation;
use nssa_core::program::{PdaSeed, ProgramId};
use sdk_lez_derivation::{
    derive_lez_metadata_account_v1, derive_lez_native_custody_account_v1, derive_lez_public_pda_v1,
    derive_lez_swap_id_v1, derive_lez_token_account_v1,
};
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

#[test]
fn sdk_pda_helpers_match_exact_upstream_v0_2_types() {
    let escrow_program: ProgramId = [1; 8];
    let ata_program: ProgramId = [3; 8];
    let swap_id = derive_lez_swap_id_v1(b"agreement-v1");

    let upstream_metadata =
        nssa_core::account::AccountId::for_public_pda(&escrow_program, &PdaSeed::new(swap_id));
    assert_eq!(
        derive_lez_public_pda_v1(&escrow_program, &swap_id),
        upstream_metadata.into_value()
    );
    assert_eq!(
        derive_lez_metadata_account_v1(&escrow_program, &swap_id),
        compute_pda(&escrow_program, &[&swap_id]).into_value()
    );

    let custody_label = seed_from_str("custody");
    assert_eq!(
        derive_lez_native_custody_account_v1(&escrow_program, &swap_id),
        compute_pda(&escrow_program, &[&custody_label, &swap_id]).into_value()
    );

    let owner = nssa_core::account::AccountId::new([3; 32]);
    let definition = nssa_core::account::AccountId::new([8; 32]);
    let upstream_ata_seed = associated_token_account_core::compute_ata_seed(owner, definition);
    let upstream_ata = associated_token_account_core::get_associated_token_account_id(
        &ata_program,
        &upstream_ata_seed,
    );
    assert_eq!(
        derive_lez_token_account_v1(&ata_program, owner.value(), definition.value()),
        upstream_ata.into_value()
    );
}
