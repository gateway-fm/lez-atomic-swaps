use std::io::Cursor;

use async_trait::async_trait;
use lez_swap_core::Participant;
use lez_zec_swap_sdk::{
    CanonicalZcashOutputObservation, FirstLockConfirmedEvidenceV1, FirstLockObservation,
    FirstLockStepV1, FirstLockTransitionError, ObservationError, PreparedFirstLockSubmissionV1,
    ProfileError, ZcashFirstLockPort, ZcashNodeSnapshot, ZcashStableTip, ZecAgreementV1,
    ZecRefundProfile,
};
use zcash_primitives::transaction::{Transaction, TxVersion};
use zcash_protocol::{TxId, consensus::BlockHeight};

use crate::rpc::{ZebraChainIdentity, ZebraChainInfo, ZebraRpc, ZebraTransactionState};

/// Typed Zebra first-lock adapter over a bounded RPC implementation.
#[derive(Clone, Debug)]
pub struct ZebraRpcSwapPort<R> {
    rpc: R,
    identity: ZebraChainIdentity,
    local_participant: Participant,
}

impl<R> ZebraRpcSwapPort<R> {
    /// Binds one RPC implementation to an immutable chain identity and local role.
    #[must_use]
    pub const fn new(rpc: R, identity: ZebraChainIdentity, local_participant: Participant) -> Self {
        Self {
            rpc,
            identity,
            local_participant,
        }
    }

    /// Configured immutable chain identity.
    #[must_use]
    pub const fn identity(&self) -> ZebraChainIdentity {
        self.identity
    }

    /// Typed transport used by this adapter.
    #[must_use]
    pub const fn rpc(&self) -> &R {
        &self.rpc
    }

    /// Role fixed to this adapter instance by the local actor.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.local_participant
    }
}

/// A rejected prepared transaction, node identity, snapshot, or submission result.
#[derive(Debug, thiserror::Error)]
pub enum ZebraFirstLockError<E: std::error::Error + Send + Sync + 'static> {
    /// This adapter accepts only the Zcash funding step.
    #[error("Zebra first-lock adapter received wrong step {0:?}")]
    WrongStep(FirstLockStepV1),
    /// The role fixed to this adapter is not the agreement's signed Zcash funder.
    #[error("Zebra funding adapter requires role {expected:?}; configured role is {actual:?}")]
    WrongRole {
        /// Role that the accepted agreement assigns to Zcash funding.
        expected: Participant,
        /// Role fixed to this adapter instance.
        actual: Participant,
    },
    /// The configured network differs from the accepted agreement.
    #[error("configured Zebra network differs from the accepted agreement")]
    ConfiguredNetworkMismatch,
    /// The configured branch differs from the accepted agreement.
    #[error("configured Zebra branch differs from the accepted agreement")]
    ConfiguredConsensusBranchMismatch,
    /// The accepted profile rejected its configured network/branch.
    #[error("configured Zebra identity violates the accepted profile: {0}")]
    Profile(#[source] ProfileError),
    /// Canonical V5 transaction decoding failed.
    #[error("prepared Zcash first-lock transaction is malformed: {0}")]
    MalformedSubmission(#[source] std::io::Error),
    /// Bytes remained after decoding exactly one transaction.
    #[error("prepared Zcash first-lock transaction contains trailing bytes")]
    TrailingSubmissionBytes,
    /// A transaction version other than V5 was supplied.
    #[error("prepared Zcash first-lock transaction is not V5")]
    WrongTransactionVersion,
    /// Canonical V5 identity differs from the durable expected identity.
    #[error("prepared Zcash first-lock transaction ID differs from durable identity")]
    ExpectedTransactionIdMismatch,
    /// The expected BIP-199 output is absent at vout zero.
    #[error("prepared Zcash first-lock transaction has no output at index zero")]
    MissingExpectedOutput,
    /// Vout zero has the wrong agreement-bound amount.
    #[error("prepared Zcash first-lock output value differs from the agreement")]
    OutputValueMismatch,
    /// Vout zero has the wrong agreement-bound P2SH script.
    #[error("prepared Zcash first-lock output script differs from the agreement")]
    OutputScriptMismatch,
    /// A typed Zebra RPC operation failed.
    #[error("typed Zebra RPC operation failed: {0}")]
    Rpc(#[source] E),
    /// The RPC chain spelling differs from the configured identity.
    #[error("Zebra RPC chain differs from configured identity")]
    RpcChainMismatch,
    /// The RPC tip branch differs from the configured identity.
    #[error("Zebra RPC consensus branch differs from configured identity")]
    RpcConsensusBranchMismatch,
    /// Height zero differs from the configured immutable genesis hash.
    #[error("Zebra genesis block differs from configured identity")]
    GenesisMismatch,
    /// Raw bytes returned by Zebra differ from the exact durable submission.
    #[error("Zebra transaction bytes differ from the exact durable submission")]
    ObservedRawTransactionMismatch,
    /// The SDK canonical output validator rejected the RPC snapshot.
    #[error("Zebra canonical output validation failed: {0}")]
    Observation(#[source] ObservationError),
    /// The SDK rejected the primitive envelope derived from the canonical snapshot.
    #[error("Zebra canonical first-lock evidence was invalid: {0}")]
    Evidence(#[source] FirstLockTransitionError),
    /// Zebra returned a different ID after accepting exact bytes.
    #[error("Zebra returned a transaction ID different from the submitted V5 identity")]
    SubmittedTransactionIdMismatch,
    /// A confirmed transport state was missing an internally required field.
    #[error("typed Zebra transaction state was internally incomplete")]
    IncompleteRpcSnapshot,
    /// The best-chain tip moved while submission identity was bracketed.
    #[error("Zebra best-chain tip changed during first-lock submission")]
    UnstableTipDuringSubmission,
}

struct ValidatedSubmission {
    transaction_id: TxId,
}

#[async_trait]
impl<R> ZcashFirstLockPort for ZebraRpcSwapPort<R>
where
    R: ZebraRpc,
{
    type Error = ZebraFirstLockError<R::Error>;

    async fn observe_first_lock(
        &self,
        agreement: &ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<FirstLockObservation, Self::Error> {
        let validated = self.validate_submission(agreement, submission)?;
        let before = self
            .rpc
            .chain_info()
            .await
            .map_err(ZebraFirstLockError::Rpc)?;
        self.validate_rpc_identity(before)?;
        self.validate_genesis().await?;

        let raw = self
            .rpc
            .raw_transaction(validated.transaction_id)
            .await
            .map_err(ZebraFirstLockError::Rpc)?;
        let state = match raw.as_ref() {
            Some(_) => self
                .rpc
                .transaction_state(validated.transaction_id)
                .await
                .map_err(ZebraFirstLockError::Rpc)?,
            None => None,
        };
        let canonical_block_hash = match &state {
            Some(ZebraTransactionState::Confirmed { block_height, .. }) => Some(
                self.rpc
                    .block_hash(*block_height)
                    .await
                    .map_err(ZebraFirstLockError::Rpc)?,
            ),
            Some(ZebraTransactionState::Mempool { .. }) | None => None,
        };
        let after = self
            .rpc
            .chain_info()
            .await
            .map_err(ZebraFirstLockError::Rpc)?;
        self.validate_rpc_identity(after)?;
        if before.tip_hash() != after.tip_hash() || before.tip_height() != after.tip_height() {
            return Ok(FirstLockObservation::Unstable);
        }

        let Some(raw) = raw else {
            return Ok(FirstLockObservation::Absent);
        };
        let Some(state) = state else {
            return Ok(FirstLockObservation::Unstable);
        };
        match state {
            ZebraTransactionState::Mempool { raw_transaction } => {
                require_exact_raw::<R::Error>(&raw_transaction, submission.exact_submission())?;
                require_exact_raw::<R::Error>(&raw_transaction, &raw)?;
                Ok(FirstLockObservation::Unstable)
            }
            ZebraTransactionState::Confirmed {
                raw_transaction,
                block_hash,
                block_height,
                confirmations,
                in_active_chain,
            } => {
                require_exact_raw::<R::Error>(&raw_transaction, submission.exact_submission())?;
                require_exact_raw::<R::Error>(&raw_transaction, &raw)?;
                let snapshot = ZcashNodeSnapshot::new(
                    self.identity.network(),
                    self.identity.consensus_branch_id(),
                    in_active_chain,
                    block_hash,
                    canonical_block_hash.ok_or(ZebraFirstLockError::IncompleteRpcSnapshot)?,
                    block_height,
                    ZcashStableTip::new(
                        before.tip_hash(),
                        before.tip_height(),
                        after.tip_hash(),
                        after.tip_height(),
                    ),
                    validated.transaction_id,
                    raw_transaction,
                    0,
                    confirmations,
                );
                let canonical = CanonicalZcashOutputObservation::validate(
                    agreement.binding().expected_output(),
                    &snapshot,
                )
                .map_err(ZebraFirstLockError::Observation)?;
                let required = ZecRefundProfile::for_id(agreement.binding().profile_id())
                    .zcash_confirmations();
                if canonical.confirmations().get() < required {
                    Ok(FirstLockObservation::Unstable)
                } else {
                    let evidence = FirstLockConfirmedEvidenceV1::new(
                        submission.step(),
                        *submission.expected_submission_id(),
                        canonical.transaction_id().to_string(),
                        canonical.confirmations().get(),
                    )
                    .map_err(ZebraFirstLockError::Evidence)?;
                    Ok(FirstLockObservation::Confirmed(evidence))
                }
            }
        }
    }

    async fn submit_first_lock(
        &self,
        agreement: &ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<(), Self::Error> {
        let validated = self.validate_submission(agreement, submission)?;
        let before = self
            .rpc
            .chain_info()
            .await
            .map_err(ZebraFirstLockError::Rpc)?;
        self.validate_rpc_identity(before)?;
        self.validate_genesis().await?;
        let submitted = self
            .rpc
            .send_raw_transaction(submission.exact_submission())
            .await
            .map_err(ZebraFirstLockError::Rpc)?;
        let after = self
            .rpc
            .chain_info()
            .await
            .map_err(ZebraFirstLockError::Rpc)?;
        self.validate_rpc_identity(after)?;
        self.validate_genesis().await?;
        if before.tip_hash() != after.tip_hash() || before.tip_height() != after.tip_height() {
            return Err(ZebraFirstLockError::UnstableTipDuringSubmission);
        }
        if submitted != validated.transaction_id {
            return Err(ZebraFirstLockError::SubmittedTransactionIdMismatch);
        }
        Ok(())
    }
}

impl<R> ZebraRpcSwapPort<R>
where
    R: ZebraRpc,
{
    fn validate_submission(
        &self,
        agreement: &ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<ValidatedSubmission, ZebraFirstLockError<R::Error>> {
        if submission.step() != FirstLockStepV1::ZcashFund {
            return Err(ZebraFirstLockError::WrongStep(submission.step()));
        }
        let expected_funder = agreement.lez_claimant();
        if self.local_participant != expected_funder {
            return Err(ZebraFirstLockError::WrongRole {
                expected: expected_funder,
                actual: self.local_participant,
            });
        }
        let expected = agreement.binding().expected_output();
        if self.identity.network() != expected.network() {
            return Err(ZebraFirstLockError::ConfiguredNetworkMismatch);
        }
        if self.identity.consensus_branch_id() != expected.consensus_branch_id() {
            return Err(ZebraFirstLockError::ConfiguredConsensusBranchMismatch);
        }
        ZecRefundProfile::for_id(agreement.binding().profile_id())
            .validate_consensus(self.identity.network(), self.identity.consensus_branch_id())
            .map_err(ZebraFirstLockError::Profile)?;

        let mut cursor = Cursor::new(submission.exact_submission());
        let transaction = Transaction::read(&mut cursor, expected.consensus_branch_id())
            .map_err(ZebraFirstLockError::MalformedSubmission)?;
        let exact_length = u64::try_from(submission.exact_submission().len())
            .map_err(|_| ZebraFirstLockError::TrailingSubmissionBytes)?;
        if cursor.position() != exact_length {
            return Err(ZebraFirstLockError::TrailingSubmissionBytes);
        }
        if transaction.version() != TxVersion::V5 {
            return Err(ZebraFirstLockError::WrongTransactionVersion);
        }
        let transaction_id = transaction.txid();
        if transaction_id.as_ref() != submission.expected_submission_id() {
            return Err(ZebraFirstLockError::ExpectedTransactionIdMismatch);
        }
        let output = transaction
            .transparent_bundle()
            .and_then(|bundle| bundle.vout.first())
            .ok_or(ZebraFirstLockError::MissingExpectedOutput)?;
        if output.value() != expected.value() {
            return Err(ZebraFirstLockError::OutputValueMismatch);
        }
        if output.script_pubkey().0.0 != expected.contract().p2sh_script_pubkey() {
            return Err(ZebraFirstLockError::OutputScriptMismatch);
        }
        Ok(ValidatedSubmission { transaction_id })
    }

    fn validate_rpc_identity(
        &self,
        info: ZebraChainInfo,
    ) -> Result<(), ZebraFirstLockError<R::Error>> {
        if info.rpc_chain() != self.identity.rpc_chain() {
            return Err(ZebraFirstLockError::RpcChainMismatch);
        }
        if info.consensus_branch_id() != self.identity.consensus_branch_id() {
            return Err(ZebraFirstLockError::RpcConsensusBranchMismatch);
        }
        Ok(())
    }

    async fn validate_genesis(&self) -> Result<(), ZebraFirstLockError<R::Error>> {
        let genesis = self
            .rpc
            .block_hash(BlockHeight::from_u32(0))
            .await
            .map_err(ZebraFirstLockError::Rpc)?;
        if genesis != self.identity.genesis_hash() {
            return Err(ZebraFirstLockError::GenesisMismatch);
        }
        Ok(())
    }
}

fn require_exact_raw<E: std::error::Error + Send + Sync + 'static>(
    actual: &[u8],
    expected: &[u8],
) -> Result<(), ZebraFirstLockError<E>> {
    if actual == expected {
        Ok(())
    } else {
        Err(ZebraFirstLockError::ObservedRawTransactionMismatch)
    }
}
