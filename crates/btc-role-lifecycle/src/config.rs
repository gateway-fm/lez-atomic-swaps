//! Role configuration: chain endpoints, policy and identity, all from a file.

use std::{
    fs,
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context as _, Result, ensure};
use lez_bridge_protocol::{
    Hex32, Participant as BridgeParticipant, RuntimeCompatibility, RuntimeDescriptor,
};
use lez_btc_swap_sdk::{BtcChainPolicyV1, BtcLezChainIdentityV1};
use lez_swap_core::Participant;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::lez;

/// The Bitcoin network the role settles on. Selected by configuration only.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BitcoinNetworkName {
    Mainnet,
    Testnet4,
    Signet,
    Regtest,
}

impl BitcoinNetworkName {
    #[must_use]
    pub const fn network(self) -> bitcoin::Network {
        match self {
            Self::Mainnet => bitcoin::Network::Bitcoin,
            Self::Testnet4 => bitcoin::Network::Testnet4,
            Self::Signet => bitcoin::Network::Signet,
            Self::Regtest => bitcoin::Network::Regtest,
        }
    }

    /// The actor's connectivity class for this network.
    #[must_use]
    pub const fn actor_connectivity(self) -> &'static str {
        match self {
            Self::Regtest => "isolated_local",
            Self::Mainnet | Self::Testnet4 | Self::Signet => "networked",
        }
    }
}

/// Bitcoin Core access and swap policy.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BitcoinConfigV1 {
    pub network: BitcoinNetworkName,
    /// JSON-RPC endpoint of the node the actor observes (`http://host:port/`).
    pub endpoint: String,
    /// Cookie file (`user:password`) for that endpoint.
    pub cookie_file: PathBuf,
    /// Wallet on the same node that funds the contract when this role funds
    /// Bitcoin. Absent when the role never funds Bitcoin.
    #[serde(default)]
    pub wallet: Option<String>,
    /// Expected genesis block hash as Core displays it (`getblockhash 0`).
    pub genesis_block_hash: String,
    pub required_confirmations: u32,
    pub refund_csv_blocks: u32,
    /// Fee reserved between the contract value and the cooperative claim.
    pub claim_fee_sat: u64,
}

/// LEZ chain identity plus the role's sidecar and signer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LezConfigV1 {
    pub channel_id: String,
    pub genesis_block_hash: String,
    pub escrow_program_id: String,
    pub authenticated_transfer_program_id: String,
    /// The role sidecar program (`lez-v02-bridge-poc`) the Node spawns per swap.
    pub sidecar_program: PathBuf,
    /// Literal-loopback LEZ node endpoints the sidecars talk to.
    pub sequencer_url: String,
    pub indexer_url: String,
    /// Loopback ports the swaps' sidecars may listen on.
    pub sidecar_port_base: u16,
    pub sidecar_port_count: u16,
    /// Hex LEZ signer key; its account is this role's LEZ owner account.
    pub signer_key_file: PathBuf,
    pub request_timeout_millis: u64,
    pub discovery_max_blocks: u32,
}

/// Recovery schedule offsets, in seconds from the reservation time.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryPolicyV1 {
    pub maker_second_lock_cutoff_seconds: u64,
    pub earlier_refund_latest_seconds: u64,
    pub later_refund_earliest_seconds: u64,
    pub required_margin_seconds: u64,
}

/// The actor program this role runs for Bitcoin swaps.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActorProgramV1 {
    pub program: PathBuf,
    pub program_sha256: String,
}

/// One role's Bitcoin-pair configuration file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BtcRoleConfigV1 {
    pub schema_version: u16,
    /// Owner-private directory holding one subdirectory per swap.
    pub swaps_root: PathBuf,
    pub bitcoin: BitcoinConfigV1,
    pub lez: LezConfigV1,
    pub recovery: RecoveryPolicyV1,
    pub actor: ActorProgramV1,
}

/// A loaded, validated role configuration with derived identities.
#[derive(Debug)]
pub struct BtcRoleRuntime {
    role: Participant,
    config: BtcRoleConfigV1,
    bitcoin_policy: BtcChainPolicyV1,
    lez_identity: BtcLezChainIdentityV1,
    lez_owner_account: [u8; 32],
}

impl BtcRoleRuntime {
    /// Loads the configuration file and derives the LEZ owner account from the
    /// signer key. Performs no network I/O.
    ///
    /// # Errors
    ///
    /// Fails on an unreadable or invalid file, a non-private swaps root, or an
    /// invalid signer key.
    pub fn load(role: Participant, path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
        let config: BtcRoleConfigV1 =
            serde_json::from_slice(&bytes).context("parse BTC role configuration")?;
        ensure!(
            config.schema_version == 1,
            "unsupported BTC role configuration schema"
        );
        ensure!(
            config.swaps_root.is_absolute(),
            "swaps_root must be absolute"
        );
        let root = fs::symlink_metadata(&config.swaps_root).context("inspect swaps_root")?;
        ensure!(
            root.is_dir()
                && root.mode().trailing_zeros() >= 6
                && root.uid() == rustix::process::geteuid().as_raw(),
            "swaps_root must be an owner-private directory"
        );
        ensure!(
            config.bitcoin.refund_csv_blocks > 0,
            "refund_csv_blocks must be nonzero"
        );
        ensure!(
            config.bitcoin.required_confirmations > 0,
            "required_confirmations must be nonzero"
        );
        ensure!(
            config.lez.request_timeout_millis > 0,
            "request_timeout_millis must be nonzero"
        );
        // Bitcoin hashes are configured as Core displays them (reversed); the
        // agreement and the actor compare internal byte order.
        let bitcoin_genesis: bitcoin::BlockHash = config
            .bitcoin
            .genesis_block_hash
            .parse()
            .context("Bitcoin genesis block hash")?;
        let bitcoin_policy = BtcChainPolicyV1::new(
            bitcoin::hashes::Hash::to_byte_array(bitcoin_genesis),
            config.bitcoin.required_confirmations,
        );
        let lez_identity = BtcLezChainIdentityV1::new(
            parse_hex32(&config.lez.genesis_block_hash, "LEZ genesis block hash")?,
            parse_hex32(&config.lez.channel_id, "LEZ channel id")?,
            parse_hex32(&config.lez.escrow_program_id, "LEZ escrow program id")?,
            parse_hex32(
                &config.lez.authenticated_transfer_program_id,
                "LEZ authenticated-transfer program id",
            )?,
        );
        let signer = lez::read_hex_secret(&config.lez.signer_key_file).context("LEZ signer key")?;
        let lez_owner_account = lez::signer_account(&signer)?;
        ensure!(
            config.lez.sidecar_port_count > 0,
            "sidecar_port_count must be nonzero"
        );
        ensure!(
            config.lez.sidecar_program.is_absolute(),
            "sidecar_program must be absolute"
        );
        Ok(Self {
            role,
            config,
            bitcoin_policy,
            lez_identity,
            lez_owner_account,
        })
    }

    #[must_use]
    pub const fn role(&self) -> Participant {
        self.role
    }

    #[must_use]
    pub const fn config(&self) -> &BtcRoleConfigV1 {
        &self.config
    }

    #[must_use]
    pub const fn bitcoin_policy(&self) -> &BtcChainPolicyV1 {
        &self.bitcoin_policy
    }

    #[must_use]
    pub const fn lez_identity(&self) -> &BtcLezChainIdentityV1 {
        &self.lez_identity
    }

    #[must_use]
    pub const fn lez_owner_account(&self) -> [u8; 32] {
        self.lez_owner_account
    }

    #[must_use]
    pub fn request_timeout(&self) -> Duration {
        Duration::from_millis(self.config.lez.request_timeout_millis)
    }

    /// The runtime descriptor this role's sidecar was started with.
    pub fn runtime_descriptor(&self) -> RuntimeDescriptor {
        RuntimeDescriptor {
            sidecar_role: bridge_participant(self.role),
            compatibility: RuntimeCompatibility::LeeV0_2_0,
            chain_id: Hex32::from_bytes(*self.lez_identity.channel_id()),
            channel_id: Hex32::from_bytes(*self.lez_identity.channel_id()),
            genesis_block_hash: Hex32::from_bytes(*self.lez_identity.genesis_block_hash()),
            escrow_program_id: Hex32::from_bytes(*self.lez_identity.escrow_program_id()),
            signer_account_id: Hex32::from_bytes(self.lez_owner_account),
        }
    }

    /// Reads the LEZ signer key again (never cached in memory).
    ///
    /// # Errors
    ///
    /// Fails when the key file is unreadable or malformed.
    pub fn lez_signer_key(&self) -> Result<Zeroizing<[u8; 32]>> {
        lez::read_hex_secret(&self.config.lez.signer_key_file)
    }
}

/// Maps a swap participant to the bridge protocol's participant.
pub const fn bridge_participant(role: Participant) -> BridgeParticipant {
    match role {
        Participant::Maker => BridgeParticipant::Maker,
        Participant::Taker => BridgeParticipant::Taker,
    }
}

pub(crate) fn parse_hex32(value: &str, name: &str) -> Result<[u8; 32]> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "{name} must be 64 lowercase hex characters"
    );
    let mut out = [0_u8; 32];
    hex::decode_to_slice(value, &mut out).with_context(|| format!("decode {name}"))?;
    Ok(out)
}
