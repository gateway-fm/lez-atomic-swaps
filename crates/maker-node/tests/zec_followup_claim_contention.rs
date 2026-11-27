//! Exercises the ZEC reference actor; compiled only with `pair-zec`.
#![cfg(feature = "pair-zec")]

//! Issue #176: the Maker's follow-up LEZ claim (`TakerSellsLez`) keeps being
//! re-planned for as long as other transactions of the Maker's LEZ account
//! take its nonce, and still lands.
//!
//! The real supervisor (`supervise_one_due_maker_actor`) spawns the real
//! `lez-zec-maker-actor`, whose `claim` command talks to a LEZ bridge sidecar
//! this test serves over loopback JSON-RPC. The sidecar keeps the account's
//! nonce: seven competing transactions take it, each costing the claim one
//! re-plan, across seven attempts the Node requeues; the eighth attempt
//! observes the claim included and the swap completes.

#[path = "support/cross_role_binary.rs"]
mod cross_role;
#[allow(dead_code)]
mod support;

use std::{
    fmt, fs,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use jsonrpsee::{
    RpcModule,
    server::{ServerBuilder, ServerHandle},
    types::ErrorObjectOwned,
};
use lez_bridge_protocol::{
    AccountIds, ChainPosition, ChainTip, EscrowMetadataFacts, EscrowState, ExactTransactionBytes,
    Hex32, METHOD_OBSERVE_REVEALING_CLAIM, METHOD_PREPARE_REVEALING_CLAIM,
    METHOD_SUBMIT_TRANSACTION, METHOD_SUPERSEDE_REVEALING_CLAIM, NativeClaimInstructionFacts,
    NativeCustodyFacts, ObserveRevealingClaimRequest, ObserveRevealingClaimResult,
    ObservedTransactionFacts, PrepareRevealingClaimRequest, PrepareRevealingClaimResult,
    PreparedTransaction, RevealingClaimFoundFacts, RevealingClaimObservation,
    RevealingClaimObservationTarget, RevealingPreimage, SubmissionOutcome,
    SubmitTransactionRequest, SubmitTransactionResult, SupersedeRevealingClaimRequest,
    SupersedeRevealingClaimResult, TransactionId,
};
use lez_maker_node::{
    MakerActorSupervisorConfig, MakerActorSupervisorResolution, MakerRpc, prepare_maker_actor,
    rpc_module, supervise_one_due_maker_actor,
};
use lez_swap_core::{
    Chain, ChainPosition as SchedulePosition, ConfirmationPolicy, Pair, Participant, Phase,
    RecoverySchedule, SwapCoordinator, SwapDirection, SwapId, TimelockSafety, UnixSeconds,
};
use lez_swap_store::{
    ActorHeldLock, MakerActorKindV1, MakerActorLeaseOwner, MakerActorManifestV1,
    MakerActorProgressObservationV1, MakerActorScheduleState, SqliteSwapStore,
    SqliteZecRecoveryStore, validate_maker_actor_program,
};
use lez_zec_swap_sdk::{
    AcceptedZecAgreementV1, CanonicalLezEscrowObservationV1, CanonicalZcashOutputObservation,
    ClaimDriveOutcome, ClaimPreimage, ClaimRecoveryStore as _, ClaimStepV1, FirstLockDriveOutcome,
    FirstLockObservation, FirstLockPlanV1, FirstLockStepV1, FirstLockSubmitOutcome,
    FollowupClaimObservationV1, LezAssetV1, LezClaimPort, LezCustodySnapshotV1,
    LezEscrowMetadataSnapshotV1, LezEscrowStatusV1, LezFirstLockPort, LezFundInstructionV1,
    LezFundTransactionSnapshotV1, LezInclusionStatusV1, LezNodeSnapshotV1, LezStableTipV1,
    LezTakerFirstLockObservationPort, LezUnixMilliseconds, MakerLockDriveOutcome,
    PreparedClaimSubmissionV1, PreparedFirstLockSubmissionV1, PreparedRefundSubmissionV1,
    ProtectedClaimKey, REVEALING_CLAIM_REPLAN_ATTENTION, RefundEligibilityObservationV1,
    RefundObservationV1, RefundSubmitOutcomeV1, RevealingClaimEvidenceV1,
    RevealingClaimObservationV1, RevealingClaimSubmitOutcomeV1, TakerFirstLockObservationV1,
    ZcashClaimContextV1, ZcashClaimPort, ZcashFirstLockPort, ZcashFundingContextV1,
    ZcashFundingObservationV1, ZcashRefundPort, ZcashTakerFirstLockObservationPort, ZecAgreementV1,
    ZecPairSdk, ZecProfileId,
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tempfile::tempdir;
use zec_reference_actor::{
    ActorConfig, revealing_claim_in_flight_marker, revealing_claim_in_flight_refund_at_ms,
    revealing_claim_in_flight_submitted_at_ms,
};

use support::{CLAIM_PREIMAGE, actor_deployment, actor_deployment_with_bridge};

const SWAP_ID: &str = "m5-followup-claim-contention";

/// A second swap of the same Maker whose next step is a LEZ lock (the escrow
/// of a swap the Taker sells Zcash in), signed by the account that signs the
/// claim. It sorts after [`SWAP_ID`]: due at the same time, the claim runs
/// first.
const LOCK_SWAP_ID: &str = "m5-followup-lock-behind-claim";

/// The agreement's profile: a local one the Taker reveals first in, whose LEZ
/// refund time is twelve minutes after the agreement. The claim's eight
/// attempts spawn sixteen real actor processes; under the two minutes of the
/// other fixtures' profile a loaded machine came within a minute of the
/// refund time, past which the claim is no longer driven.
const PROFILE: ZecProfileId = ZecProfileId::DeterministicLocalDevnetV7;

/// The Maker account's nonce when its claim is first planned.
const FIRST_NONCE: u128 = 7;

/// The fixture config's claim recovery key (`support::actor_deployment`).
const CLAIM_KEY_ID: &str = "m5-integration-authority-claim-v1";
const CLAIM_KEY_MATERIAL: [u8; 32] = [0x7a; 32];

/// The Taker's LEZ funding the Maker observed (transaction, block, tip).
const TAKER_LEZ_FUNDING_ID: [u8; 32] = [0x31; 32];
const TAKER_LEZ_FUNDING_HEIGHT: u64 = 100;
const TAKER_LEZ_FUNDING_TIP: u64 = 102;

/// The Maker's Zcash funding and the Taker's Zcash claim of it, as the Maker's
/// store records them (canonical display-order transaction ids).
const MAKER_ZCASH_FUNDING_ID: [u8; 32] = [0x81; 32];
const MAKER_ZCASH_FUNDING_TXID: &str =
    "abababababababababababababababababababababababababababababababab";
const TAKER_ZCASH_CLAIM_ID: [u8; 32] = [0xc2; 32];
const TAKER_ZCASH_CLAIM_TXID: &str =
    "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

/// Where the fake LEZ chain includes the Maker's claim, and its tip then.
const CLAIM_HEIGHT: u64 = 130;
const CLAIM_TIP_HEIGHT: u64 = 160;

/// The supervisor's requeue delay, and the logical seconds between attempts.
const REQUEUE_DELAY_SECONDS: u64 = 5;

/// How long before the swap's LEZ refund time an attempt may still start: an
/// attempt started this close to it is the machine's slowness, not the claim
/// (past the refund time the actor no longer drives the claim).
const REFUND_TIME_GUARD_MS: u64 = 15_000;

/// How long a pass waits for its actor's lock to be released (see
/// `maker_actor_supervisor.rs`).
const PASS_LOCK_RELEASE: Duration = Duration::from_secs(30);

/// A transaction of the Maker's LEZ account other than this swap's claim,
/// placed against the claim's submissions (counted from one, over the run).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Competitor {
    /// Included just before the claim's submission of that ordinal arrives:
    /// the submission finds its nonce taken.
    BeforeSubmission(usize),
    /// Included just after that submission was accepted, at the same nonce:
    /// the accepted claim is dropped from the mempool and never included.
    AfterAcceptedSubmission(usize),
}

/// One thing the fake sidecar did, in the order it happened.
#[derive(Clone, Debug, Eq, PartialEq)]
enum SidecarEvent {
    Prepare {
        nonce: u128,
    },
    Observe {
        nonce: u128,
        found: bool,
    },
    Competitor {
        nonce: u128,
    },
    Submit {
        planned: u128,
        live: u128,
        accepted: bool,
    },
    Supersede {
        index: u32,
        superseded: u128,
        replanned: u128,
    },
    /// A request the scenario does not expect.
    Refused(String),
}

impl fmt::Display for SidecarEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Prepare { nonce } => write!(formatter, "prepare@{nonce}"),
            Self::Observe { nonce, found: true } => write!(formatter, "observe@{nonce}=found"),
            Self::Observe {
                nonce,
                found: false,
            } => write!(formatter, "observe@{nonce}=absent"),
            Self::Competitor { nonce } => write!(formatter, "competitor@{nonce}"),
            Self::Submit {
                planned,
                accepted: true,
                ..
            } => write!(formatter, "submit@{planned}=accepted"),
            Self::Submit {
                planned,
                live,
                accepted: false,
            } => write!(formatter, "submit@{planned}=superseded(live {live})"),
            Self::Supersede {
                index,
                superseded,
                replanned,
            } => write!(formatter, "supersede#{index}:{superseded}->{replanned}"),
            Self::Refused(reason) => write!(formatter, "refused({reason})"),
        }
    }
}

/// One plan of the Maker's claim: its exact transaction and signer nonce.
#[derive(Clone, Debug)]
struct PlannedClaim {
    transaction: PreparedTransaction,
    nonce: u128,
}

impl PlannedClaim {
    fn at(nonce: u128) -> Self {
        let bytes = format!("fake-lez-followup-claim/nonce={nonce}").into_bytes();
        let id: [u8; 32] = Sha256::digest(&bytes).into();
        Self {
            transaction: PreparedTransaction::new(
                TransactionId::from_bytes(id),
                ExactTransactionBytes::new(bytes).unwrap(),
            ),
            nonce,
        }
    }
}

/// The fake LEZ chain as the sidecar sees it: the Maker account's nonce, the
/// claim's plans, and the escrow the claim spends.
#[derive(Debug)]
struct Ledger {
    live_nonce: u128,
    competitors: Vec<Competitor>,
    /// Every plan of the claim, oldest first; the last one is open.
    claims: Vec<PlannedClaim>,
    replans: u32,
    submissions: usize,
    included: Option<TransactionId>,
    preimage: Option<[u8; 32]>,
    metadata_account: Option<Hex32>,
    custody_account: Option<Hex32>,
    events: Vec<SidecarEvent>,
}

impl Ledger {
    fn refuse(&mut self, reason: &str) -> ErrorObjectOwned {
        self.events.push(SidecarEvent::Refused(reason.to_owned()));
        ErrorObjectOwned::owned(-32_000, reason.to_owned(), None::<()>)
    }

    fn planned(&self, id: TransactionId) -> Option<PlannedClaim> {
        self.claims
            .iter()
            .find(|claim| claim.transaction.transaction_id == id)
            .cloned()
    }

    fn compete(&mut self) {
        self.events.push(SidecarEvent::Competitor {
            nonce: self.live_nonce,
        });
        self.live_nonce += 1;
    }

    fn prepare(
        &mut self,
        request: PrepareRevealingClaimRequest,
    ) -> Result<Value, ErrorObjectOwned> {
        if !self.claims.is_empty() {
            return Err(self.refuse("the claim is already planned"));
        }
        self.preimage = Some(*request.preimage().expose_secret());
        let claim = PlannedClaim::at(self.live_nonce);
        self.events
            .push(SidecarEvent::Prepare { nonce: claim.nonce });
        self.claims.push(claim.clone());
        Ok(json(&PrepareRevealingClaimResult::new(
            request.context,
            claim.transaction,
        )))
    }

    fn observe(
        &mut self,
        request: ObserveRevealingClaimRequest,
    ) -> Result<Value, ErrorObjectOwned> {
        let RevealingClaimObservationTarget::Exact {
            claim_transaction_id,
        } = request.target
        else {
            return Err(self.refuse("only the Maker's own claim is observed"));
        };
        let Some(claim) = self.planned(claim_transaction_id) else {
            return Err(self.refuse("observation of a claim that was never planned"));
        };
        let found = self.included == Some(claim_transaction_id);
        self.events.push(SidecarEvent::Observe {
            nonce: claim.nonce,
            found,
        });
        let tip = ChainTip::new(Hex32::from_bytes([0x90; 32]), CLAIM_TIP_HEIGHT);
        let observation = if found {
            let (Some(metadata), Some(custody), Some(preimage)) =
                (self.metadata_account, self.custody_account, self.preimage)
            else {
                return Err(self.refuse("the escrow of the claim is unknown"));
            };
            let terms = &request.terms;
            let claimant = terms.claimant_account_id();
            let program = request.runtime.escrow_program_id;
            RevealingClaimObservation::found(RevealingClaimFoundFacts::new(
                ObservedTransactionFacts::new(
                    claim.transaction.transaction_id,
                    claim.transaction.exact_bytes,
                    ChainPosition::new(Hex32::from_bytes([0x82; 32]), CLAIM_HEIGHT, 0),
                    AccountIds::new(vec![claimant]).unwrap(),
                    true,
                ),
                NativeClaimInstructionFacts::new(
                    program,
                    AccountIds::new(vec![metadata, custody, claimant]).unwrap(),
                    terms.swap_id(),
                    RevealingPreimage::new(preimage),
                ),
                EscrowMetadataFacts::from_lee_v0_2_native_terms(
                    metadata,
                    program,
                    custody,
                    terms,
                    EscrowState::Claimed,
                ),
                NativeCustodyFacts::new(custody, terms.authenticated_transfer_program_id(), 0),
            ))
        } else {
            RevealingClaimObservation::Absent
        };
        Ok(json(&ObserveRevealingClaimResult::new(
            request.context,
            tip,
            observation,
            tip,
        )))
    }

    fn submit(&mut self, request: SubmitTransactionRequest) -> Result<Value, ErrorObjectOwned> {
        let id = request.transaction.transaction_id;
        let Some(claim) = self
            .planned(id)
            .filter(|claim| claim.transaction == request.transaction)
        else {
            return Err(self.refuse("submission of bytes that were never planned"));
        };
        self.submissions += 1;
        let ordinal = self.submissions;
        if self
            .competitors
            .contains(&Competitor::BeforeSubmission(ordinal))
        {
            self.compete();
        }
        let outcome = if claim.nonce < self.live_nonce {
            self.events.push(SidecarEvent::Submit {
                planned: claim.nonce,
                live: self.live_nonce,
                accepted: false,
            });
            SubmissionOutcome::Superseded {
                planned_nonce: claim.nonce,
                live_nonce: self.live_nonce,
            }
        } else {
            self.events.push(SidecarEvent::Submit {
                planned: claim.nonce,
                live: self.live_nonce,
                accepted: true,
            });
            if self
                .competitors
                .contains(&Competitor::AfterAcceptedSubmission(ordinal))
            {
                self.compete();
            } else {
                // The next block includes it: the account's nonce moves on.
                self.included = Some(id);
                self.live_nonce += 1;
            }
            SubmissionOutcome::Accepted
        };
        Ok(json(&SubmitTransactionResult::new(
            request.context,
            id,
            outcome,
        )))
    }

    fn supersede(
        &mut self,
        request: SupersedeRevealingClaimRequest,
    ) -> Result<Value, ErrorObjectOwned> {
        let superseded_id = request.superseded_claim_transaction_id;
        let Some(open) = self
            .claims
            .last()
            .filter(|open| open.transaction.transaction_id == superseded_id)
            .cloned()
        else {
            return Err(self.refuse("re-plan of a claim that is not the open one"));
        };
        if open.nonce >= self.live_nonce || self.included == Some(superseded_id) {
            return Err(self.refuse("re-plan of a claim whose nonce was not taken"));
        }
        let replanned = PlannedClaim::at(self.live_nonce);
        self.replans += 1;
        self.events.push(SidecarEvent::Supersede {
            index: self.replans,
            superseded: open.nonce,
            replanned: replanned.nonce,
        });
        self.claims.push(replanned.clone());
        Ok(json(&SupersedeRevealingClaimResult::new(
            request.context,
            superseded_id,
            open.nonce,
            replanned.nonce,
            self.replans,
            replanned.transaction,
        )))
    }
}

fn json<T: serde::Serialize>(result: &T) -> Value {
    serde_json::to_value(result).unwrap()
}

/// The LEZ bridge sidecar of the Maker, served to the real actor over the
/// real transport: the four requests a follow-up claim makes.
struct FakeLezSidecar {
    endpoint: String,
    ledger: Arc<Mutex<Ledger>>,
    _server: ServerHandle,
}

impl FakeLezSidecar {
    async fn start(competitors: &[Competitor]) -> Self {
        let ledger = Arc::new(Mutex::new(Ledger {
            live_nonce: FIRST_NONCE,
            competitors: competitors.to_vec(),
            claims: Vec::new(),
            replans: 0,
            submissions: 0,
            included: None,
            preimage: None,
            metadata_account: None,
            custody_account: None,
            events: Vec::new(),
        }));
        let mut module = RpcModule::new(Arc::clone(&ledger));
        module
            .register_method(METHOD_PREPARE_REVEALING_CLAIM, |params, ledger, _| {
                ledger.lock().unwrap().prepare(params.one()?)
            })
            .unwrap();
        module
            .register_method(METHOD_OBSERVE_REVEALING_CLAIM, |params, ledger, _| {
                ledger.lock().unwrap().observe(params.one()?)
            })
            .unwrap();
        module
            .register_method(METHOD_SUBMIT_TRANSACTION, |params, ledger, _| {
                ledger.lock().unwrap().submit(params.one()?)
            })
            .unwrap();
        module
            .register_method(METHOD_SUPERSEDE_REVEALING_CLAIM, |params, ledger, _| {
                ledger.lock().unwrap().supersede(params.one()?)
            })
            .unwrap();
        let server = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", server.local_addr().unwrap());
        Self {
            endpoint,
            ledger,
            _server: server.start(module),
        }
    }

    /// The escrow the agreement's claim spends, as the chain holds it.
    fn hold_escrow_of(&self, agreement: &ZecAgreementV1) {
        let mut ledger = self.ledger.lock().unwrap();
        ledger.metadata_account =
            Some(Hex32::from_bytes(*agreement.lez_terms().metadata_account()));
        ledger.custody_account = Some(Hex32::from_bytes(*agreement.lez_terms().custody_account()));
    }

    fn events(&self) -> Vec<SidecarEvent> {
        self.ledger.lock().unwrap().events.clone()
    }
}

/// A chain port the fixture does not script.
#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("the Maker fixture does not script {0}")]
struct Unscripted(&'static str);

/// The chains as the Maker saw them before its follow-up claim: the Taker's
/// canonical LEZ funding, the Maker's own Zcash funding once sent, and the
/// Taker's Zcash claim revealing the preimage.
#[derive(Clone, Debug, Default)]
struct SettledChains {
    maker_zcash_funding_sent: Arc<AtomicBool>,
}

#[async_trait]
impl LezTakerFirstLockObservationPort for SettledChains {
    type Error = Unscripted;

    async fn observe_taker_first_lock(
        &self,
        agreement: &ZecAgreementV1,
        _previous: Option<&CanonicalLezEscrowObservationV1>,
    ) -> Result<TakerFirstLockObservationV1, Self::Error> {
        Ok(TakerFirstLockObservationV1::CanonicalLez(Box::new(
            canonical_lez_taker_lock(agreement),
        )))
    }
}

#[async_trait]
impl ZcashTakerFirstLockObservationPort for SettledChains {
    type Error = Unscripted;

    async fn observe_taker_first_lock(
        &self,
        _agreement: &ZecAgreementV1,
        _previous: Option<&CanonicalZcashOutputObservation>,
    ) -> Result<TakerFirstLockObservationV1, Self::Error> {
        Err(Unscripted("a Zcash first lock of the Taker"))
    }
}

#[async_trait]
impl LezFirstLockPort for SettledChains {
    type Error = Unscripted;

    async fn observe_first_lock(
        &self,
        _agreement: &ZecAgreementV1,
        _submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<FirstLockObservation, Self::Error> {
        Err(Unscripted("a LEZ lock of the Maker"))
    }

    async fn submit_first_lock(
        &self,
        _agreement: &ZecAgreementV1,
        _submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<FirstLockSubmitOutcome, Self::Error> {
        Err(Unscripted("a LEZ lock of the Maker"))
    }
}

#[async_trait]
impl ZcashFirstLockPort for SettledChains {
    type Error = Unscripted;

    async fn observe_first_lock(
        &self,
        _agreement: &ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<FirstLockObservation, Self::Error> {
        if !self.maker_zcash_funding_sent.load(Ordering::SeqCst) {
            return Ok(FirstLockObservation::Absent);
        }
        lez_zec_swap_sdk::FirstLockConfirmedEvidenceV1::from_observation(
            submission.step(),
            *submission.expected_submission_id(),
            MAKER_ZCASH_FUNDING_TXID,
            3,
        )
        .map(FirstLockObservation::Confirmed)
        .map_err(|_| Unscripted("the Maker's Zcash funding evidence"))
    }

    async fn submit_first_lock(
        &self,
        _agreement: &ZecAgreementV1,
        _submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<(), Self::Error> {
        self.maker_zcash_funding_sent.store(true, Ordering::SeqCst);
        Ok(())
    }
}

#[async_trait]
impl LezClaimPort for SettledChains {
    type Error = Unscripted;

    async fn prepare_revealing_claim(
        &self,
        _agreement: &ZecAgreementV1,
        _preimage: &ClaimPreimage,
    ) -> Result<PreparedClaimSubmissionV1, Self::Error> {
        Err(Unscripted("a LEZ claim: the real actor makes it"))
    }

    async fn observe_prepared_revealing_claim(
        &self,
        _agreement: &ZecAgreementV1,
        _prepared: &PreparedClaimSubmissionV1,
    ) -> Result<RevealingClaimObservationV1, Self::Error> {
        Err(Unscripted("a LEZ claim: the real actor makes it"))
    }

    async fn observe_counterparty_revealing_claim(
        &self,
        _agreement: &ZecAgreementV1,
    ) -> Result<RevealingClaimObservationV1, Self::Error> {
        Err(Unscripted("a LEZ claim: the real actor makes it"))
    }

    async fn submit_revealing_claim(
        &self,
        _agreement: &ZecAgreementV1,
        _prepared: &PreparedClaimSubmissionV1,
    ) -> Result<RevealingClaimSubmitOutcomeV1, Self::Error> {
        Err(Unscripted("a LEZ claim: the real actor makes it"))
    }
}

#[async_trait]
impl ZcashClaimPort for SettledChains {
    type Error = Unscripted;

    async fn observe_funding_before_reveal(
        &self,
        _agreement: &ZecAgreementV1,
        _context: &ZcashFundingContextV1,
    ) -> Result<ZcashFundingObservationV1, Self::Error> {
        Err(Unscripted("a Zcash claim of the Maker"))
    }

    async fn prepare_followup_claim(
        &self,
        _agreement: &ZecAgreementV1,
        _context: &ZcashClaimContextV1,
        _preimage: &ClaimPreimage,
    ) -> Result<PreparedClaimSubmissionV1, Self::Error> {
        Err(Unscripted("a Zcash claim of the Maker"))
    }

    async fn observe_prepared_followup_claim(
        &self,
        _agreement: &ZecAgreementV1,
        _context: &ZcashClaimContextV1,
        _prepared: &PreparedClaimSubmissionV1,
    ) -> Result<FollowupClaimObservationV1, Self::Error> {
        Err(Unscripted("a Zcash claim of the Maker"))
    }

    async fn observe_counterparty_followup_claim(
        &self,
        _agreement: &ZecAgreementV1,
        _context: &ZcashClaimContextV1,
    ) -> Result<FollowupClaimObservationV1, Self::Error> {
        Err(Unscripted("a Zcash claim of the Maker"))
    }

    async fn submit_followup_claim(
        &self,
        _agreement: &ZecAgreementV1,
        _context: &ZcashClaimContextV1,
        _prepared: &PreparedClaimSubmissionV1,
    ) -> Result<(), Self::Error> {
        Err(Unscripted("a Zcash claim of the Maker"))
    }

    async fn observe_counterparty_revealing_claim(
        &self,
        agreement: &ZecAgreementV1,
        _context: &ZcashClaimContextV1,
    ) -> Result<RevealingClaimObservationV1, Self::Error> {
        RevealingClaimEvidenceV1::from_observed_zcash_claim(
            agreement,
            TAKER_ZCASH_CLAIM_ID,
            TAKER_ZCASH_CLAIM_TXID,
            3,
            ClaimPreimage::new(CLAIM_PREIMAGE),
        )
        .map(RevealingClaimObservationV1::Confirmed)
        .map_err(|_| Unscripted("the Taker's Zcash claim evidence"))
    }
}

#[async_trait]
impl ZcashRefundPort for SettledChains {
    type Error = Unscripted;

    async fn observe_refund_eligibility(
        &self,
        _agreement: &ZecAgreementV1,
        _context: &ZcashFundingContextV1,
    ) -> Result<RefundEligibilityObservationV1, Self::Error> {
        Err(Unscripted("a Zcash refund"))
    }

    async fn prepare_refund(
        &self,
        _agreement: &ZecAgreementV1,
        _context: &ZcashFundingContextV1,
    ) -> Result<PreparedRefundSubmissionV1, Self::Error> {
        Err(Unscripted("a Zcash refund"))
    }

    async fn observe_prepared_refund(
        &self,
        _agreement: &ZecAgreementV1,
        _context: &ZcashFundingContextV1,
        _prepared: &PreparedRefundSubmissionV1,
    ) -> Result<RefundObservationV1, Self::Error> {
        Err(Unscripted("a Zcash refund"))
    }

    async fn observe_counterparty_refund(
        &self,
        _agreement: &ZecAgreementV1,
        _context: &ZcashFundingContextV1,
    ) -> Result<RefundObservationV1, Self::Error> {
        Err(Unscripted("a Zcash refund"))
    }

    async fn submit_refund(
        &self,
        _agreement: &ZecAgreementV1,
        _context: &ZcashFundingContextV1,
        _prepared: &PreparedRefundSubmissionV1,
    ) -> Result<RefundSubmitOutcomeV1, Self::Error> {
        Err(Unscripted("a Zcash refund"))
    }
}

/// The Taker's LEZ funding of the agreement's escrow, canonical and three
/// blocks deep (as `zec_sdk_recovery.rs` in `lez-swap-store` fabricates it).
fn canonical_lez_taker_lock(agreement: &ZecAgreementV1) -> CanonicalLezEscrowObservationV1 {
    let terms = agreement.lez_terms();
    let LezAssetV1::Native {
        authenticated_transfer_program_id,
    } = terms.asset()
    else {
        panic!("the fixture agreement sells native LEZ")
    };
    let depositor = *agreement.lez_account(agreement.lez_depositor());
    let claimant = *agreement.lez_account(agreement.lez_claimant());
    let metadata = LezEscrowMetadataSnapshotV1::new(
        2,
        *agreement.onchain_swap_id(),
        *agreement.agreement_commitment(),
        *agreement.secret_digest(),
        depositor,
        depositor,
        claimant,
        claimant,
        *terms.custody_account(),
        *authenticated_transfer_program_id,
        *authenticated_transfer_program_id,
        [0; 32],
        terms.amount(),
        agreement.lez_refund_at_ms(),
        LezEscrowStatusV1::Funded,
    );
    let block = [0x41; 32];
    let transaction = LezFundTransactionSnapshotV1::new(
        TAKER_LEZ_FUNDING_ID,
        *terms.escrow_program_id(),
        depositor,
        vec![
            *terms.metadata_account(),
            *terms.custody_account(),
            depositor,
        ],
        LezFundInstructionV1::Native {
            swap_id: *agreement.onchain_swap_id(),
        },
        true,
        true,
        TAKER_LEZ_FUNDING_HEIGHT,
        block,
        block,
        LezInclusionStatusV1::Pending,
    );
    let tip = [0x42; 32];
    CanonicalLezEscrowObservationV1::validate(
        agreement,
        &LezNodeSnapshotV1::new(
            terms.chain().environment(),
            *terms.chain().channel_id(),
            *terms.chain().genesis_block_hash(),
            LezStableTipV1::new(tip, TAKER_LEZ_FUNDING_TIP, tip, TAKER_LEZ_FUNDING_TIP),
            transaction,
            *terms.escrow_program_id(),
            *terms.metadata_account(),
            metadata,
            *terms.custody_account(),
            LezCustodySnapshotV1::Native {
                program_owner: *authenticated_transfer_program_id,
                balance: terms.amount(),
            },
        ),
    )
    .expect("agreement-bound canonical LEZ funding")
}

fn claim_key() -> ProtectedClaimKey {
    ProtectedClaimKey::new(CLAIM_KEY_ID, CLAIM_KEY_MATERIAL).unwrap()
}

/// Brings the Maker's own role store (the one its actor config names) to
/// `claim_evidence_available` through the SDK: the agreement activated, the
/// Taker's LEZ funding observed, the Maker's Zcash funding projected, and the
/// Taker's Zcash claim, with its preimage, recorded.
async fn maker_store_at_claim_evidence(role_state_db: &Path, accepted: AcceptedZecAgreementV1) {
    let chains = SettledChains::default();
    let store =
        SqliteZecRecoveryStore::open_claim_capable(role_state_db, Participant::Maker, claim_key())
            .expect("create the Maker's role store");
    let sdk = ZecPairSdk::new(Participant::Maker, (), (), chains.clone(), chains, store);
    let mut maker = sdk.activate(accepted).await.expect("Maker activation");
    maker
        .observe_taker_first_lock()
        .await
        .expect("the Maker observes the Taker's canonical LEZ funding");
    let plan = FirstLockPlanV1::zcash(
        PreparedFirstLockSubmissionV1::new(
            FirstLockStepV1::ZcashFund,
            MAKER_ZCASH_FUNDING_ID,
            vec![0xb1],
        )
        .unwrap(),
    )
    .unwrap();
    let mut funded = None;
    for _ in 0..4 {
        let outcome = maker
            .drive_maker_lock(plan.clone())
            .await
            .expect("drive the Maker's Zcash funding");
        if let MakerLockDriveOutcome::Lock(FirstLockDriveOutcome::ReadyForFundingProjection(
            evidence,
        )) = outcome
        {
            funded = Some(evidence);
            break;
        }
    }
    maker
        .project_maker_lock(funded.expect("the Maker's Zcash funding became ready"))
        .await
        .expect("project the Maker's Zcash funding");
    assert_eq!(maker.status(), Phase::BothLegsLocked);
    assert_eq!(
        maker
            .drive_claim(LezUnixMilliseconds::new(unix_now_ms()))
            .await
            .expect("the Maker reads the Taker's Zcash claim"),
        ClaimDriveOutcome::Projected {
            step: ClaimStepV1::RevealingZcash,
            revision: 3,
        }
    );
    assert_eq!(maker.status(), Phase::ClaimEvidenceAvailable);
}

/// The Node's own coordinator row for the swap (its schedule is not read by
/// the supervisor; as `maker_actor_supervisor.rs` registers it).
fn swap(id: &str) -> SwapCoordinator {
    let direction = SwapDirection::TakerSellsForeign;
    SwapCoordinator::new_with_direction(
        SwapId::new(id).unwrap(),
        Pair::Zcash,
        direction,
        ConfirmationPolicy::new(2).unwrap(),
        RecoverySchedule::new(
            Pair::Zcash,
            direction,
            SchedulePosition::block_height(Chain::Lez, 100),
            SchedulePosition::block_height(Chain::Zcash, 120),
            TimelockSafety::between(Chain::Lez, Chain::Zcash, 1_000, 1_200, 100).unwrap(),
        )
        .unwrap(),
    )
}

/// The real Zcash Maker actor, installed as a Node installs it: a private
/// copy pinned by its SHA-256.
///
/// The copy is stripped of its symbols where `strip` exists. The supervisor
/// reads, hashes, seals and wipes the program before every child, two per
/// attempt: an unstripped development build (175 MB) took some twenty seconds
/// per child on a loaded machine. Stripping changes no code the actor runs.
fn install_actor_program(root: &Path) -> (PathBuf, [u8; 32]) {
    let built = cross_role::workspace_binary("lez-zec-maker-actor");
    let program = root.join("lez-zec-maker-actor");
    fs::copy(built, &program).unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
    // Without `strip` the test runs the unstripped copy.
    let _ = Command::new("strip").arg(&program).status();
    let sha256: [u8; 32] = Sha256::digest(fs::read(&program).unwrap()).into();
    validate_maker_actor_program(&program, sha256)
        .expect("the actor satisfies the production artifact policy");
    (program, sha256)
}

fn unix_now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

/// A Zcash Maker actor script at `taker_lock_confirmed` whose status names the
/// LEZ escrow as the Maker's pending lock and whose drive sends its
/// initialization; each command is appended to `log` (as `maker_lock_program`
/// in `maker_actor_supervisor.rs`).
fn lez_lock_program(log: &Path) -> String {
    format!(
        "#!/bin/sh\n\
         test \"$1\" = \"--config-fd\" || exit 91\n\
         test \"$2\" = \"196\" || exit 92\n\
         printf '%s\\n' \"$3\" >> '{log}'\n\
         case \"$3\" in\n\
           status) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"state\":\"active\",\"phase\":\"taker_lock_confirmed\",\"revision\":1,\"next_action\":\"wait\",\"maker_lock\":\"lez\"}}' ;;\n\
           drive) printf '%s\\n' '{{\"schema_version\":1,\"role\":\"maker\",\"command\":\"drive\",\"outcome\":\"submitted\",\"operation\":\"lez_initialize\",\"phase\":\"taker_lock_confirmed\",\"revision\":1,\"next_action\":\"wait\"}}' ;;\n\
           *) exit 95 ;;\n\
         esac\n",
        log = log.display()
    )
}

/// Registers [`LOCK_SWAP_ID`], due at `now`, and returns its command log.
fn register_lez_lock(store: &mut SqliteSwapStore, root: &Path, now: u64) -> PathBuf {
    let lock_root = root.join("lock-swap");
    fs::create_dir(&lock_root).unwrap();
    fs::set_permissions(&lock_root, fs::Permissions::from_mode(0o700)).unwrap();
    let log = lock_root.join("commands.log");
    let deployment = actor_deployment(&lock_root, LOCK_SWAP_ID);
    let state = ActorConfig::load_private(&deployment.source_config)
        .unwrap()
        .role_state_db()
        .to_path_buf();
    let script = lez_lock_program(&log);
    let program = lock_root.join("lez-lock-actor");
    fs::write(&program, &script).unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
    store.save(&swap(LOCK_SWAP_ID)).unwrap();
    store
        .register_maker_actor(
            &MakerActorManifestV1::new(
                SwapId::new(LOCK_SWAP_ID).unwrap(),
                MakerActorKindV1::Zcash,
                deployment.source_config.clone(),
                Sha256::digest(fs::read(&deployment.source_config).unwrap()).into(),
                program,
                Sha256::digest(script.as_bytes()).into(),
                state,
            )
            .unwrap(),
            now,
        )
        .unwrap();
    let record = store
        .maker_actor_process(&SwapId::new(LOCK_SWAP_ID).unwrap())
        .unwrap()
        .unwrap();
    prepare_maker_actor(&record).expect("exact deployment preflight of the lock's actor");
    log
}

fn drives(log: &Path) -> usize {
    fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|command| *command == "drive")
        .count()
}

/// What one supervisor attempt is expected to do.
struct Attempt {
    /// The requests the sidecar serves and the competing transactions it
    /// includes during the attempt, in order.
    sidecar: &'static [&'static str],
    /// The outcome word of the actor's `claim`.
    outcome: &'static str,
    /// Whether the actor names the nonce contention (more than
    /// [`REVEALING_CLAIM_REPLAN_ATTENTION`] re-plans recorded).
    contention: bool,
    /// Whether the claim's in-flight marker exists after the attempt.
    in_flight: bool,
}

/// The run, attempt by attempt, from nonce [`FIRST_NONCE`] on. An attempt
/// re-plans once: superseded again, it waits for the next (`awaiting_observation`).
const ATTEMPTS: [Attempt; 8] = [
    // Planned at 7; both the plan and its re-plan find their nonce taken.
    Attempt {
        sidecar: &[
            "prepare@7",
            "observe@7=absent",
            "competitor@7",
            "submit@7=superseded(live 8)",
            "supersede#1:7->8",
            "observe@8=absent",
            "competitor@8",
            "submit@8=superseded(live 9)",
        ],
        outcome: "awaiting_observation",
        contention: false,
        in_flight: false,
    },
    // The re-plan is accepted, and a competing transaction is included at its
    // nonce before it: it is dropped from the mempool.
    Attempt {
        sidecar: &[
            "observe@8=absent",
            "submit@8=superseded(live 9)",
            "supersede#2:8->9",
            "observe@9=absent",
            "submit@9=accepted",
            "competitor@9",
        ],
        outcome: "submitted",
        contention: false,
        in_flight: true,
    },
    Attempt {
        sidecar: &[
            "observe@9=absent",
            "submit@9=superseded(live 10)",
            "supersede#3:9->10",
            "observe@10=absent",
            "competitor@10",
            "submit@10=superseded(live 11)",
        ],
        outcome: "awaiting_observation",
        contention: false,
        in_flight: true,
    },
    Attempt {
        sidecar: &[
            "observe@10=absent",
            "submit@10=superseded(live 11)",
            "supersede#4:10->11",
            "observe@11=absent",
            "competitor@11",
            "submit@11=superseded(live 12)",
        ],
        outcome: "awaiting_observation",
        contention: false,
        in_flight: true,
    },
    Attempt {
        sidecar: &[
            "observe@11=absent",
            "submit@11=superseded(live 12)",
            "supersede#5:11->12",
            "observe@12=absent",
            "competitor@12",
            "submit@12=superseded(live 13)",
        ],
        outcome: "awaiting_observation",
        contention: false,
        in_flight: true,
    },
    // The sixth re-plan: the actor names the contention from here on.
    Attempt {
        sidecar: &[
            "observe@12=absent",
            "submit@12=superseded(live 13)",
            "supersede#6:12->13",
            "observe@13=absent",
            "competitor@13",
            "submit@13=superseded(live 14)",
        ],
        outcome: "awaiting_observation",
        contention: true,
        in_flight: true,
    },
    // The seventh re-plan is accepted and nothing competes: it is included.
    Attempt {
        sidecar: &[
            "observe@13=absent",
            "submit@13=superseded(live 14)",
            "supersede#7:13->14",
            "observe@14=absent",
            "submit@14=accepted",
        ],
        outcome: "submitted",
        contention: true,
        in_flight: true,
    },
    Attempt {
        sidecar: &["observe@14=found"],
        outcome: "projected",
        contention: false,
        in_flight: false,
    },
];

/// The competing transactions of the Maker's account that produce [`ATTEMPTS`].
const COMPETITORS: [Competitor; 7] = [
    Competitor::BeforeSubmission(1),
    Competitor::BeforeSubmission(2),
    Competitor::AfterAcceptedSubmission(4),
    Competitor::BeforeSubmission(6),
    Competitor::BeforeSubmission(8),
    Competitor::BeforeSubmission(10),
    Competitor::BeforeSubmission(12),
];

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one claim followed attempt by attempt, from its first plan to the completed swap"
)]
fn a_followup_lez_claim_re_planned_more_than_five_times_across_attempts_still_lands() {
    let root = tempdir().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
    // Built before the agreement exists: the claim must land before the
    // agreement's LEZ refund time, and a build can take minutes.
    let (program, program_sha256) = install_actor_program(root.path());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let sidecar = runtime.block_on(FakeLezSidecar::start(&COMPETITORS));

    let deployment = actor_deployment_with_bridge(root.path(), SWAP_ID, PROFILE, &sidecar.endpoint);
    let actor_config = ActorConfig::load_private(&deployment.source_config).unwrap();
    let role_state_db = actor_config.role_state_db().to_path_buf();
    let accepted = AcceptedZecAgreementV1::accept_wire_at(
        actor_config
            .load_activate_material()
            .unwrap()
            .signed_agreement_wire(),
        UnixSeconds::new(deployment.agreement_basis_time),
        Participant::Maker,
        0,
    )
    .unwrap();
    let lez_refund_at_ms = accepted.agreement().lez_refund_at_ms();
    sidecar.hold_escrow_of(accepted.agreement());
    runtime.block_on(maker_store_at_claim_evidence(&role_state_db, accepted));

    let swap_id = SwapId::new(SWAP_ID).unwrap();
    let database = root.path().join("maker.sqlite3");
    let mut store = SqliteSwapStore::open(&database).unwrap();
    store.save(&swap(SWAP_ID)).unwrap();
    store
        .register_maker_actor(
            &MakerActorManifestV1::new(
                swap_id.clone(),
                MakerActorKindV1::Zcash,
                deployment.source_config.clone(),
                Sha256::digest(fs::read(&deployment.source_config).unwrap()).into(),
                program,
                program_sha256,
                role_state_db.clone(),
            )
            .unwrap(),
            10,
        )
        .unwrap();
    prepare_maker_actor(&store.list_maker_actor_processes().unwrap().remove(0))
        .expect("exact deployment preflight");
    // The owner's view of the swap, as the Node's own RPC serves it.
    let monitor = rpc_module(MakerRpc::new(SqliteSwapStore::open(&database).unwrap())).unwrap();

    // The attempt timeout is also how long a submitted claim holds the
    // account's locks from its submission (and no longer than its refund
    // time): the longest a Node accepts, so the lock below waits for the
    // claim itself for as long as this run takes.
    let config =
        MakerActorSupervisorConfig::new(Duration::from_mins(5), REQUEUE_DELAY_SECONDS, 30, 8_192)
            .unwrap();
    let lock_swap_id = SwapId::new(LOCK_SWAP_ID).unwrap();
    let mut lock_log = None;
    let owner = MakerActorLeaseOwner::new([0x76; 16]).unwrap();
    let marker = revealing_claim_in_flight_marker(&role_state_db).unwrap();
    let started = Instant::now();
    let mut now = 10;
    let mut served = 0;
    let mut submitted_at_ms = None;
    for (index, attempt) in ATTEMPTS.iter().enumerate() {
        let number = index + 1;
        let last = number == ATTEMPTS.len();
        let before_ms = unix_now_ms();
        assert!(
            before_ms + REFUND_TIME_GUARD_MS < lez_refund_at_ms,
            "attempt {number} would start {} ms before the LEZ refund time: the machine is too \
             slow for the profile's LEZ refund delay",
            lez_refund_at_ms.saturating_sub(before_ms)
        );
        // The requeue delay has passed, by the Node's clock, and no sooner.
        assert!(
            store
                .list_due_maker_actor_ids(now - 1, 1)
                .unwrap()
                .is_empty()
                || index == 0
        );
        let outcome = supervise_one_due_maker_actor(&mut store, owner, now, &config)
            .unwrap()
            .expect("the swap's actor is due");
        assert_eq!(outcome.swap_id(), &swap_id, "attempt {number}");
        let events: Vec<String> = sidecar.events()[served..]
            .iter()
            .map(ToString::to_string)
            .collect();
        served += events.len();
        let record = store.maker_actor_process(&swap_id).unwrap().unwrap();
        eprintln!(
            "attempt {number} (+{} ms): {:?}, sidecar {events:?}",
            started.elapsed().as_millis(),
            outcome.resolution()
        );
        ActorHeldLock::acquire_for_within(
            record.swap_id(),
            record.manifest().state_database_path(),
            PASS_LOCK_RELEASE,
        )
        .expect("the actor's lock is released after its pass");

        assert_eq!(events, attempt.sidecar, "attempt {number}");
        assert_eq!(record.last_failure_class(), None, "attempt {number}");
        assert_eq!(
            record.attempt_count(),
            u64::try_from(number).unwrap(),
            "attempt {number}"
        );
        assert_eq!(
            outcome.resolution(),
            if last {
                MakerActorSupervisorResolution::Terminal
            } else {
                MakerActorSupervisorResolution::Requeued
            },
            "attempt {number}"
        );
        assert_eq!(
            record.schedule_state(),
            if last {
                MakerActorScheduleState::Terminal
            } else {
                MakerActorScheduleState::Queued
            },
            "attempt {number}"
        );
        // `status` then `claim`: the effect ran because the status named the
        // LEZ claim, and the swap stays at it until the claim is final.
        assert_eq!(
            store
                .maker_actor_progress(&swap_id)
                .unwrap()
                .unwrap()
                .observation(),
            &if last {
                MakerActorProgressObservationV1::active("completed", 4, "complete").unwrap()
            } else {
                MakerActorProgressObservationV1::active("claim_evidence_available", 3, "claim_lez")
                    .unwrap()
            },
            "attempt {number}"
        );
        let view: Value = runtime
            .block_on(monitor.call("maker_actor_monitor_v1", [json!({"id": SWAP_ID})]))
            .unwrap();
        if last {
            // A finished swap shows no last outcome.
            assert_eq!(view.get("last_outcome"), None, "{view}");
        } else {
            let last_outcome = &view["last_outcome"];
            assert_eq!(last_outcome["step"], "claim", "attempt {number}: {view}");
            assert_eq!(
                last_outcome["outcome"], attempt.outcome,
                "attempt {number}: {view}"
            );
            assert_eq!(
                last_outcome["operation"], "lez_followup_claim",
                "attempt {number}: {view}"
            );
            assert_eq!(
                last_outcome.get("waiting_for"),
                attempt
                    .contention
                    .then(|| {
                        json!({
                            "kind": "lez_claim_nonce_contention",
                            "chain": "lez",
                            "until_unix_seconds": lez_refund_at_ms / 1_000,
                        })
                    })
                    .as_ref(),
                "attempt {number}: {view}"
            );
        }

        // The in-flight marker: written by the first accepted submission, kept
        // while the claim is re-planned, removed once the claim is final.
        assert_eq!(marker.is_file(), attempt.in_flight, "attempt {number}");
        let recorded = revealing_claim_in_flight_submitted_at_ms(&role_state_db);
        if attempt.in_flight {
            assert_eq!(
                revealing_claim_in_flight_refund_at_ms(&role_state_db),
                Some(lez_refund_at_ms),
                "attempt {number}"
            );
            if attempt.outcome == "submitted" {
                // New bytes at a new nonce record their own submission time.
                let at = recorded.expect("a submitted claim records its time");
                assert!(
                    (before_ms..=unix_now_ms()).contains(&at),
                    "attempt {number}: {at}"
                );
                submitted_at_ms = Some(at);
            } else {
                assert_eq!(recorded, submitted_at_ms, "attempt {number}");
            }
        } else {
            assert_eq!(recorded, None, "attempt {number}");
        }

        // A LEZ lock of the same account, taken on once the claim reached the
        // gate, is due after every attempt of the claim. While the claim is
        // pending (waiting at the gate after the first attempt, submitted and
        // within its hold after the second) it is neither planned nor sent;
        // it goes once the claim is final.
        let lock_log =
            lock_log.get_or_insert_with(|| register_lez_lock(&mut store, root.path(), now));
        let lock = supervise_one_due_maker_actor(&mut store, owner, now, &config)
            .unwrap()
            .expect("the lock's actor is due");
        assert_eq!(lock.swap_id(), &lock_swap_id, "attempt {number}");
        let lock_record = store.maker_actor_process(&lock_swap_id).unwrap().unwrap();
        if last {
            assert_eq!(
                lock.resolution(),
                MakerActorSupervisorResolution::Requeued,
                "after the claim completed"
            );
            assert_eq!(lock_record.last_failure_class(), None);
            assert_eq!(drives(lock_log), 1, "the lock goes once the claim is final");
        } else {
            assert_eq!(
                (lock.resolution(), lock_record.last_failure_class()),
                (
                    MakerActorSupervisorResolution::Backoff,
                    Some("lez_lock_waits_for_claim")
                ),
                "attempt {number}"
            );
            assert_eq!(
                drives(lock_log),
                0,
                "attempt {number}: no lock while the claim is pending"
            );
        }
        now += REQUEUE_DELAY_SECONDS;
    }
    assert_eq!(
        store.list_due_maker_actor_ids(now, 2).unwrap(),
        std::slice::from_ref(&lock_swap_id),
        "a completed swap is not driven again; the lock goes on"
    );

    // What the sidecar served: more re-plans than the attention bound, in
    // order, each at a later nonce, and the claim submitted in every attempt
    // but the last.
    let events = sidecar.events();
    let replans: Vec<(u32, u128, u128)> = events
        .iter()
        .filter_map(|event| match event {
            SidecarEvent::Supersede {
                index,
                superseded,
                replanned,
            } => Some((*index, *superseded, *replanned)),
            _ => None,
        })
        .collect();
    assert!(replans.len() > usize::try_from(REVEALING_CLAIM_REPLAN_ATTENTION).unwrap());
    assert_eq!(
        replans,
        (1..=7_u32)
            .map(|index| {
                let superseded = FIRST_NONCE + u128::from(index) - 1;
                (index, superseded, superseded + 1)
            })
            .collect::<Vec<_>>()
    );
    let submissions: Vec<bool> = events
        .iter()
        .filter_map(|event| match event {
            SidecarEvent::Submit { accepted, .. } => Some(*accepted),
            _ => None,
        })
        .collect();
    assert_eq!(submissions.len(), 14);
    assert_eq!(
        submissions.iter().filter(|accepted| **accepted).count(),
        2,
        "accepted once and dropped, accepted again and included"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, SidecarEvent::Competitor { .. }))
            .count(),
        COMPETITORS.len()
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SidecarEvent::Refused(_))),
        "{events:?}"
    );

    // The Maker's own store: every re-plan recorded with both nonces, before
    // the LEZ refund time, and the swap completed.
    let role_store =
        SqliteZecRecoveryStore::open_claim_capable(&role_state_db, Participant::Maker, claim_key())
            .unwrap();
    let recorded = runtime
        .block_on(role_store.load_claim_supersessions(&swap_id))
        .unwrap();
    assert_eq!(
        recorded
            .iter()
            .map(|supersession| (
                supersession.replan_index(),
                supersession.superseded_nonce(),
                supersession.replanned_nonce()
            ))
            .collect::<Vec<_>>(),
        replans
    );
    assert!(
        recorded
            .iter()
            .all(|supersession| supersession.lez_refund_margin_ms() > 0)
    );
    let completed = runtime
        .block_on(
            ZecPairSdk::new(Participant::Maker, (), (), (), (), role_store)
                .resume_all_capable(&swap_id),
        )
        .unwrap()
        .expect("the Maker's swap");
    assert_eq!(completed.status(), Phase::Completed);
    assert_eq!(completed.revision(), 4);
    eprintln!(
        "the claim landed {} ms after its first attempt, {} ms before the LEZ refund time",
        started.elapsed().as_millis(),
        lez_refund_at_ms.saturating_sub(unix_now_ms())
    );
}
