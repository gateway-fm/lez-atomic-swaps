//! Two LEZ/XMR swaps through the whole lifecycle, for both roles, in one
//! process: one that completes and one that refunds, each settled by a role
//! that has lost its session state and recovers the Monero spend key from
//! what it persisted and the public LEZ signature alone.
//!
//! Run it against the devnet's Monero regtest (lane 1), from the repository
//! root:
//!
//! ```sh
//! (cd deploy && LEZ_MONERO=1 bash scripts/up.sh)        # once; monerod and the funding wallet-rpc
//! python3 deploy/scripts/monero-lane.py fund --role maker  # once on a new stack: creates and funds `funding`
//! cargo run -p xmr-reference-actor --example full-lifecycle
//! ```
//!
//! Environment (defaults in brackets):
//!
//! - `LEZ_XMR_EXAMPLE_NETWORK` \[`regtest`\]: `regtest` runs the Monero leg
//!   on the devnet's regtest and mines its confirmations; `stagenet` runs it
//!   on Monero Stagenet under the `PublicStagenet` profile: it funds the
//!   shared address and sweeps it with transactions that Stagenet's miners
//!   confirm, waits for the ten confirmations of each, and sizes the modelled
//!   windows for that profile (C, R, P = +9.6 h, +21.6 h, +30 h, the `fast`
//!   windows times 108). `docs/monero-stagenet-guide.md` sets it up.
//! - `LEZ_XMR_EXAMPLE_MONERO` \[`live`\]: `live` runs the Monero leg and stops
//!   with an error when monerod does not answer; `off` runs everything else
//!   and says on every Monero step that it was skipped.
//! - `LEZ_XMR_MONEROD_URL` \[`http://127.0.0.1:18081/`\] and
//!   `LEZ_XMR_FUNDING_WALLET_URL` \[`http://127.0.0.1:18083/`\]: monerod and
//!   the funding wallet-rpc as `deploy/` publishes them (lane k:
//!   `18081 + 10(k-1)` and `18083 + 10(k-1)`). Literal loopback only.
//! - `LEZ_XMR_MONERO_LOGINS` \[`deploy/runtime/monero`\]: the directory with
//!   the Digest logins `daemon.login` and `funding.login` (`user:password`)
//!   that `deploy/scripts/gen-config.sh` mints (lane k:
//!   `deploy/runtime/lanes/<k>/monero`).
//! - `LEZ_XMR_FUNDING_WALLET` \[`funding`\]: the wallet the funding wallet-rpc
//!   opens; the example leaves it open again when it ends.
//! - `LEZ_XMR_FUNDING_WALLET_PASSWORD_FILE` \[none: no password\]: a file
//!   holding that wallet's password.
//! - `LEZ_XMR_EXAMPLE_SEED` \[`full-lifecycle` on regtest; none on
//!   Stagenet\]: the seed of every key. On Stagenet it must be your own
//!   random secret (`openssl rand -hex 32`): the run refuses the default.
//!
//! What runs for real, on the types the Nodes run (`xmr-reference-actor`,
//! `lez-adaptor-role-runner`, `lez-xmr-swap-sdk`, `lez-xmr-monero-adapter`):
//! both roles' cross-curve DLEQ proofs (proved and verified), the shared
//! Monero address, Stage A composed by each role and countersigned, both
//! `MuSig2` adaptor sessions in each role's `SQLite` adaptor journal with the
//! Taker's claim partial withheld (R3 to R6), Stage B, the Maker's claim
//! completion from the published Taker partial, the Taker's refund
//! completion, secret extraction and `ReconstructedMoneroSpendKey`. On the
//! Monero regtest: the Maker's funding of the shared address from the funding
//! wallet, ten mined confirmations, the Taker's view-only check of the exact
//! output with `MoneroOutputVerifier`, and the sweep with the rebuilt key.
//!
//! What is modelled: the LEZ escrow. Its sidecars run inside the Node
//! containers and are not reachable from the host, so tags 13 to 17 run
//! against a small in-memory escrow that enforces the guest's rules: states
//! `Funded`, `XmrClaimAuthorized`, `Claimed`, `Refunded` and `Punished`, the
//! claim-partial commitment check of tag 14, the BIP340 check of the claim
//! and refund signatures under the per-swap aggregate keys, tags 13 to 15
//! before R, tag 16 in \[R, P), tag 17 from P. Its clock is a model LEZ
//! consensus clock, not wall time; the windows are the `fast` profile's
//! (C, R, P = +320, +720, +1000 s). The LEZ accounts, program ids and the
//! three message hashes are fixed stand-ins for what the Stage-A planner
//! derives from the LEZ transactions. The Chat transport is the function call.
//!
//! The partial-information-loss recovery: after the revealing signature is
//! on chain, the recovering role drops its session state and deletes its
//! adaptor journal. It keeps only what a role root persists: its Monero
//! share, the shared view key, Stage A and Stage B. With the public escrow
//! (the tag-14 claim partial and the tag-15 or tag-16 signature) that is
//! enough for the SDK's `XmrActivatedAgreementV1::recover_claim_spend_key`
//! (the claim pre-signature aggregated again from Stage B's Maker partial and
//! nonces and the published Taker partial) and `recover_refund_spend_key`
//! (the refund pre-signature is in Stage B itself).
//!
//! Keys, shares and the view key come from a seeded `ChaCha20` stream, so the
//! shared addresses and the Stage-A commitments repeat run to run; the
//! `MuSig2` nonces come from the operating system, as they must, so Stage B
//! and the signatures differ each run. A repeated live run funds the same
//! shared address again; the sweep spends exactly the output of this run's
//! amount. No private key byte is printed.

use std::{
    env, fs,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    str::FromStr as _,
    sync::OnceLock,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, anyhow, bail, ensure};
use lez_adaptor_role_runner::{
    CeremonySeat, Role, ValidatedSession, accept_published_peer_partial_and_adapt,
    extract_verified_adaptor_secret, read_final_signature_packet,
};
use lez_adaptor_signature::{adapt_presignature, verify_final_signature};
use lez_swap_store::{AdaptorSessionPhase, SqliteAdaptorSessionJournal};
use lez_xmr_monero_adapter::{
    ExpectedMoneroOutput, LoopbackRpcEndpoint, MoneroAddress, MoneroChainIdentity,
    MoneroChainIdentityAttestor, MoneroEvidenceError, MoneroNetwork, MoneroOutputVerifier,
    MoneroRegtestMining, MoneroRegtestWalletEffects, MoneroTransactionId,
};
use lez_xmr_swap_sdk::{
    CrossCurveDleqProofV1, CrossCurveScalar, MoneroAddressNetworkV1, MoneroPrivateViewKey,
    MoneroSharedAddressV1, ReconstructedMoneroSpendKey, ValidatedXmrActivationBodyV1,
    ValidatedXmrAgreementBodyV1, XmrActivatedAgreementV1, XmrActivationBodyV1, XmrAgreementBodyV1,
    XmrAgreementV1, XmrLezInitializePlanV1, XmrLezLockCandidateV1, XmrLezLockStatusV1,
    XmrLezTermsV1, XmrMessagesV1, XmrMoneroTermsV1, XmrNamedProfileV1, XmrParticipantIdentityV1,
    XmrParticipantsV1, XmrSessionTranscriptV1, XmrSwapDirectionV1, XmrWindowsV1,
};
use rand_chacha::{
    ChaCha20Rng,
    rand_core::{RngCore as _, SeedableRng as _},
};
use secp256k1::{Keypair, Message, PublicKey, Secp256k1, SecretKey, XOnlyPublicKey, schnorr};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

/// The model LEZ clock's anchor (the take), Unix seconds.
const ANCHOR_S: u64 = 1_900_000_000;
/// The `fast` profile's windows from the anchor: C, R and P.
const FUNDING_CUTOFF_S: u64 = 320;
const REFUND_S: u64 = 720;
const PUNISH_S: u64 = 1_000;
/// Stand-ins for what the Stage-A planner reads from the LEZ runtime.
const LEZ_CHANNEL_ID: [u8; 32] = [0x1c; 32];
const LEZ_GENESIS: [u8; 32] = [0x1e; 32];
const ESCROW_PROGRAM_ID: [u32; 8] = [0xe5c0; 8];
const TRANSFER_PROGRAM_ID: [u32; 8] = [0x7a5f; 8];
/// The genesis an offline run commits in place of monerod's.
const OFFLINE_MONERO_GENESIS: [u8; 32] = [0x4d; 32];
/// The escrow guest's tag-14 commitment domain
/// (`compat/lez-v0.2-provisional/escrow/src/lib.rs`).
const CLAIM_PARTIAL_DOMAIN: &[u8] = b"logos.gateway.lez-xmr.claim-partial-commitment.v1\0";
/// The password of the per-swap regtest wallets this example restores.
const EXAMPLE_WALLET_PASSWORD: &str = "lez-full-lifecycle-example";
const ADAPTOR_JOURNAL: &str = "adaptor.sqlite3";

/// The Monero network of the run, from `LEZ_XMR_EXAMPLE_NETWORK`.
static NETWORK: OnceLock<MoneroNetwork> = OnceLock::new();
/// How long the Stagenet run waits for ten confirmations of one transaction.
const STAGENET_CONFIRMATION_WAIT: Duration = Duration::from_hours(3);

fn network() -> MoneroNetwork {
    *NETWORK.get().unwrap_or(&MoneroNetwork::Regtest)
}

/// The model clock's multiplier: 1 on regtest; on Stagenet 108, so that the
/// `fast` windows meet the `PublicStagenet` minimums (R - C >= 12 h,
/// P - R >= 2 h).
fn clock_scale() -> u64 {
    match network() {
        MoneroNetwork::Regtest => 1,
        MoneroNetwork::Stagenet => 108,
    }
}

fn address_network() -> MoneroAddressNetworkV1 {
    match network() {
        MoneroNetwork::Regtest => MoneroAddressNetworkV1::Regtest,
        MoneroNetwork::Stagenet => MoneroAddressNetworkV1::Stagenet,
    }
}

fn named_profile() -> XmrNamedProfileV1 {
    match network() {
        MoneroNetwork::Regtest => XmrNamedProfileV1::AcceleratedRegtest,
        MoneroNetwork::Stagenet => XmrNamedProfileV1::PublicStagenet,
    }
}

/// The seed every key derives from. On Regtest the public default is fine;
/// on Stagenet the example funds a real output whose keys derive from it, so
/// the seed must be the operator's own random secret: with the default,
/// anyone sweeps the funded output first.
fn example_seed(seed: Option<String>, network: MoneroNetwork) -> Result<String> {
    const DEFAULT_SEED: &str = "full-lifecycle";
    match (seed, network) {
        (None, MoneroNetwork::Regtest) => Ok(DEFAULT_SEED.to_owned()),
        (Some(seed), MoneroNetwork::Regtest) => Ok(seed),
        (Some(seed), MoneroNetwork::Stagenet) if seed != DEFAULT_SEED && seed.len() >= 32 => {
            Ok(seed)
        }
        (_, MoneroNetwork::Stagenet) => bail!(
            "on Stagenet LEZ_XMR_EXAMPLE_SEED must be your own random secret of at least 32 \
             characters (openssl rand -hex 32): every key of the run derives from it, and with \
             a known seed anyone can sweep the funded output first"
        ),
    }
}

#[cfg(test)]
mod seed_tests {
    use super::{MoneroNetwork, example_seed};

    #[test]
    fn stagenet_never_runs_on_the_public_default_seed() {
        assert_eq!(
            example_seed(None, MoneroNetwork::Regtest).unwrap(),
            "full-lifecycle"
        );
        for refused in [
            None,
            Some("full-lifecycle".to_owned()),
            Some("short".to_owned()),
        ] {
            assert!(example_seed(refused, MoneroNetwork::Stagenet).is_err());
        }
        let own = "ab".repeat(32);
        assert_eq!(
            example_seed(Some(own.clone()), MoneroNetwork::Stagenet).unwrap(),
            own
        );
    }
}

fn main() -> Result<()> {
    let network = match env::var("LEZ_XMR_EXAMPLE_NETWORK").as_deref() {
        Err(_) | Ok("regtest") => MoneroNetwork::Regtest,
        Ok("stagenet") => MoneroNetwork::Stagenet,
        Ok(other) => bail!("LEZ_XMR_EXAMPLE_NETWORK is `regtest` or `stagenet`, not `{other}`"),
    };
    NETWORK
        .set(network)
        .map_err(|_| anyhow!("the network is set once"))?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> Result<()> {
    let workdir = tempfile::TempDir::new()?;
    let seed = example_seed(env::var("LEZ_XMR_EXAMPLE_SEED").ok(), network())?;
    let monero = Monero::connect().await?;
    let monero_genesis = match &monero {
        Monero::Live(live) => live.identity.genesis_hash(),
        Monero::Offline => OFFLINE_MONERO_GENESIS,
    };
    // Stagenet coins come from a faucet: a tenth of the regtest amounts.
    let piconero = |regtest: u64| match network() {
        MoneroNetwork::Regtest => regtest,
        MoneroNetwork::Stagenet => regtest / 10,
    };
    let swaps = [
        Facts::new(
            1,
            Branch::Claim,
            piconero(200_000_000_000),
            500,
            monero_genesis,
        ),
        Facts::new(
            2,
            Branch::Refund,
            piconero(100_000_000_000),
            250,
            monero_genesis,
        ),
    ];
    if let Monero::Live(live) = &monero {
        live.ensure_funds(swaps.iter().map(|facts| facts.amount_piconero).sum())
            .await?;
    }
    let mut outcomes = Vec::new();
    let root = fs::canonicalize(workdir.path())?;
    for facts in swaps {
        outcomes.push(run_swap(&facts, &seed, &monero, &root).await?);
    }
    if let Monero::Live(live) = &monero {
        live.effects.open_role_wallet().await?;
    }
    println!();
    for outcome in &outcomes {
        println!("{outcome}");
    }
    println!("private_key_bytes_printed=false");
    Ok(())
}

// ------------------------------------------------------------------ the swap

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Branch {
    Claim,
    Refund,
}

struct Facts {
    label: String,
    index: u8,
    branch: Branch,
    swap_id: [u8; 32],
    amount_piconero: u64,
    lez_amount: u128,
    monero_genesis: [u8; 32],
}

impl Facts {
    fn new(
        index: u8,
        branch: Branch,
        amount_piconero: u64,
        lez_amount: u128,
        monero_genesis: [u8; 32],
    ) -> Self {
        let name = match branch {
            Branch::Claim => "claim",
            Branch::Refund => "refund",
        };
        Self {
            label: format!("swap {index} {name}"),
            index,
            branch,
            swap_id: tagged_hash(b"swap-id", &[index]),
            amount_piconero,
            lez_amount,
            monero_genesis,
        }
    }

    fn say(&self, at_s: Option<u64>, line: &str) {
        match at_s {
            Some(at) => println!("[{} | LEZ +{:>6} s] {line}", self.label, at * clock_scale()),
            None => println!("[{} |             ] {line}", self.label),
        }
    }
}

async fn run_swap(facts: &Facts, seed: &str, monero: &Monero, workdir: &Path) -> Result<String> {
    println!();
    facts.say(None, "== begin ==");
    // The Taker makes the per-swap view key and hands it to the Maker in R1;
    // the Maker's material is made around it.
    let mut taker = Party::generate(facts, seed, Role::Taker, None, workdir)?;
    let mut maker = Party::generate(facts, seed, Role::Maker, Some(&taker.durable.view), workdir)?;
    agree_stage_a(facts, &mut maker, &taker)?;
    countersign_stage_a(facts, &mut maker, &mut taker)?;
    run_ceremony(facts, &mut maker, &mut taker)?;

    // Tag 13 and the fund, by the Taker right after the take.
    let plan = taker.activated()?.lez_initialize_plan(taker.agreement()?)?;
    let mut escrow = ModelEscrow::lock(plan, taker.lez_account, at_ms(30))?;
    facts.say(
        Some(30),
        &format!(
            "tag 13 + fund: LEZ {} locked, escrow {:?}",
            facts.lez_amount, escrow.state
        ),
    );
    check_lock_before_cutoff(facts, &maker, &escrow)?;

    let shared = maker.agreement()?.shared_address().clone();
    let funded = monero.fund(facts, &shared).await?;
    monero
        .verify_output(facts, &taker, &shared, funded.as_ref())
        .await?;

    let outcome = match facts.branch {
        Branch::Claim => {
            settle_claim(
                facts,
                &maker,
                &mut taker,
                &mut escrow,
                monero,
                funded.as_ref(),
            )
            .await?
        }
        Branch::Refund => {
            settle_refund(
                facts,
                &mut maker,
                &taker,
                &mut escrow,
                monero,
                funded.as_ref(),
            )
            .await?
        }
    };
    facts.say(None, "== end ==");
    Ok(outcome)
}

// --------------------------------------------------------------- the roles

/// What a role root persists: its Monero share, the shared view key and both
/// stages (`xmr-share.key`, `monero-view.key`, `stage-a.wire`, `stage-b.wire`).
struct Durable {
    share: Zeroizing<[u8; 32]>,
    view: Zeroizing<[u8; 32]>,
    stage_a: Vec<u8>,
    stage_b: Vec<u8>,
}

/// The session keys the ceremony signs with, kept in memory.
struct SessionKeys {
    agreement: Zeroizing<[u8; 32]>,
    claim: Zeroizing<[u8; 32]>,
    refund: Zeroizing<[u8; 32]>,
}

struct Party {
    role: Role,
    lez_account: [u8; 32],
    keys: Option<SessionKeys>,
    durable: Durable,
    packet: PublicPacket,
    journal: PathBuf,
    agreement: Option<XmrAgreementV1>,
    activated: Option<XmrActivatedAgreementV1>,
}

/// The canonical public role packet, as R1 carries it.
#[derive(Clone)]
struct PublicPacket {
    identity: XmrParticipantIdentityV1,
    proof_wire: Vec<u8>,
    public_view_key: [u8; 32],
}

impl Party {
    fn generate(
        facts: &Facts,
        seed: &str,
        role: Role,
        view: Option<&[u8; 32]>,
        workdir: &Path,
    ) -> Result<Self> {
        let tag: &[u8] = match role {
            Role::Maker => b"maker",
            Role::Taker => b"taker",
        };
        let mut rng = ChaCha20Rng::from_seed(tagged_hash(
            seed.as_bytes(),
            &[&[facts.index], tag].concat(),
        ));
        let keys = SessionKeys {
            agreement: secp_secret(&mut rng),
            claim: secp_secret(&mut rng),
            refund: secp_secret(&mut rng),
        };
        let share = monero_scalar(&mut rng);
        let view = view.map_or_else(|| monero_scalar(&mut rng), |key| Zeroizing::new(*key));
        let proof = CrossCurveDleqProofV1::prove(
            &CrossCurveScalar::from_monero_little_endian(*share)?,
            &mut rng,
        )?;
        proof.verify()?;
        let lez_account = tagged_hash(b"lez-owner", &[&[facts.index], tag].concat());
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
        let root = workdir.join(format!("swap-{}-{}", facts.index, role_name(role)));
        // The journal store wants an owner-private, symlink-free directory.
        fs::create_dir(&root)?;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        facts.say(
            None,
            &format!(
                "{}: material from the seeded stream; DLEQ proved and verified, spend share {}",
                role_name(role),
                short(&proof.ed25519_public_key())
            ),
        );
        Ok(Self {
            role,
            lez_account,
            keys: Some(keys),
            durable: Durable {
                share,
                view,
                stage_a: Vec::new(),
                stage_b: Vec::new(),
            },
            packet,
            journal: root.join(ADAPTOR_JOURNAL),
            agreement: None,
            activated: None,
        })
    }

    fn keys(&self) -> Result<&SessionKeys> {
        self.keys.as_ref().context("the session keys are gone")
    }

    fn agreement(&self) -> Result<&XmrAgreementV1> {
        self.agreement.as_ref().context("no Stage A in memory")
    }

    fn activated(&self) -> Result<&XmrActivatedAgreementV1> {
        self.activated.as_ref().context("no Stage B in memory")
    }

    fn view_key(&self) -> Result<MoneroPrivateViewKey> {
        Ok(MoneroPrivateViewKey::from_monero_little_endian(
            *self.durable.view,
        )?)
    }

    fn share(&self) -> Result<CrossCurveScalar> {
        Ok(CrossCurveScalar::from_monero_little_endian(
            *self.durable.share,
        )?)
    }

    fn sessions(&self) -> Result<(ValidatedSession, ValidatedSession)> {
        let agreement = self.agreement()?;
        Ok((
            ValidatedSession::from_untweaked_context(
                agreement.claim_session_descriptor().context()?,
            )?,
            ValidatedSession::from_untweaked_context(
                agreement.refund_session_descriptor().context()?,
            )?,
        ))
    }

    /// A restart that lost everything but the role root's durable files: the
    /// in-memory agreement, Stage B and session keys are dropped and the
    /// adaptor journal is deleted.
    fn lose_session_state(&mut self, facts: &Facts) -> Result<()> {
        self.keys = None;
        self.agreement = None;
        self.activated = None;
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let path = PathBuf::from(format!("{}{suffix}", self.journal.display()));
            if path.exists() {
                fs::remove_file(&path)?;
            }
        }
        ensure!(!self.journal.exists(), "the adaptor journal is still there");
        facts.say(
            None,
            &format!(
                "{}: restarted with its session state lost: no journal, no session keys, no parsed stages; \
                 kept its share, the view key, Stage A ({} B) and Stage B ({} B)",
                role_name(self.role),
                self.durable.stage_a.len(),
                self.durable.stage_b.len()
            ),
        );
        Ok(())
    }
}

// ------------------------------------------------------------ R1, R2: Stage A

/// Both roles compose Stage A from the same facts; the Taker refuses anything
/// but byte equality with the Maker's plan.
fn compose_stage_a(
    facts: &Facts,
    maker: &PublicPacket,
    taker: &PublicPacket,
) -> Result<ValidatedXmrAgreementBodyV1> {
    let maker_proof = CrossCurveDleqProofV1::from_wire_bytes(&maker.proof_wire)?;
    let taker_proof = CrossCurveDleqProofV1::from_wire_bytes(&taker.proof_wire)?;
    ensure!(
        maker.public_view_key == taker.public_view_key,
        "the packets name different view keys"
    );
    let participants = XmrParticipantsV1::new(maker.identity.clone(), taker.identity.clone());
    let claim_key = participants.claim_aggregate_x_only_key()?;
    let refund_key = participants.refund_aggregate_x_only_key()?;
    let shared = MoneroSharedAddressV1::derive_from_public_view_key(
        address_network(),
        &maker_proof,
        &taker_proof,
        taker.public_view_key,
    )?;
    let profile = named_profile();
    let derived = |purpose: &[u8]| tagged_hash(purpose, &facts.swap_id);
    let body = XmrAgreementBodyV1::new(
        XmrSwapDirectionV1::TakerSellsLez,
        profile,
        facts.swap_id,
        participants,
        XmrMoneroTermsV1::new(
            address_network(),
            facts.monero_genesis,
            facts.amount_piconero,
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
            facts.lez_amount,
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

/// R1: the Maker checks the Taker's packet and view key, composes Stage A
/// and signs it.
fn agree_stage_a(facts: &Facts, maker: &mut Party, taker: &Party) -> Result<()> {
    ensure!(
        taker.view_key()?.public_key() == taker.packet.public_view_key,
        "the Taker's view key is not its packet's"
    );
    let validated = compose_stage_a(facts, &maker.packet, &taker.packet)?;
    maker.durable.stage_a = validated.encode_unsigned_wire()?;
    let signature = sign_commitment(&maker.keys()?.agreement, validated.commitment())?;
    maker.durable.stage_a.extend_from_slice(&signature);
    facts.say(
        None,
        &format!(
            "R1 xmr_reserve_v1: Maker composed and signed Stage A {} ({} B unsigned)",
            short(&validated.commitment()),
            maker.durable.stage_a.len() - signature.len()
        ),
    );
    Ok(())
}

/// R2: the Taker recomposes Stage A, requires the Maker's bytes, verifies
/// the Maker's signature and countersigns; the Maker accepts the result.
fn countersign_stage_a(facts: &Facts, maker: &mut Party, taker: &mut Party) -> Result<()> {
    let (unsigned, maker_signature) = maker
        .durable
        .stage_a
        .split_at(maker.durable.stage_a.len() - 64);
    let maker_signature: [u8; 64] = maker_signature.try_into()?;
    let expected = compose_stage_a(facts, &maker.packet, &taker.packet)?;
    ensure!(
        expected.encode_unsigned_wire()? == unsigned,
        "the Maker's Stage A is not the one the Taker composes"
    );
    let taker_signature = sign_commitment(&taker.keys()?.agreement, expected.commitment())?;
    let agreement = expected.attach_signatures(maker_signature, taker_signature)?;
    let wire = agreement.encode_wire()?;
    taker.durable.stage_a.clone_from(&wire);
    taker.agreement = Some(agreement);

    let reserved = ValidatedXmrAgreementBodyV1::from_unsigned_wire(unsigned)?.commitment();
    let accepted = XmrAgreementV1::from_wire(&wire)?;
    ensure!(
        accepted.agreement_commitment() == reserved,
        "the countersigned Stage A is not the one the Maker reserved"
    );
    maker.durable.stage_a = wire;
    let address = accepted.shared_address().address_string();
    maker.agreement = Some(accepted);
    facts.say(
        None,
        &format!(
            "R2 xmr_chat_agreement_v1: Taker recomposed Stage A byte for byte and countersigned; \
             shared 2-of-2 Monero address {}",
            short_str(&address)
        ),
    );
    Ok(())
}

// ------------------------------------------------- R3 to R6: sessions, Stage B

/// Both adaptor sessions in each role's journal, as `ceremony.rs` drives
/// them: the Taker opens every round, its claim partial never leaves it.
#[allow(clippy::too_many_lines)]
fn run_ceremony(facts: &Facts, maker: &mut Party, taker: &mut Party) -> Result<()> {
    let (maker_claim_session, maker_refund_session) = maker.sessions()?;
    let (taker_claim_session, taker_refund_session) = taker.sessions()?;
    let mut maker_claim = CeremonySeat::open(&maker.journal, maker_claim_session, Role::Maker)?;
    let mut maker_refund = CeremonySeat::open(&maker.journal, maker_refund_session, Role::Maker)?;
    let mut taker_claim = CeremonySeat::open(&taker.journal, taker_claim_session, Role::Taker)?;
    let mut taker_refund = CeremonySeat::open(&taker.journal, taker_refund_session, Role::Taker)?;
    let (maker_keys, taker_keys) = (maker.keys()?, taker.keys()?);

    // R3: nonce commitments.
    let taker_commitments = (
        taker_claim.reserve(&taker_keys.claim)?,
        taker_refund.reserve(&taker_keys.refund)?,
    );
    let maker_commitments = (
        maker_claim.reserve(&maker_keys.claim)?,
        maker_refund.reserve(&maker_keys.refund)?,
    );
    maker_claim.accept_commitment(&taker_commitments.0)?;
    maker_refund.accept_commitment(&taker_commitments.1)?;
    taker_claim.accept_commitment(&maker_commitments.0)?;
    taker_refund.accept_commitment(&maker_commitments.1)?;
    facts.say(
        None,
        "R3 xmr_ceremony_commit_v1: nonce commitments of both sessions exchanged",
    );

    // R4: nonces; the Maker signs both partials, the Taker both of its own
    // and keeps the claim partial in its journal.
    let taker_nonces = (taker_claim.reveal_nonce()?, taker_refund.reveal_nonce()?);
    let maker_nonces = (maker_claim.reveal_nonce()?, maker_refund.reveal_nonce()?);
    let maker_partials = (
        maker_claim.accept_nonce_sign(&taker_nonces.0, &maker_keys.claim)?,
        maker_refund.accept_nonce_sign(&taker_nonces.1, &maker_keys.refund)?,
    );
    let withheld_claim_partial =
        taker_claim.accept_nonce_sign(&maker_nonces.0, &taker_keys.claim)?;
    let taker_refund_partial =
        taker_refund.accept_nonce_sign(&maker_nonces.1, &taker_keys.refund)?;
    taker_claim.accept_peer_partial(&maker_partials.0)?;
    taker_refund.accept_peer_partial(&maker_partials.1)?;
    facts.say(
        None,
        "R4 xmr_ceremony_nonce_v1: nonces opened, Maker partials verified; both Taker presignatures aggregated",
    );

    // R5: the Taker composes Stage B from its journal and signs it; the Maker
    // aggregates the refund presignature, validates Stage B and signs.
    let taker_view = taker.view_key()?;
    let stage_b = compose_stage_b(taker.agreement()?, &taker.journal, &taker_view)?;
    let unsigned_stage_b = stage_b.encode_unsigned_wire()?;
    let taker_signature = sign_commitment(&taker_keys.agreement, stage_b.commitment())?;
    maker_refund.accept_peer_partial(&taker_refund_partial)?;
    let maker_presignature = maker_refund.presignature()?;
    let maker_view = maker.view_key()?;
    let validate = || {
        ValidatedXmrActivationBodyV1::from_unsigned_wire(
            maker.agreement()?,
            &unsigned_stage_b,
            &maker_view,
        )
        .map_err(anyhow::Error::from)
    };
    ensure!(
        validate()?.body().refund_presignature() == maker_presignature,
        "the Taker's Stage B is not the Maker's refund session"
    );
    let maker_signature = sign_commitment(&maker_keys.agreement, validate()?.commitment())?;
    validate()?.attach_signatures(maker_signature, taker_signature)?;
    facts.say(
        None,
        &format!(
            "R5 xmr_ceremony_partial_v1: Stage B {} validated and signed by the Maker; refund presignature converged",
            short(&stage_b.commitment())
        ),
    );

    // R6: the Taker activates; the Maker accepts the activated Stage B.
    ensure!(
        stage_b.body().refund_presignature() == maker_presignature,
        "refund presignatures did not converge"
    );
    let wire = stage_b
        .attach_signatures(maker_signature, taker_signature)?
        .encode_wire()?;
    let maker_activated =
        XmrActivatedAgreementV1::from_wire(maker.agreement()?, &wire, &maker_view)?;
    ensure!(maker_activated.body().refund_presignature() == maker_presignature);
    let taker_activated =
        XmrActivatedAgreementV1::from_wire(taker.agreement()?, &wire, &taker_view)?;

    // The withheld partial is in nothing that crossed Chat; Stage B commits it.
    let partial = journal_claim_partial(taker)?;
    for sent in [&taker_refund_partial, &unsigned_stage_b, &wire] {
        ensure!(
            !sent.windows(32).any(|bytes| bytes == partial.as_slice()),
            "the Taker's claim partial crossed Chat"
        );
    }
    taker_activated.verify_published_taker_claim_partial(taker.agreement()?, *partial)?;
    ensure!(!withheld_claim_partial.is_empty());
    ensure!(maker_claim.phase()? == Some(AdaptorSessionPhase::PartialPersisted));
    ensure!(taker_claim.phase()? == Some(AdaptorSessionPhase::PresignatureVerified));
    facts.say(
        None,
        &format!(
            "R6 xmr_chat_activate_v1: Stage B {} B activated by both; the Taker's claim partial stayed in its journal \
             (Maker claim seat at {:?})",
            wire.len(),
            AdaptorSessionPhase::PartialPersisted
        ),
    );
    maker.durable.stage_b.clone_from(&wire);
    taker.durable.stage_b = wire;
    maker.activated = Some(maker_activated);
    taker.activated = Some(taker_activated);
    Ok(())
}

/// What `compose_activation_body` builds from the Taker's completed journal.
fn compose_stage_b(
    agreement: &XmrAgreementV1,
    journal: &Path,
    view: &MoneroPrivateViewKey,
) -> Result<ValidatedXmrActivationBodyV1> {
    let claim = completed_taker_session(journal, &agreement.claim_session_descriptor())?;
    let refund = completed_taker_session(journal, &agreement.refund_session_descriptor())?;
    let body = XmrActivationBodyV1::new(
        agreement.agreement_commitment(),
        agreement.claim_context_binding(),
        claim.transcript,
        claim.maker_partial,
        agreement.claim_partial_context_binding(&claim.transcript, claim.maker_partial)?,
        agreement.commit_taker_claim_partial(
            &claim.transcript,
            claim.maker_partial,
            claim.taker_partial,
        )?,
        agreement.refund_context_binding(),
        refund.transcript,
        refund.maker_partial,
        refund.taker_partial,
        refund.presignature,
    );
    Ok(ValidatedXmrActivationBodyV1::validate(
        agreement, body, view,
    )?)
}

struct CompletedTakerSession {
    transcript: XmrSessionTranscriptV1,
    maker_partial: [u8; 32],
    taker_partial: [u8; 32],
    presignature: [u8; 65],
}

fn completed_taker_session(
    journal: &Path,
    descriptor: &lez_xmr_swap_sdk::XmrAdaptorSessionDescriptorV1,
) -> Result<CompletedTakerSession> {
    let session = ValidatedSession::from_untweaked_context(descriptor.context()?)?;
    let identity = session.identity(Role::Taker);
    let snapshot = SqliteAdaptorSessionJournal::open_existing(journal)?
        .load(identity.session_id())?
        .filter(|snapshot| snapshot.phase() == AdaptorSessionPhase::PresignatureVerified)
        .context("the Taker session is not complete")?;
    let missing = || anyhow!("the Taker journal is incomplete");
    Ok(CompletedTakerSession {
        transcript: XmrSessionTranscriptV1::new(
            *snapshot.peer_commitment().ok_or_else(missing)?.bytes(),
            *snapshot.own_commitment().bytes(),
            *snapshot.peer_public_nonce().ok_or_else(missing)?.bytes(),
            *snapshot.own_public_nonce().ok_or_else(missing)?.bytes(),
        ),
        maker_partial: *snapshot.peer_partial().ok_or_else(missing)?.bytes(),
        taker_partial: *snapshot.own_partial().ok_or_else(missing)?.bytes(),
        presignature: *snapshot.presignature().ok_or_else(missing)?.bytes(),
    })
}

/// The claim partial the Taker withheld (`XmrTakerCeremony::claim_partial`).
fn journal_claim_partial(taker: &Party) -> Result<Zeroizing<[u8; 32]>> {
    let (claim, _) = taker.sessions()?;
    let identity = claim.identity(Role::Taker);
    let snapshot = SqliteAdaptorSessionJournal::open_existing(&taker.journal)?
        .load(identity.session_id())?
        .filter(|snapshot| snapshot.phase() == AdaptorSessionPhase::PresignatureVerified)
        .context("the claim presignature is not verified")?;
    Ok(Zeroizing::new(
        *snapshot
            .own_partial()
            .context("the claim partial is not durable")?
            .bytes(),
    ))
}

/// The Maker funds Monero only on a lock final at a LEZ time no later than C.
fn check_lock_before_cutoff(facts: &Facts, maker: &Party, escrow: &ModelEscrow) -> Result<()> {
    let agreement = maker.agreement()?;
    let plan = maker.activated()?.lez_initialize_plan(agreement)?;
    let candidate = XmrLezLockCandidateV1::new(
        plan.clone(),
        plan.metadata_version(),
        XmrLezLockStatusV1::Funded,
        agreement.body().lez().required_finality_units(),
        escrow.funded_at_ms,
        escrow.funding_transaction,
        tagged_hash(b"lez-block", &escrow.funding_transaction),
    );
    let validated = maker
        .activated()?
        .validate_lez_lock_candidate(agreement, &candidate)?;
    facts.say(
        Some(30),
        &format!(
            "Maker: LEZ lock {} matches Stage B and is final before C (+{} s)",
            short(&validated.funding_transaction_id()),
            FUNDING_CUTOFF_S * clock_scale()
        ),
    );
    Ok(())
}

// ---------------------------------------------------------- the claim branch

async fn settle_claim(
    facts: &Facts,
    maker: &Party,
    taker: &mut Party,
    escrow: &mut ModelEscrow,
    monero: &Monero,
    funded: Option<&Funded>,
) -> Result<String> {
    // Tag 14: the Taker publishes its claim partial; the escrow checks it
    // against the commitment tag 13 stored.
    let partial = journal_claim_partial(taker)?;
    let mut wrong = *partial;
    wrong[31] ^= 1;
    report_refusal(
        facts,
        400,
        "tag 14 with another partial",
        escrow.authorize_claim(taker.lez_account, wrong, at_ms(400)),
    );
    escrow.authorize_claim(taker.lez_account, *partial, at_ms(400))?;
    facts.say(
        Some(400),
        &format!(
            "tag 14: Taker published its claim partial, escrow {:?}",
            escrow.state
        ),
    );

    // Tag 15: the Maker, on a tag 14 final before R - (R - C)/2, aggregates
    // the claim presignature from the published partial and completes it
    // with its Monero share.
    let claim_deadline = REFUND_S - (REFUND_S - FUNDING_CUTOFF_S) / 2;
    ensure!(
        400 < claim_deadline,
        "the tag 14 is past the Maker's claim window"
    );
    let published = escrow.claim_partial.context("no published claim partial")?;
    maker
        .activated()?
        .verify_published_taker_claim_partial(maker.agreement()?, published)?;
    let (claim_session, _) = maker.sessions()?;
    let packet = maker.journal.with_file_name("claim-final-signature.json");
    accept_published_peer_partial_and_adapt(
        &maker.journal,
        &claim_session,
        Role::Maker,
        published,
        maker.share()?.adaptor_scalar_big_endian(),
        &packet,
    )?;
    let claim_signature = read_final_signature_packet(&packet, &claim_session)?;
    escrow.claim(claim_signature, at_ms(450))?;
    facts.say(
        Some(450),
        &format!(
            "tag 15: Maker completed the claim signature with its share; LEZ {} to the Maker, escrow {:?}",
            facts.lez_amount, escrow.state
        ),
    );
    report_refusal(
        facts,
        750,
        "tag 16 after the claim",
        escrow.refund([0; 64], at_ms(750)),
    );

    // The Taker as the Node does it, from its journal.
    let (taker_claim_session, _) = taker.sessions()?;
    let extracted = extract_verified_adaptor_secret(
        &taker.journal,
        &taker_claim_session,
        Role::Taker,
        claim_signature,
    )?;
    let from_journal = ReconstructedMoneroSpendKey::reconstruct(
        taker.agreement()?.shared_address(),
        taker.agreement()?.maker_proof(),
        taker.share()?,
        extracted.into_big_endian_bytes(),
    )?;
    facts.say(
        None,
        &format!(
            "Taker (journal): Maker's share extracted from tag 15; spend key rebuilt, opens {}",
            short(&from_journal.public_key())
        ),
    );
    drop(from_journal);

    // The Taker after losing its session state: role root plus the chain.
    taker.lose_session_state(facts)?;
    let key = recover_claim_key(&taker.durable, &escrow.public_view())?;
    facts.say(
        None,
        &format!(
            "Taker (recovered): claim presignature re-aggregated from Stage B and the tag-14 partial; \
             Maker's share extracted from tag 15, checked against its DLEQ proof; spend key opens {}",
            short(&key.public_key())
        ),
    );
    let view = MoneroPrivateViewKey::from_monero_little_endian(*taker.durable.view)?;
    let shared = XmrAgreementV1::from_wire(&taker.durable.stage_a)?
        .shared_address()
        .clone();
    let sweep = monero
        .sweep(facts, "taker-claim", &shared, key, view, funded)
        .await?;
    Ok(format!(
        "{}: Completed (escrow {:?}); Monero swept by the Taker with the recovered key: {sweep}",
        facts.label, escrow.state
    ))
}

/// The claim key from what the Taker's role root keeps and the public escrow.
fn recover_claim_key(
    durable: &Durable,
    chain: &PublicEscrow,
) -> Result<ReconstructedMoneroSpendKey> {
    let agreement = XmrAgreementV1::from_wire(&durable.stage_a)?;
    let view = MoneroPrivateViewKey::from_monero_little_endian(*durable.view)?;
    let activated = XmrActivatedAgreementV1::from_wire(&agreement, &durable.stage_b, &view)?;
    let taker_partial = chain.claim_partial.context("no tag 14 on chain")?;
    let claim_signature = chain.claim_signature.context("no tag 15 on chain")?;
    // Stage B's Maker partial and nonces with the tag-14 partial give the
    // claim presignature again; tag 15 reveals the Maker's share.
    Ok(activated.recover_claim_spend_key(
        &agreement,
        CrossCurveScalar::from_monero_little_endian(*durable.share)?,
        taker_partial,
        claim_signature,
    )?)
}

// --------------------------------------------------------- the refund branch

async fn settle_refund(
    facts: &Facts,
    maker: &mut Party,
    taker: &Party,
    escrow: &mut ModelEscrow,
    monero: &Monero,
    funded: Option<&Funded>,
) -> Result<String> {
    // The Taker never authorises; nothing settles before R.
    report_refusal(
        facts,
        700,
        "tag 16 before R",
        escrow.refund([0; 64], at_ms(700)),
    );
    report_refusal(
        facts,
        700,
        "tag 17 before P",
        escrow.punish(maker.lez_account, at_ms(700)),
    );

    // Tag 16 in [R, P): the Taker completes Stage B's refund presignature
    // with its Monero share (`xmr-reference-tag16`).
    let (_, refund_session) = taker.sessions()?;
    let presignature =
        CeremonySeat::open_existing(&taker.journal, refund_session.clone(), Role::Taker)?
            .presignature()?;
    ensure!(
        presignature == taker.activated()?.body().refund_presignature(),
        "the Taker's refund presignature differs from Stage B"
    );
    let refund_signature = adapt_presignature(
        refund_session.context(),
        presignature,
        taker.share()?.adaptor_scalar_big_endian(),
    )?;
    verify_final_signature(refund_session.context(), refund_signature)?;
    escrow.refund(refund_signature, at_ms(750))?;
    facts.say(
        Some(750),
        &format!(
            "tag 16: Taker completed the refund signature with its share; LEZ {} back to the Taker, escrow {:?}",
            facts.lez_amount, escrow.state
        ),
    );
    report_refusal(
        facts,
        1_010,
        "tag 17 after the refund",
        escrow.punish(maker.lez_account, at_ms(1_010)),
    );

    // The Maker as the Node does it, from its journal.
    let (_, maker_refund_session) = maker.sessions()?;
    let extracted = extract_verified_adaptor_secret(
        &maker.journal,
        &maker_refund_session,
        Role::Maker,
        refund_signature,
    )?;
    let from_journal = ReconstructedMoneroSpendKey::reconstruct(
        maker.agreement()?.shared_address(),
        maker.agreement()?.taker_proof(),
        maker.share()?,
        extracted.into_big_endian_bytes(),
    )?;
    facts.say(
        None,
        &format!(
            "Maker (journal): Taker's share extracted from tag 16; spend key rebuilt, opens {}",
            short(&from_journal.public_key())
        ),
    );
    drop(from_journal);

    // The Maker after losing its session state: role root plus the chain.
    maker.lose_session_state(facts)?;
    let key = recover_refund_key(&maker.durable, &escrow.public_view())?;
    facts.say(
        None,
        &format!(
            "Maker (recovered): Taker's share extracted from tag 16 with Stage B's refund presignature, \
             checked against its DLEQ proof; spend key opens {}",
            short(&key.public_key())
        ),
    );
    let view = MoneroPrivateViewKey::from_monero_little_endian(*maker.durable.view)?;
    let shared = XmrAgreementV1::from_wire(&maker.durable.stage_a)?
        .shared_address()
        .clone();
    let sweep = monero
        .sweep(facts, "maker-refund", &shared, key, view, funded)
        .await?;
    Ok(format!(
        "{}: Refunded (escrow {:?}); Monero swept back by the Maker with the recovered key: {sweep}",
        facts.label, escrow.state
    ))
}

/// The refund key from what the Maker's role root keeps and the public escrow.
fn recover_refund_key(
    durable: &Durable,
    chain: &PublicEscrow,
) -> Result<ReconstructedMoneroSpendKey> {
    let agreement = XmrAgreementV1::from_wire(&durable.stage_a)?;
    let view = MoneroPrivateViewKey::from_monero_little_endian(*durable.view)?;
    let activated = XmrActivatedAgreementV1::from_wire(&agreement, &durable.stage_b, &view)?;
    let refund_signature = chain.refund_signature.context("no tag 16 on chain")?;
    // Tag 16 completes Stage B's refund presignature: it reveals the Taker's
    // share.
    Ok(activated.recover_refund_spend_key(
        &agreement,
        CrossCurveScalar::from_monero_little_endian(*durable.share)?,
        refund_signature,
    )?)
}

fn report_refusal(facts: &Facts, at_s: u64, what: &str, result: Result<()>) {
    match result {
        Err(error) => facts.say(Some(at_s), &format!("escrow refused {what}: {error}")),
        Ok(()) => panic!("the escrow accepted {what}"),
    }
}

// ---------------------------------------------------------- the LEZ escrow

/// The escrow states tags 13 to 17 move between. The guest records a tag 17
/// as `Claimed`; the model names it `Punished` to tell the branches apart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EscrowState {
    Funded,
    XmrClaimAuthorized,
    Claimed,
    Refunded,
    Punished,
}

/// An in-memory stand-in for the LEZ v0.2 escrow's native-XMR instructions.
struct ModelEscrow {
    plan: XmrLezInitializePlanV1,
    state: EscrowState,
    funded_at_ms: u64,
    funding_transaction: [u8; 32],
    claim_partial: Option<[u8; 32]>,
    claim_signature: Option<[u8; 64]>,
    refund_signature: Option<[u8; 64]>,
}

/// What anyone reads from the escrow account and its transactions.
struct PublicEscrow {
    claim_partial: Option<[u8; 32]>,
    claim_signature: Option<[u8; 64]>,
    refund_signature: Option<[u8; 64]>,
}

impl ModelEscrow {
    /// Tag 13 `InitializeNativeXmr` and the native fund, before R.
    fn lock(plan: XmrLezInitializePlanV1, signer: [u8; 32], now_ms: u64) -> Result<Self> {
        ensure!(
            signer == plan.depositor_account(),
            "tag 13 is the Taker owner's"
        );
        ensure!(
            now_ms < plan.refund_at_ms(),
            "tag 13 is valid only before R"
        );
        ensure!(plan.claim_partial_commitment() != [0; 32] && plan.amount() > 0);
        let funding_transaction = tagged_hash(b"lez-fund-tx", &plan.swap_id());
        Ok(Self {
            plan,
            state: EscrowState::Funded,
            funded_at_ms: now_ms,
            funding_transaction,
            claim_partial: None,
            claim_signature: None,
            refund_signature: None,
        })
    }

    /// Tag 14 `AuthorizeNativeXmrClaim`: Funded to `XmrClaimAuthorized`.
    fn authorize_claim(&mut self, signer: [u8; 32], partial: [u8; 32], now_ms: u64) -> Result<()> {
        ensure!(
            signer == self.plan.depositor_account(),
            "tag 14 is the Taker owner's"
        );
        ensure!(
            now_ms < self.plan.refund_at_ms(),
            "tag 14 is valid only before R"
        );
        ensure!(
            self.state == EscrowState::Funded,
            "the escrow is {:?}, not Funded",
            self.state
        );
        let commitment: [u8; 32] = Sha256::new()
            .chain_update(CLAIM_PARTIAL_DOMAIN)
            .chain_update(self.plan.claim_partial_context_binding())
            .chain_update(partial)
            .finalize()
            .into();
        ensure!(
            partial != [0; 32] && commitment == self.plan.claim_partial_commitment(),
            "the claim partial does not open the stored commitment"
        );
        self.claim_partial = Some(partial);
        self.state = EscrowState::XmrClaimAuthorized;
        Ok(())
    }

    /// Tag 15 `ClaimNativeXmr`: `XmrClaimAuthorized` to Claimed, before R.
    fn claim(&mut self, signature: [u8; 64], now_ms: u64) -> Result<()> {
        ensure!(
            now_ms < self.plan.refund_at_ms(),
            "tag 15 is valid only before R"
        );
        ensure!(
            self.state == EscrowState::XmrClaimAuthorized,
            "the escrow is {:?}, not XmrClaimAuthorized",
            self.state
        );
        verify_bip340(
            self.plan.claim_aggregate_x_only_key(),
            self.plan.claim_message_hash(),
            signature,
        )?;
        self.claim_signature = Some(signature);
        self.state = EscrowState::Claimed;
        Ok(())
    }

    /// Tag 16 `RefundNativeXmr`: Funded or `XmrClaimAuthorized` to Refunded,
    /// in [R, P).
    fn refund(&mut self, signature: [u8; 64], now_ms: u64) -> Result<()> {
        ensure!(
            (self.plan.refund_at_ms()..self.plan.punish_at_ms()).contains(&now_ms),
            "tag 16 is valid only in [R, P)"
        );
        ensure!(
            matches!(
                self.state,
                EscrowState::Funded | EscrowState::XmrClaimAuthorized
            ),
            "the escrow is {:?}",
            self.state
        );
        verify_bip340(
            self.plan.refund_aggregate_x_only_key(),
            self.plan.refund_message_hash(),
            signature,
        )?;
        self.refund_signature = Some(signature);
        self.state = EscrowState::Refunded;
        Ok(())
    }

    /// Tag 17 `PunishNativeXmr`: from P, by the Maker's owner.
    fn punish(&mut self, signer: [u8; 32], now_ms: u64) -> Result<()> {
        ensure!(
            signer == self.plan.claimant_account(),
            "tag 17 is the Maker owner's"
        );
        ensure!(
            now_ms >= self.plan.punish_at_ms(),
            "tag 17 is valid only from P"
        );
        ensure!(
            matches!(
                self.state,
                EscrowState::Funded | EscrowState::XmrClaimAuthorized
            ),
            "the escrow is {:?}",
            self.state
        );
        self.state = EscrowState::Punished;
        Ok(())
    }

    const fn public_view(&self) -> PublicEscrow {
        PublicEscrow {
            claim_partial: self.claim_partial,
            claim_signature: self.claim_signature,
            refund_signature: self.refund_signature,
        }
    }
}

fn verify_bip340(x_only_key: [u8; 32], message: [u8; 32], signature: [u8; 64]) -> Result<()> {
    Secp256k1::verification_only()
        .verify_schnorr(
            &schnorr::Signature::from_slice(&signature)?,
            &Message::from_digest(message),
            &XOnlyPublicKey::from_slice(&x_only_key)?,
        )
        .context("the signature does not verify under the aggregate key")
}

// ------------------------------------------------------------- the Monero leg

enum Monero {
    Live(Box<LiveMonero>),
    Offline,
}

struct LiveMonero {
    daemon: LoopbackRpcEndpoint,
    wallet: LoopbackRpcEndpoint,
    effects: MoneroRegtestWalletEffects,
    attestor: MoneroChainIdentityAttestor,
    identity: MoneroChainIdentity,
    funding_address: MoneroAddress,
    /// Regtest only: this example mines its own confirmations there; on
    /// Stagenet there is no mining capability and it waits for miners.
    mining: Option<MoneroRegtestMining>,
}

struct Funded {
    transaction: MoneroTransactionId,
    restore_height: u64,
}

impl Monero {
    async fn connect() -> Result<Self> {
        let mode = env::var("LEZ_XMR_EXAMPLE_MONERO").unwrap_or_else(|_| "live".into());
        match mode.as_str() {
            "off" => {
                println!(
                    "Monero leg: OFF (LEZ_XMR_EXAMPLE_MONERO=off); no funding, verification or sweep is sent"
                );
                return Ok(Self::Offline);
            }
            "live" => {}
            other => bail!("LEZ_XMR_EXAMPLE_MONERO is `live` or `off`, not `{other}`"),
        }
        let logins = env::var_os("LEZ_XMR_MONERO_LOGINS").map_or_else(
            || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/runtime/monero"),
            PathBuf::from,
        );
        let endpoint =
            |variable: &str, default: &str, login: &str| -> Result<LoopbackRpcEndpoint> {
                let url = env::var(variable).unwrap_or_else(|_| default.into());
                let path = logins.join(login);
                let line = Zeroizing::new(fs::read_to_string(&path).with_context(|| {
                    format!("read {} (set LEZ_XMR_MONERO_LOGINS)", path.display())
                })?);
                let (user, password) = line
                    .trim()
                    .split_once(':')
                    .context("a login is user:password")?;
                Ok(LoopbackRpcEndpoint::new(&url, user, password)?)
            };
        let daemon = endpoint(
            "LEZ_XMR_MONEROD_URL",
            "http://127.0.0.1:18081/",
            "daemon.login",
        )?;
        let wallet = endpoint(
            "LEZ_XMR_FUNDING_WALLET_URL",
            "http://127.0.0.1:18083/",
            "funding.login",
        )?;
        let funding_wallet =
            env::var("LEZ_XMR_FUNDING_WALLET").unwrap_or_else(|_| "funding".into());
        let funding_password = match env::var_os("LEZ_XMR_FUNDING_WALLET_PASSWORD_FILE") {
            Some(path) => Zeroizing::new(
                fs::read_to_string(&path)
                    .context("read LEZ_XMR_FUNDING_WALLET_PASSWORD_FILE")?
                    .trim()
                    .to_owned(),
            ),
            None => Zeroizing::new(String::new()),
        };
        let attestor = MoneroChainIdentityAttestor::new(network(), &daemon)?;
        let identity = attestor.discover().await.with_context(|| {
            format!(
                "monerod at {} does not answer, or serves another network than {:?} \
                 (start it with `LEZ_MONERO=1 bash scripts/up.sh`, \
                 or run with LEZ_XMR_EXAMPLE_MONERO=off)",
                daemon.base_url(),
                network()
            )
        })?;
        if network() == MoneroNetwork::Stagenet {
            attestor
                .verify_network()
                .await
                .context("the daemon's get_info does not name Stagenet")?;
        }
        let effects = MoneroRegtestWalletEffects::new(&daemon, &wallet)?
            .with_role_wallet(funding_wallet, funding_password.to_string())?;
        let funding_address = effects.primary_standard_address().await?;
        let mining = MoneroRegtestMining::where_the_role_mines(identity, &daemon).await?;
        println!(
            "Monero leg: LIVE {:?}; monerod {} (genesis {}, tip {}), funding wallet-rpc {} ({})",
            network(),
            daemon.base_url(),
            short(&identity.genesis_hash()),
            attestor.tip_height().await?,
            wallet.base_url(),
            short_str(&funding_address.to_string())
        );
        Ok(Self::Live(Box::new(LiveMonero {
            daemon,
            wallet,
            effects,
            attestor,
            identity,
            funding_address,
            mining,
        })))
    }

    /// The Maker funds the exact amount to the shared address and mines ten
    /// confirmations.
    async fn fund(&self, facts: &Facts, shared: &MoneroSharedAddressV1) -> Result<Option<Funded>> {
        let Self::Live(live) = self else {
            facts.say(Some(60), "Maker: Monero funding SKIPPED (offline)");
            return Ok(None);
        };
        let restore_height = live.attestor.tip_height().await?;
        let Some(mining) = &live.mining else {
            let transaction = live
                .effects
                .fund_shared_exact_once(shared, facts.amount_piconero)
                .await?;
            facts.say(
                Some(60),
                &format!(
                    "Maker: sent {} piconero to the shared address {} in {} at Stagenet height {restore_height}",
                    facts.amount_piconero,
                    short_str(&shared.address_string()),
                    hex::encode(transaction.0)
                ),
            );
            return Ok(Some(Funded {
                transaction,
                restore_height,
            }));
        };
        let funding = live
            .effects
            .fund_shared_exact_and_confirm(mining, shared, facts.amount_piconero)
            .await?;
        facts.say(
            Some(60),
            &format!(
                "Maker: funded {} piconero to the shared address in {}, mined to height {}",
                funding.amount_piconero(),
                hex::encode(funding.transaction_id().0),
                funding.confirmation_tip_height()
            ),
        );
        Ok(Some(Funded {
            transaction: funding.transaction_id(),
            restore_height,
        }))
    }

    /// The Taker checks the exact output at ten confirmations with a
    /// view-only wallet of the shared address.
    async fn verify_output(
        &self,
        facts: &Facts,
        taker: &Party,
        shared: &MoneroSharedAddressV1,
        funded: Option<&Funded>,
    ) -> Result<()> {
        let (Self::Live(live), Some(funded)) = (self, funded) else {
            facts.say(Some(90), "Taker: Monero output check SKIPPED (offline)");
            return Ok(());
        };
        live.effects
            .restore_shared_view_only(
                shared,
                taker.view_key()?,
                wallet_name(facts, "taker-view", funded),
                EXAMPLE_WALLET_PASSWORD.into(),
                funded.restore_height,
            )
            .await?;
        let verifier = MoneroOutputVerifier::new(live.identity, &live.daemon, &live.wallet)?;
        let expected = ExpectedMoneroOutput::new(
            funded.transaction,
            MoneroAddress::from_str(&shared.address_string())?,
            facts.amount_piconero,
        )?;
        let observed = live
            .await_confirmed(
                facts,
                "Taker: the shared output",
                &verifier,
                &expected,
                || live.effects.refresh_from_height(funded.restore_height),
            )
            .await?;
        facts.say(
            Some(90),
            &format!(
                "Taker: view-only wallet sees the exact output: block {} at height {}, {} confirmations",
                short(&observed.containing_block_hash()),
                observed.containing_block_height(),
                observed.confirmations()
            ),
        );
        Ok(())
    }

    /// Sweeps the shared output with the rebuilt key to the funding wallet.
    async fn sweep(
        &self,
        facts: &Facts,
        purpose: &str,
        shared: &MoneroSharedAddressV1,
        key: ReconstructedMoneroSpendKey,
        view: MoneroPrivateViewKey,
        funded: Option<&Funded>,
    ) -> Result<String> {
        let (Self::Live(live), Some(funded)) = (self, funded) else {
            drop(key);
            facts.say(
                None,
                "Monero sweep SKIPPED (offline); the rebuilt key opens the shared address",
            );
            return Ok("sweep skipped (offline)".into());
        };
        let Some(mining) = &live.mining else {
            return live
                .sweep_on_stagenet(facts, purpose, shared, key, view, funded)
                .await;
        };
        let sweep = live
            .effects
            .restore_shared_and_sweep(
                mining,
                shared,
                key,
                view,
                wallet_name(facts, purpose, funded),
                EXAMPLE_WALLET_PASSWORD.into(),
                funded.restore_height,
                facts.amount_piconero,
                live.funding_address,
                live.funding_address,
            )
            .await?;
        let receipt = format!(
            "tx {} received {} piconero (fee {}), mined to height {}",
            hex::encode(sweep.transaction_id().0),
            sweep.received_amount_piconero(),
            sweep.fee_piconero(),
            sweep.confirmation_tip_height()
        );
        facts.say(
            None,
            &format!("swept the shared output with the rebuilt key: {receipt}"),
        );
        Ok(receipt)
    }
}

impl LiveMonero {
    /// The funding wallet needs unlocked coins for both swaps; regtest
    /// coinbase unlocks after 60 blocks.
    async fn ensure_funds(&self, needed: u64) -> Result<()> {
        let Some(mining) = &self.mining else {
            // Nothing to mine: the wallet's coins come from a faucet.
            self.effects.open_role_wallet().await?;
            let (_, balance, unlocked) = self.effects.role_wallet_balance().await?;
            println!(
                "Monero leg: the Stagenet funding wallet holds {balance} piconero, {unlocked} unlocked; the swaps need {needed} and fees"
            );
            ensure!(
                unlocked > needed,
                "the Stagenet funding wallet needs more than {needed} unlocked piconero"
            );
            return Ok(());
        };
        for _ in 0..4 {
            self.effects.open_role_wallet().await?;
            self.effects.refresh_from_height(0).await?;
            let (_, _, unlocked) = self.effects.role_wallet_balance().await?;
            if unlocked > needed.saturating_add(1_000_000_000_000) {
                return Ok(());
            }
            println!(
                "Monero leg: the funding wallet holds {unlocked} piconero unlocked; mining 70 blocks"
            );
            for _ in 0..7 {
                self.effects
                    .mine_confirmations(mining, self.funding_address)
                    .await?;
            }
        }
        bail!("the funding wallet has too little unlocked Monero after mining")
    }
}

impl LiveMonero {
    /// The exact output at ten confirmations. On regtest they are mined
    /// already; on Stagenet this waits for them, rereading every 30 s.
    async fn await_confirmed<F, Fut>(
        &self,
        facts: &Facts,
        what: &str,
        verifier: &MoneroOutputVerifier,
        expected: &ExpectedMoneroOutput,
        refresh: F,
    ) -> Result<lez_xmr_monero_adapter::VerifiedMoneroOutputObservation>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<
                Output = Result<(), lez_xmr_monero_adapter::MoneroWalletEffectError>,
            >,
    {
        let started = Instant::now();
        let mut last = String::new();
        loop {
            // Through a public node a refresh can outlast the RPC deadline
            // while the wallet-rpc goes on scanning: on Stagenet that is one
            // more wait, not an error.
            if let Err(error) = refresh().await {
                if network() != MoneroNetwork::Stagenet
                    || started.elapsed() >= STAGENET_CONFIRMATION_WAIT
                {
                    return Err(error.into());
                }
                facts.say(
                    None,
                    &format!("{what}: the wallet is still refreshing ({error})"),
                );
                tokio::task::block_in_place(|| std::thread::sleep(Duration::from_secs(30)));
                continue;
            }
            match verifier.verify(expected).await {
                Ok(observed) => return Ok(observed),
                Err(error)
                    if network() == MoneroNetwork::Stagenet
                        && is_pending(&error)
                        && started.elapsed() < STAGENET_CONFIRMATION_WAIT =>
                {
                    let tip = self
                        .attestor
                        .tip_height()
                        .await
                        .map_or_else(|_| "unread".to_owned(), |tip| tip.to_string());
                    let now = format!("{error}");
                    if now != last {
                        facts.say(
                            None,
                            &format!(
                                "{what}: {now} (Stagenet tip {tip}, {} s waited)",
                                started.elapsed().as_secs()
                            ),
                        );
                        last = now;
                    }
                    tokio::task::block_in_place(|| std::thread::sleep(Duration::from_secs(30)));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    /// Sweeps with the rebuilt key, then waits for the sweep's ten
    /// confirmations in the funding wallet.
    async fn sweep_on_stagenet(
        &self,
        facts: &Facts,
        purpose: &str,
        shared: &MoneroSharedAddressV1,
        key: ReconstructedMoneroSpendKey,
        view: MoneroPrivateViewKey,
        funded: &Funded,
    ) -> Result<String> {
        let sweep_height = self.attestor.tip_height().await?;
        let submitted = self
            .effects
            .restore_shared_and_sweep_once(
                shared,
                key,
                view,
                wallet_name(facts, purpose, funded),
                EXAMPLE_WALLET_PASSWORD.into(),
                funded.restore_height,
                facts.amount_piconero,
                self.funding_address,
            )
            .await?;
        facts.say(
            None,
            &format!(
                "swept the shared output with the rebuilt key in {} ({} piconero, fee {}); waiting for its confirmations",
                hex::encode(submitted.transaction_id().0),
                submitted.received_amount_piconero(),
                submitted.fee_piconero()
            ),
        );
        self.effects.open_role_wallet().await?;
        let verifier = MoneroOutputVerifier::new(self.identity, &self.daemon, &self.wallet)?;
        let expected = ExpectedMoneroOutput::new(
            submitted.transaction_id(),
            self.funding_address,
            submitted.received_amount_piconero(),
        )?;
        let observed = self
            .await_confirmed(facts, "the sweep", &verifier, &expected, || {
                self.effects.refresh_from_height(sweep_height)
            })
            .await?;
        let receipt = format!(
            "tx {} received {} piconero (fee {}), in block {} at Stagenet height {}, {} confirmations",
            hex::encode(submitted.transaction_id().0),
            submitted.received_amount_piconero(),
            submitted.fee_piconero(),
            short(&observed.containing_block_hash()),
            observed.containing_block_height(),
            observed.confirmations()
        );
        facts.say(
            None,
            &format!("swept the shared output with the rebuilt key: {receipt}"),
        );
        Ok(receipt)
    }
}

/// An output that is not yet at ten confirmations: in the pool, not yet seen
/// by the wallet, or short of blocks; or a read that did not get through (a
/// public node limits the connections of one address and drops some).
const fn is_pending(error: &MoneroEvidenceError) -> bool {
    matches!(
        error,
        MoneroEvidenceError::Rpc { .. }
            | MoneroEvidenceError::MissingWalletTransfer
            | MoneroEvidenceError::WalletTransferInPool
            | MoneroEvidenceError::OutputNotUnlocked
            | MoneroEvidenceError::DaemonMissedTransaction
            | MoneroEvidenceError::DaemonTransactionInPool
            | MoneroEvidenceError::InsufficientConfirmations
            | MoneroEvidenceError::NonzeroUnlockDistance
            | MoneroEvidenceError::UnstableTip
    )
}

fn wallet_name(facts: &Facts, purpose: &str, funded: &Funded) -> String {
    format!(
        "lez-example-s{}-{purpose}-h{}",
        facts.index, funded.restore_height
    )
}

// ------------------------------------------------------------------ helpers

fn tagged_hash(tag: &[u8], data: &[u8]) -> [u8; 32] {
    Sha256::new()
        .chain_update(b"lez-xmr-full-lifecycle-example\0")
        .chain_update(tag)
        .chain_update([0])
        .chain_update(data)
        .finalize()
        .into()
}

fn at_ms(offset_s: u64) -> u64 {
    (ANCHOR_S + offset_s * clock_scale()) * 1_000
}

const fn role_name(role: Role) -> &'static str {
    match role {
        Role::Maker => "Maker",
        Role::Taker => "Taker",
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
fn monero_scalar(rng: &mut ChaCha20Rng) -> Zeroizing<[u8; 32]> {
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
    Ok(
        PublicKey::from_secret_key(&Secp256k1::new(), &SecretKey::from_slice(&**secret)?)
            .serialize(),
    )
}

/// BIP340 over a Stage-A or Stage-B commitment, as `reserve.rs` signs.
fn sign_commitment(key: &Zeroizing<[u8; 32]>, commitment: [u8; 32]) -> Result<[u8; 64]> {
    let secp = Secp256k1::new();
    let mut keypair = Keypair::from_seckey_slice(&secp, &**key)?;
    let signature = secp
        .sign_schnorr_no_aux_rand(&Message::from_digest(commitment), &keypair)
        .serialize();
    keypair.non_secure_erase();
    Ok(signature)
}

fn short(bytes: &[u8]) -> String {
    format!("{}…", hex::encode(&bytes[..8]))
}

fn short_str(text: &str) -> String {
    format!("{}…", text.chars().take(16).collect::<String>())
}
