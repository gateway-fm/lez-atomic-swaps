//! Canonical transparent-only Zcash transaction construction for BIP-199 spends.

use orchard::bundle as orchard_bundle;
use sapling::bundle as sapling_bundle;
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use sha2::{Digest, Sha256};
use zcash_primitives::transaction::{
    Authorization as TransactionAuthorization, Authorized as TransactionAuthorized, Transaction,
    TransactionData, TxVersion,
    sighash::{SignableInput as TransactionSignableInput, signature_hash},
    txid::TxIdDigester,
};
use zcash_protocol::{
    consensus::{BlockHeight, BranchId},
    value::Zatoshis,
};
use zcash_script::script::Code;
use zcash_transparent::{
    address::{Script, TransparentAddress},
    bundle::{
        Authorization as TransparentAuthorization, Authorized as TransparentAuthorized, Bundle,
        OutPoint, TxIn, TxOut,
    },
    sighash::{
        SIGHASH_ALL, SighashType, SignableInput as TransparentSignableInput,
        TransparentAuthorizingContext,
    },
};

use crate::{Bip199Contract, ScriptBuildError};

/// Errors produced while validating or signing a transparent BIP-199 spend.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TransactionBuildError {
    /// The fee would consume more than the contract output contains.
    #[error("transaction fee exceeds the input value")]
    FeeExceedsInput,
    /// The supplied secret key does not control the selected contract branch.
    #[error("secret key does not control the selected BIP-199 branch")]
    WrongSpendingKey,
    /// The supplied preimage does not match the contract's SHA-256 digest.
    #[error("preimage does not match the BIP-199 secret digest")]
    WrongPreimage,
    /// The selected transaction version cannot be used in the requested epoch.
    #[error("transaction version {transaction_version:?} is invalid in branch {branch_id:?}")]
    UnsupportedConsensusBranch {
        /// The rejected consensus branch.
        branch_id: BranchId,
        /// The transaction version selected by this transparent-only adapter.
        transaction_version: TxVersion,
    },
    /// The fetched funding output is not locked by the supplied BIP-199 contract.
    #[error("funding output scriptPubKey does not match the BIP-199 contract")]
    FundingScriptMismatch,
    /// A script stack could not be encoded under consensus limits.
    #[error(transparent)]
    Script(#[from] ScriptBuildError),
    /// Canonical transaction serialization failed.
    #[error("canonical transaction serialization failed: {0}")]
    Serialization(String),
}

/// Immutable inputs needed to spend one transparent BIP-199 output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransparentSpendRequest {
    prevout: OutPoint,
    funding_output: TxOut,
    destination: TransparentAddress,
    fee: Zatoshis,
    output_value: Zatoshis,
    expiry_height: BlockHeight,
    consensus_branch_id: BranchId,
}

impl TransparentSpendRequest {
    /// Validates the actual funding output, fee, and V5 consensus epoch.
    ///
    /// `funding_output` must be the output fetched for `prevout`, not a locally
    /// synthesized amount. Its script must exactly match `contract` so ZIP-244
    /// commits to the value and script that are expected to exist on chain.
    ///
    /// # Errors
    ///
    /// Returns a typed error when the branch is incompatible with V5, the
    /// funding script does not match `contract`, or `fee` exceeds the funding
    /// output value.
    pub fn new(
        contract: &Bip199Contract,
        prevout: OutPoint,
        funding_output: TxOut,
        destination: TransparentAddress,
        fee: Zatoshis,
        expiry_height: BlockHeight,
        consensus_branch_id: BranchId,
    ) -> Result<Self, TransactionBuildError> {
        if !TxVersion::V5.valid_in_branch(consensus_branch_id) {
            return Err(TransactionBuildError::UnsupportedConsensusBranch {
                branch_id: consensus_branch_id,
                transaction_version: TxVersion::V5,
            });
        }
        let contract_script = Script(Code(contract.p2sh_script_pubkey().to_vec()));
        if funding_output.script_pubkey() != &contract_script {
            return Err(TransactionBuildError::FundingScriptMismatch);
        }
        let output_value =
            (funding_output.value() - fee).ok_or(TransactionBuildError::FeeExceedsInput)?;
        Ok(Self {
            prevout,
            funding_output,
            destination,
            fee,
            output_value,
            expiry_height,
            consensus_branch_id,
        })
    }

    /// Returns the contract outpoint being spent.
    #[must_use]
    pub const fn prevout(&self) -> &OutPoint {
        &self.prevout
    }

    /// Returns the value of the contract output being spent.
    #[must_use]
    pub fn prevout_value(&self) -> Zatoshis {
        self.funding_output.value()
    }

    /// Returns the fetched funding output committed by the signature hash.
    #[must_use]
    pub const fn funding_output(&self) -> &TxOut {
        &self.funding_output
    }

    /// Returns the destination transparent address.
    #[must_use]
    pub const fn destination(&self) -> TransparentAddress {
        self.destination
    }

    /// Returns the exact miner fee.
    #[must_use]
    pub const fn fee(&self) -> Zatoshis {
        self.fee
    }

    /// Returns the transaction expiry height.
    #[must_use]
    pub const fn expiry_height(&self) -> BlockHeight {
        self.expiry_height
    }

    /// Returns the ZIP-244 consensus branch identifier.
    #[must_use]
    pub const fn consensus_branch_id(&self) -> BranchId {
        self.consensus_branch_id
    }
}

#[derive(Debug)]
struct UnsignedTransparent {
    prevouts: Vec<TxOut>,
}

impl TransparentAuthorization for UnsignedTransparent {
    type ScriptSig = ();
}

impl TransparentAuthorizingContext for UnsignedTransparent {
    fn input_amounts(&self) -> Vec<Zatoshis> {
        self.prevouts.iter().map(TxOut::value).collect()
    }

    fn input_scriptpubkeys(&self) -> Vec<Script> {
        self.prevouts
            .iter()
            .map(|output| output.script_pubkey().clone())
            .collect()
    }
}

#[derive(Debug)]
struct UnsignedTransaction;

impl TransactionAuthorization for UnsignedTransaction {
    type TransparentAuth = UnsignedTransparent;
    type SaplingAuth = sapling_bundle::Authorized;
    type OrchardAuth = orchard_bundle::Authorized;
}

/// Builds and signs the refund branch of a transparent BIP-199 contract.
///
/// The transaction's `nLockTime` is exactly the contract deadline and its
/// input sequence is [`crate::REFUND_INPUT_SEQUENCE`], enabling CLTV.
///
/// # Errors
///
/// Fails closed when the key does not own the refund branch or canonical
/// script/transaction construction fails.
pub fn build_refund_transaction(
    contract: &Bip199Contract,
    request: &TransparentSpendRequest,
    secret_key: &SecretKey,
) -> Result<Transaction, TransactionBuildError> {
    build_signed_transaction(
        contract,
        request,
        secret_key,
        contract.refund_pubkey_hash(),
        contract.refund_lock_time(),
        contract.refund_input_sequence(),
        Bip199Contract::refund_script_sig,
    )
}

/// Builds and signs the claim branch of a transparent BIP-199 contract.
///
/// # Errors
///
/// Fails closed when the key does not own the claimant branch, the preimage
/// does not match, or canonical script/transaction construction fails.
pub fn build_claim_transaction(
    contract: &Bip199Contract,
    request: &TransparentSpendRequest,
    secret_key: &SecretKey,
    preimage: &[u8],
) -> Result<Transaction, TransactionBuildError> {
    let preimage_digest: [u8; 32] = Sha256::digest(preimage).into();
    if preimage_digest != contract.secret_digest() {
        return Err(TransactionBuildError::WrongPreimage);
    }

    build_signed_transaction(
        contract,
        request,
        secret_key,
        contract.claimant_pubkey_hash(),
        0,
        u32::MAX,
        |contract, signature, public_key| {
            contract.claim_script_sig(signature, public_key, preimage)
        },
    )
}

fn build_signed_transaction<F>(
    contract: &Bip199Contract,
    request: &TransparentSpendRequest,
    secret_key: &SecretKey,
    expected_pubkey_hash: [u8; 20],
    lock_time: u32,
    sequence: u32,
    script_sig: F,
) -> Result<Transaction, TransactionBuildError>
where
    F: FnOnce(&Bip199Contract, &[u8], &[u8]) -> Result<Vec<u8>, ScriptBuildError>,
{
    let contract_script = Script(Code(contract.p2sh_script_pubkey().to_vec()));
    if request.funding_output.script_pubkey() != &contract_script {
        return Err(TransactionBuildError::FundingScriptMismatch);
    }
    let secp = Secp256k1::signing_only();
    let public_key = PublicKey::from_secret_key(&secp, secret_key);
    if pubkey_hash(&public_key) != expected_pubkey_hash {
        return Err(TransactionBuildError::WrongSpendingKey);
    }

    let redeem_script = Script(Code(contract.redeem_script().to_vec()));
    let prevout_script = request.funding_output.script_pubkey().clone();
    let prevout = request.funding_output.clone();
    let unsigned_bundle = Bundle {
        vin: vec![TxIn::from_parts(request.prevout.clone(), (), sequence)],
        vout: vec![TxOut::new(
            request.output_value,
            request.destination.script().into(),
        )],
        authorization: UnsignedTransparent {
            prevouts: vec![prevout],
        },
    };
    let unsigned = TransactionData::<UnsignedTransaction>::from_parts(
        TxVersion::V5,
        request.consensus_branch_id,
        lock_time,
        request.expiry_height,
        Some(unsigned_bundle),
        None,
        None,
        None,
    );
    let txid_parts = unsigned.digest(TxIdDigester);
    let bundle = unsigned
        .transparent_bundle()
        .expect("constructed with transparent bundle");
    let signable = TransparentSignableInput::from_parts(
        bundle,
        SighashType::ALL,
        0,
        &redeem_script,
        &prevout_script,
        request.funding_output.value(),
    )
    .expect("single input is present");
    let digest = signature_hash(
        &unsigned,
        &TransactionSignableInput::Transparent(signable),
        &txid_parts,
    );
    let signature = secp.sign_ecdsa(&Message::from_digest(*digest.as_ref()), secret_key);
    let mut signature_bytes = signature.serialize_der().to_vec();
    signature_bytes.push(SIGHASH_ALL);
    let script_sig = Script(Code(script_sig(
        contract,
        &signature_bytes,
        &public_key.serialize(),
    )?));

    let authorized_bundle = Bundle {
        vin: vec![TxIn::from_parts(
            request.prevout.clone(),
            script_sig,
            sequence,
        )],
        vout: vec![TxOut::new(
            request.output_value,
            request.destination.script().into(),
        )],
        authorization: TransparentAuthorized,
    };
    TransactionData::<TransactionAuthorized>::from_parts(
        TxVersion::V5,
        request.consensus_branch_id,
        lock_time,
        request.expiry_height,
        Some(authorized_bundle),
        None,
        None,
        None,
    )
    .freeze()
    .map_err(|error| TransactionBuildError::Serialization(error.to_string()))
}

fn pubkey_hash(public_key: &PublicKey) -> [u8; 20] {
    match TransparentAddress::from_pubkey(public_key) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!("public keys always yield P2PKH"),
    }
}
