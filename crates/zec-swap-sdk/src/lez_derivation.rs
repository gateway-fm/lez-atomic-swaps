//! Dependency-light LEZ public-PDA derivation shared with upstream compatibility tests.

use sha2::{Digest, Sha256};

const LEZ_SWAP_ID_V1_DOMAIN: &[u8] = b"logos.gateway.lez-swap-id.v1\0";

/// Derives the deterministic on-chain swap identifier from an application identifier.
#[must_use]
pub fn derive_lez_swap_id_v1(application_swap_id: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(LEZ_SWAP_ID_V1_DOMAIN);
    hasher.update(application_swap_id);
    hasher.finalize().into()
}

/// Derives a public LEZ v0.2 PDA byte-for-byte like pinned `lee_core` v0.2.0.
#[must_use]
pub fn derive_lez_public_pda_v1(program_id: &[u32; 8], seed: &[u8; 32]) -> [u8; 32] {
    const PREFIX: &[u8; 32] = b"/LEE/v0.2/AccountId/PDA/\0\0\0\0\0\0\0\0";
    let mut preimage = [0_u8; 96];
    preimage[..32].copy_from_slice(PREFIX);
    for (index, word) in program_id.iter().enumerate() {
        let offset = 32 + index * 4;
        preimage[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
    }
    preimage[64..].copy_from_slice(seed);
    Sha256::digest(preimage).into()
}

/// Derives a public NSSA PDA byte-for-byte like the LEZ v0.1.2 runtime.
///
/// The explicit versioned name prevents compatibility evidence from being mislabeled as LEZ
/// v0.2 evidence. The two runtimes use different domain separators for otherwise identical PDA
/// inputs.
#[must_use]
pub fn derive_nssa_v0_1_2_public_pda_v1(program_id: &[u32; 8], seed: &[u8; 32]) -> [u8; 32] {
    const PREFIX: &[u8; 32] = b"/NSSA/v0.2/AccountId/PDA/\0\0\0\0\0\0\0";
    let mut preimage = [0_u8; 96];
    preimage[..32].copy_from_slice(PREFIX);
    for (index, word) in program_id.iter().enumerate() {
        let offset = 32 + index * 4;
        preimage[offset..offset + 4].copy_from_slice(&word.to_le_bytes());
    }
    preimage[64..].copy_from_slice(seed);
    Sha256::digest(preimage).into()
}

/// Derives the v1 escrow metadata PDA from the exact escrow program and on-chain swap ID.
#[must_use]
pub fn derive_lez_metadata_account_v1(
    escrow_program_id: &[u32; 8],
    onchain_swap_id: &[u8; 32],
) -> [u8; 32] {
    derive_lez_public_pda_v1(escrow_program_id, onchain_swap_id)
}

/// Derives the escrow metadata PDA for the pinned LEZ v0.1.2 NSSA runtime.
#[must_use]
pub fn derive_nssa_v0_1_2_metadata_account_v1(
    escrow_program_id: &[u32; 8],
    onchain_swap_id: &[u8; 32],
) -> [u8; 32] {
    derive_nssa_v0_1_2_public_pda_v1(escrow_program_id, onchain_swap_id)
}

/// Derives native custody using pinned SPEL `custody`/swap multi-seed semantics.
#[must_use]
pub fn derive_lez_native_custody_account_v1(
    escrow_program_id: &[u32; 8],
    onchain_swap_id: &[u8; 32],
) -> [u8; 32] {
    let mut custody_label = [0_u8; 32];
    custody_label[..7].copy_from_slice(b"custody");
    let mut combined = Sha256::new();
    combined.update(custody_label);
    combined.update(onchain_swap_id);
    derive_lez_public_pda_v1(escrow_program_id, &combined.finalize().into())
}

/// Derives native custody for the pinned LEZ v0.1.2 NSSA runtime.
#[must_use]
pub fn derive_nssa_v0_1_2_native_custody_account_v1(
    escrow_program_id: &[u32; 8],
    onchain_swap_id: &[u8; 32],
) -> [u8; 32] {
    let mut custody_label = [0_u8; 32];
    custody_label[..7].copy_from_slice(b"custody");
    let mut combined = Sha256::new();
    combined.update(custody_label);
    combined.update(onchain_swap_id);
    derive_nssa_v0_1_2_public_pda_v1(escrow_program_id, &combined.finalize().into())
}

/// Derives an exact LEZ v0.2 associated-token account from program, owner, and definition.
#[must_use]
pub fn derive_lez_token_account_v1(
    ata_program_id: &[u32; 8],
    owner: &[u8; 32],
    definition: &[u8; 32],
) -> [u8; 32] {
    let mut seed = Sha256::new();
    seed.update(owner);
    seed.update(definition);
    derive_lez_public_pda_v1(ata_program_id, &seed.finalize().into())
}

/// Derives an associated-token account for the pinned LEZ v0.1.2 NSSA runtime.
#[must_use]
pub fn derive_nssa_v0_1_2_token_account_v1(
    ata_program_id: &[u32; 8],
    owner: &[u8; 32],
    definition: &[u8; 32],
) -> [u8; 32] {
    let mut seed = Sha256::new();
    seed.update(owner);
    seed.update(definition);
    derive_nssa_v0_1_2_public_pda_v1(ata_program_id, &seed.finalize().into())
}
