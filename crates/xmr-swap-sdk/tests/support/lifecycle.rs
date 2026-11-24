//! Both roles of a LEZ/XMR swap, a model of the LEZ escrow's native-XMR
//! instructions (tags 13 to 17) and a model of the shared Monero output,
//! shared by `examples/full-lifecycle.rs` and `tests/full_lifecycle.rs` (both
//! include this file with `#[path]`).
//!
//! What is real: every key, share, DLEQ proof, Stage A, both `MuSig2`
//! adaptor sessions, Stage B, the claim and refund signatures and the spend-key
//! recovery are this SDK's types and `lez-adaptor-signature`'s sessions, as
//! the Nodes use them. What is modelled: the LEZ escrow (its states, windows,
//! the tag-14 commitment check and the BIP-340 check of tags 15 and 16 under
//! the swap's aggregate keys, on a model LEZ clock) and the Monero chain (an
//! output at the shared address that only the key opening its public spend
//! key can sweep). Chat is a function call. Deterministic keys from a seed;
//! the `MuSig2` nonces come from the operating system. No I/O.
#![allow(dead_code)]

use std::error::Error;

use lez_adaptor_signature::{
    AdaptorSigner, SigningRole, adapt_presignature, aggregate_adaptor_presignature,
    extract_adaptor_secret, verify_final_signature,
};
use lez_swap_core::{Chain, Pair, Participant, SwapDirection};
use lez_xmr_swap_sdk::{
    CrossCurveDleqProofV1, CrossCurveScalar, MoneroAddressNetworkV1, MoneroPrivateViewKey,
    MoneroSharedAddressV1, ReconstructedMoneroSpendKey, ValidatedXmrActivationBodyV1,
    ValidatedXmrAgreementBodyV1, XmrActivatedAgreementV1, XmrActivationBodyV1, XmrAgreementBodyV1,
    XmrAgreementV1, XmrLezInitializePlanV1, XmrLezLockCandidateV1, XmrLezLockStatusV1,
    XmrLezTermsV1, XmrMessagesV1, XmrMoneroTermsV1, XmrNamedProfileV1, XmrParticipantIdentityV1,
    XmrParticipantsV1, XmrRoleV1, XmrSessionTranscriptV1, XmrSwapDirectionV1, XmrWindowsV1,
};
use rand_chacha::{
    ChaCha20Rng,
    rand_core::{RngCore as _, SeedableRng as _},
};
use secp256k1::{Keypair, Message, PublicKey, Secp256k1, SecretKey, XOnlyPublicKey, schnorr};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;
/// One of the runs below.
pub type Run = fn(&str, &mut Narrator) -> Result<Settlement>;

/// The model LEZ clock's anchor (the take), Unix seconds.
pub const ANCHOR_S: u64 = 1_900_000_000;
/// The devnet `fast` profile's windows from the anchor: C, R and P.
pub const FUNDING_CUTOFF_S: u64 = 320;
pub const REFUND_S: u64 = 720;
pub const PUNISH_S: u64 = 1_000;
/// The Maker claims only on a tag 14 it sees before R - (R - C)/2.
pub const CLAIM_DEADLINE_S: u64 = REFUND_S - (REFUND_S - FUNDING_CUTOFF_S) / 2;
/// Stand-ins for what the Stage-A planner reads from the LEZ runtime and
/// monerod: the agreement commits them, nothing checks them against a chain.
const LEZ_CHANNEL_ID: [u8; 32] = [0x1c; 32];
const LEZ_GENESIS: [u8; 32] = [0x1e; 32];
const ESCROW_PROGRAM_ID: [u32; 8] = [0xe5c0; 8];
const TRANSFER_PROGRAM_ID: [u32; 8] = [0x7a5f; 8];
const MONERO_GENESIS: [u8; 32] = [0x4d; 32];
/// The escrow guest's tag-14 commitment domain
/// (`compat/lez-v0.2-provisional/escrow/src/lib.rs`).
const CLAIM_PARTIAL_DOMAIN: &[u8] = b"logos.gateway.lez-xmr.claim-partial-commitment.v1\0";

/// Every key of a run derives from this seed unless the caller names another.
pub const DEFAULT_SEED: &str = "lez-xmr-swap-sdk full-lifecycle";

// ------------------------------------------------------------------ narration

/// Prints a swap's steps (the example) or keeps them quiet (the tests).
#[derive(Debug)]
pub struct Narrator {
    label: String,
    echo: bool,
    pub lines: Vec<String>,
}

impl Narrator {
    #[must_use]
    pub fn new(label: &str, echo: bool) -> Self {
        Self {
            label: label.to_owned(),
            echo,
            lines: Vec::new(),
        }
    }

    pub fn say(&mut self, at_s: Option<u64>, line: &str) {
        let text = match at_s {
            Some(at) => format!("[{} | LEZ +{at:>5} s] {line}", self.label),
            None => format!("[{} |            ] {line}", self.label),
        };
        if self.echo {
            println!("{text}");
        }
        self.lines.push(text);
    }

    /// Records a step the escrow, the SDK or the model must refuse.
    ///
    /// # Errors
    ///
    /// Fails the run when the step was accepted.
    pub fn refused<T>(
        &mut self,
        at_s: Option<u64>,
        what: &str,
        result: std::result::Result<T, impl std::fmt::Display>,
    ) -> Result<()> {
        match result {
            Err(error) => {
                self.say(at_s, &format!("refused {what}: {error}"));
                Ok(())
            }
            Ok(_) => Err(format!("{what} was accepted").into()),
        }
    }
}

// ------------------------------------------------------------------ the roles

/// What a role root keeps on disk: its Monero share, the per-swap view key
/// and both stages. This survives a restart.
pub struct Durable {
    pub share: Zeroizing<[u8; 32]>,
    pub view: Zeroizing<[u8; 32]>,
    pub stage_a: Vec<u8>,
    pub stage_b: Vec<u8>,
}

struct SessionKeys {
    agreement: Zeroizing<[u8; 32]>,
    claim: Zeroizing<[u8; 32]>,
    refund: Zeroizing<[u8; 32]>,
}

/// A role's adaptor-session state: both signers and, for the Taker, the
/// claim partial it withholds. The Nodes keep this in their adaptor journal;
/// a restart that loses the journal loses it.
struct Journal {
    claim: AdaptorSigner,
    refund: AdaptorSigner,
    withheld_claim_partial: Option<Zeroizing<[u8; 32]>>,
}

/// The canonical public role packet, as R1 carries it.
#[derive(Clone)]
struct PublicPacket {
    identity: XmrParticipantIdentityV1,
    proof_wire: Vec<u8>,
    public_view_key: [u8; 32],
}

pub struct Party {
    pub role: XmrRoleV1,
    pub lez_account: [u8; 32],
    pub durable: Durable,
    keys: Option<SessionKeys>,
    journal: Option<Journal>,
    agreement: Option<XmrAgreementV1>,
    activated: Option<XmrActivatedAgreementV1>,
    packet: PublicPacket,
}

impl Party {
    fn generate(index: u8, seed: &str, role: XmrRoleV1, view: Option<&[u8; 32]>) -> Result<Self> {
        let tag = role_tag(role);
        let mut rng =
            ChaCha20Rng::from_seed(tagged_hash(seed.as_bytes(), &[&[index], tag].concat()));
        let keys = SessionKeys {
            agreement: secp_secret(&mut rng),
            claim: secp_secret(&mut rng),
            refund: secp_secret(&mut rng),
        };
        let share = cross_curve_scalar(&mut rng);
        let view = view.map_or_else(|| cross_curve_scalar(&mut rng), |key| Zeroizing::new(*key));
        let proof = CrossCurveDleqProofV1::prove(
            &CrossCurveScalar::from_monero_little_endian(*share)?,
            &mut rng,
        )?;
        proof.verify()?;
        let lez_account = tagged_hash(b"lez-owner", &[&[index], tag].concat());
        let packet = PublicPacket {
            identity: XmrParticipantIdentityV1::new(
                lez_account,
                public_key(&keys.agreement)?,
                public_key(&keys.claim)?,
                public_key(&keys.refund)?,
            ),
            proof_wire: proof.to_wire_bytes()?,
            public_view_key: MoneroPrivateViewKey::from_monero_little_endian(*view)?.public_key(),
        };
        Ok(Self {
            role,
            lez_account,
            durable: Durable {
                share,
                view,
                stage_a: Vec::new(),
                stage_b: Vec::new(),
            },
            keys: Some(keys),
            journal: None,
            agreement: None,
            activated: None,
            packet,
        })
    }

    fn keys(&self) -> Result<&SessionKeys> {
        self.keys
            .as_ref()
            .ok_or_else(|| "the session keys are gone".into())
    }

    fn journal_mut(&mut self) -> Result<&mut Journal> {
        self.journal
            .as_mut()
            .ok_or_else(|| "the adaptor journal is gone".into())
    }

    fn agreement(&self) -> Result<&XmrAgreementV1> {
        self.agreement
            .as_ref()
            .ok_or_else(|| "no Stage A in memory".into())
    }

    fn activated(&self) -> Result<&XmrActivatedAgreementV1> {
        self.activated
            .as_ref()
            .ok_or_else(|| "no Stage B in memory".into())
    }

    fn view_key(&self) -> Result<MoneroPrivateViewKey> {
        Ok(MoneroPrivateViewKey::from_monero_little_endian(
            *self.durable.view,
        )?)
    }

    /// The role's own Monero spend-key share, from its role root.
    ///
    /// # Errors
    ///
    /// Fails only if the stored share is not canonical.
    pub fn share(&self) -> Result<CrossCurveScalar> {
        Ok(CrossCurveScalar::from_monero_little_endian(
            *self.durable.share,
        )?)
    }

    /// Whether the role still holds its in-memory session state and journal.
    #[must_use]
    pub const fn has_session_state(&self) -> bool {
        self.journal.is_some() && self.keys.is_some() && self.activated.is_some()
    }

    /// A restart that lost everything but the role root: the session keys,
    /// the adaptor journal (with the Taker's withheld claim partial) and the
    /// parsed stages are gone.
    pub fn lose_session_state(&mut self, narrator: &mut Narrator, at_s: Option<u64>) {
        self.keys = None;
        self.journal = None;
        self.agreement = None;
        self.activated = None;
        narrator.say(
            at_s,
            &format!(
                "{}: restarted with its session state lost: no adaptor journal, no session keys; \
                 kept its share, the view key, Stage A ({} B) and Stage B ({} B)",
                role_name(self.role),
                self.durable.stage_a.len(),
                self.durable.stage_b.len()
            ),
        );
    }

    /// Stage A and Stage B parsed and validated again from the role root.
    ///
    /// # Errors
    ///
    /// Fails if either stage no longer validates against the view key.
    pub fn restored(&self) -> Result<(XmrAgreementV1, XmrActivatedAgreementV1)> {
        let agreement = XmrAgreementV1::from_wire(&self.durable.stage_a)?;
        let activated = XmrActivatedAgreementV1::from_wire(
            &agreement,
            &self.durable.stage_b,
            &self.view_key()?,
        )?;
        Ok((agreement, activated))
    }
}

// ------------------------------------------------------------ R1, R2: Stage A

#[derive(Debug, Clone, Copy)]
pub struct Terms {
    pub index: u8,
    pub swap_id: [u8; 32],
    pub amount_piconero: u64,
    pub lez_amount: u128,
}

impl Terms {
    #[must_use]
    pub fn new(index: u8, amount_piconero: u64, lez_amount: u128) -> Self {
        Self {
            index,
            swap_id: tagged_hash(b"swap-id", &[index]),
            amount_piconero,
            lez_amount,
        }
    }
}

/// Both roles compose Stage A from the same facts; the Taker refuses
/// anything but byte equality with the Maker's.
fn compose_stage_a(
    terms: &Terms,
    maker: &PublicPacket,
    taker: &PublicPacket,
) -> Result<ValidatedXmrAgreementBodyV1> {
    let maker_proof = CrossCurveDleqProofV1::from_wire_bytes(&maker.proof_wire)?;
    let taker_proof = CrossCurveDleqProofV1::from_wire_bytes(&taker.proof_wire)?;
    check(
        maker.public_view_key == taker.public_view_key,
        "the packets name different view keys",
    )?;
    let participants = XmrParticipantsV1::new(maker.identity.clone(), taker.identity.clone());
    let claim_key = participants.claim_aggregate_x_only_key()?;
    let refund_key = participants.refund_aggregate_x_only_key()?;
    let shared = MoneroSharedAddressV1::derive_from_public_view_key(
        MoneroAddressNetworkV1::Regtest,
        &maker_proof,
        &taker_proof,
        taker.public_view_key,
    )?;
    let profile = XmrNamedProfileV1::AcceleratedRegtest;
    let derived = |purpose: &[u8]| tagged_hash(purpose, &terms.swap_id);
    let body = XmrAgreementBodyV1::new(
        XmrSwapDirectionV1::TakerSellsLez,
        profile,
        terms.swap_id,
        participants,
        XmrMoneroTermsV1::new(
            MoneroAddressNetworkV1::Regtest,
            MONERO_GENESIS,
            terms.amount_piconero,
            profile.required_monero_confirmations(),
            maker.proof_wire.clone(),
            taker.proof_wire.clone(),
            shared.public_view_key(),
            shared.public_spend_key(),
            shared.address_string(),
        ),
        XmrLezTermsV1::new(
            LEZ_CHANNEL_ID,
            LEZ_GENESIS,
            ESCROW_PROGRAM_ID,
            TRANSFER_PROGRAM_ID,
            profile.required_lez_finality_units(),
            derived(b"metadata-account"),
            derived(b"custody-account"),
            taker.identity.lez_owner_account(),
            maker.identity.lez_owner_account(),
            claim_key,
            XmrLezTermsV1::authority_account_for_key(claim_key),
            refund_key,
            XmrLezTermsV1::authority_account_for_key(refund_key),
            maker_proof.transcript_commitment(),
            taker_proof.transcript_commitment(),
            terms.lez_amount,
        ),
        XmrMessagesV1::new(
            derived(b"claim-message"),
            derived(b"refund-message"),
            derived(b"punish-message"),
        ),
        XmrWindowsV1::new(at_ms(FUNDING_CUTOFF_S), at_ms(REFUND_S), at_ms(PUNISH_S)),
    );
    Ok(ValidatedXmrAgreementBodyV1::validate(body)?)
}

fn agree_stage_a(
    terms: &Terms,
    maker: &mut Party,
    taker: &mut Party,
    narrator: &mut Narrator,
) -> Result<()> {
    // R1: the Maker checks the Taker's view key against its packet, composes
    // Stage A and signs it.
    check(
        taker.view_key()?.public_key() == taker.packet.public_view_key,
        "the Taker's view key is not its packet's",
    )?;
    let validated = compose_stage_a(terms, &maker.packet, &taker.packet)?;
    let unsigned = validated.encode_unsigned_wire()?;
    let maker_signature = sign_commitment(&maker.keys()?.agreement, validated.commitment())?;
    narrator.say(
        None,
        &format!(
            "R1: Maker composed and signed Stage A {} ({} B unsigned)",
            short(&validated.commitment()),
            unsigned.len()
        ),
    );

    // R2: the Taker recomposes Stage A, requires the same bytes, verifies the
    // Maker's signature by attaching it, and countersigns.
    let expected = compose_stage_a(terms, &maker.packet, &taker.packet)?;
    check(
        expected.encode_unsigned_wire()? == unsigned,
        "the Maker's Stage A is not the one the Taker composes",
    )?;
    let taker_signature = sign_commitment(&taker.keys()?.agreement, expected.commitment())?;
    let agreement = expected.attach_signatures(maker_signature, taker_signature)?;
    let wire = agreement.encode_wire()?;
    let accepted = XmrAgreementV1::from_wire(&wire)?;
    check(
        accepted.agreement_commitment()
            == ValidatedXmrAgreementBodyV1::from_unsigned_wire(&unsigned)?.commitment(),
        "the countersigned Stage A is not the one the Maker signed",
    )?;
    narrator.say(
        None,
        &format!(
            "R2: Taker recomposed Stage A byte for byte and countersigned; both DLEQ proofs verified; \
             shared 2-of-2 Monero address {}",
            short_str(&accepted.shared_address().address_string())
        ),
    );
    taker.durable.stage_a.clone_from(&wire);
    taker.agreement = Some(agreement);
    maker.durable.stage_a = wire;
    maker.agreement = Some(accepted);
    Ok(())
}

// ------------------------------------------------- R3 to R6: sessions, Stage B

struct Round {
    commitment: [u8; 32],
    nonce: [u8; 66],
}

/// Both adaptor sessions, the Taker's claim partial withheld, Stage B
/// composed by the Taker, validated and countersigned by the Maker, and
/// activated by both.
#[allow(clippy::too_many_lines)]
fn run_ceremony(maker: &mut Party, taker: &mut Party, narrator: &mut Narrator) -> Result<()> {
    let claim_context = maker.agreement()?.claim_session_descriptor().context()?;
    let refund_context = maker.agreement()?.refund_session_descriptor().context()?;
    let mut maker_journal = Journal {
        claim: AdaptorSigner::new(
            claim_context.clone(),
            SigningRole::Maker,
            *maker.keys()?.claim,
        )?,
        refund: AdaptorSigner::new(
            refund_context.clone(),
            SigningRole::Maker,
            *maker.keys()?.refund,
        )?,
        withheld_claim_partial: None,
    };
    let mut taker_journal = Journal {
        claim: AdaptorSigner::new(claim_context, SigningRole::Taker, *taker.keys()?.claim)?,
        refund: AdaptorSigner::new(refund_context, SigningRole::Taker, *taker.keys()?.refund)?,
        withheld_claim_partial: None,
    };

    // R3: nonce commitments of both sessions.
    let maker_commitments = (
        maker_journal.claim.nonce_commitment(),
        maker_journal.refund.nonce_commitment(),
    );
    let taker_commitments = (
        taker_journal.claim.nonce_commitment(),
        taker_journal.refund.nonce_commitment(),
    );
    maker_journal
        .claim
        .accept_peer_commitment(taker_commitments.0)?;
    maker_journal
        .refund
        .accept_peer_commitment(taker_commitments.1)?;
    taker_journal
        .claim
        .accept_peer_commitment(maker_commitments.0)?;
    taker_journal
        .refund
        .accept_peer_commitment(maker_commitments.1)?;
    narrator.say(None, "R3: nonce commitments of both sessions exchanged");

    // R4: nonces opened against the commitments; both roles sign both
    // sessions. The Taker keeps its claim partial.
    let maker_claim = Round {
        commitment: maker_commitments.0,
        nonce: maker_journal.claim.public_nonce()?,
    };
    let maker_refund = Round {
        commitment: maker_commitments.1,
        nonce: maker_journal.refund.public_nonce()?,
    };
    let taker_claim = Round {
        commitment: taker_commitments.0,
        nonce: taker_journal.claim.public_nonce()?,
    };
    let taker_refund = Round {
        commitment: taker_commitments.1,
        nonce: taker_journal.refund.public_nonce()?,
    };
    maker_journal.claim.accept_peer_nonce(taker_claim.nonce)?;
    maker_journal.refund.accept_peer_nonce(taker_refund.nonce)?;
    taker_journal.claim.accept_peer_nonce(maker_claim.nonce)?;
    taker_journal.refund.accept_peer_nonce(maker_refund.nonce)?;
    let maker_claim_partial = maker_journal.claim.create_partial_signature()?;
    let maker_refund_partial = maker_journal.refund.create_partial_signature()?;
    let withheld = Zeroizing::new(taker_journal.claim.create_partial_signature()?);
    let taker_refund_partial = taker_journal.refund.create_partial_signature()?;
    taker_journal
        .claim
        .accept_peer_partial_signature(maker_claim_partial)?;
    taker_journal
        .refund
        .accept_peer_partial_signature(maker_refund_partial)?;
    narrator.say(
        None,
        "R4: nonces opened, Maker partials verified; the Taker aggregated both presignatures and withheld its claim partial",
    );

    // R5: the Taker composes Stage B and signs it; the Maker verifies the
    // Taker's refund partial, aggregates the same refund presignature,
    // validates Stage B against its own view key and signs.
    let claim_transcript = XmrSessionTranscriptV1::new(
        maker_claim.commitment,
        taker_claim.commitment,
        maker_claim.nonce,
        taker_claim.nonce,
    );
    let refund_transcript = XmrSessionTranscriptV1::new(
        maker_refund.commitment,
        taker_refund.commitment,
        maker_refund.nonce,
        taker_refund.nonce,
    );
    let taker_agreement = taker.agreement()?;
    let body = XmrActivationBodyV1::new(
        taker_agreement.agreement_commitment(),
        taker_agreement.claim_context_binding(),
        claim_transcript,
        maker_claim_partial,
        taker_agreement.claim_partial_context_binding(&claim_transcript, maker_claim_partial)?,
        taker_agreement.commit_taker_claim_partial(
            &claim_transcript,
            maker_claim_partial,
            *withheld,
        )?,
        taker_agreement.refund_context_binding(),
        refund_transcript,
        maker_refund_partial,
        taker_refund_partial,
        taker_journal.refund.presignature()?,
    );
    let stage_b =
        ValidatedXmrActivationBodyV1::validate(taker_agreement, body, &taker.view_key()?)?;
    let unsigned = stage_b.encode_unsigned_wire()?;
    let taker_signature = sign_commitment(&taker.keys()?.agreement, stage_b.commitment())?;

    maker_journal
        .refund
        .accept_peer_partial_signature(taker_refund_partial)?;
    let maker_view = maker.view_key()?;
    let checked = ValidatedXmrActivationBodyV1::from_unsigned_wire(
        maker.agreement()?,
        &unsigned,
        &maker_view,
    )?;
    check(
        checked.body().refund_presignature() == maker_journal.refund.presignature()?,
        "the Taker's Stage B is not the Maker's refund session",
    )?;
    let maker_signature = sign_commitment(&maker.keys()?.agreement, checked.commitment())?;
    narrator.say(
        None,
        &format!(
            "R5: Stage B {} validated and signed by the Maker; refund presignatures converged",
            short(&checked.commitment())
        ),
    );

    // R6: the Taker activates; both hold the countersigned Stage B.
    let wire = stage_b
        .attach_signatures(maker_signature, taker_signature)?
        .encode_wire()?;
    let maker_activated =
        XmrActivatedAgreementV1::from_wire(maker.agreement()?, &wire, &maker_view)?;
    let taker_activated =
        XmrActivatedAgreementV1::from_wire(taker.agreement()?, &wire, &taker.view_key()?)?;
    for sent in [&taker_refund_partial[..], &unsigned, &wire] {
        check(
            !sent.windows(32).any(|bytes| bytes == withheld.as_slice()),
            "the Taker's claim partial crossed Chat",
        )?;
    }
    taker_activated.verify_published_taker_claim_partial(taker.agreement()?, *withheld)?;
    let coordinator = maker_activated.initial_coordinator(maker.agreement()?)?;
    check(
        coordinator.pair() == Pair::Monero
            && coordinator.direction() == SwapDirection::TakerSellsLez
            && coordinator.first_claimant() == Participant::Maker
            && coordinator
                .recovery_schedule()
                .deadline_for_chain(Chain::Lez)
                .is_some(),
        "Stage B does not give the LEZ-first XMR coordinator",
    )?;
    narrator.say(
        None,
        &format!(
            "R6: Stage B ({} B) activated by both; the Taker's claim partial crossed nothing, Stage B commits to it; \
             coordinator {:?} {:?}, first claimant {:?}",
            wire.len(),
            coordinator.pair(),
            coordinator.direction(),
            coordinator.first_claimant()
        ),
    );
    taker_journal.withheld_claim_partial = Some(withheld);
    maker.durable.stage_b.clone_from(&wire);
    taker.durable.stage_b = wire;
    maker.activated = Some(maker_activated);
    taker.activated = Some(taker_activated);
    maker.journal = Some(maker_journal);
    taker.journal = Some(taker_journal);
    Ok(())
}

// ---------------------------------------------------------- the LEZ escrow

/// The escrow states tags 13 to 17 move between. The guest records a tag 17
/// as `Claimed`; the model names it `Punished` to tell the branches apart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EscrowState {
    Funded,
    XmrClaimAuthorized,
    Claimed,
    Refunded,
    Punished,
}

/// An in-memory model of the LEZ v0.2 escrow's native-XMR instructions.
pub struct ModelEscrow {
    plan: XmrLezInitializePlanV1,
    pub state: EscrowState,
    pub funded_at_ms: u64,
    pub funding_transaction: [u8; 32],
    pub claim_partial: Option<[u8; 32]>,
    pub claim_signature: Option<[u8; 64]>,
    pub refund_signature: Option<[u8; 64]>,
    pub lez_paid_to: Option<XmrRoleV1>,
}

impl ModelEscrow {
    /// Tag 13 `InitializeNativeXmr` and the native fund, by the Taker owner,
    /// before R.
    ///
    /// # Errors
    ///
    /// Refuses another signer or a lock from R.
    pub fn lock(plan: XmrLezInitializePlanV1, signer: [u8; 32], now_s: u64) -> Result<Self> {
        check(
            signer == plan.depositor_account(),
            "tag 13 is the Taker owner's",
        )?;
        check(
            at_ms(now_s) < plan.refund_at_ms(),
            "tag 13 is valid only before R",
        )?;
        check(
            plan.claim_partial_commitment() != [0; 32] && plan.amount() > 0,
            "the plan has no claim-partial commitment or no amount",
        )?;
        let funding_transaction = tagged_hash(b"lez-fund-tx", &plan.swap_id());
        Ok(Self {
            plan,
            state: EscrowState::Funded,
            funded_at_ms: at_ms(now_s),
            funding_transaction,
            claim_partial: None,
            claim_signature: None,
            refund_signature: None,
            lez_paid_to: None,
        })
    }

    /// Tag 14 `AuthorizeNativeXmrClaim`: Funded to `XmrClaimAuthorized`,
    /// before R, with the partial that opens the stored commitment.
    ///
    /// # Errors
    ///
    /// Refuses another signer, another partial, a wrong state or R reached.
    pub fn authorize_claim(
        &mut self,
        signer: [u8; 32],
        partial: [u8; 32],
        now_s: u64,
    ) -> Result<()> {
        check(
            signer == self.plan.depositor_account(),
            "tag 14 is the Taker owner's",
        )?;
        check(
            at_ms(now_s) < self.plan.refund_at_ms(),
            "tag 14 is valid only before R",
        )?;
        self.require(&[EscrowState::Funded])?;
        let commitment: [u8; 32] = Sha256::new()
            .chain_update(CLAIM_PARTIAL_DOMAIN)
            .chain_update(self.plan.claim_partial_context_binding())
            .chain_update(partial)
            .finalize()
            .into();
        check(
            partial != [0; 32] && commitment == self.plan.claim_partial_commitment(),
            "the claim partial does not open the stored commitment",
        )?;
        self.claim_partial = Some(partial);
        self.state = EscrowState::XmrClaimAuthorized;
        Ok(())
    }

    /// Tag 15 `ClaimNativeXmr`: `XmrClaimAuthorized` to Claimed, before R,
    /// under the claim aggregate key. LEZ to the Maker.
    ///
    /// # Errors
    ///
    /// Refuses a wrong state, R reached or a signature that does not verify.
    pub fn claim(&mut self, signature: [u8; 64], now_s: u64) -> Result<()> {
        check(
            at_ms(now_s) < self.plan.refund_at_ms(),
            "tag 15 is valid only before R",
        )?;
        self.require(&[EscrowState::XmrClaimAuthorized])?;
        verify_bip340(
            self.plan.claim_aggregate_x_only_key(),
            self.plan.claim_message_hash(),
            signature,
        )?;
        self.claim_signature = Some(signature);
        self.state = EscrowState::Claimed;
        self.lez_paid_to = Some(XmrRoleV1::Maker);
        Ok(())
    }

    /// Tag 16 `RefundNativeXmr`: Funded or `XmrClaimAuthorized` to Refunded,
    /// in [R, P), under the refund aggregate key. LEZ back to the Taker.
    ///
    /// # Errors
    ///
    /// Refuses outside [R, P), a wrong state or a signature that does not verify.
    pub fn refund(&mut self, signature: [u8; 64], now_s: u64) -> Result<()> {
        check(
            (self.plan.refund_at_ms()..self.plan.punish_at_ms()).contains(&at_ms(now_s)),
            "tag 16 is valid only in [R, P)",
        )?;
        self.require(&[EscrowState::Funded, EscrowState::XmrClaimAuthorized])?;
        verify_bip340(
            self.plan.refund_aggregate_x_only_key(),
            self.plan.refund_message_hash(),
            signature,
        )?;
        self.refund_signature = Some(signature);
        self.state = EscrowState::Refunded;
        self.lez_paid_to = Some(XmrRoleV1::Taker);
        Ok(())
    }

    /// Tag 17 `PunishNativeXmr`: from P, by the Maker owner. LEZ to the Maker.
    ///
    /// # Errors
    ///
    /// Refuses another signer, before P, or a settled escrow.
    pub fn punish(&mut self, signer: [u8; 32], now_s: u64) -> Result<()> {
        check(
            signer == self.plan.claimant_account(),
            "tag 17 is the Maker owner's",
        )?;
        check(
            at_ms(now_s) >= self.plan.punish_at_ms(),
            "tag 17 is valid only from P",
        )?;
        self.require(&[EscrowState::Funded, EscrowState::XmrClaimAuthorized])?;
        self.state = EscrowState::Punished;
        self.lez_paid_to = Some(XmrRoleV1::Maker);
        Ok(())
    }

    fn require(&self, states: &[EscrowState]) -> Result<()> {
        check(
            states.contains(&self.state),
            &format!("the escrow is {:?}", self.state),
        )
    }
}

fn verify_bip340(x_only_key: [u8; 32], message: [u8; 32], signature: [u8; 64]) -> Result<()> {
    Secp256k1::verification_only()
        .verify_schnorr(
            &secp_ok(schnorr::Signature::from_slice(&signature))?,
            &Message::from_digest(message),
            &secp_ok(XOnlyPublicKey::from_slice(&x_only_key))?,
        )
        .map_err(|_| "the signature does not verify under the aggregate key".into())
}

// -------------------------------------------------------- the Monero output

/// The Maker's output at the shared 2-of-2 address. Spending it needs the
/// private key of the address's public spend key: the sum of both shares.
pub struct SharedOutput {
    address: MoneroSharedAddressV1,
    pub amount_piconero: u64,
    pub confirmations: u32,
    pub swept_by: Option<XmrRoleV1>,
}

impl SharedOutput {
    /// Sweeps the output with a rebuilt key.
    ///
    /// # Errors
    ///
    /// Refuses a key that does not open the address, or a second sweep.
    pub fn sweep(&mut self, by: XmrRoleV1, key: &ReconstructedMoneroSpendKey) -> Result<()> {
        check(self.swept_by.is_none(), "the output is already spent")?;
        check(
            key.public_key() == self.address.public_spend_key(),
            "the key does not open the shared address",
        )?;
        self.swept_by = Some(by);
        Ok(())
    }
}

// ------------------------------------------------------------------ one swap

/// A swap from the take to both locks: Stage A, Stage B, tag 13 and the
/// fund, the Maker's lock check before C, the Monero funding and the
/// Taker's view-only check of it.
pub struct Swap {
    pub terms: Terms,
    pub maker: Party,
    pub taker: Party,
    pub escrow: ModelEscrow,
    pub output: SharedOutput,
}

/// What a recovery produced, kept for the tests' checks.
pub struct Settlement {
    pub swap: Swap,
    /// The spend key the role that swept rebuilt after its restart.
    pub recovered: Option<Zeroizing<[u8; 32]>>,
    /// The same role's key from its journal, before the restart.
    pub from_journal: Option<Zeroizing<[u8; 32]>>,
}

impl Swap {
    /// Takes, agrees, activates and locks both legs.
    ///
    /// # Errors
    ///
    /// Fails on any refused step.
    pub fn lock_both(terms: Terms, seed: &str, narrator: &mut Narrator) -> Result<Self> {
        // The Taker makes the per-swap view key and hands it over in R1.
        let mut taker = Party::generate(terms.index, seed, XmrRoleV1::Taker, None)?;
        let mut maker = Party::generate(
            terms.index,
            seed,
            XmrRoleV1::Maker,
            Some(&taker.durable.view),
        )?;
        narrator.say(
            None,
            &format!(
                "both roles: keys and Monero shares from the seed; DLEQ proofs proved and verified \
                 (Maker share {}, Taker share {})",
                short(&maker.share()?.monero_public_key()?),
                short(&taker.share()?.monero_public_key()?)
            ),
        );
        agree_stage_a(&terms, &mut maker, &mut taker, narrator)?;
        run_ceremony(&mut maker, &mut taker, narrator)?;

        // Tag 13 and the fund, by the Taker right after the take.
        let plan = taker.activated()?.lez_initialize_plan(taker.agreement()?)?;
        let escrow = ModelEscrow::lock(plan, taker.lez_account, 30)?;
        narrator.say(
            Some(30),
            &format!(
                "tag 13 + fund: LEZ {} locked by the Taker, escrow {:?}",
                terms.lez_amount, escrow.state
            ),
        );

        // The Maker funds Monero only on a lock final before C.
        let agreement = maker.agreement()?;
        let activated = maker.activated()?;
        let plan = activated.lez_initialize_plan(agreement)?;
        let candidate = |finalized_at_ms| {
            XmrLezLockCandidateV1::new(
                plan.clone(),
                plan.metadata_version(),
                XmrLezLockStatusV1::Funded,
                agreement.body().lez().required_finality_units(),
                finalized_at_ms,
                escrow.funding_transaction,
                tagged_hash(b"lez-block", &escrow.funding_transaction),
            )
        };
        narrator.refused(
            None,
            "a LEZ lock final after C",
            activated
                .validate_lez_lock_candidate(agreement, &candidate(at_ms(FUNDING_CUTOFF_S) + 1)),
        )?;
        let validated =
            activated.validate_lez_lock_candidate(agreement, &candidate(escrow.funded_at_ms))?;
        narrator.say(
            Some(30),
            &format!(
                "Maker: LEZ lock {} matches Stage B and is final before C (+{FUNDING_CUTOFF_S} s)",
                short(&validated.funding_transaction_id())
            ),
        );

        // The Maker funds the shared address; the Taker checks, with its own
        // view key, that the output is at the address both proofs and that key
        // give.
        let address = agreement.shared_address().clone();
        let output = SharedOutput {
            address: address.clone(),
            amount_piconero: terms.amount_piconero,
            confirmations: agreement.body().monero().required_confirmations(),
            swept_by: None,
        };
        narrator.say(
            Some(60),
            &format!(
                "Maker: {} piconero at the shared address {} (model: {} confirmations)",
                terms.amount_piconero,
                short_str(&address.address_string()),
                output.confirmations
            ),
        );
        let taker_agreement = taker.agreement()?;
        let address_from_view_key = MoneroSharedAddressV1::derive(
            MoneroAddressNetworkV1::Regtest,
            taker_agreement.maker_proof(),
            taker_agreement.taker_proof(),
            &taker.view_key()?,
        )?;
        check(
            address_from_view_key == *taker_agreement.shared_address()
                && output.amount_piconero == taker_agreement.body().monero().amount_piconero()
                && output.confirmations >= taker_agreement.body().monero().required_confirmations(),
            "the Taker does not see the agreed output",
        )?;
        narrator.say(
            Some(90),
            "Taker: the output is the agreed amount at the address its view key and both proofs give, at 10 confirmations",
        );
        Ok(Self {
            terms,
            maker,
            taker,
            escrow,
            output,
        })
    }

    // ------------------------------------------------------- claim pieces

    /// Tag 14 from the Taker's journal (its withheld claim partial).
    ///
    /// # Errors
    ///
    /// Fails without a journal, or when the escrow refuses.
    pub fn taker_authorizes_claim(&mut self, now_s: u64, narrator: &mut Narrator) -> Result<()> {
        let partial = *self
            .taker
            .journal_mut()?
            .withheld_claim_partial
            .clone()
            .ok_or("no withheld claim partial")?;
        let mut other = partial;
        other[31] ^= 1;
        narrator.refused(
            Some(now_s),
            "tag 14 with another partial",
            self.escrow
                .authorize_claim(self.taker.lez_account, other, now_s),
        )?;
        self.escrow
            .authorize_claim(self.taker.lez_account, partial, now_s)?;
        narrator.say(
            Some(now_s),
            &format!(
                "tag 14: Taker published its claim partial, escrow {:?}",
                self.escrow.state
            ),
        );
        Ok(())
    }

    /// Tag 15 by a Maker that lost its journal after tag 14: the claim
    /// presignature aggregated from Stage B and the published partial,
    /// completed with its share from its role root.
    ///
    /// # Errors
    ///
    /// Fails if Stage B or the published partial do not check, or the escrow refuses.
    pub fn maker_claims_from_stage_b(&mut self, now_s: u64, narrator: &mut Narrator) -> Result<()> {
        check(
            now_s < CLAIM_DEADLINE_S,
            "the tag 14 is past the Maker's claim window",
        )?;
        let (agreement, activated) = self.maker.restored()?;
        let published = self.escrow.claim_partial.ok_or("no tag 14 on chain")?;
        activated.verify_published_taker_claim_partial(&agreement, published)?;
        let context = agreement.claim_session_descriptor().context()?;
        let transcript = activated.body().claim_transcript();
        let presignature = aggregate_adaptor_presignature(
            &context,
            transcript.maker_public_nonce(),
            transcript.taker_public_nonce(),
            activated.body().maker_claim_partial(),
            published,
        )?;
        let signature = adapt_presignature(
            &context,
            presignature,
            self.maker.share()?.adaptor_scalar_big_endian(),
        )?;
        self.escrow.claim(signature, now_s)?;
        narrator.say(
            Some(now_s),
            &format!(
                "tag 15: Maker aggregated the claim presignature from Stage B and the tag-14 partial, \
                 completed it with its share; LEZ {} to the Maker, escrow {:?}",
                self.terms.lez_amount, self.escrow.state
            ),
        );
        Ok(())
    }

    /// The Taker's key from its journal after tag 15: its claim
    /// presignature and the published signature give the Maker's share.
    ///
    /// # Errors
    ///
    /// Fails without a journal or a tag 15.
    pub fn taker_key_from_journal(&mut self) -> Result<ReconstructedMoneroSpendKey> {
        let signature = self.escrow.claim_signature.ok_or("no tag 15 on chain")?;
        let context = self
            .taker
            .agreement()?
            .claim_session_descriptor()
            .context()?;
        let presignature = self.taker.journal_mut()?.claim.presignature()?;
        let extracted = extract_adaptor_secret(&context, presignature, signature)?;
        let agreement = self.taker.agreement()?;
        Ok(ReconstructedMoneroSpendKey::reconstruct(
            agreement.shared_address(),
            agreement.maker_proof(),
            self.taker.share()?,
            extracted,
        )?)
    }

    /// The Taker's key after a restart: Stage A, Stage B, its share and what
    /// LEZ shows (tag 14's partial and tag 15's signature).
    ///
    /// # Errors
    ///
    /// Fails when the chain lacks tag 14 or tag 15, or the SDK refuses.
    pub fn taker_recovers_claim_key(&self) -> Result<ReconstructedMoneroSpendKey> {
        let (agreement, activated) = self.taker.restored()?;
        let partial = self.escrow.claim_partial.ok_or("no tag 14 on chain")?;
        let signature = self.escrow.claim_signature.ok_or("no tag 15 on chain")?;
        Ok(activated.recover_claim_spend_key(
            &agreement,
            self.taker.share()?,
            partial,
            signature,
        )?)
    }

    // ------------------------------------------------------ refund pieces

    /// Tag 16 from Stage B alone: its refund presignature completed with the
    /// Taker's share from its role root. Needs no journal.
    ///
    /// # Errors
    ///
    /// Fails if Stage B does not restore, or the escrow refuses.
    pub fn taker_refunds_from_stage_b(
        &mut self,
        now_s: u64,
        narrator: &mut Narrator,
    ) -> Result<()> {
        let (agreement, activated) = self.taker.restored()?;
        let context = agreement.refund_session_descriptor().context()?;
        let signature = adapt_presignature(
            &context,
            activated.body().refund_presignature(),
            self.taker.share()?.adaptor_scalar_big_endian(),
        )?;
        verify_final_signature(&context, signature)?;
        self.escrow.refund(signature, now_s)?;
        narrator.say(
            Some(now_s),
            &format!(
                "tag 16: Taker completed Stage B's refund presignature with its share; LEZ {} back to the Taker, escrow {:?}",
                self.terms.lez_amount, self.escrow.state
            ),
        );
        Ok(())
    }

    /// The Maker's key from its journal after tag 16.
    ///
    /// # Errors
    ///
    /// Fails without a journal or a tag 16.
    pub fn maker_key_from_journal(&mut self) -> Result<ReconstructedMoneroSpendKey> {
        let signature = self.escrow.refund_signature.ok_or("no tag 16 on chain")?;
        let context = self
            .maker
            .agreement()?
            .refund_session_descriptor()
            .context()?;
        let presignature = self.maker.journal_mut()?.refund.presignature()?;
        let extracted = extract_adaptor_secret(&context, presignature, signature)?;
        let agreement = self.maker.agreement()?;
        Ok(ReconstructedMoneroSpendKey::reconstruct(
            agreement.shared_address(),
            agreement.taker_proof(),
            self.maker.share()?,
            extracted,
        )?)
    }

    /// The Maker's key after a restart: Stage A, Stage B, its share and
    /// tag 16's signature.
    ///
    /// # Errors
    ///
    /// Fails when the chain lacks tag 16, or the SDK refuses.
    pub fn maker_recovers_refund_key(&self) -> Result<ReconstructedMoneroSpendKey> {
        let (agreement, activated) = self.maker.restored()?;
        let signature = self.escrow.refund_signature.ok_or("no tag 16 on chain")?;
        Ok(activated.recover_refund_spend_key(&agreement, self.maker.share()?, signature)?)
    }

    /// The SDK's refusals around a recovery: another own share, the other
    /// session's signature and a partial Stage B does not commit to.
    ///
    /// # Errors
    ///
    /// Fails if any of them is accepted.
    pub fn recovery_refusals(&self, narrator: &mut Narrator) -> Result<()> {
        let wrong_share = || {
            CrossCurveScalar::from_monero_little_endian(*cross_curve_scalar(
                &mut ChaCha20Rng::from_seed([0x77; 32]),
            ))
        };
        if let (Some(partial), Some(signature)) =
            (self.escrow.claim_partial, self.escrow.claim_signature)
        {
            let (agreement, activated) = self.taker.restored()?;
            narrator.refused(
                None,
                "claim recovery with another share than the Taker's",
                activated.recover_claim_spend_key(&agreement, wrong_share()?, partial, signature),
            )?;
            let mut forged = partial;
            forged[31] ^= 1;
            narrator.refused(
                None,
                "claim recovery with a partial Stage B does not commit to",
                activated.recover_claim_spend_key(
                    &agreement,
                    self.taker.share()?,
                    forged,
                    signature,
                ),
            )?;
            narrator.refused(
                None,
                "refund recovery from the claim signature",
                activated.recover_refund_spend_key(&agreement, self.maker.share()?, signature),
            )?;
        }
        if let Some(signature) = self.escrow.refund_signature {
            let (agreement, activated) = self.maker.restored()?;
            narrator.refused(
                None,
                "refund recovery with another share than the Maker's",
                activated.recover_refund_spend_key(&agreement, wrong_share()?, signature),
            )?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- the runs

/// Completed: tag 14, then the Maker restarts without its journal and still
/// claims (tag 15) from Stage B and the published partial; the Taker rebuilds
/// the key from its journal, restarts without it, rebuilds the same key from
/// Stage A, Stage B, its share and the chain, and sweeps.
///
/// # Errors
///
/// Fails on any step the protocol must accept, or any refusal it must make.
pub fn completed(seed: &str, narrator: &mut Narrator) -> Result<Settlement> {
    let mut swap = Swap::lock_both(Terms::new(1, 200_000_000_000, 500), seed, narrator)?;
    swap.taker_authorizes_claim(400, narrator)?;
    swap.maker.lose_session_state(narrator, Some(420));
    swap.maker_claims_from_stage_b(450, narrator)?;
    narrator.refused(
        Some(750),
        "tag 16 after the claim",
        swap.escrow.refund([0; 64], 750),
    )?;

    let from_journal = swap.taker_key_from_journal()?;
    narrator.say(
        None,
        &format!(
            "Taker (journal): Maker's share extracted from tag 15 and checked against its DLEQ proof; key opens {}",
            short(&from_journal.public_key())
        ),
    );
    swap.taker.lose_session_state(narrator, None);
    let recovered = swap.taker_recovers_claim_key()?;
    check(
        recovered.public_key() == from_journal.public_key(),
        "the recovered key is not the journal's",
    )?;
    narrator.say(
        None,
        &format!(
            "Taker (recovered): claim presignature aggregated again from Stage B and tag 14, Maker's share \
             extracted from tag 15 and checked against its DLEQ proof; key opens {}",
            short(&recovered.public_key())
        ),
    );
    swap.recovery_refusals(narrator)?;
    swap.output.sweep(XmrRoleV1::Taker, &recovered)?;
    narrator.say(
        None,
        "Taker: swept the shared output with the recovered key",
    );
    narrator.refused(
        None,
        "a second sweep",
        swap.output.sweep(XmrRoleV1::Maker, &recovered),
    )?;
    Ok(Settlement {
        swap,
        recovered: Some(recovered.into_monero_little_endian()),
        from_journal: Some(from_journal.into_monero_little_endian()),
    })
}

/// Refunded: the Taker loses its journal before it authorises, so its
/// withheld claim partial is gone and it can only refund; it refunds in
/// [R, P) from Stage B and its share. The Maker rebuilds the key from its
/// journal, restarts without it, rebuilds the same key from Stage A, Stage
/// B, its share and tag 16, and sweeps its Monero back.
///
/// # Errors
///
/// Fails on any step the protocol must accept, or any refusal it must make.
pub fn refunded(seed: &str, narrator: &mut Narrator) -> Result<Settlement> {
    let mut swap = Swap::lock_both(Terms::new(2, 100_000_000_000, 250), seed, narrator)?;
    swap.taker.lose_session_state(narrator, Some(100));
    let attempt = swap.taker_authorizes_claim(400, narrator);
    narrator.refused(
        Some(400),
        "tag 14 from a Taker without its journal",
        attempt,
    )?;
    narrator.refused(
        Some(700),
        "tag 16 before R",
        swap.escrow.refund([0; 64], 700),
    )?;
    let maker_account = swap.maker.lez_account;
    narrator.refused(
        Some(700),
        "tag 17 before P",
        swap.escrow.punish(maker_account, 700),
    )?;
    swap.taker_refunds_from_stage_b(750, narrator)?;
    narrator.refused(
        Some(1_010),
        "tag 17 after the refund",
        swap.escrow.punish(maker_account, 1_010),
    )?;
    settle_refund_for_the_maker(swap, narrator)
}

/// Refunded after authorising: tag 14 is on chain but the Maker does not
/// claim before R; tag 15 is refused from R, the Taker refunds from
/// `XmrClaimAuthorized`, and the Maker, after a restart, rebuilds the key
/// from tag 16 and sweeps back.
///
/// # Errors
///
/// Fails on any step the protocol must accept, or any refusal it must make.
pub fn refunded_after_authorisation(seed: &str, narrator: &mut Narrator) -> Result<Settlement> {
    let mut swap = Swap::lock_both(Terms::new(3, 150_000_000_000, 300), seed, narrator)?;
    swap.taker_authorizes_claim(400, narrator)?;
    narrator.say(Some(400), "Maker: down from here until after R");
    narrator.refused(
        Some(REFUND_S),
        "the Maker's tag 15 at R",
        swap.maker_claims_late(REFUND_S),
    )?;
    swap.taker_refunds_from_stage_b(760, narrator)?;
    settle_refund_for_the_maker(swap, narrator)
}

impl Swap {
    /// A tag 15 the Maker builds from its journal, sent at `now_s`.
    fn maker_claims_late(&mut self, now_s: u64) -> Result<()> {
        let published = self.escrow.claim_partial.ok_or("no tag 14 on chain")?;
        let share = self.maker.share()?.adaptor_scalar_big_endian();
        let context = self
            .maker
            .agreement()?
            .claim_session_descriptor()
            .context()?;
        let journal = self.maker.journal_mut()?;
        journal.claim.accept_peer_partial_signature(published)?;
        let presignature = journal.claim.presignature()?;
        let signature = adapt_presignature(&context, presignature, share)?;
        self.escrow.claim(signature, now_s)
    }
}

fn settle_refund_for_the_maker(mut swap: Swap, narrator: &mut Narrator) -> Result<Settlement> {
    let from_journal = swap.maker_key_from_journal()?;
    narrator.say(
        None,
        &format!(
            "Maker (journal): Taker's share extracted from tag 16 and checked against its DLEQ proof; key opens {}",
            short(&from_journal.public_key())
        ),
    );
    swap.maker.lose_session_state(narrator, None);
    let recovered = swap.maker_recovers_refund_key()?;
    check(
        recovered.public_key() == from_journal.public_key(),
        "the recovered key is not the journal's",
    )?;
    narrator.say(
        None,
        &format!(
            "Maker (recovered): Taker's share extracted from tag 16 with Stage B's refund presignature and \
             checked against its DLEQ proof; key opens {}",
            short(&recovered.public_key())
        ),
    );
    swap.recovery_refusals(narrator)?;
    swap.output.sweep(XmrRoleV1::Maker, &recovered)?;
    narrator.say(None, "Maker: swept its Monero back with the recovered key");
    Ok(Settlement {
        swap,
        recovered: Some(recovered.into_monero_little_endian()),
        from_journal: Some(from_journal.into_monero_little_endian()),
    })
}

/// Punished: the Taker neither authorises nor refunds before P. Tag 16 is
/// refused from P, the Maker takes the LEZ with tag 17, and no signature on
/// chain reveals a share: neither role can rebuild the key, and the Monero
/// stays at the shared address.
///
/// # Errors
///
/// Fails on any step the protocol must accept, or any refusal it must make.
pub fn punished(seed: &str, narrator: &mut Narrator) -> Result<Settlement> {
    let mut swap = Swap::lock_both(Terms::new(4, 120_000_000_000, 200), seed, narrator)?;
    narrator.say(Some(90), "Taker: silent from here");
    let (taker_account, maker_account) = (swap.taker.lez_account, swap.maker.lez_account);
    narrator.refused(
        Some(PUNISH_S),
        "tag 17 by the Taker owner",
        swap.escrow.punish(taker_account, PUNISH_S),
    )?;
    let (agreement, activated) = swap.taker.restored()?;
    let context = agreement.refund_session_descriptor().context()?;
    let late_refund = adapt_presignature(
        &context,
        activated.body().refund_presignature(),
        swap.taker.share()?.adaptor_scalar_big_endian(),
    )?;
    narrator.refused(
        Some(PUNISH_S),
        "the Taker's tag 16 at P",
        swap.escrow.refund(late_refund, PUNISH_S),
    )?;
    swap.escrow.punish(maker_account, PUNISH_S)?;
    narrator.say(
        Some(PUNISH_S),
        &format!(
            "tag 17: Maker took the LEZ {} at P, escrow {:?}",
            swap.terms.lez_amount, swap.escrow.state
        ),
    );
    narrator.refused(
        None,
        "the Maker's key without a tag 16",
        swap.maker_recovers_refund_key(),
    )?;
    narrator.refused(
        None,
        "the Taker's key without a tag 15",
        swap.taker_recovers_claim_key(),
    )?;
    narrator.say(
        None,
        "no tag 15 or 16 on chain: neither share is revealed, the Monero stays at the shared address",
    );
    Ok(Settlement {
        swap,
        recovered: None,
        from_journal: None,
    })
}

// ------------------------------------------------------------------ helpers

/// `secp256k1` errors without its `std` feature, as this module's errors.
fn secp_ok<T>(result: std::result::Result<T, secp256k1::Error>) -> Result<T> {
    result.map_err(|error| format!("secp256k1: {error}").into())
}

fn check(condition: bool, what: &str) -> Result<()> {
    if condition { Ok(()) } else { Err(what.into()) }
}

fn tagged_hash(tag: &[u8], data: &[u8]) -> [u8; 32] {
    Sha256::new()
        .chain_update(b"lez-xmr-swap-sdk-full-lifecycle-example\0")
        .chain_update(tag)
        .chain_update([0])
        .chain_update(data)
        .finalize()
        .into()
}

#[must_use]
pub const fn at_ms(offset_s: u64) -> u64 {
    (ANCHOR_S + offset_s) * 1_000
}

#[must_use]
pub const fn role_name(role: XmrRoleV1) -> &'static str {
    match role {
        XmrRoleV1::Maker => "Maker",
        XmrRoleV1::Taker => "Taker",
    }
}

const fn role_tag(role: XmrRoleV1) -> &'static [u8] {
    match role {
        XmrRoleV1::Maker => b"maker",
        XmrRoleV1::Taker => b"taker",
    }
}

fn secp_secret(rng: &mut ChaCha20Rng) -> Zeroizing<[u8; 32]> {
    loop {
        let mut bytes = Zeroizing::new([0_u8; 32]);
        rng.fill_bytes(&mut *bytes);
        if SecretKey::from_slice(&*bytes).is_ok() {
            return bytes;
        }
    }
}

/// A canonical scalar below 2^252, valid on both curves.
fn cross_curve_scalar(rng: &mut ChaCha20Rng) -> Zeroizing<[u8; 32]> {
    loop {
        let mut bytes = Zeroizing::new([0_u8; 32]);
        rng.fill_bytes(&mut *bytes);
        bytes[31] &= 0x0f;
        if CrossCurveScalar::from_monero_little_endian(*bytes).is_ok() {
            return bytes;
        }
    }
}

fn public_key(secret: &Zeroizing<[u8; 32]>) -> Result<[u8; 33]> {
    Ok(PublicKey::from_secret_key(
        &Secp256k1::new(),
        &secp_ok(SecretKey::from_slice(&**secret))?,
    )
    .serialize())
}

/// BIP-340 over a Stage-A or Stage-B commitment.
fn sign_commitment(key: &Zeroizing<[u8; 32]>, commitment: [u8; 32]) -> Result<[u8; 64]> {
    let secp = Secp256k1::new();
    let mut keypair = secp_ok(Keypair::from_seckey_slice(&secp, &**key))?;
    let signature = secp
        .sign_schnorr_no_aux_rand(&Message::from_digest(commitment), &keypair)
        .serialize();
    keypair.non_secure_erase();
    Ok(signature)
}

#[must_use]
pub fn short(bytes: &[u8]) -> String {
    format!("{}…", hex::encode(&bytes[..8]))
}

fn short_str(text: &str) -> String {
    format!("{}…", text.chars().take(16).collect::<String>())
}
