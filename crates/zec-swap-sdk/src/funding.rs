//! Deterministic, actor-scoped funding of transparent BIP-199 contracts.

use std::collections::HashSet;

use orchard::bundle as orchard_bundle;
use sapling::bundle as sapling_bundle;
use secp256k1::{PublicKey, SecretKey};
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
use zcash_script::script::{Code, PubKey};
use zcash_transparent::{
    address::TransparentAddress,
    builder::{TransparentBuilder, TransparentSigningSet, Unauthorized as TransparentUnauthorized},
    bundle::{OutPoint, TxOut},
};

use crate::Bip199Contract;

/// Failures produced while selecting coins or building a funding transaction.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum FundingBuildError {
    /// The selected consensus epoch does not support V5 transactions.
    #[error("transaction version {transaction_version:?} is invalid in branch {branch_id:?}")]
    UnsupportedConsensusBranch {
        /// The rejected consensus branch.
        branch_id: BranchId,
        /// The transaction version selected by this transparent-only adapter.
        transaction_version: TxVersion,
    },
    /// The contract output must contain a positive value.
    #[error("contract value must be positive")]
    EmptyContractValue,
    /// The owner public key does not match a candidate output script.
    #[error("candidate UTXO is not a P2PKH output controlled by the funding actor")]
    UtxoScriptMismatch,
    /// A candidate outpoint was supplied more than once.
    #[error("candidate outpoint is duplicated")]
    DuplicateOutpoint,
    /// The candidates cannot cover the contract value and target fee.
    #[error("insufficient funds: available {available:?}, required {required:?}")]
    InsufficientFunds {
        /// Total value of valid candidate outputs.
        available: Zatoshis,
        /// Contract value plus the requested fee.
        required: Zatoshis,
    },
    /// Checked Zcash amount arithmetic exceeded the monetary range.
    #[error("funding amount arithmetic overflowed the Zcash monetary range")]
    AmountOverflow,
    /// The signing key is not the funding actor key committed by the request.
    #[error("secret key does not control the funding actor's P2PKH outputs")]
    WrongFundingKey,
    /// Canonical transparent construction or signing rejected the request.
    #[error("canonical transparent transaction construction failed: {0}")]
    CanonicalBuilder(String),
    /// Canonical transaction serialization failed.
    #[error("canonical transaction serialization failed: {0}")]
    Serialization(String),
}

/// A fetched transparent output that is available to fund a swap.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransparentUtxo {
    outpoint: OutPoint,
    output: TxOut,
}

impl TransparentUtxo {
    /// Creates a candidate from its chain outpoint and fetched output.
    #[must_use]
    pub const fn new(outpoint: OutPoint, output: TxOut) -> Self {
        Self { outpoint, output }
    }

    /// Returns the chain outpoint consumed by this candidate.
    #[must_use]
    pub const fn outpoint(&self) -> &OutPoint {
        &self.outpoint
    }

    /// Returns the fetched output, including its value and locking script.
    #[must_use]
    pub const fn output(&self) -> &TxOut {
        &self.output
    }
}

/// Validated inputs and deterministic policy for funding one contract output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransparentFundingRequest {
    candidates: Vec<TransparentUtxo>,
    funding_pubkey: PublicKey,
    contract_value: Zatoshis,
    target_fee: Zatoshis,
    minimum_change: Zatoshis,
    expiry_height: BlockHeight,
    consensus_branch_id: BranchId,
}

impl TransparentFundingRequest {
    /// Validates actor ownership, duplicate outpoints, amounts, and the V5 epoch.
    ///
    /// Every candidate must be a P2PKH output controlled by `funding_pubkey`.
    /// Change is always returned to that same key, keeping the private signing
    /// key outside the request and making the role boundary explicit.
    ///
    /// # Errors
    ///
    /// Returns a typed error for an incompatible branch, empty contract value,
    /// duplicate outpoint, foreign/non-P2PKH candidate, or amount overflow.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        candidates: Vec<TransparentUtxo>,
        funding_pubkey: PublicKey,
        contract_value: Zatoshis,
        target_fee: Zatoshis,
        minimum_change: Zatoshis,
        expiry_height: BlockHeight,
        consensus_branch_id: BranchId,
    ) -> Result<Self, FundingBuildError> {
        if !TxVersion::V5.valid_in_branch(consensus_branch_id) {
            return Err(FundingBuildError::UnsupportedConsensusBranch {
                branch_id: consensus_branch_id,
                transaction_version: TxVersion::V5,
            });
        }
        if contract_value.is_zero() {
            return Err(FundingBuildError::EmptyContractValue);
        }
        let owner_script: zcash_transparent::address::Script =
            funding_address(&funding_pubkey).script().into();
        let mut outpoints = HashSet::with_capacity(candidates.len());
        for candidate in &candidates {
            if candidate.output.script_pubkey() != &owner_script {
                return Err(FundingBuildError::UtxoScriptMismatch);
            }
            if !outpoints.insert((*candidate.outpoint.hash(), candidate.outpoint.n())) {
                return Err(FundingBuildError::DuplicateOutpoint);
            }
        }
        if (contract_value + target_fee).is_none() {
            return Err(FundingBuildError::AmountOverflow);
        }

        Ok(Self {
            candidates,
            funding_pubkey,
            contract_value,
            target_fee,
            minimum_change,
            expiry_height,
            consensus_branch_id,
        })
    }

    /// Returns the validated candidate set.
    #[must_use]
    pub fn candidates(&self) -> &[TransparentUtxo] {
        &self.candidates
    }

    /// Returns the public key controlling every candidate and any change.
    #[must_use]
    pub const fn funding_pubkey(&self) -> &PublicKey {
        &self.funding_pubkey
    }

    /// Returns the exact value locked into the BIP-199 P2SH output.
    #[must_use]
    pub const fn contract_value(&self) -> Zatoshis {
        self.contract_value
    }

    /// Returns the requested minimum miner fee before dust absorption.
    #[must_use]
    pub const fn target_fee(&self) -> Zatoshis {
        self.target_fee
    }

    /// Returns the minimum value for creating a change output.
    #[must_use]
    pub const fn minimum_change(&self) -> Zatoshis {
        self.minimum_change
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

/// Deterministically selected inputs and the resulting fee/change decision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FundingSelection {
    selected: Vec<TransparentUtxo>,
    selected_value: Zatoshis,
    change: Option<Zatoshis>,
    fee: Zatoshis,
}

impl FundingSelection {
    /// Returns selected inputs in canonical policy order.
    #[must_use]
    pub fn selected(&self) -> &[TransparentUtxo] {
        &self.selected
    }

    /// Returns the total selected input value.
    #[must_use]
    pub const fn selected_value(&self) -> Zatoshis {
        self.selected_value
    }

    /// Returns a change value only when it meets the configured threshold.
    #[must_use]
    pub const fn change(&self) -> Option<Zatoshis> {
        self.change
    }

    /// Returns the actual fee, including any absorbed sub-threshold change.
    #[must_use]
    pub const fn fee(&self) -> Zatoshis {
        self.fee
    }
}

/// Selects actor-owned UTXOs using deterministic largest-first ordering.
///
/// Equal-value candidates are ordered lexicographically by transaction hash,
/// then output index. Selection stops as soon as the contract value and target
/// fee are covered. A positive remainder below `minimum_change` is absorbed
/// into the fee; no dust change output is created.
///
/// # Errors
///
/// Returns [`FundingBuildError::InsufficientFunds`] when all candidates cannot
/// cover the contract value and target fee, or `AmountOverflow` if summation
/// exceeds the Zcash monetary range.
pub fn select_funding_utxos(
    request: &TransparentFundingRequest,
) -> Result<FundingSelection, FundingBuildError> {
    let required =
        (request.contract_value + request.target_fee).ok_or(FundingBuildError::AmountOverflow)?;
    let mut candidates = request.candidates.clone();
    candidates.sort_by(|left, right| {
        right
            .output
            .value()
            .cmp(&left.output.value())
            .then_with(|| left.outpoint.hash().cmp(right.outpoint.hash()))
            .then_with(|| left.outpoint.n().cmp(&right.outpoint.n()))
    });

    let available = candidates
        .iter()
        .try_fold(Zatoshis::ZERO, |total, candidate| {
            (total + candidate.output.value()).ok_or(FundingBuildError::AmountOverflow)
        })?;
    if available < required {
        return Err(FundingBuildError::InsufficientFunds {
            available,
            required,
        });
    }

    let mut selected = Vec::new();
    let mut selected_value = Zatoshis::ZERO;
    for candidate in candidates {
        selected_value =
            (selected_value + candidate.output.value()).ok_or(FundingBuildError::AmountOverflow)?;
        selected.push(candidate);
        if selected_value >= required {
            break;
        }
    }

    let remainder = (selected_value - required).ok_or(FundingBuildError::AmountOverflow)?;
    let (change, fee) = if remainder.is_positive() && remainder >= request.minimum_change {
        (Some(remainder), request.target_fee)
    } else {
        (
            None,
            (request.target_fee + remainder).ok_or(FundingBuildError::AmountOverflow)?,
        )
    };

    Ok(FundingSelection {
        selected,
        selected_value,
        change,
        fee,
    })
}

#[derive(Debug)]
struct FundingUnsigned;

impl TransactionAuthorization for FundingUnsigned {
    type TransparentAuth = TransparentUnauthorized;
    type SaplingAuth = sapling_bundle::Authorized;
    type OrchardAuth = orchard_bundle::Authorized;
}

/// Builds and ZIP-244-signs a transparent V5 transaction funding `contract`.
///
/// The first output is always the exact BIP-199 P2SH contract output. Optional
/// change is the second output and returns only to the funding actor.
///
/// # Errors
///
/// Fails closed for selection errors, a foreign signing key, or canonical
/// librustzcash construction/signing failures.
pub fn build_funding_transaction(
    contract: &Bip199Contract,
    request: &TransparentFundingRequest,
    secret_key: &SecretKey,
) -> Result<Transaction, FundingBuildError> {
    let mut signing_set = TransparentSigningSet::new();
    let signing_pubkey = signing_set.add_key(*secret_key);
    if signing_pubkey != request.funding_pubkey {
        return Err(FundingBuildError::WrongFundingKey);
    }
    let selection = select_funding_utxos(request)?;
    let mut builder = TransparentBuilder::empty();
    for candidate in selection.selected() {
        builder
            .add_p2pkh_input(
                request.funding_pubkey,
                candidate.outpoint.clone(),
                candidate.output.clone(),
            )
            .map_err(|error| FundingBuildError::CanonicalBuilder(error.to_string()))?;
    }
    let contract_script = PubKey::parse(&Code(contract.p2sh_script_pubkey().to_vec()))
        .map_err(|error| FundingBuildError::CanonicalBuilder(format!("{error:?}")))?;
    let contract_address = TransparentAddress::from_script_pubkey(&contract_script)
        .ok_or_else(|| FundingBuildError::CanonicalBuilder("invalid BIP-199 P2SH script".into()))?;
    builder
        .add_output(&contract_address, request.contract_value)
        .map_err(|error| FundingBuildError::CanonicalBuilder(error.to_string()))?;
    if let Some(change) = selection.change {
        builder
            .add_output(&funding_address(&request.funding_pubkey), change)
            .map_err(|error| FundingBuildError::CanonicalBuilder(error.to_string()))?;
    }

    let unsigned_bundle = builder.build().ok_or_else(|| {
        FundingBuildError::CanonicalBuilder("funding transaction has no bundle".into())
    })?;
    let unsigned = TransactionData::<FundingUnsigned>::from_parts(
        TxVersion::V5,
        request.consensus_branch_id,
        0,
        request.expiry_height,
        Some(unsigned_bundle),
        None,
        None,
        None,
    );
    let txid_parts = unsigned.digest(TxIdDigester);
    let authorized_bundle = unsigned
        .transparent_bundle()
        .ok_or_else(|| {
            FundingBuildError::CanonicalBuilder("funding transaction has no bundle".into())
        })?
        .clone()
        .apply_signatures(
            |input| {
                *signature_hash(
                    &unsigned,
                    &TransactionSignableInput::Transparent(input),
                    &txid_parts,
                )
                .as_ref()
            },
            &signing_set,
        )
        .map_err(|error| FundingBuildError::CanonicalBuilder(error.to_string()))?;

    TransactionData::<TransactionAuthorized>::from_parts(
        TxVersion::V5,
        request.consensus_branch_id,
        0,
        request.expiry_height,
        Some(authorized_bundle),
        None,
        None,
        None,
    )
    .freeze()
    .map_err(|error| FundingBuildError::Serialization(error.to_string()))
}

fn funding_address(public_key: &PublicKey) -> TransparentAddress {
    TransparentAddress::from_pubkey(public_key)
}
