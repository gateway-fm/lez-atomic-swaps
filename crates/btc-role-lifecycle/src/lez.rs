//! LEZ side: account derivations, witnessed-escrow terms, and the role
//! sidecar the Node prepares escrows and claims through.

use std::path::Path;

/// Upper bound for a sidecar capability file.
const MAX_CAPABILITY_BYTES: usize = 4096;

use anyhow::{Context as _, Result, ensure};
use lez_bridge_client::{BridgeClient, BridgeClientConfig, BridgeClientError, SidecarCapability};
use lez_bridge_protocol::{
    ErrorCode, Hex32, MessageContext, ObserveFinalizedClockRequest, PrepareWitnessedClaimRequest,
    PrepareWitnessedClaimResult, PrepareWitnessedEscrowRequest, PrepareWitnessedEscrowResult,
    RequestId, RunId, RuntimeDescriptor, SchemaVersion, TransactionId, WitnessedNativeEscrowTerms,
    WitnessedNativeEscrowTermsInput,
};
use lez_btc_swap_sdk::BtcAgreementV1;
use lez_swap_core::Participant;
use lez_zec_swap_sdk::{
    derive_lez_metadata_account_v1, derive_lez_native_custody_account_v1,
    derive_lez_public_account_v0_2,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use sha2::Digest as _;
use zeroize::Zeroizing;

use crate::config::{BtcRoleRuntime, bridge_participant};

/// Which side of the LEZ escrow a role takes for one direction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LezRole {
    Depositor,
    Claimant,
}

/// Reads a 64-character lowercase hex scalar from an owner-private file.
///
/// # Errors
///
/// Fails when the file is missing, not owner-private, or not a valid scalar.
pub fn read_hex_secret(path: &Path) -> Result<Zeroizing<[u8; 32]>> {
    let bytes = Zeroizing::new(crate::layout::read_private(path, 65)?);
    let encoded = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
    ensure!(
        encoded.len() == 64
            && encoded
                .iter()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "{} must hold 64 lowercase hex characters",
        path.display()
    );
    let mut secret = Zeroizing::new([0_u8; 32]);
    hex::decode_to_slice(encoded, secret.as_mut())?;
    let _ = SecretKey::from_slice(secret.as_ref()).context("invalid secp256k1 scalar")?;
    Ok(secret)
}

/// The official LEZ v0.2 public account of a signer key.
///
/// # Errors
///
/// Fails on an invalid scalar.
pub fn signer_account(secret: &Zeroizing<[u8; 32]>) -> Result<[u8; 32]> {
    let key = SecretKey::from_slice(secret.as_ref()).context("invalid LEZ signer scalar")?;
    let x_only = PublicKey::from_secret_key(&Secp256k1::signing_only(), &key)
        .x_only_public_key()
        .0
        .serialize();
    Ok(derive_lez_public_account_v0_2(&x_only))
}

/// The LEZ account that holds the aggregate (both roles) authority.
#[must_use]
pub fn aggregate_authority_account(aggregate_x_only: &[u8; 32]) -> [u8; 32] {
    derive_lez_public_account_v0_2(aggregate_x_only)
}

/// The escrow program's metadata and native custody accounts for a swap.
#[must_use]
pub fn escrow_accounts(escrow_program_id: &[u8; 32], swap_id: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    let words = program_id_words(escrow_program_id);
    (
        derive_lez_metadata_account_v1(&words, swap_id),
        derive_lez_native_custody_account_v1(&words, swap_id),
    )
}

/// The runtime's little-endian word view of a 32-byte program id.
#[must_use]
pub fn program_id_words(bytes: &[u8; 32]) -> [u32; 8] {
    let mut words = [0_u32; 8];
    for (word, chunk) in words.iter_mut().zip(bytes.chunks_exact(4)) {
        let mut le = [0_u8; 4];
        le.copy_from_slice(chunk);
        *word = u32::from_le_bytes(le);
    }
    words
}

/// The nonzero funding id a claim is prepared against before the escrow
/// exists. The sidecar's claim message binds the escrow accounts, the
/// claimant and the authority nonce, never the funding id, and the prepared
/// claim carries no funding id, so the placeholder never reaches the actor.
#[must_use]
pub fn planning_escrow_funding_placeholder(swap_id: &[u8; 32]) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"lez-atomic-swaps/planning-escrow-funding/v1\0");
    hasher.update(swap_id);
    hasher.finalize().into()
}

/// The facts both roles agree on before the agreement exists.
#[derive(Clone, Debug)]
pub struct PlanningTermsInput {
    pub swap_id: [u8; 32],
    /// Placeholder binding until the agreement commitment exists.
    pub terms_hash: [u8; 32],
    pub depositor: Participant,
    pub depositor_account: [u8; 32],
    pub claimant: Participant,
    pub claimant_account: [u8; 32],
    pub aggregate_x_only: [u8; 32],
    pub amount: u128,
    pub refund_at_ms: u64,
    pub authenticated_transfer_program_id: [u8; 32],
}

/// Witnessed-escrow terms for the planning round.
///
/// # Errors
///
/// Fails when the roles or accounts collide or an amount is zero.
pub fn planning_terms(input: &PlanningTermsInput) -> Result<WitnessedNativeEscrowTerms> {
    WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
        swap_id: Hex32::from_bytes(input.swap_id),
        terms_hash: Hex32::from_bytes(input.terms_hash),
        depositor: bridge_participant(input.depositor),
        depositor_account_id: Hex32::from_bytes(input.depositor_account),
        claimant: bridge_participant(input.claimant),
        claimant_account_id: Hex32::from_bytes(aggregate_or(input.claimant_account)),
        aggregate_authority_account_id: Hex32::from_bytes(aggregate_authority_account(
            &input.aggregate_x_only,
        )),
        aggregate_x_only_public_key: Hex32::from_bytes(input.aggregate_x_only),
        amount: input.amount,
        refund_at_ms: input.refund_at_ms,
        authenticated_transfer_program_id: Hex32::from_bytes(
            input.authenticated_transfer_program_id,
        ),
    })
    .context("witnessed escrow planning terms")
}

const fn aggregate_or(account: [u8; 32]) -> [u8; 32] {
    account
}

/// The exact terms the actor rebuilds from a countersigned agreement.
///
/// # Errors
///
/// Fails when the agreement's LEZ terms are internally inconsistent.
pub fn agreement_terms(agreement: &BtcAgreementV1) -> Result<WitnessedNativeEscrowTerms> {
    let signed = agreement.lez_terms();
    WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
        swap_id: Hex32::from_bytes(*agreement.body().swap_id()),
        terms_hash: Hex32::from_bytes(*agreement.agreement_commitment()),
        depositor: bridge_participant(agreement.lez_depositor()),
        depositor_account_id: Hex32::from_bytes(*signed.depositor_account()),
        claimant: bridge_participant(agreement.lez_claimant()),
        claimant_account_id: Hex32::from_bytes(*signed.claimant_account()),
        aggregate_authority_account_id: Hex32::from_bytes(*signed.aggregate_authority_account()),
        aggregate_x_only_public_key: Hex32::from_bytes(
            agreement.p2tr_contract().aggregate_internal_key_bytes(),
        ),
        amount: signed.amount(),
        refund_at_ms: signed.refund_at_ms(),
        authenticated_transfer_program_id: Hex32::from_bytes(
            *signed.authenticated_transfer_program_id(),
        ),
    })
    .context("witnessed escrow terms from agreement")
}

/// A prepared escrow: the exact request the sidecar answered and its result.
#[derive(Clone, Debug)]
pub struct PreparedEscrow {
    pub request: PrepareWitnessedEscrowRequest,
    pub result: PrepareWitnessedEscrowResult,
}

impl PreparedEscrow {
    #[must_use]
    pub fn funding_transaction_id(&self) -> [u8; 32] {
        *self.result.funding.transaction_id.as_bytes()
    }
}

/// This role's LEZ sidecar, bound to one run id and runtime descriptor.
pub struct LezSidecar {
    client: BridgeClient,
    run_id: RunId,
    runtime: RuntimeDescriptor,
}

impl std::fmt::Debug for LezSidecar {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LezSidecar")
            .field("run_id", &self.run_id)
            .field("sidecar_role", &self.runtime.sidecar_role)
            .finish_non_exhaustive()
    }
}

impl LezSidecar {
    /// Connects to one swap's sidecar at `endpoint` with its capability.
    ///
    /// # Errors
    ///
    /// Fails when the capability file is unreadable or the client config is
    /// invalid. Performs no network I/O.
    pub fn connect_to(
        runtime: &BtcRoleRuntime,
        endpoint: &str,
        capability_file: &Path,
        run_id: RunId,
    ) -> Result<Self> {
        let capability_bytes = crate::layout::read_private(capability_file, MAX_CAPABILITY_BYTES)?;
        let capability =
            SidecarCapability::new(String::from_utf8(capability_bytes)?.trim().to_owned())
                .context("sidecar capability")?;
        let descriptor = runtime.runtime_descriptor();
        let client = BridgeClient::connect(BridgeClientConfig::new(
            endpoint.to_owned(),
            capability,
            run_id.clone(),
            descriptor.clone(),
            runtime.request_timeout(),
        ))
        .context("connect LEZ sidecar client")?;
        Ok(Self {
            client,
            run_id,
            runtime: descriptor,
        })
    }

    pub const fn runtime(&self) -> &RuntimeDescriptor {
        &self.runtime
    }

    fn context(&self) -> Result<MessageContext> {
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|_| anyhow::anyhow!("OS randomness unavailable"))?;
        Ok(MessageContext {
            schema_version: SchemaVersion::current(),
            run_id: self.run_id.clone(),
            request_id: RequestId::new(hex::encode(random)).context("request id")?,
            sidecar_role: self.runtime.sidecar_role,
        })
    }

    /// The finalized LEZ tip height, used as the actor's discovery start.
    ///
    /// The indexer answers `moving_tip` when finality advances between its
    /// two reads; that is a race, not a fault, so the read is repeated a
    /// bounded number of times before it fails the take.
    ///
    /// # Errors
    ///
    /// Fails when the sidecar is unreachable, keeps reporting a moving tip, or
    /// answers for another runtime.
    pub async fn finalized_height(&self) -> Result<u64> {
        const MOVING_TIP_ATTEMPTS: usize = 5;
        for attempt in 1..=MOVING_TIP_ATTEMPTS {
            let observed = self
                .client
                .observe_finalized_clock(ObserveFinalizedClockRequest {
                    context: self.context()?,
                    runtime: self.runtime.clone(),
                })
                .await;
            match observed {
                Ok(result) => return Ok(result.clock.height),
                Err(BridgeClientError::Remote(remote))
                    if remote.code() == ErrorCode::MovingTip && attempt < MOVING_TIP_ATTEMPTS =>
                {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
                Err(error) => return Err(error).context("observe finalized LEZ clock"),
            }
        }
        unreachable!("the loop returns on success or on its last attempt")
    }

    /// Prepares (does not submit) the depositor's escrow initialization and
    /// funding transactions for `terms`.
    ///
    /// # Errors
    ///
    /// Fails when the sidecar refuses the terms or is unreachable.
    pub async fn prepare_escrow(
        &self,
        terms: WitnessedNativeEscrowTerms,
    ) -> Result<PreparedEscrow> {
        let request = PrepareWitnessedEscrowRequest {
            context: self.context()?,
            runtime: self.runtime.clone(),
            terms,
        };
        let result = self
            .client
            .prepare_witnessed_escrow(request.clone())
            .await
            .context("prepare witnessed LEZ escrow")?;
        ensure!(
            result.context == request.context,
            "sidecar answered another request"
        );
        Ok(PreparedEscrow { request, result })
    }

    /// Prepares the claimant's witnessed claim message for `terms`.
    ///
    /// # Errors
    ///
    /// Fails when the sidecar refuses the terms or is unreachable.
    pub async fn prepare_claim(
        &self,
        terms: WitnessedNativeEscrowTerms,
        escrow_funding_transaction_id: [u8; 32],
    ) -> Result<PrepareWitnessedClaimResult> {
        let request = PrepareWitnessedClaimRequest {
            context: self.context()?,
            runtime: self.runtime.clone(),
            terms,
            funding_transaction_id: TransactionId::from_bytes(escrow_funding_transaction_id),
        };
        let result = self
            .client
            .prepare_witnessed_claim(request.clone())
            .await
            .context("prepare witnessed LEZ claim")?;
        ensure!(
            result.context == request.context
                && result.claim.preparation_request_id == request.context.request_id,
            "sidecar answered another claim request"
        );
        Ok(result)
    }
}

/// Which escrow side `role` takes for `agreement`'s direction.
#[must_use]
pub fn lez_role(agreement: &BtcAgreementV1, role: Participant) -> LezRole {
    if agreement.lez_depositor() == role {
        LezRole::Depositor
    } else {
        LezRole::Claimant
    }
}
