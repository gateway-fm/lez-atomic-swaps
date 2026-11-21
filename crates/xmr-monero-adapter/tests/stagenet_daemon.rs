//! The adapter and the SDK against a real `monerod --stagenet` and a real
//! `monero-wallet-rpc --stagenet`, as `scripts/run-monero-stagenet-ci.sh`
//! starts them (the pinned 0.18.5.1 release). Ignored by default; the script
//! runs them with `--ignored` and these variables:
//!
//! - `MONERO_STAGENET_DAEMON_URL`, `MONERO_STAGENET_WALLET_URL`: literal
//!   loopback origins of monerod and wallet-rpc.
//! - `MONERO_STAGENET_DAEMON_LOGIN_FILE`, `MONERO_STAGENET_WALLET_LOGIN_FILE`:
//!   files holding the Digest login `user:password`.
//! - `MONERO_STAGENET_ROLE_WALLET`: a wallet the wallet-rpc already holds,
//!   without a password; the restore tests close it and open it again.
//!
//! What they prove: the Stagenet chain identity and network check, the
//! refusal of that daemon by every check a Regtest Node makes, the network
//! byte of the SDK's 2-of-2 shared address as the official wallet reads it,
//! and the refusal of a Regtest (mainnet-byte) shared address by a Stagenet
//! wallet and by the Stagenet output verifier.

use std::{env, fs};

use lez_bridge_protocol::RunId;
use lez_xmr_monero_adapter::{
    ExpectedMoneroOutput, LoopbackRpcEndpoint, MONERO_STAGENET_GENESIS_HASH, MoneroAddress,
    MoneroChainIdentity, MoneroChainIdentityAttestor, MoneroEvidenceError, MoneroNetwork,
    MoneroOutputVerifier, MoneroRegtestWalletEffects, MoneroTopologyError, MoneroTopologyVerifier,
    MoneroTransactionId, MoneroWalletEffectError,
};
use lez_xmr_swap_sdk::{
    CrossCurveDleqProofV1, CrossCurveScalar, MoneroAddressNetworkV1, MoneroPrivateViewKey,
    MoneroSharedAddressV1,
};
use monero_rpc::monero::Network;

fn endpoint(url: &str, login: &str) -> LoopbackRpcEndpoint {
    let url = env::var(url).unwrap_or_else(|_| panic!("{url} is required"));
    let path = env::var(login).unwrap_or_else(|_| panic!("{login} is required"));
    let line = fs::read_to_string(&path).expect("read the Digest login file");
    let (user, password) = line.trim().split_once(':').expect("login is user:password");
    LoopbackRpcEndpoint::new(&url, user, password).expect("literal-loopback endpoint")
}

fn daemon() -> LoopbackRpcEndpoint {
    endpoint(
        "MONERO_STAGENET_DAEMON_URL",
        "MONERO_STAGENET_DAEMON_LOGIN_FILE",
    )
}

fn wallet() -> LoopbackRpcEndpoint {
    endpoint(
        "MONERO_STAGENET_WALLET_URL",
        "MONERO_STAGENET_WALLET_LOGIN_FILE",
    )
}

fn role_wallet() -> String {
    env::var("MONERO_STAGENET_ROLE_WALLET").expect("MONERO_STAGENET_ROLE_WALLET is required")
}

/// Two fresh roles' DLEQ-bound spend shares and one shared view key.
fn shared_keys() -> (
    CrossCurveDleqProofV1,
    CrossCurveDleqProofV1,
    MoneroPrivateViewKey,
) {
    let prove = || {
        let scalar = CrossCurveScalar::generate().expect("entropy");
        CrossCurveDleqProofV1::prove(&scalar, &mut rand_core::OsRng).expect("DLEQ proof")
    };
    (
        prove(),
        prove(),
        MoneroPrivateViewKey::generate().expect("entropy"),
    )
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "needs the monerod --stagenet that scripts/run-monero-stagenet-ci.sh starts"]
async fn stagenet_daemon_identity_and_network_are_exact() {
    let daemon = daemon();

    let stagenet = MoneroChainIdentityAttestor::new(MoneroNetwork::Stagenet, &daemon)
        .expect("Stagenet attestor");
    let identity = stagenet.discover().await.expect("Stagenet genesis");
    assert_eq!(identity.network(), MoneroNetwork::Stagenet);
    assert_eq!(identity.genesis_hash(), MONERO_STAGENET_GENESIS_HASH);
    stagenet
        .verify_network()
        .await
        .expect("get_info names Stagenet");
    let tip = stagenet.tip_height().await.expect("tip height");
    println!(
        "stagenet monerod: genesis {}, tip {tip}",
        hex::encode(identity.genesis_hash())
    );

    // What a Node configured for the regtest profile does today: its
    // attestor and its peerless-topology check both refuse this daemon.
    let regtest = MoneroChainIdentityAttestor::new(MoneroNetwork::Regtest, &daemon)
        .expect("Regtest attestor");
    assert!(matches!(
        regtest.discover().await,
        Err(MoneroEvidenceError::GenesisOfAnotherNetwork {
            network: MoneroNetwork::Regtest
        })
    ));
    assert!(matches!(
        regtest.verify_network().await,
        Err(MoneroEvidenceError::DaemonNetworkMismatch)
    ));
    let wallet = wallet();
    let regtest_identity =
        MoneroChainIdentity::new(MoneroNetwork::Regtest, MONERO_STAGENET_GENESIS_HASH)
            .expect("an identity to hand the verifier");
    let topology = MoneroTopologyVerifier::for_one_wallet_rpc(
        RunId::new("stagenet-ci-topology".to_owned()).expect("run id"),
        regtest_identity,
        &daemon,
        &wallet,
    )
    .expect("Regtest topology verifier");
    assert_eq!(
        topology
            .verify()
            .await
            .expect_err("not the isolated Regtest"),
        MoneroTopologyError::DaemonProfileMismatch
    );
    // The Stagenet topology check of the same daemon:
    // `stagenet_topology_admits_only_a_synchronized_daemon_with_a_fresh_tip`.
    let _ = identity;
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "needs the monerod --stagenet that scripts/run-monero-stagenet-ci.sh starts"]
async fn stagenet_topology_admits_only_a_synchronized_daemon_with_a_fresh_tip() {
    let daemon = daemon();
    let wallet = wallet();
    let identity = MoneroChainIdentityAttestor::new(MoneroNetwork::Stagenet, &daemon)
        .expect("Stagenet attestor")
        .discover()
        .await
        .expect("Stagenet genesis");
    let topology = MoneroTopologyVerifier::for_one_wallet_rpc(
        RunId::new("stagenet-ci-topology".to_owned()).expect("run id"),
        identity,
        &daemon,
        &wallet,
    )
    .expect("Stagenet topology verifier");
    // A Node's funding receipt needs this attestation. CI's daemon syncs a
    // bounded prefix and is never at the tip, which the check must see; a
    // daemon at the tip (MONERO_STAGENET_EXPECT_SYNCED=1) must pass it.
    let attested = topology.verify().await;
    if std::env::var("MONERO_STAGENET_EXPECT_SYNCED").as_deref() == Ok("1") {
        let attestation = attested.expect("a synchronized Stagenet daemon");
        assert_eq!(attestation.chain_identity(), identity);
        assert!(!attestation.offline());
        println!("stagenet topology: peers {}", attestation.peer_count());
    } else {
        assert_eq!(
            attested.expect_err("a daemon still syncing"),
            MoneroTopologyError::DaemonNotSynchronized
        );
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "needs the monero-wallet-rpc --stagenet that scripts/run-monero-stagenet-ci.sh starts"]
async fn stagenet_wallet_restores_the_shared_view_only_wallet() {
    let daemon = daemon();
    let wallet = wallet();
    let identity = MoneroChainIdentityAttestor::new(MoneroNetwork::Stagenet, &daemon)
        .expect("Stagenet attestor")
        .discover()
        .await
        .expect("Stagenet genesis");

    let (maker, taker, view) = shared_keys();
    let view_bytes = *view.into_monero_little_endian();
    let view = || MoneroPrivateViewKey::from_monero_little_endian(view_bytes).expect("view key");
    let stagenet =
        MoneroSharedAddressV1::derive(MoneroAddressNetworkV1::Stagenet, &maker, &taker, &view())
            .expect("Stagenet shared address");
    let regtest =
        MoneroSharedAddressV1::derive(MoneroAddressNetworkV1::Regtest, &maker, &taker, &view())
            .expect("Regtest shared address");
    let parsed: MoneroAddress = stagenet.address_string().parse().expect("an address");
    assert_eq!(parsed.network, Network::Stagenet);
    assert!(stagenet.address_string().starts_with('5'));
    let mainnet_bytes: MoneroAddress = regtest.address_string().parse().expect("an address");
    assert_eq!(mainnet_bytes.network, Network::Mainnet);
    assert_eq!(stagenet.public_spend_key(), regtest.public_spend_key());

    let effects = MoneroRegtestWalletEffects::new(&daemon, &wallet)
        .expect("wallet effects")
        .with_role_wallet(role_wallet(), String::new())
        .expect("role wallet");
    let tip = MoneroChainIdentityAttestor::new(MoneroNetwork::Stagenet, &daemon)
        .expect("attestor")
        .tip_height()
        .await
        .expect("tip");
    let suffix = hex::encode(&stagenet.public_spend_key()[..6]);

    // The official Stagenet wallet takes the SDK's address and returns it
    // byte for byte from the view key alone.
    effects
        .restore_shared_view_only(
            &stagenet,
            view(),
            format!("shared-view-{suffix}"),
            "stagenet-ci-view".to_owned(),
            tip,
        )
        .await
        .expect("Stagenet wallet-rpc restores the shared view-only wallet");
    effects
        .refresh_from_height(tip)
        .await
        .expect("refresh the view-only wallet");

    // The same keys in the Regtest (mainnet-byte) encoding are refused.
    assert!(matches!(
        effects
            .restore_shared_view_only(
                &regtest,
                view(),
                format!("shared-regtest-{suffix}"),
                "stagenet-ci-view".to_owned(),
                tip,
            )
            .await,
        Err(MoneroWalletEffectError::Rpc { .. })
    ));

    // The Stagenet output verifier refuses a Regtest destination before any
    // RPC, and reads the real wallet and daemon for a Stagenet one.
    effects.open_role_wallet().await.expect("role wallet back");
    let verifier = MoneroOutputVerifier::new(identity, &daemon, &wallet).expect("verifier");
    let unknown = MoneroTransactionId::from([0x5a; 32]);
    assert!(matches!(
        verifier
            .verify(&ExpectedMoneroOutput::new(unknown, mainnet_bytes, 1).expect("terms"))
            .await,
        Err(MoneroEvidenceError::DestinationNetworkMismatch)
    ));
    let observed = verifier
        .verify(&ExpectedMoneroOutput::new(unknown, parsed, 1).expect("terms"))
        .await
        .expect_err("no such transaction");
    println!("stagenet verifier on an unknown transaction: {observed}");
    assert!(!matches!(
        observed,
        MoneroEvidenceError::DestinationNetworkMismatch
    ));
}
