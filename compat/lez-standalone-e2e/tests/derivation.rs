use nssa::AccountId;

#[path = "../../../crates/zec-swap-sdk/src/lez_derivation.rs"]
mod sdk_lez_derivation;

use sdk_lez_derivation::{
    derive_lez_metadata_account_v1, derive_lez_native_custody_account_v1, derive_lez_swap_id_v1,
    derive_lez_token_account_v1, derive_nssa_v0_1_2_metadata_account_v1,
    derive_nssa_v0_1_2_native_custody_account_v1, derive_nssa_v0_1_2_token_account_v1,
};

#[test]
fn sdk_nssa_compatibility_derivations_equal_pinned_official_v0_1_2() {
    let escrow_program = [1_u32; 8];
    let ata_program = [3_u32; 8];
    let onchain_swap_id = derive_lez_swap_id_v1(b"agreement-v1");

    let official_metadata =
        spel_framework_core::pda::compute_pda(&escrow_program, &[&onchain_swap_id]);
    assert_eq!(
        derive_nssa_v0_1_2_metadata_account_v1(&escrow_program, &onchain_swap_id),
        official_metadata.to_bytes()
    );
    assert_ne!(
        derive_lez_metadata_account_v1(&escrow_program, &onchain_swap_id),
        official_metadata.to_bytes(),
        "LEE v0.2 evidence must not be mislabeled as NSSA v0.1.2 evidence"
    );

    let custody_label = spel_framework_core::pda::seed_from_str("custody");
    let official_custody =
        spel_framework_core::pda::compute_pda(&escrow_program, &[&custody_label, &onchain_swap_id]);
    assert_eq!(
        derive_nssa_v0_1_2_native_custody_account_v1(&escrow_program, &onchain_swap_id),
        official_custody.to_bytes()
    );
    assert_ne!(
        derive_lez_native_custody_account_v1(&escrow_program, &onchain_swap_id),
        official_custody.to_bytes()
    );

    let owner = AccountId::new([3; 32]);
    let definition = AccountId::new([8; 32]);
    let official_token = ata_core::get_associated_token_account_id(
        &ata_program,
        &ata_core::compute_ata_seed(owner, definition),
    );
    assert_eq!(
        derive_nssa_v0_1_2_token_account_v1(&ata_program, &[3; 32], &[8; 32]),
        official_token.to_bytes()
    );
    assert_ne!(
        derive_lez_token_account_v1(&ata_program, &[3; 32], &[8; 32]),
        official_token.to_bytes()
    );
}
