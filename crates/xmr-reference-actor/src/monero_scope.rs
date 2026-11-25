//! The Monero network a swap's evidence was observed on, and what each
//! network admits.
//!
//! The network is the agreement's: Stage A's profile fixes the address
//! network and its genesis the chain, and both parties signed it. Every
//! Monero record the actors write names its scope, and every reader checks
//! it against the record's own genesis (which the agreement binds):
//!
//! | | Regtest | Stagenet |
//! |---|---|---|
//! | scope | `isolated_official_monero_regtest` | `public_monero_stagenet` |
//! | peers | none (the daemon is offline) | any (the daemon syncs) |
//! | daemon version | the pinned release | the pinned release, or none where a restricted public RPC hides it |
//! | restore height | 0 (the chain is a few hundred blocks) | at most the funding's block |
//! | confirmations | mined by the role | mined by Stagenet's miners; the role waits |

use anyhow::{Context as _, Result};
use lez_xmr_monero_adapter::{
    MONERO_STAGENET_GENESIS_HASH, MoneroChainIdentity, MoneroChainIdentityAttestor, MoneroNetwork,
};
use lez_xmr_swap_sdk::{MoneroAddressNetworkV1, XmrAgreementV1};

use crate::M4_MONERO_DAEMON_VERSION;

const REGTEST_SCOPE: &str = "isolated_official_monero_regtest";
const STAGENET_SCOPE: &str = "public_monero_stagenet";
/// How long before the funding cutoff `C` a Maker's view-only wallet starts
/// scanning on Stagenet: the funding is sent after the take and before `C`,
/// so it is in a block mined after `C` less the profile's cutoff offset
/// (ten minutes on the devnet's Stagenet profile). Six hours covers any
/// offset a role config admits ([`MAX_STAGENET_FUNDING_CUTOFF_SECONDS`])
/// and Monero's two hours of timestamp slack.
const STAGENET_RESTORE_LOOKBACK_SECONDS: u64 = 6 * 60 * 60;
/// The longest funding cutoff offset a Stagenet role config admits.
pub const MAX_STAGENET_FUNDING_CUTOFF_SECONDS: u64 = 3 * 60 * 60;

/// The Monero network of a swap's agreement.
#[must_use]
pub const fn agreement_monero_network(agreement: &XmrAgreementV1) -> MoneroNetwork {
    address_network_monero(agreement.body().monero().network())
}

/// The adapter's network for an address network of the SDK.
#[must_use]
pub const fn address_network_monero(network: MoneroAddressNetworkV1) -> MoneroNetwork {
    match network {
        MoneroAddressNetworkV1::Regtest => MoneroNetwork::Regtest,
        MoneroAddressNetworkV1::Stagenet => MoneroNetwork::Stagenet,
    }
}

/// The agreement's chain: its network and the genesis both parties signed.
///
/// # Errors
///
/// Fails for an all-zero genesis.
pub fn agreement_monero_identity(agreement: &XmrAgreementV1) -> Result<MoneroChainIdentity> {
    MoneroChainIdentity::new(
        agreement_monero_network(agreement),
        agreement.body().monero().genesis_hash(),
    )
    .context("the Stage-A Monero chain identity is invalid")
}

/// Where a swap's view-only and reconstructed wallets start scanning: 0 on
/// Regtest; on Stagenet the last block stamped at least
/// `STAGENET_RESTORE_LOOKBACK_SECONDS` before `C`, so a wallet never scans
/// Stagenet's two million blocks from genesis. The same agreement and chain
/// give the same height, so every tool of one swap restores alike.
///
/// # Errors
///
/// Fails closed when the daemon's headers cannot be read.
pub async fn agreement_restore_height(
    agreement: &XmrAgreementV1,
    attestor: &MoneroChainIdentityAttestor,
) -> Result<u64> {
    match agreement_monero_network(agreement) {
        MoneroNetwork::Regtest => Ok(0),
        MoneroNetwork::Stagenet => {
            let cutoff_seconds = agreement.body().windows().maker_xmr_funding_cutoff_ms() / 1_000;
            attestor
                .height_at_or_before(
                    cutoff_seconds.saturating_sub(STAGENET_RESTORE_LOOKBACK_SECONDS),
                )
                .await
                .context("find the Stagenet restore height before the funding cutoff")
        }
    }
}

/// What the Monero evidence of one network must say.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MoneroEvidenceScope(MoneroNetwork);

impl MoneroEvidenceScope {
    pub(crate) const fn new(network: MoneroNetwork) -> Self {
        Self(network)
    }

    /// The scope of a record by the genesis it names (lowercase hex); the
    /// readers check that genesis against the agreement.
    pub(crate) fn of_genesis_hex(genesis: &str) -> Self {
        if genesis == hex::encode(MONERO_STAGENET_GENESIS_HASH) {
            Self(MoneroNetwork::Stagenet)
        } else {
            Self(MoneroNetwork::Regtest)
        }
    }

    /// The scope of the chain whose genesis is `genesis`.
    pub(crate) fn of_genesis(genesis: [u8; 32]) -> Self {
        if genesis == MONERO_STAGENET_GENESIS_HASH {
            Self(MoneroNetwork::Stagenet)
        } else {
            Self(MoneroNetwork::Regtest)
        }
    }

    pub(crate) const fn network(self) -> MoneroNetwork {
        self.0
    }

    /// The `network_scope` the records carry.
    pub(crate) const fn name(self) -> &'static str {
        match self.0 {
            MoneroNetwork::Regtest => REGTEST_SCOPE,
            MoneroNetwork::Stagenet => STAGENET_SCOPE,
        }
    }

    /// Regtest's daemon is offline and peerless; a public network's has peers.
    pub(crate) const fn admits_peer_count(self, peers: u64) -> bool {
        match self.0 {
            MoneroNetwork::Regtest => peers == 0,
            MoneroNetwork::Stagenet => true,
        }
    }

    /// The pinned release; on Stagenet also no version at all, which a
    /// restricted public RPC reports.
    pub(crate) fn admits_daemon_version(self, version: &str) -> bool {
        version == M4_MONERO_DAEMON_VERSION
            || (self.0 == MoneroNetwork::Stagenet && version.is_empty())
    }

    /// A restore height for an output in `containing_block_height`.
    pub(crate) const fn admits_restore_height(
        self,
        restore_height: u64,
        containing_block_height: u64,
    ) -> bool {
        match self.0 {
            MoneroNetwork::Regtest => restore_height == 0,
            MoneroNetwork::Stagenet => restore_height <= containing_block_height,
        }
    }
}

#[cfg(test)]
mod tests {
    use lez_xmr_monero_adapter::{MONERO_STAGENET_GENESIS_HASH, MoneroNetwork};

    use super::MoneroEvidenceScope;

    #[test]
    fn each_network_admits_its_own_evidence() {
        let regtest = MoneroEvidenceScope::of_genesis_hex(&"ab".repeat(32));
        let stagenet =
            MoneroEvidenceScope::of_genesis_hex(&hex::encode(MONERO_STAGENET_GENESIS_HASH));
        assert_eq!(regtest.network(), MoneroNetwork::Regtest);
        assert_eq!(stagenet.network(), MoneroNetwork::Stagenet);
        assert_eq!(regtest.name(), "isolated_official_monero_regtest");
        assert_eq!(stagenet.name(), "public_monero_stagenet");
        assert!(regtest.admits_peer_count(0) && !regtest.admits_peer_count(1));
        assert!(stagenet.admits_peer_count(0) && stagenet.admits_peer_count(12));
        assert!(regtest.admits_daemon_version(crate::M4_MONERO_DAEMON_VERSION));
        assert!(!regtest.admits_daemon_version(""));
        assert!(stagenet.admits_daemon_version(""));
        assert!(!stagenet.admits_daemon_version("0.18.4.0-release"));
        assert!(regtest.admits_restore_height(0, 100) && !regtest.admits_restore_height(1, 100));
        assert!(stagenet.admits_restore_height(100, 100));
        assert!(!stagenet.admits_restore_height(101, 100));
    }
}
