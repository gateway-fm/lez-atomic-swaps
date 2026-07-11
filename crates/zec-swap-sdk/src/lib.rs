//! Transparent Zcash protocol adapter for LEZ atomic swaps.

use zcash_script::{
    Opcode, op,
    opcode::PushValue,
    pattern, pv,
    script::{Component, Evaluable},
};

/// Failures while encoding a BIP-199 spend stack.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ScriptBuildError {
    /// A stack item exceeds Zcash Script's consensus push-size limit.
    #[error("{field} is too large for a script stack item: {length} bytes")]
    StackItemTooLarge {
        /// The input being encoded.
        field: &'static str,
        /// Its rejected byte length.
        length: usize,
    },
}

/// The exact SHA-256/CLTV P2PKH redeem script specified by BIP-199.
///
/// The contract stores script-level public-key hashes. Key derivation and
/// ownership belong to the transparent-wallet adapter, which uses canonical
/// librustzcash types.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bip199Contract {
    redeem_script: Vec<u8>,
    p2sh_script_pubkey: Vec<u8>,
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

        let redeem_script = Component(opcodes);
        let p2sh_script_pubkey = Component(pattern::pay_to_script_hash(&redeem_script)).to_bytes();

        Self {
            redeem_script: redeem_script.to_bytes(),
            p2sh_script_pubkey,
        }
    }

    /// Returns the consensus-encoded redeem script bytes.
    #[must_use]
    pub fn redeem_script(&self) -> &[u8] {
        &self.redeem_script
    }

    /// Returns the consensus-encoded P2SH script pubkey that funds this contract.
    #[must_use]
    pub fn p2sh_script_pubkey(&self) -> &[u8] {
        &self.p2sh_script_pubkey
    }

    /// Encodes `[signature, claimant_pubkey, preimage, true, redeem_script]`.
    ///
    /// The signature must include its Zcash sighash type byte. Signature and
    /// public-key validity are checked by the consensus interpreter, not this
    /// push-only serializer.
    ///
    /// # Errors
    ///
    /// Returns [`ScriptBuildError::StackItemTooLarge`] if any supplied item
    /// exceeds the consensus push-size limit.
    pub fn claim_script_sig(
        &self,
        signature: &[u8],
        claimant_pubkey: &[u8],
        preimage: &[u8],
    ) -> Result<Vec<u8>, ScriptBuildError> {
        self.spend_script_sig(
            &[
                stack_item("signature", signature)?,
                stack_item("claimant public key", claimant_pubkey)?,
                stack_item("preimage", preimage)?,
                pv::_1,
            ],
            "claim redeem script",
        )
    }

    /// Encodes `[signature, refund_pubkey, false, redeem_script]`.
    ///
    /// The spending transaction must use this contract's absolute lock time and
    /// a non-final input sequence; Zebra remains the consensus authority.
    ///
    /// # Errors
    ///
    /// Returns [`ScriptBuildError::StackItemTooLarge`] if any supplied item
    /// exceeds the consensus push-size limit.
    pub fn refund_script_sig(
        &self,
        signature: &[u8],
        refund_pubkey: &[u8],
    ) -> Result<Vec<u8>, ScriptBuildError> {
        self.spend_script_sig(
            &[
                stack_item("signature", signature)?,
                stack_item("refund public key", refund_pubkey)?,
                pv::_0,
            ],
            "refund redeem script",
        )
    }

    fn spend_script_sig<const N: usize>(
        &self,
        stack: &[PushValue; N],
        redeem_field: &'static str,
    ) -> Result<Vec<u8>, ScriptBuildError> {
        let mut stack = stack.to_vec();
        stack.push(stack_item(redeem_field, &self.redeem_script)?);
        Ok(Component(stack).to_bytes())
    }
}

fn stack_item(field: &'static str, bytes: &[u8]) -> Result<PushValue, ScriptBuildError> {
    pv::push_value(bytes).ok_or(ScriptBuildError::StackItemTooLarge {
        field,
        length: bytes.len(),
    })
}
