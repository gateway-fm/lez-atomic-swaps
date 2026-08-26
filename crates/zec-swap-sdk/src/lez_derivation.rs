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

/// Derives the LEZ v0.2 account ID for one exact x-only signing public key.
///
/// The domain is pinned to LEZ v0.2.0 commit
/// `a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a` and its
/// `lee/state_machine/src/signature/public_key.rs` implementation.
#[must_use]
pub fn derive_lez_public_account_v0_2(x_only_public_key: &[u8; 32]) -> [u8; 32] {
    const PREFIX: &[u8; 32] = b"/LEE/v0.3/AccountId/Public/\0\0\0\0\0";
    let mut hasher = Sha256::new();
    hasher.update(PREFIX);
    hasher.update(x_only_public_key);
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

#[cfg(test)]
mod tests {
    use sha2::{Digest as _, Sha256};

    use super::derive_lez_public_account_v0_2;

    #[test]
    fn v0_2_public_account_matches_official_default_identities() {
        let maker: [u8; 32] =
            hex::decode("1b84c5567b126440995d3ed5aaba0565d71e1834604819ff9c17f5e9d5dd078f")
                .unwrap()
                .try_into()
                .unwrap();
        let taker: [u8; 32] =
            hex::decode("4d4b6cd1361032ca9bd2aeb9d900aa4d45d9ead80ac9423374c451a7254d0766")
                .unwrap()
                .try_into()
                .unwrap();

        assert_eq!(
            hex::encode(derive_lez_public_account_v0_2(&maker)),
            "94b3cefdc7335256e802987a50f336cfed7053992c3bcc318054a0e3d8956166"
        );
        assert_eq!(
            hex::encode(derive_lez_public_account_v0_2(&taker)),
            "1e916b03cf49c0e6a03feecf124536d867f45c5e7cf82a108d1377120ee28ccc"
        );
    }

    #[test]
    fn v0_2_public_account_is_domain_separated() {
        let public_key = [7_u8; 32];
        let unprefixed: [u8; 32] = Sha256::digest(public_key).into();
        assert_ne!(derive_lez_public_account_v0_2(&public_key), unprefixed);
    }
}
