//! Transparent Zcash protocol adapter for LEZ atomic swaps.

use zcash_script::{
    Opcode, op, pattern,
    script::{Component, Evaluable},
};

/// The exact SHA-256/CLTV P2PKH redeem script specified by BIP-199.
///
/// The contract stores script-level public-key hashes. Key derivation and
/// ownership belong to the transparent-wallet adapter, which uses canonical
/// librustzcash types.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bip199Contract {
    redeem_script: Vec<u8>,
}

impl Bip199Contract {
    /// Constructs the exact BIP-199 common-tail layout.
    ///
    /// `refund_lock_time` is the absolute Zcash `nLockTime` threshold. The
    /// spending transaction must also use a non-final input sequence for CLTV
    /// to take effect.
    #[must_use]
    pub fn new(
        refund_lock_time: u32,
        refund_pubkey_hash: [u8; 20],
        secret_digest: [u8; 32],
        claimant_pubkey_hash: [u8; 20],
    ) -> Self {
        let claim_branch = [
            &[op::SHA256][..],
            &pattern::equals(pattern::push_256b_hash(&secret_digest), true),
            &[
                op::DUP,
                op::HASH160,
                Opcode::PushValue(pattern::push_160b_hash(&claimant_pubkey_hash)),
            ],
        ]
        .concat();
        let refund_branch = [
            &pattern::check_lock_time_verify(refund_lock_time)[..],
            &[
                op::DUP,
                op::HASH160,
                Opcode::PushValue(pattern::push_160b_hash(&refund_pubkey_hash)),
            ],
        ]
        .concat();
        let mut opcodes = pattern::branch(&claim_branch, &refund_branch);
        opcodes.extend([op::EQUALVERIFY, op::CHECKSIG]);

        Self {
            redeem_script: Component(opcodes).to_bytes(),
        }
    }

    /// Returns the consensus-encoded redeem script bytes.
    #[must_use]
    pub fn redeem_script(&self) -> &[u8] {
        &self.redeem_script
    }
}
