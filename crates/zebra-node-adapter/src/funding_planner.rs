//! Agreement-committed planning from exact Zebra `gettxout` candidates.

use std::{fmt, io::Cursor};

use async_trait::async_trait;
use lez_swap_core::Participant;
use lez_zec_swap_sdk::{
    Bip199Contract, FirstLockIntentError, FirstLockPlanV1, FirstLockStepV1, MAX_ZEC_FUNDING_INPUTS,
    PreparedFirstLockSubmissionV1, TransparentFundingRequest, TransparentUtxo,
    ZecAgreementExecutionError, ZecAgreementV1, select_funding_utxos,
};
use secp256k1::PublicKey;
use zcash_primitives::transaction::{Transaction, TxVersion};
use zcash_protocol::consensus::BlockHeight;
use zcash_transparent::{
    address::{Script, TransparentAddress},
    bundle::OutPoint,
};

use crate::{ZebraChainIdentity, ZebraChainInfo, ZebraRpc};

/// Trusted, narrow role- and key-bearing capability for canonical Zcash funding.
///
/// Implementations must sign the exact request passed by the planner. The planner
/// independently checks transaction policy and shape, but consensus authorization
/// remains the signer and eventual Zebra broadcast-validation boundary.
#[async_trait]
pub trait ZebraFundingSigner: Send + Sync {
    /// Structured key-provider, construction, or serialization error.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Fixed actor role owning this capability.
    fn participant(&self) -> Participant;

    /// Public key controlled by this capability.
    fn public_key(&self) -> PublicKey;

    /// Builds, signs, and canonically serializes the exact agreement-derived request.
    async fn sign_funding(
        &self,
        contract: &Bip199Contract,
        request: &TransparentFundingRequest,
    ) -> Result<Vec<u8>, Self::Error>;
}

/// Production planner for one agreement-committed set of exact Zebra outpoints.
#[derive(Clone)]
pub struct ExactOutpointZcashFundingPlanner<R, S> {
    rpc: R,
    signer: S,
    identity: ZebraChainIdentity,
    local_participant: Participant,
}

impl<R, S> ExactOutpointZcashFundingPlanner<R, S> {
    /// Binds one typed Zebra transport, immutable chain identity, role, and signer.
    #[must_use]
    pub const fn new(
        rpc: R,
        identity: ZebraChainIdentity,
        local_participant: Participant,
        signer: S,
    ) -> Self {
        Self {
            rpc,
            signer,
            identity,
            local_participant,
        }
    }

    /// Typed transport used for the stable exact-outpoint query.
    #[must_use]
    pub const fn rpc(&self) -> &R {
        &self.rpc
    }

    /// Immutable configured chain identity.
    #[must_use]
    pub const fn identity(&self) -> ZebraChainIdentity {
        self.identity
    }

    /// Role authorized to construct the funding plan.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.local_participant
    }

    /// Role-scoped signing capability.
    #[must_use]
    pub const fn signer(&self) -> &S {
        &self.signer
    }
}

impl<R, S> fmt::Debug for ExactOutpointZcashFundingPlanner<R, S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ExactOutpointZcashFundingPlanner")
            .field("rpc", &"[REDACTED]")
            .field("signer", &"[REDACTED]")
            .field("identity", &self.identity)
            .field("local_participant", &self.local_participant)
            .finish()
    }
}

/// Fail-closed exact-outpoint funding planning failure.
#[derive(thiserror::Error)]
pub enum ExactOutpointZcashFundingPlannerError<RE, SE>
where
    RE: std::error::Error + Send + Sync + 'static,
    SE: std::error::Error + Send + Sync + 'static,
{
    /// Configured role is not the signed Zcash funder.
    #[error("configured actor role is not the signed Zcash funder")]
    WrongRole,
    /// Signer capability is fixed to another role.
    #[error("Zcash funding signer role differs from the configured actor")]
    SignerRoleMismatch,
    /// Signer does not control the signed funding key.
    #[error("Zcash funding signer does not control the agreement key")]
    SignerKeyMismatch,
    /// Candidate count is outside the agreement wire bound.
    #[error("Zcash funding candidate count is empty or exceeds the fixed maximum")]
    InvalidCandidateCount,
    /// Configured network differs from the signed agreement.
    #[error("configured Zebra network differs from the signed agreement")]
    ConfiguredNetworkMismatch,
    /// Configured branch differs from the signed agreement.
    #[error("configured Zebra branch differs from the signed agreement")]
    ConfiguredConsensusBranchMismatch,
    /// Live RPC chain spelling differs from immutable configuration.
    #[error("Zebra RPC chain differs from configured identity")]
    RpcChainMismatch,
    /// Live RPC branch differs from immutable configuration.
    #[error("Zebra RPC consensus branch differs from configured identity")]
    RpcConsensusBranchMismatch,
    /// Height-zero hash differs from immutable configuration.
    #[error("Zebra genesis block differs from configured identity")]
    GenesisMismatch,
    /// An exact candidate is not in the current UTXO set.
    #[error("an agreement funding candidate is unavailable")]
    CandidateUnavailable,
    /// A candidate is not confirmed.
    #[error("an agreement funding candidate is not confirmed")]
    CandidateUnconfirmed,
    /// A `gettxout` answer names a different best-chain tip.
    #[error("a Zebra UTXO response names a different best-chain tip")]
    UtxoTipMismatch,
    /// Tip samples bracketing all exact UTXO queries differ.
    #[error("Zebra tip changed while planning agreement funding")]
    UnstableTip,
    /// Typed Zebra transport operation failed.
    #[error("typed Zebra RPC operation failed")]
    Rpc(#[source] RE),
    /// Signed agreement rejected the exact fetched candidate set or derived policy.
    #[error("signed agreement rejected the exact Zcash funding request")]
    Agreement(#[source] ZecAgreementExecutionError),
    /// Role-local signing, construction, or serialization failed.
    #[error("role-local Zcash funding signing failed")]
    Signer(#[source] SE),
    /// Returned bytes are not one canonical transaction.
    #[error("signed Zcash funding bytes are malformed")]
    MalformedSignedTransaction(#[source] std::io::Error),
    /// Returned bytes contain data after the canonical transaction.
    #[error("signed Zcash funding bytes contain trailing data")]
    TrailingSignedTransaction,
    /// Re-serialization differs from the exact signer output.
    #[error("signed Zcash funding bytes are not canonical")]
    NonCanonicalSignedTransaction,
    /// Funding signer returned a non-V5 transaction.
    #[error("signed Zcash funding transaction is not V5")]
    WrongTransactionVersion,
    /// Signed transaction does not lock the exact agreement output first.
    #[error("signed Zcash funding transaction differs from agreement output policy")]
    SignedTransactionPolicyMismatch,
    /// Canonical in-memory serialization unexpectedly failed.
    #[error("canonical Zcash funding serialization failed")]
    Serialization(#[source] std::io::Error),
    /// Prepared first-lock record rejected the exact signed material.
    #[error("prepared Zcash first-lock plan is invalid")]
    Intent(#[source] FirstLockIntentError),
}

impl<RE, SE> fmt::Debug for ExactOutpointZcashFundingPlannerError<RE, SE>
where
    RE: std::error::Error + Send + Sync + 'static,
    SE: std::error::Error + Send + Sync + 'static,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::WrongRole => "WrongRole",
            Self::SignerRoleMismatch => "SignerRoleMismatch",
            Self::SignerKeyMismatch => "SignerKeyMismatch",
            Self::InvalidCandidateCount => "InvalidCandidateCount",
            Self::ConfiguredNetworkMismatch => "ConfiguredNetworkMismatch",
            Self::ConfiguredConsensusBranchMismatch => "ConfiguredConsensusBranchMismatch",
            Self::RpcChainMismatch => "RpcChainMismatch",
            Self::RpcConsensusBranchMismatch => "RpcConsensusBranchMismatch",
            Self::GenesisMismatch => "GenesisMismatch",
            Self::CandidateUnavailable => "CandidateUnavailable",
            Self::CandidateUnconfirmed => "CandidateUnconfirmed",
            Self::UtxoTipMismatch => "UtxoTipMismatch",
            Self::UnstableTip => "UnstableTip",
            Self::Rpc(_) => "Rpc([REDACTED])",
            Self::Agreement(_) => "Agreement([REDACTED])",
            Self::Signer(_) => "Signer([REDACTED])",
            Self::MalformedSignedTransaction(_) => "MalformedSignedTransaction([REDACTED])",
            Self::TrailingSignedTransaction => "TrailingSignedTransaction",
            Self::NonCanonicalSignedTransaction => "NonCanonicalSignedTransaction",
            Self::WrongTransactionVersion => "WrongTransactionVersion",
            Self::SignedTransactionPolicyMismatch => "SignedTransactionPolicyMismatch",
            Self::Serialization(_) => "Serialization([REDACTED])",
            Self::Intent(_) => "Intent([REDACTED])",
        })
    }
}

impl<R, S> ExactOutpointZcashFundingPlanner<R, S>
where
    R: ZebraRpc,
    S: ZebraFundingSigner,
{
    /// Builds one exact Zcash funding plan from signed candidate outpoints.
    ///
    /// # Errors
    ///
    /// Fails closed unless role, key, chain, genesis, every exact UTXO, stable
    /// tip, signed input-set commitment, canonical build, and serialization agree.
    pub async fn plan(
        &self,
        agreement: &ZecAgreementV1,
        candidate_outpoints: Vec<OutPoint>,
    ) -> Result<FirstLockPlanV1, ExactOutpointZcashFundingPlannerError<R::Error, S::Error>> {
        if candidate_outpoints.is_empty() || candidate_outpoints.len() > MAX_ZEC_FUNDING_INPUTS {
            return Err(ExactOutpointZcashFundingPlannerError::InvalidCandidateCount);
        }
        self.validate_authority(agreement)?;
        let before = self.sample_validated_tip().await?;
        self.validate_genesis().await?;

        let mut candidates = Vec::with_capacity(candidate_outpoints.len());
        for outpoint in candidate_outpoints {
            let unspent = self
                .rpc
                .unspent_output(&outpoint)
                .await
                .map_err(ExactOutpointZcashFundingPlannerError::Rpc)?
                .ok_or(ExactOutpointZcashFundingPlannerError::CandidateUnavailable)?;
            if unspent.confirmations() == 0 {
                return Err(ExactOutpointZcashFundingPlannerError::CandidateUnconfirmed);
            }
            if unspent.best_block() != before.tip_hash() {
                return Err(ExactOutpointZcashFundingPlannerError::UtxoTipMismatch);
            }
            candidates.push(TransparentUtxo::new(outpoint, unspent.output().clone()));
        }

        let after = self.sample_validated_tip().await?;
        if !same_tip(before, after) {
            return Err(ExactOutpointZcashFundingPlannerError::UnstableTip);
        }
        let request = agreement
            .funding_request(candidates, before.tip_height())
            .map_err(ExactOutpointZcashFundingPlannerError::Agreement)?;
        let exact = self
            .signer
            .sign_funding(agreement.binding().expected_output().contract(), &request)
            .await
            .map_err(ExactOutpointZcashFundingPlannerError::Signer)?;
        Self::prepare_plan(agreement, &request, exact)
    }

    fn validate_authority(
        &self,
        agreement: &ZecAgreementV1,
    ) -> Result<(), ExactOutpointZcashFundingPlannerError<R::Error, S::Error>> {
        let funder = agreement.lez_claimant();
        if self.local_participant != funder {
            return Err(ExactOutpointZcashFundingPlannerError::WrongRole);
        }
        if self.signer.participant() != self.local_participant {
            return Err(ExactOutpointZcashFundingPlannerError::SignerRoleMismatch);
        }
        if self.signer.public_key() != *agreement.zcash_key(funder) {
            return Err(ExactOutpointZcashFundingPlannerError::SignerKeyMismatch);
        }
        let expected = agreement.binding().expected_output();
        if self.identity.network() != expected.network() {
            return Err(ExactOutpointZcashFundingPlannerError::ConfiguredNetworkMismatch);
        }
        if self.identity.consensus_branch_id() != expected.consensus_branch_id() {
            return Err(ExactOutpointZcashFundingPlannerError::ConfiguredConsensusBranchMismatch);
        }
        Ok(())
    }

    async fn sample_validated_tip(
        &self,
    ) -> Result<ZebraChainInfo, ExactOutpointZcashFundingPlannerError<R::Error, S::Error>> {
        let info = self
            .rpc
            .chain_info()
            .await
            .map_err(ExactOutpointZcashFundingPlannerError::Rpc)?;
        if info.rpc_chain() != self.identity.rpc_chain() {
            return Err(ExactOutpointZcashFundingPlannerError::RpcChainMismatch);
        }
        if info.consensus_branch_id() != self.identity.consensus_branch_id() {
            return Err(ExactOutpointZcashFundingPlannerError::RpcConsensusBranchMismatch);
        }
        Ok(info)
    }

    async fn validate_genesis(
        &self,
    ) -> Result<(), ExactOutpointZcashFundingPlannerError<R::Error, S::Error>> {
        let genesis = self
            .rpc
            .block_hash(BlockHeight::from_u32(0))
            .await
            .map_err(ExactOutpointZcashFundingPlannerError::Rpc)?;
        if genesis != self.identity.genesis_hash() {
            return Err(ExactOutpointZcashFundingPlannerError::GenesisMismatch);
        }
        Ok(())
    }

    fn prepare_plan(
        agreement: &ZecAgreementV1,
        request: &TransparentFundingRequest,
        exact: Vec<u8>,
    ) -> Result<FirstLockPlanV1, ExactOutpointZcashFundingPlannerError<R::Error, S::Error>> {
        let expected = agreement.binding().expected_output();
        let mut cursor = Cursor::new(exact.as_slice());
        let transaction = Transaction::read(&mut cursor, expected.consensus_branch_id())
            .map_err(ExactOutpointZcashFundingPlannerError::MalformedSignedTransaction)?;
        let exact_length = u64::try_from(exact.len())
            .map_err(|_| ExactOutpointZcashFundingPlannerError::TrailingSignedTransaction)?;
        if cursor.position() != exact_length {
            return Err(ExactOutpointZcashFundingPlannerError::TrailingSignedTransaction);
        }
        if transaction.version() != TxVersion::V5 {
            return Err(ExactOutpointZcashFundingPlannerError::WrongTransactionVersion);
        }
        if transaction.consensus_branch_id() != request.consensus_branch_id()
            || transaction.expiry_height() != request.expiry_height()
            || transaction.lock_time() != 0
            || transaction.sapling_bundle().is_some()
            || transaction.orchard_bundle().is_some()
        {
            return Err(ExactOutpointZcashFundingPlannerError::SignedTransactionPolicyMismatch);
        }
        let bundle = transaction
            .transparent_bundle()
            .ok_or(ExactOutpointZcashFundingPlannerError::SignedTransactionPolicyMismatch)?;
        let output = bundle
            .vout
            .first()
            .ok_or(ExactOutpointZcashFundingPlannerError::SignedTransactionPolicyMismatch)?;
        if output.value() != expected.value()
            || output.script_pubkey().0.0 != expected.contract().p2sh_script_pubkey()
        {
            return Err(ExactOutpointZcashFundingPlannerError::SignedTransactionPolicyMismatch);
        }
        let selection = select_funding_utxos(request)
            .map_err(|_| ExactOutpointZcashFundingPlannerError::SignedTransactionPolicyMismatch)?;
        // Consensus-valid script authorization is deliberately not reimplemented here.
        // Nonempty authorization plus exact policy is checked locally; signature validity
        // remains the trusted signer and Zebra broadcast-validation boundary.
        if bundle.vin.len() != selection.selected().len()
            || bundle
                .vin
                .iter()
                .zip(selection.selected())
                .any(|(input, selected)| {
                    input.prevout() != selected.outpoint()
                        || input.sequence() != u32::MAX
                        || input.script_sig().0.0.is_empty()
                })
        {
            return Err(ExactOutpointZcashFundingPlannerError::SignedTransactionPolicyMismatch);
        }
        let expected_output_count = if selection.change().is_some() { 2 } else { 1 };
        if bundle.vout.len() != expected_output_count {
            return Err(ExactOutpointZcashFundingPlannerError::SignedTransactionPolicyMismatch);
        }
        if let Some(change) = selection.change() {
            let change_script: Script = TransparentAddress::from_pubkey(request.funding_pubkey())
                .script()
                .into();
            if bundle.vout[1].value() != change || bundle.vout[1].script_pubkey() != &change_script
            {
                return Err(ExactOutpointZcashFundingPlannerError::SignedTransactionPolicyMismatch);
            }
        }
        let mut canonical = Vec::new();
        transaction
            .write(&mut canonical)
            .map_err(ExactOutpointZcashFundingPlannerError::Serialization)?;
        if canonical != exact {
            return Err(ExactOutpointZcashFundingPlannerError::NonCanonicalSignedTransaction);
        }
        let prepared = PreparedFirstLockSubmissionV1::new(
            FirstLockStepV1::ZcashFund,
            *transaction.txid().as_ref(),
            exact,
        )
        .map_err(ExactOutpointZcashFundingPlannerError::Intent)?;
        FirstLockPlanV1::zcash(prepared).map_err(ExactOutpointZcashFundingPlannerError::Intent)
    }
}

fn same_tip(before: ZebraChainInfo, after: ZebraChainInfo) -> bool {
    before.tip_height() == after.tip_height() && before.tip_hash() == after.tip_hash()
}
