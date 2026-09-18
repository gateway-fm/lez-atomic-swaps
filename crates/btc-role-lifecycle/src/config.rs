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
    /// Legacy testnet, which is what keyless public RPC providers serve.
    Testnet3,
    Signet,
    Regtest,
}

impl BitcoinNetworkName {
    #[must_use]
    pub const fn network(self) -> bitcoin::Network {
        match self {
            Self::Mainnet => bitcoin::Network::Bitcoin,
            Self::Testnet4 => bitcoin::Network::Testnet4,
            Self::Testnet3 => bitcoin::Network::Testnet,
            Self::Signet => bitcoin::Network::Signet,
            Self::Regtest => bitcoin::Network::Regtest,
        }
    }

    /// The actor's connectivity class for this network.
    #[must_use]
    pub const fn actor_connectivity(self) -> &'static str {
        match self {
            Self::Regtest => "isolated_local",
            Self::Testnet4 => "testnet4_networked",
            Self::Testnet3 => "testnet3_networked",
            Self::Mainnet | Self::Signet => "networked",
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
    /// Absent, payouts land on a per-swap key Core cannot see or spend.
    /// The two roles must not share an address: the agreement rejects that.
    #[serde(default)]
    pub claim_destination_address: Option<String>,
    /// Expected genesis block hash as Core displays it (`getblockhash 0`).
    pub genesis_block_hash: String,
    pub required_confirmations: u32,
    pub refund_csv_blocks: u32,
    /// Fee reserved between the contract value and the cooperative claim.
    pub claim_fee_sat: u64,
    #[serde(default)]
    pub lock_fee: LockFeePolicyV1,
}

/// What a Bitcoin lock may pay in fees. The lock is the transaction the signed
/// path spends, so it can never be fee-bumped: its fee is decided once, at take
/// time. Left to Core's wallet estimator alone, two 10,000 sat testnet4 locks
/// paid 56,064 sat each during a backlog.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct LockFeePolicyV1 {
    /// Blocks `estimatesmartfee` is asked to confirm within.
    pub confirmation_target: u16,
    /// The rate when the node has no estimate. Runs of empty blocks leave
    /// testnet4's estimator without one, and a lock priced at the relay
    /// minimum then waits out its own cutoff.
    pub fallback_sat_per_vb: u64,
    /// Ceiling on the rate, whatever the estimator says.
    pub max_sat_per_vb: u64,
    /// A lock whose fee exceeds this share of its value is refused.
    pub max_percent_of_value: u64,
}

impl Default for LockFeePolicyV1 {
    fn default() -> Self {
        Self {
            confirmation_target: 6,
            fallback_sat_per_vb: 20,
            max_sat_per_vb: 25,
            max_percent_of_value: 5,
        }
    }
}

impl LockFeePolicyV1 {
    /// The rate to fund at: the estimate, or the fallback when the node has
    /// none, floored at the 1 sat/vB relay minimum and capped.
    #[must_use]
    pub fn rate_sat_per_vb(&self, estimate_btc_per_kvb: Option<f64>) -> u64 {
        // BTC/kvB to sat/vB is a factor of 1e5; a rate is far below 2^53.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let estimate = estimate_btc_per_kvb
            .filter(|rate| rate.is_finite() && *rate > 0.0)
            .map_or(self.fallback_sat_per_vb, |rate| (rate * 1e5).ceil() as u64);
        estimate.clamp(1, self.max_sat_per_vb.max(1))
    }

    /// Whether `fee_sat` is an acceptable price for locking `value_sat`.
    #[must_use]
    pub fn admits(&self, fee_sat: u64, value_sat: u64) -> bool {
        u128::from(fee_sat) * 100 <= u128::from(value_sat) * u128::from(self.max_percent_of_value)
    }
}

/// Which LEZ network the identity below belongs to. LEZ names a chain by its
/// channel and genesis, which say nothing about what is at stake on it, so the
/// class is stated -- and a Node refuses to pair real money with a test chain.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LezNetworkName {
    Mainnet,
    Testnet,
    /// A private local chain; what a configuration written before this field means.
    #[default]
    Devnet,
}

/// LEZ chain identity plus the role's sidecar and signer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LezConfigV1 {
    #[serde(default)]
    pub network: LezNetworkName,
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

/// Upper bound for the role configuration file.
const MAX_CONFIG_BYTES: usize = 64 * 1024;

/// A loaded, validated role configuration with derived identities.
#[derive(Debug)]
pub struct BtcRoleRuntime {
    role: Participant,
    config: BtcRoleConfigV1,
    bitcoin_policy: BtcChainPolicyV1,
    lez_identity: BtcLezChainIdentityV1,
    lez_owner_account: [u8; 32],
    bitcoin_claim_destination: Option<Vec<u8>>,
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
        crate::layout::vet_configured_path(path, "BTC role configuration")?;
        let bytes = crate::layout::read_private(path, MAX_CONFIG_BYTES)?;
        let config: BtcRoleConfigV1 =
            serde_json::from_slice(&bytes).context("parse BTC role configuration")?;
        ensure!(
            config.schema_version == 1,
            "unsupported BTC role configuration schema"
        );
        // Every file the configuration names is opened by that exact name;
        // reject anything relative or tree-walking before it is ever used.
        for (label, configured) in [
            ("swaps_root", &config.swaps_root),
            ("bitcoin.cookie_file", &config.bitcoin.cookie_file),
            ("lez.sidecar_program", &config.lez.sidecar_program),
            ("lez.signer_key_file", &config.lez.signer_key_file),
            ("actor.program", &config.actor.program),
        ] {
            crate::layout::vet_configured_path(configured, label)?;
        }
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
            config.bitcoin.lock_fee.confirmation_target > 0
                && config.bitcoin.lock_fee.fallback_sat_per_vb > 0
                && config.bitcoin.lock_fee.max_sat_per_vb > 0
                && config.bitcoin.lock_fee.max_percent_of_value > 0,
            "lock_fee bounds must be nonzero"
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
        ensure_networks_agree(&config, bitcoin_genesis)?;
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
        // Catch a wrong-network address here, not inside a signed agreement.
        let bitcoin_claim_destination = config
            .bitcoin
            .claim_destination_address
            .as_deref()
            .map(|address| {
                let parsed = address
                    .parse::<bitcoin::Address<bitcoin::address::NetworkUnchecked>>()
                    .context("bitcoin.claim_destination_address")?;
                let checked = parsed
                    .require_network(config.bitcoin.network.network())
                    .context("bitcoin.claim_destination_address is for another network")?;
                Ok::<_, anyhow::Error>(checked.script_pubkey().into_bytes())
            })
            .transpose()?;
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
            bitcoin_claim_destination,
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

    /// `None` leaves the bootstrap to mint a per-swap key.
    #[must_use]
    pub fn bitcoin_claim_destination(&self) -> Option<&[u8]> {
        self.bitcoin_claim_destination.as_deref()
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

/// The network is chosen by configuration alone, so the configuration must not
/// be able to lie about it: the genesis has to be the named network's, and
/// mainnet settles only against mainnet. A swap binds both chains, and one
/// real leg against one worthless leg is a loss.
fn ensure_networks_agree(
    config: &BtcRoleConfigV1,
    bitcoin_genesis: bitcoin::BlockHash,
) -> Result<()> {
    let network = config.bitcoin.network;
    ensure!(
        bitcoin_genesis
            == bitcoin::blockdata::constants::genesis_block(network.network()).block_hash(),
        "bitcoin.genesis_block_hash is not the {network:?} genesis"
    );
    ensure!(
        (network == BitcoinNetworkName::Mainnet) == (config.lez.network == LezNetworkName::Mainnet),
        "mainnet settles only against mainnet: bitcoin.network is {network:?}, lez.network is {:?}",
        config.lez.network
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::LockFeePolicyV1;

    #[test]
    fn lock_fee_rate_follows_the_estimate_between_the_relay_floor_and_the_cap() {
        let policy = LockFeePolicyV1::default();
        // 0.00002 BTC/kvB is 2 sat/vB; a fractional rate rounds up.
        assert_eq!(policy.rate_sat_per_vb(Some(0.000_02)), 2);
        assert_eq!(policy.rate_sat_per_vb(Some(0.000_021)), 3);
        // The testnet4 backlog that priced a 10,000 sat lock at 56,064 sat.
        assert_eq!(
            policy.rate_sat_per_vb(Some(0.003_64)),
            policy.max_sat_per_vb
        );
        // No estimate (regtest, or testnet4 after a run of empty blocks), or
        // nonsense: the fallback, itself under the cap.
        for absent in [None, Some(0.0), Some(-1.0), Some(f64::NAN)] {
            assert_eq!(policy.rate_sat_per_vb(absent), policy.fallback_sat_per_vb);
        }
        let low_cap = LockFeePolicyV1 {
            max_sat_per_vb: 4,
            ..policy
        };
        assert_eq!(low_cap.rate_sat_per_vb(None), 4);
        // A real but tiny estimate is floored at the relay minimum.
        assert_eq!(policy.rate_sat_per_vb(Some(0.000_000_1)), 1);
    }

    #[test]
    fn lock_fee_is_refused_above_its_share_of_the_value() {
        let policy = LockFeePolicyV1::default();
        assert!(policy.admits(157, 10_000));
        assert!(policy.admits(500, 10_000));
        assert!(!policy.admits(501, 10_000));
        assert!(!policy.admits(56_064, 10_000));
        assert!(policy.admits(u64::MAX / 100, u64::MAX));
    }

    #[test]
    fn a_config_without_a_lock_fee_section_gets_the_defaults() {
        let policy: LockFeePolicyV1 = serde_json::from_str("{}").expect("defaults");
        assert_eq!(policy, LockFeePolicyV1::default());
        let partial: LockFeePolicyV1 =
            serde_json::from_str(r#"{"max_sat_per_vb": 4}"#).expect("partial override");
        assert_eq!(partial.max_sat_per_vb, 4);
        assert_eq!(partial.confirmation_target, 6);
        assert!(serde_json::from_str::<LockFeePolicyV1>(r#"{"max_rate": 4}"#).is_err());
    }
}
