//! The one key to `generateblocks`.
//!
//! On Regtest nothing confirms a transfer but the role itself, so the
//! wallet effects that mine ([`crate::MoneroRegtestWalletEffects`]'s
//! `*_and_confirm`, `restore_shared_and_sweep` and `mine_confirmations`)
//! take a [`MoneroRegtestMining`]. It exists only for a Regtest chain
//! identity whose daemon has just said, in its own `get_info`, that it is
//! the `fakechain`: there is no value of it for Stagenet, so a role on a
//! public network cannot mine, only wait for the confirmations miners give
//! (`fund_shared_exact_once`, `restore_shared_and_sweep_once`, and the
//! [`crate::MoneroOutputVerifier`]).

use thiserror::Error;

use super::{LoopbackRpcEndpoint, MoneroChainIdentity, MoneroNetwork, topology};

/// Proof that this role mines its own confirmations: the chain is Regtest,
/// and its daemon says so.
///
/// No public constructor but [`Self::prove`]; not `Clone`.
#[derive(Debug)]
#[must_use]
pub struct MoneroRegtestMining {
    daemon_origin: String,
}

impl MoneroRegtestMining {
    /// The mining capability for `identity` on `daemon`.
    ///
    /// # Errors
    ///
    /// [`MoneroRegtestMiningError::NotRegtest`] for any other network, before
    /// any RPC (a caller on a public network waits instead);
    /// [`MoneroRegtestMiningError::DaemonIsNotRegtest`] when the daemon's
    /// `get_info` does not name the `fakechain`, or cannot be read.
    pub async fn prove(
        identity: MoneroChainIdentity,
        daemon: &LoopbackRpcEndpoint,
    ) -> Result<Self, MoneroRegtestMiningError> {
        if identity.network() != MoneroNetwork::Regtest {
            return Err(MoneroRegtestMiningError::NotRegtest {
                network: identity.network(),
            });
        }
        topology::verify_daemon_network(daemon, MoneroNetwork::Regtest)
            .await
            .map_err(|_| MoneroRegtestMiningError::DaemonIsNotRegtest)?;
        Ok(Self {
            daemon_origin: daemon.base_url.clone(),
        })
    }

    /// The mining capability where the chain needs one: on Regtest the
    /// proof (the daemon must say it is the `fakechain`), on a public network
    /// `None`, for its miners confirm.
    ///
    /// # Errors
    ///
    /// [`MoneroRegtestMiningError::DaemonIsNotRegtest`] for a Regtest
    /// identity whose daemon does not prove it.
    pub async fn where_the_role_mines(
        identity: MoneroChainIdentity,
        daemon: &LoopbackRpcEndpoint,
    ) -> Result<Option<Self>, MoneroRegtestMiningError> {
        match Self::prove(identity, daemon).await {
            Ok(mining) => Ok(Some(mining)),
            Err(MoneroRegtestMiningError::NotRegtest { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// A proof for a test's fake daemon, which answers no `get_info`.
    #[cfg(test)]
    pub(crate) fn for_tests(daemon: &LoopbackRpcEndpoint) -> Self {
        Self {
            daemon_origin: daemon.base_url.clone(),
        }
    }

    /// Whether this proof is for `daemon`; a mining effect refuses another.
    pub(crate) fn covers(&self, daemon_origin: &str) -> bool {
        self.daemon_origin == daemon_origin
    }
}

/// Why a role may not mine.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum MoneroRegtestMiningError {
    /// The chain is a public network: miners confirm, the role waits.
    #[error("Monero {network:?} is mined by its miners, never by a role")]
    NotRegtest {
        /// The chain's network.
        network: MoneroNetwork,
    },
    /// A Regtest identity whose daemon does not say it is the `fakechain`.
    #[error("the Monero daemon did not prove the Regtest fakechain")]
    DaemonIsNotRegtest,
}

#[cfg(test)]
mod tests {
    use super::{MoneroRegtestMining, MoneroRegtestMiningError};
    use crate::{LoopbackRpcEndpoint, MONERO_STAGENET_GENESIS_HASH, MoneroChainIdentity};

    #[tokio::test]
    async fn no_mining_capability_exists_for_stagenet() {
        // No daemon listens here: the refusal comes before any RPC.
        let daemon =
            LoopbackRpcEndpoint::new("http://127.0.0.1:9", "daemon", "secret").expect("endpoint");
        let stagenet =
            MoneroChainIdentity::new(crate::MoneroNetwork::Stagenet, MONERO_STAGENET_GENESIS_HASH)
                .expect("identity");
        assert!(matches!(
            MoneroRegtestMining::where_the_role_mines(stagenet, &daemon).await,
            Ok(None)
        ));
        assert_eq!(
            MoneroRegtestMining::prove(stagenet, &daemon).await.err(),
            Some(MoneroRegtestMiningError::NotRegtest {
                network: crate::MoneroNetwork::Stagenet
            })
        );
        let regtest =
            MoneroChainIdentity::new(crate::MoneroNetwork::Regtest, [1; 32]).expect("identity");
        assert_eq!(
            MoneroRegtestMining::prove(regtest, &daemon).await.err(),
            Some(MoneroRegtestMiningError::DaemonIsNotRegtest)
        );
        assert_eq!(
            MoneroRegtestMining::where_the_role_mines(regtest, &daemon)
                .await
                .err(),
            Some(MoneroRegtestMiningError::DaemonIsNotRegtest)
        );
    }
}
