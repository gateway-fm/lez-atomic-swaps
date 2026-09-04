//! Synthesis of the schema-6 actor configuration from one swap's artifacts,
//! and activation of the actor in-process.

use std::path::Path;

use anyhow::{Context as _, Result, ensure};
use btc_reference_actor::{ActorCommand, ActorConfig, ActorRole, execute_actor_command};
use btc_role_preflight::RoleSecret;
use lez_btc_swap_sdk::BtcAgreementV1;
use lez_swap_core::{Participant, SwapDirection};
use sha2::{Digest as _, Sha256};

use crate::{
    ceremony::LegSessions,
    config::BtcRoleRuntime,
    layout::{SwapLayout, write_private_exact},
    lez::PreparedEscrow,
    sidecar::SwapSidecar,
};

/// The Maker's lock material for its direction.
#[derive(Debug)]
pub enum MakerLockMaterial<'a> {
    /// Maker funds Bitcoin: the exact signed funding transaction (hex).
    Bitcoin { funding_transaction_hex: &'a str },
    /// Maker deposits LEZ: its prepared escrow.
    Lez(&'a PreparedEscrow),
}

/// Everything one role needs to write and validate its actor configuration.
#[derive(Debug)]
pub struct ActorSynthesis<'a> {
    pub runtime: &'a BtcRoleRuntime,
    pub layout: &'a SwapLayout,
    pub agreement: &'a BtcAgreementV1,
    pub agreement_wire: &'a [u8],
    /// The swap's own sidecar the actor observes and submits through.
    pub sidecar: &'a SwapSidecar,
    pub sessions: LegSessions,
    pub accepted_at_unix_seconds: u64,
    pub lez_discovery_start_height: u64,
    /// Bytes of the claimant's `PrepareWitnessedClaimResult` JSON.
    pub prepared_claim_json: &'a [u8],
    pub maker_lock: Option<MakerLockMaterial<'a>>,
}

/// Writes the actor bundle (hex secrets, prepared material, configuration)
/// and reloads the configuration through the actor's own strict loader.
///
/// # Errors
///
/// Fails when required material is missing for the role/direction or the
/// actor rejects the configuration.
#[allow(clippy::too_many_lines)]
pub fn synthesize(input: &ActorSynthesis<'_>) -> Result<ActorConfig> {
    let role = input.runtime.role();
    let agreement = input.agreement;
    let layout = input.layout;
    let role_root = layout.role_root();
    ensure!(
        agreement.participant(role).musig2_public_key()
            == &public_key(&role_root.read_secret(RoleSecret::Agreement)?)?,
        "role root agreement key does not match the agreement"
    );
    let config = input.runtime.config();

    // Secrets the actor reads as hex, copied from the raw role-root scalars.
    let mut refund_key_file = None;
    if agreement.bitcoin_funder() == role {
        let secret = role_root.read_secret(RoleSecret::BitcoinRefund)?;
        write_private_exact(
            &layout.actor_refund_key_file(),
            format!("{}\n", hex::encode(&secret[..])).as_bytes(),
        )?;
        refund_key_file = Some(layout.actor_refund_key_file());
    }
    let mut adaptor_secret_file = None;
    if role == Participant::Taker {
        let secret = role_root.read_secret(RoleSecret::Adaptor)?;
        write_private_exact(
            &layout.actor_adaptor_secret_file(),
            format!("{}\n", hex::encode(&secret[..])).as_bytes(),
        )?;
        adaptor_secret_file = Some(layout.actor_adaptor_secret_file());
    }
    write_private_exact(&layout.prepared_claim_file(), input.prepared_claim_json)?;

    let maker_lock = match (role, agreement.direction(), &input.maker_lock) {
        (Participant::Taker, _, None) => None,
        (
            Participant::Maker,
            SwapDirection::TakerSellsLez,
            Some(MakerLockMaterial::Bitcoin {
                funding_transaction_hex,
            }),
        ) => {
            write_private_exact(
                &layout.funding_transaction_file(),
                format!("{funding_transaction_hex}\n").as_bytes(),
            )?;
            Some(serde_json::json!({
                "chain": "bitcoin",
                "exact_funding_transaction_file": layout.funding_transaction_file(),
            }))
        }
        (
            Participant::Maker,
            SwapDirection::TakerSellsForeign,
            Some(MakerLockMaterial::Lez(escrow)),
        ) => {
            write_private_exact(
                &layout.escrow_request_file(),
                &canonical_json(&escrow.request)?,
            )?;
            write_private_exact(
                &layout.escrow_result_file(),
                &canonical_json(&escrow.result)?,
            )?;
            Some(serde_json::json!({
                "chain": "lez",
                "preparation_request_file": layout.escrow_request_file(),
                "preparation_result_file": layout.escrow_result_file(),
            }))
        }
        _ => anyhow::bail!("maker lock material does not fit the role and direction"),
    };

    let mut value = serde_json::json!({
        "schema_version": 6,
        "role": match role { Participant::Maker => "maker", Participant::Taker => "taker" },
        "agreement_file": role_root.agreement_file(),
        "state_db": layout.actor_state_db(),
        "accepted_at_unix_seconds": input.accepted_at_unix_seconds,
        "agreement_sha256": hex::encode(Sha256::digest(input.agreement_wire)),
        "bitcoin_core": {
            "endpoint": config.bitcoin.endpoint,
            "cookie_file": config.bitcoin.cookie_file,
            "connectivity": config.bitcoin.network.actor_connectivity(),
        },
        "lez_bridge": {
            "endpoint": input.sidecar.endpoint(),
            "capability_file": input.sidecar.capability_file(),
            "run_id": input.sidecar.run_id(),
            "runtime": input.runtime.runtime_descriptor(),
            "request_timeout_millis": config.lez.request_timeout_millis,
            "discovery_start_height": input.lez_discovery_start_height,
            "discovery_max_blocks": config.lez.discovery_max_blocks,
        },
        "signing": {
            "bitcoin": { "session_id": hex::encode(input.sessions.bitcoin), "journal_db": layout.bitcoin_journal() },
            "lez": { "session_id": hex::encode(input.sessions.lez), "journal_db": layout.lez_journal() },
            "prepared_witnessed_claim_result_file": layout.prepared_claim_file(),
        },
        "refund": {},
    });
    if let Some(path) = adaptor_secret_file {
        value["signing"]["adaptor_secret_file"] = serde_json::json!(path);
    }
    if let Some(path) = refund_key_file {
        value["refund"]["bitcoin_refund_key_file"] = serde_json::json!(path);
    }
    if let Some(lock) = maker_lock {
        value["maker_lock"] = lock;
    }
    let mut bytes = serde_json::to_vec_pretty(&value)?;
    bytes.push(b'\n');
    write_private_exact(&layout.actor_config_file(), &bytes)?;
    let loaded = ActorConfig::load_private(layout.actor_config_file()).map_err(|error| {
        anyhow::anyhow!("actor rejected the synthesized configuration: {error}")
    })?;
    ensure!(
        loaded.role()
            == match role {
                Participant::Maker => ActorRole::Maker,
                Participant::Taker => ActorRole::Taker,
            },
        "synthesized actor role mismatch"
    );
    Ok(loaded)
}

/// Activates the actor (validating every piece of signing material); a
/// replay of an already-activated actor is not an error.
///
/// # Errors
///
/// Fails when the actor refuses activation.
pub async fn activate(config: &ActorConfig) -> Result<()> {
    execute_actor_command(config, ActorCommand::Activate)
        .await
        .map(|_| ())
        .map_err(|error| anyhow::anyhow!("actor activation failed: {error:?}"))
}

/// Upper bound for a file the actor bundle references (the actor program is
/// the largest, a few hundred MB unstripped).
const MAX_HASHED_FILE_BYTES: usize = 1024 * 1024 * 1024;

/// SHA-256 of a file the actor bundle references (for receipts).
///
/// # Errors
///
/// Fails when the path is not absolute and normalized, or the file cannot be
/// read as a regular file within its bound.
pub fn file_sha256(path: &Path) -> Result<[u8; 32]> {
    Ok(Sha256::digest(crate::layout::read_vetted(path, MAX_HASHED_FILE_BYTES)?).into())
}

fn canonical_json<T: serde::Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn public_key(secret: &zeroize::Zeroizing<[u8; 32]>) -> Result<[u8; 33]> {
    let key = secp256k1::SecretKey::from_slice(secret.as_ref()).context("agreement key")?;
    Ok(
        secp256k1::PublicKey::from_secret_key(&secp256k1::Secp256k1::signing_only(), &key)
            .serialize(),
    )
}
