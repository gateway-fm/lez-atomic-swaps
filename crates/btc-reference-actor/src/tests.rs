use std::{
    fs::{self, OpenOptions},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _, symlink},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use super::*;
use bitcoin::{
    Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness, absolute,
    secp256k1::{Keypair, Message, PublicKey, Secp256k1, SecretKey},
    transaction,
};
use lez_bridge_adapter::BtcLezAssetBridgeBindingV2;
use lez_bridge_protocol::{
    AccountIds, ChainPosition, CompleteWitnessedClaimRequest, CompleteWitnessedClaimResult,
    EscrowState, ExactMessageBytes, ExactTransactionBytes, FinalizedBlockIdentity,
    FinalizedWitnessedAssetFundingFactsV2, FinalizedWitnessedAssetUnavailableReasonV2,
    FinalizedWitnessedFundingFacts, NativeCustodyFacts, NativeEscrowAccountFacts,
    NativeFundInstructionFacts, NativeRefundFoundFacts, NativeRefundInstructionFacts,
    ObservedTransactionFacts, PrepareWitnessedAssetEscrowV2Request,
    PrepareWitnessedAssetEscrowV2Result, PrepareWitnessedClaimResult, PreparedTransaction,
    PreparedWitnessedClaim, SubmissionOutcome, SubmitTransactionRequest, SubmitTransactionResult,
    TokenHoldingFactsV2, TransactionId, WitnessedAssetClaimInstructionFactsV2,
    WitnessedAssetCustodyFactsV2, WitnessedAssetEffectInstructionFactsV2,
    WitnessedAssetPrepareStepV2, WitnessedAssetPreparedEffectV2, WitnessedClaimInstructionFacts,
    WitnessedEscrowMetadataFacts, WitnessedLezAssetV2,
};
use lez_btc_swap_sdk::{
    AdaptorSessionContext, BTC_AGREEMENT_SCHEMA_V1, BTC_LEZ_ASSET_EXTENSION_SCHEMA_V1,
    BtcAgreementBodyV1, BtcAgreementRecordV1, BtcChainPolicyV1, BtcClaimTermsV1, BtcFundingTermsV1,
    BtcLezAssetExtensionBodyV1, BtcLezAssetExtensionRecordV1, BtcLezAssetExtensionV1,
    BtcLezAssetV1, BtcLezCustomTokenTermsV1, BtcLezTermsV1, BtcP2trTermsV1,
    BtcParticipantIdentityV1, BtcParticipantsV1, BtcRecoveryPlanV1, CsvBlockDelay,
    FreshAdaptorNonce, P2trSwapOutput, PersistedAdaptorSigningMaterial, RefundXOnlyKey,
    SigningRole, TwoPartyAggregateKey, aggregate_adaptor_presignature,
    sign_persisted_adaptor_partial, verify_adaptor_partial_signature, verify_nonce_commitment,
};
use lez_swap_core::SwapDirection;
use lez_swap_store::{
    AdaptorNonceCommitment, AdaptorPartialSignature, AdaptorPresignature, AdaptorPublicNonce,
    AdaptorSessionReservation, BtcTerminalOutcome, SecretNonceBytes,
};
use serde_json::Value;
use tempfile::TempDir;
use tokio::sync::Barrier;

#[allow(dead_code)]
#[path = "../../btc-core-adapter/tests/support.rs"]
mod support;

struct ActorFixture {
    directory: TempDir,
    config: ActorConfig,
    agreement: BtcAgreementV1,
    agreement_wire: Vec<u8>,
}

#[test]
fn schema5_asset_refund_effect_and_request_identity_bind_asset_role_and_target() {
    for (direction, role, transition) in [
        (
            SwapDirection::TakerSellsForeign,
            ActorRole::Maker,
            RefundTransition::MakerLeg,
        ),
        (
            SwapDirection::TakerSellsLez,
            ActorRole::Taker,
            RefundTransition::TakerLeg,
        ),
    ] {
        let mut fixture = ActorFixture::for_direction(direction, role);
        let extension = configure_schema5_asset_extension(&mut fixture);
        let binding =
            BtcLezAssetBridgeBindingV2::new(&fixture.agreement, &extension, extension.asset())
                .expect("asset refund binding");
        assert_eq!(binding.depositor(), transition.funded_participant());
        let transaction = asset_claim_transaction();
        let effect = prepared_lez_asset_refund_effect(
            &fixture.config,
            &fixture.agreement,
            &extension,
            transition,
            transaction.clone(),
        )
        .expect("prepare asset refund effect");
        assert_eq!(
            effect.effect.agreement_commitment(),
            *extension.asset_commitment()
        );
        assert_eq!(effect.effect.key().local_role(), role.sdk());
        assert_eq!(
            effect.effect.key().operation(),
            PublicEffectOperation::Refund
        );
        assert_eq!(effect.transaction, transaction);

        let state_id = lez_asset_refund_request_id(
            &fixture.config,
            &fixture.agreement,
            &extension,
            &binding,
            transition,
            "observe_witnessed_asset_refund",
            Some(NativeRefundObservationTarget::StateOnly),
            None,
        )
        .expect("state request identity");
        let exact_id = lez_asset_refund_request_id(
            &fixture.config,
            &fixture.agreement,
            &extension,
            &binding,
            transition,
            "observe_witnessed_asset_refund",
            Some(NativeRefundObservationTarget::Exact {
                refund_transaction_id: transaction.transaction_id,
                window: fixture.config.discovery_window().expect("window"),
            }),
            None,
        )
        .expect("exact request identity");
        assert_ne!(state_id, exact_id);

        let mut legacy = fixture.config.clone();
        legacy.schema_version = CONFIG_SCHEMA_VERSION;
        assert_eq!(
            prepared_lez_asset_refund_effect(
                &legacy,
                &fixture.agreement,
                &extension,
                transition,
                transaction,
            ),
            Err(ActorCommandError::AgreementBindingInvalid)
        );
    }
}

impl ActorFixture {
    fn new() -> Self {
        Self::with_agreement(
            support::swap_fixture().agreement,
            ActorRole::Taker,
            support::MAKER_SECRET,
            support::TAKER_SECRET,
            support::ADAPTOR_SECRET,
            support::REFUND_SECRET,
            true,
        )
    }

    fn without_activation_material() -> Self {
        Self::with_agreement(
            support::swap_fixture().agreement,
            ActorRole::Taker,
            support::MAKER_SECRET,
            support::TAKER_SECRET,
            support::ADAPTOR_SECRET,
            support::REFUND_SECRET,
            false,
        )
    }

    fn for_direction(direction: SwapDirection, role: ActorRole) -> Self {
        Self::with_agreement(
            directional_agreement(direction),
            role,
            [1; 32],
            [2; 32],
            [7; 32],
            match direction {
                SwapDirection::TakerSellsForeign => [4; 32],
                SwapDirection::TakerSellsLez => [3; 32],
            },
            true,
        )
    }

    fn with_agreement(
        agreement: BtcAgreementV1,
        role: ActorRole,
        maker_secret: [u8; 32],
        taker_secret: [u8; 32],
        adaptor_secret: [u8; 32],
        bitcoin_refund_secret: [u8; 32],
        seed_material: bool,
    ) -> Self {
        let directory = tempfile::tempdir().expect("actor tempdir");
        let agreement_wire = agreement.encode_wire().expect("agreement wire");
        let agreement_file = directory.path().join("agreement.json");
        fs::write(&agreement_file, &agreement_wire).expect("write agreement");
        let config = ActorConfig {
            schema_version: LEGACY_CONFIG_SCHEMA_VERSION,
            role,
            agreement_file,
            agreement_sha256: None,
            state_db: directory.path().join("actor.sqlite3"),
            accepted_at_unix_seconds: 1_700_000_000,
            bitcoin_core: BitcoinCoreConfig {
                endpoint: "http://127.0.0.1:1".into(),
                cookie_file: directory.path().join("bitcoin.cookie"),
                connectivity: BitcoinConnectivity::IsolatedLocal,
            },
            lez_bridge: LezBridgeConfig {
                endpoint: "http://127.0.0.1:2".into(),
                capability_file: directory.path().join("lez.capability"),
                run_id: RunId::new("m3-internal-actor-test").expect("run ID"),
                runtime: RuntimeDescriptor::new(
                    role.bridge(),
                    RuntimeCompatibility::LeeV0_2_0,
                    Hex32::from_bytes([99; 32]),
                    Hex32::from_bytes([17; 32]),
                    Hex32::from_bytes([18; 32]),
                    Hex32::from_bytes([15; 32]),
                    Hex32::from_bytes(match role {
                        ActorRole::Maker => [10; 32],
                        ActorRole::Taker => [11; 32],
                    }),
                ),
                request_timeout_millis: 1_000,
                discovery_start_height: 1,
                discovery_max_blocks: 10,
            },
            signing: ClaimRecoveryConfig {
                bitcoin: SigningSessionConfig {
                    session_id: Hex32::from_bytes([41; 32]),
                    journal_db: directory.path().join("bitcoin-adaptor.sqlite3"),
                },
                lez: SigningSessionConfig {
                    session_id: Hex32::from_bytes([42; 32]),
                    journal_db: directory.path().join("lez-adaptor.sqlite3"),
                },
                prepared_witnessed_claim_result_file: directory
                    .path()
                    .join("prepared-witnessed-claim.json"),
                adaptor_secret_file: (role == ActorRole::Taker)
                    .then(|| directory.path().join("adaptor-secret.key")),
            },
            maker_lock: None,
            taker_first_lock: None,
            asset_extension: None,
            refund: RefundAuthorityConfig {
                bitcoin_refund_key_file: (role.sdk() == agreement.bitcoin_funder())
                    .then(|| directory.path().join("bitcoin-refund.key")),
            },
        };
        config.validate().expect("valid test config");
        if seed_material {
            if let Some(path) = &config.signing.adaptor_secret_file {
                write_private_secret(path, adaptor_secret);
            }
            if let Some(path) = &config.refund.bitcoin_refund_key_file {
                write_private_secret(path, bitcoin_refund_secret);
            }
            seed_activation_material(&config, &agreement, maker_secret, taker_secret);
        }
        Self {
            directory,
            config,
            agreement,
            agreement_wire,
        }
    }
}

fn write_private_secret(path: &Path, secret: [u8; 32]) {
    fs::write(path, format!("{}\n", hex::encode(secret))).expect("write adaptor secret");
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .expect("owner-only adaptor secret");
}

fn seed_activation_material(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    maker_secret: [u8; 32],
    taker_secret: [u8; 32],
) {
    let preparation_request_id =
        RequestId::new("prepared-claim-001").expect("preparation request ID");
    let prepared = PrepareWitnessedClaimResult::new(
        MessageContext::new(
            config.lez_bridge.run_id.clone(),
            preparation_request_id.clone(),
            bridge_participant(agreement.lez_claimant()),
        ),
        PreparedWitnessedClaim::new(
            preparation_request_id,
            Hex32::from_bytes(support::lez_claim_message_hash()),
            ExactMessageBytes::new(support::LEZ_PREPARED_MESSAGE_BYTES.to_vec())
                .expect("prepared message bytes"),
        ),
    );
    fs::write(
        &config.signing.prepared_witnessed_claim_result_file,
        serde_json::to_vec(&prepared).expect("prepared claim JSON"),
    )
    .expect("write prepared claim");

    seed_signing_journal(
        config,
        agreement,
        BtcAdaptorSessionDomain::Bitcoin,
        &config.signing.bitcoin,
        maker_secret,
        taker_secret,
    );
    seed_signing_journal(
        config,
        agreement,
        BtcAdaptorSessionDomain::Lez,
        &config.signing.lez,
        maker_secret,
        taker_secret,
    );
}

#[allow(clippy::too_many_lines)] // The fixture spells out every durable ceremony transition.
fn seed_signing_journal(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    domain: BtcAdaptorSessionDomain,
    signing: &SigningSessionConfig,
    maker_secret: [u8; 32],
    taker_secret: [u8; 32],
) {
    let session_id = *signing.session_id.as_bytes();
    let context = agreement
        .adaptor_session_context(domain, session_id)
        .expect("agreement-derived signing context");
    let maker_nonce = FreshAdaptorNonce::generate(&context, SigningRole::Maker, maker_secret)
        .expect("maker fresh nonce");
    let taker_nonce = FreshAdaptorNonce::generate(&context, SigningRole::Taker, taker_secret)
        .expect("taker fresh nonce");
    let (local_role, local_secret, local_nonce, peer_role, peer_secret, peer_nonce) =
        match config.role {
            ActorRole::Maker => (
                SigningRole::Maker,
                maker_secret,
                &maker_nonce,
                SigningRole::Taker,
                taker_secret,
                &taker_nonce,
            ),
            ActorRole::Taker => (
                SigningRole::Taker,
                taker_secret,
                &taker_nonce,
                SigningRole::Maker,
                maker_secret,
                &maker_nonce,
            ),
        };
    let identity = AdaptorSessionIdentity::new(
        session_id,
        config.role.signer(),
        context.durable_context_binding(),
        context.message(),
        context.adaptor_point(),
        context.ordered_public_keys(),
    );
    let mut journal =
        SqliteAdaptorSessionJournal::open(&signing.journal_db).expect("create signer journal");
    let _ = journal
        .reserve(AdaptorSessionReservation::new(
            identity.clone(),
            SecretNonceBytes::new(*local_nonce.secret_nonce()),
            AdaptorPublicNonce::new(local_nonce.public_nonce()),
            AdaptorNonceCommitment::new(local_nonce.commitment()),
        ))
        .expect("reserve local nonce");
    let _ = journal
        .record_peer_commitment(
            &identity,
            AdaptorNonceCommitment::new(peer_nonce.commitment()),
        )
        .expect("record peer commitment");
    verify_nonce_commitment(
        &context,
        peer_role,
        peer_nonce.commitment(),
        peer_nonce.public_nonce(),
    )
    .expect("verify peer nonce");
    let _ = journal
        .record_verified_peer_public_nonce(
            &identity,
            AdaptorPublicNonce::new(peer_nonce.public_nonce()),
        )
        .expect("record peer nonce");
    let own_partial = journal
        .sign_and_persist_partial(&identity, |material| {
            sign_persisted_adaptor_partial(
                &context,
                local_role,
                local_secret,
                PersistedAdaptorSigningMaterial::new(
                    *material.identity().signing_domain(),
                    material.secret_nonce(),
                    *material.own_public_nonce().bytes(),
                    local_nonce.commitment(),
                    peer_nonce.commitment(),
                    *material.peer_public_nonce().bytes(),
                ),
            )
            .map(AdaptorPartialSignature::new)
            .map_err(|_| ())
        })
        .expect("persist local partial")
        .partial();
    let peer_partial = sign_persisted_adaptor_partial(
        &context,
        peer_role,
        peer_secret,
        PersistedAdaptorSigningMaterial::new(
            context.durable_context_binding(),
            peer_nonce.secret_nonce(),
            peer_nonce.public_nonce(),
            peer_nonce.commitment(),
            local_nonce.commitment(),
            local_nonce.public_nonce(),
        ),
    )
    .expect("peer partial");
    let (maker_public_nonce, taker_public_nonce, maker_partial, taker_partial) = match config.role {
        ActorRole::Maker => (
            local_nonce.public_nonce(),
            peer_nonce.public_nonce(),
            *own_partial.bytes(),
            peer_partial,
        ),
        ActorRole::Taker => (
            peer_nonce.public_nonce(),
            local_nonce.public_nonce(),
            peer_partial,
            *own_partial.bytes(),
        ),
    };
    verify_adaptor_partial_signature(
        &context,
        peer_role,
        maker_public_nonce,
        taker_public_nonce,
        peer_partial,
    )
    .expect("verify peer partial");
    let _ = journal
        .record_verified_peer_partial(&identity, AdaptorPartialSignature::new(peer_partial))
        .expect("record peer partial");
    let presignature = aggregate_adaptor_presignature(
        &context,
        maker_public_nonce,
        taker_public_nonce,
        maker_partial,
        taker_partial,
    )
    .expect("aggregate presignature");
    let _ = journal
        .record_verified_presignature(&identity, AdaptorPresignature::new(presignature))
        .expect("record presignature");
}

fn test_secret(byte: u8) -> SecretKey {
    SecretKey::from_slice(&[byte; 32]).expect("valid fixture secret")
}

fn compressed_public_key(secret: &SecretKey) -> [u8; 33] {
    PublicKey::from_secret_key(&Secp256k1::new(), secret).serialize()
}

fn x_only_public_key(secret: &SecretKey) -> [u8; 32] {
    Keypair::from_secret_key(&Secp256k1::new(), secret)
        .x_only_public_key()
        .0
        .serialize()
}

fn claim_destination(secret: &SecretKey) -> Vec<u8> {
    let key = Keypair::from_secret_key(&Secp256k1::new(), secret)
        .x_only_public_key()
        .0;
    ScriptBuf::new_p2tr(&Secp256k1::verification_only(), key, None).into_bytes()
}

fn agreement_signature(secret: &SecretKey, commitment: [u8; 32]) -> [u8; 64] {
    let secp = Secp256k1::new();
    secp.sign_schnorr_no_aux_rand(
        &Message::from_digest(commitment),
        &Keypair::from_secret_key(&secp, secret),
    )
    .serialize()
}

fn directional_funding_transaction(script_pubkey: Vec<u8>) -> Transaction {
    Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: Txid::from_byte_array([42; 32]),
                vout: 0,
            },
            script_sig: ScriptBuf::from_bytes(vec![0x51]),
            sequence: Sequence::MAX,
            witness: Witness::default(),
        }],
        output: vec![
            TxOut {
                value: Amount::from_sat(1),
                script_pubkey: ScriptBuf::new(),
            },
            TxOut {
                value: Amount::from_sat(100_000),
                script_pubkey: ScriptBuf::from_bytes(script_pubkey),
            },
        ],
    }
}

fn exact_directional_funding(agreement: &BtcAgreementV1) -> Transaction {
    directional_funding_transaction(agreement.p2tr_contract().script_pubkey_bytes().to_vec())
}

#[allow(clippy::too_many_lines)]
fn directional_agreement(direction: SwapDirection) -> BtcAgreementV1 {
    let maker_secret = test_secret(1);
    let taker_secret = test_secret(2);
    let maker_refund_secret = test_secret(3);
    let taker_refund_secret = test_secret(4);
    let maker_claim_secret = test_secret(5);
    let taker_claim_secret = test_secret(6);
    let adaptor_secret = test_secret(7);
    let participants = BtcParticipantsV1::new(
        BtcParticipantIdentityV1::new(
            [10; 32],
            compressed_public_key(&maker_secret),
            x_only_public_key(&maker_refund_secret),
            claim_destination(&maker_claim_secret),
        ),
        BtcParticipantIdentityV1::new(
            [11; 32],
            compressed_public_key(&taker_secret),
            x_only_public_key(&taker_refund_secret),
            claim_destination(&taker_claim_secret),
        ),
    );
    let adaptor_point = compressed_public_key(&adaptor_secret);
    let aggregate_key = AdaptorSessionContext::untweaked(
        [
            compressed_public_key(&maker_secret),
            compressed_public_key(&taker_secret),
        ],
        [30; 32],
        adaptor_point,
        [31; 32],
    )
    .expect("aggregate context")
    .output_key();
    let bitcoin_funder = match direction {
        SwapDirection::TakerSellsForeign => Participant::Taker,
        SwapDirection::TakerSellsLez => Participant::Maker,
    };
    let refund_key = match bitcoin_funder {
        Participant::Maker => x_only_public_key(&maker_refund_secret),
        Participant::Taker => x_only_public_key(&taker_refund_secret),
    };
    let contract = P2trSwapOutput::new(
        TwoPartyAggregateKey::from_bytes(aggregate_key).expect("aggregate key"),
        RefundXOnlyKey::from_bytes(refund_key).expect("refund key"),
        CsvBlockDelay::new(144).expect("CSV"),
    )
    .expect("P2TR contract");
    let funding_transaction =
        directional_funding_transaction(contract.script_pubkey_bytes().to_vec());
    let funding = BtcFundingTermsV1::new(
        funding_transaction.compute_txid().to_byte_array(),
        1,
        100_000,
    );
    let claim = lez_btc_swap_sdk::CooperativeKeyPathSpend::new(
        &contract,
        OutPoint {
            txid: Txid::from_byte_array(*funding.transaction_id()),
            vout: funding.output_index(),
        },
        Amount::from_sat(funding.value_sat()),
        vec![TxOut {
            value: Amount::from_sat(99_000),
            script_pubkey: ScriptBuf::from_bytes(
                participants
                    .for_participant(bitcoin_funder.other())
                    .claim_destination_script_pubkey()
                    .to_vec(),
            ),
        }],
    )
    .expect("cooperative claim");
    let lez_depositor = match direction {
        SwapDirection::TakerSellsForeign => Participant::Maker,
        SwapDirection::TakerSellsLez => Participant::Taker,
    };
    let lez_claimant = lez_depositor.other();
    let body = BtcAgreementBodyV1::new(
        [20; 32],
        direction,
        BtcChainPolicyV1::new([8; 32], support::REQUIRED_CONFIRMATIONS),
        participants.clone(),
        adaptor_point,
        BtcLezTermsV1::new(
            [17; 32],
            [18; 32],
            [15; 32],
            [16; 32],
            [12; 32],
            [13; 32],
            [14; 32],
            *participants
                .for_participant(lez_depositor)
                .lez_owner_account(),
            *participants
                .for_participant(lez_claimant)
                .lez_owner_account(),
            5_000,
            match direction {
                SwapDirection::TakerSellsForeign => 1_700_000_100_000,
                SwapDirection::TakerSellsLez => 1_700_000_500_000,
            },
            support::lez_claim_message_hash(),
        ),
        BtcP2trTermsV1::from_contract(&contract),
        funding,
        BtcClaimTermsV1::from_spend(&claim).expect("claim terms"),
        BtcRecoveryPlanV1::new(
            1_000,
            1_144,
            1_699_999_800,
            1_700_000_100,
            1_700_000_500,
            300,
        ),
    );
    let commitment = body.commitment();
    BtcAgreementV1::validate(BtcAgreementRecordV1::from_parts(
        BTC_AGREEMENT_SCHEMA_V1,
        body,
        commitment,
        agreement_signature(&maker_secret, commitment),
        agreement_signature(&taker_secret, commitment),
    ))
    .expect("valid directional agreement")
}

const TEST_LEZ_INITIALIZATION_ID: [u8; 32] = [81; 32];
const TEST_LEZ_FUNDING_ID: [u8; 32] = [82; 32];
const TEST_LEZ_INITIALIZATION_BYTES: &[u8] = b"exact-lez-initialize";
const TEST_LEZ_CUSTODY_CREATION_ID: [u8; 32] = [83; 32];
const TEST_LEZ_CUSTODY_CREATION_BYTES: &[u8] = b"exact-lez-create-custody-ata";
const TEST_LEZ_FUNDING_BYTES: &[u8] = b"exact-lez-fund";
const TEST_TAKER_LEZ_INITIALIZATION_ID: &str = "taker-lez-initialize";
const TEST_TAKER_LEZ_FUNDING_ID: &str = "taker-lez-fund";
const TEST_TAKER_LEZ_INITIALIZATION_BYTES: &[u8] = b"exact-taker-lez-initialize";
const TEST_TAKER_LEZ_FUNDING_BYTES: &[u8] = b"exact-taker-lez-fund";

fn configure_schema4_maker_material(fixture: &mut ActorFixture) {
    assert_eq!(fixture.config.role, ActorRole::Maker);
    fixture.config.schema_version = CONFIG_SCHEMA_VERSION;
    fixture.config.maker_lock = Some(match fixture.agreement.direction() {
        SwapDirection::TakerSellsForeign => {
            let request_path = fixture.directory.path().join("maker-lez-request.json");
            let result_path = fixture.directory.path().join("maker-lez-result.json");
            let context = MessageContext::new(
                fixture.config.lez_bridge.run_id.clone(),
                RequestId::new("maker-lock-preparation").expect("request ID"),
                BridgeParticipant::Maker,
            );
            let request = PrepareWitnessedEscrowRequest::new(
                context.clone(),
                fixture.config.lez_bridge.runtime.clone(),
                witnessed_lez_terms(&fixture.agreement).expect("witnessed terms"),
            );
            let result = PrepareWitnessedEscrowResult::new(
                context,
                PreparedTransaction::new(
                    TransactionId::from_bytes(TEST_LEZ_INITIALIZATION_ID),
                    ExactTransactionBytes::new(TEST_LEZ_INITIALIZATION_BYTES.to_vec())
                        .expect("initialization bytes"),
                ),
                PreparedTransaction::new(
                    TransactionId::from_bytes(TEST_LEZ_FUNDING_ID),
                    ExactTransactionBytes::new(TEST_LEZ_FUNDING_BYTES.to_vec())
                        .expect("funding bytes"),
                ),
            );
            fs::write(
                &request_path,
                serde_json::to_vec(&request).expect("request JSON"),
            )
            .expect("write request");
            fs::write(
                &result_path,
                serde_json::to_vec(&result).expect("result JSON"),
            )
            .expect("write result");
            MakerLockMaterialConfig::Lez {
                preparation_request_file: request_path,
                preparation_result_file: result_path,
            }
        }
        SwapDirection::TakerSellsLez => {
            let funding_path = fixture.directory.path().join("maker-bitcoin-funding.hex");
            let mut encoded =
                hex::encode(serialize(&exact_directional_funding(&fixture.agreement))).into_bytes();
            encoded.push(b'\n');
            fs::write(&funding_path, encoded).expect("write exact Bitcoin funding");
            MakerLockMaterialConfig::Bitcoin {
                exact_funding_transaction_file: funding_path,
            }
        }
    });
    fixture.config.validate().expect("schema-4 maker config");
}

fn upgrade_fixture_for_supervised_provision(fixture: &mut ActorFixture) {
    fs::set_permissions(fixture.directory.path(), fs::Permissions::from_mode(0o700))
        .expect("private provision parent");
    if fixture.config.role == ActorRole::Maker {
        configure_schema4_maker_material(fixture);
    }
    fixture.config.schema_version = SUPERVISED_CONFIG_SCHEMA_VERSION;
    fixture.config.agreement_sha256 = Some(Hex32::from_bytes(
        Sha256::digest(&fixture.agreement_wire).into(),
    ));
    fixture.config.validate().expect("supervised source config");
    let (source, _) = load_agreement(&fixture.config).expect("supervised source agreement");
    validate_activation_material(&fixture.config, &source)
        .expect("supervised source activation authority");
}

#[test]
fn maker_and_taker_provision_role_only_bundles_with_exact_replay() {
    for role in [ActorRole::Maker, ActorRole::Taker] {
        let mut fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, role);
        upgrade_fixture_for_supervised_provision(&mut fixture);
        let output = fixture.directory.path().join("published-actor");
        let accepted_at = fixture.config.accepted_at_unix_seconds;
        assert_eq!(
            fs::symlink_metadata(fixture.directory.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o700
        );
        assert_eq!(
            fs::canonicalize(fixture.directory.path()).unwrap(),
            fixture.directory.path()
        );
        let mut expected_config = fixture.config.clone();
        expected_config.agreement_file = output.join("shared/agreement-v1.borsh");
        expected_config.state_db = output.join(match role {
            ActorRole::Maker => "maker/state/actor.sqlite3",
            ActorRole::Taker => "taker/state/actor.sqlite3",
        });
        expected_config.validate().expect("rebound config shape");
        let first = match role {
            ActorRole::Maker => provision_btc_maker_actor_from_config(
                &fixture.config,
                &fixture.agreement_wire,
                accepted_at,
                &output,
            ),
            ActorRole::Taker => provision_btc_taker_actor_from_config(
                &fixture.config,
                &fixture.agreement_wire,
                accepted_at,
                &output,
            ),
        }
        .expect("first role-fixed publication");

        let (role_name, other_role) = match role {
            ActorRole::Maker => ("maker", "taker"),
            ActorRole::Taker => ("taker", "maker"),
        };
        let agreement_sha256: [u8; 32] = Sha256::digest(&fixture.agreement_wire).into();
        let config_bytes = fs::read(first.config_file()).unwrap();
        let agreement_bytes = fs::read(first.agreement_file()).unwrap();
        assert!(!first.was_replay());
        assert_eq!(first.role(), role);
        assert_eq!(first.swap_id(), fixture.agreement.coordinator().id());
        assert_eq!(first.agreement_sha256(), agreement_sha256);
        assert_eq!(
            first.config_sha256(),
            <[u8; 32]>::from(Sha256::digest(&config_bytes))
        );
        assert!(first.config_file().starts_with(output.join(role_name)));
        assert!(first.state_database().starts_with(output.join(role_name)));
        assert!(output.join("shared").is_dir());
        assert!(output.join(role_name).join("state").is_dir());
        assert!(!output.join(other_role).exists());

        let published = ActorConfig::load_private(first.config_file()).unwrap();
        assert_eq!(published.schema_version, SUPERVISED_CONFIG_SCHEMA_VERSION);
        assert_eq!(published.role(), role);
        assert_eq!(published.state_db(), first.state_database());
        assert_eq!(published.agreement_sha256(), Some(agreement_sha256));
        assert_eq!(published.supervised_swap_id().unwrap(), *first.swap_id());

        let config_inode = fs::symlink_metadata(first.config_file()).unwrap().ino();
        let agreement_inode = fs::symlink_metadata(first.agreement_file()).unwrap().ino();
        let replay = match role {
            ActorRole::Maker => provision_btc_maker_actor_from_config(
                &fixture.config,
                &fixture.agreement_wire,
                accepted_at,
                &output,
            ),
            ActorRole::Taker => provision_btc_taker_actor_from_config(
                &fixture.config,
                &fixture.agreement_wire,
                accepted_at,
                &output,
            ),
        }
        .expect("exact role-fixed replay");
        assert!(replay.was_replay());
        assert_eq!(fs::read(replay.config_file()).unwrap(), config_bytes);
        assert_eq!(fs::read(replay.agreement_file()).unwrap(), agreement_bytes);
        assert_eq!(
            fs::symlink_metadata(replay.config_file()).unwrap().ino(),
            config_inode
        );
        assert_eq!(
            fs::symlink_metadata(replay.agreement_file()).unwrap().ino(),
            agreement_inode
        );
        assert!(!output.join(other_role).exists());
    }
}

#[test]
fn role_specific_provision_wrappers_reject_opposite_sources_without_output() {
    for (source_role, call_maker) in [(ActorRole::Taker, true), (ActorRole::Maker, false)] {
        let mut fixture =
            ActorFixture::for_direction(SwapDirection::TakerSellsForeign, source_role);
        upgrade_fixture_for_supervised_provision(&mut fixture);
        let output = fixture.directory.path().join("published-actor");
        let result = if call_maker {
            provision_btc_maker_actor_from_config(
                &fixture.config,
                &fixture.agreement_wire,
                fixture.config.accepted_at_unix_seconds,
                &output,
            )
        } else {
            provision_btc_taker_actor_from_config(
                &fixture.config,
                &fixture.agreement_wire,
                fixture.config.accepted_at_unix_seconds,
                &output,
            )
        };

        assert_eq!(result.unwrap_err(), BtcActorProvisionError::Invalid);
        assert!(!output.exists());
    }
}

#[test]
fn provision_rejects_preseeded_destination_without_clobbering_marker() {
    const MARKER_BYTES: &[u8] = b"preexisting actor destination\n";

    for role in [ActorRole::Maker, ActorRole::Taker] {
        let mut fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, role);
        upgrade_fixture_for_supervised_provision(&mut fixture);
        let output = fixture.directory.path().join("published-actor");
        fs::create_dir(&output).expect("create preexisting actor destination");
        fs::set_permissions(&output, fs::Permissions::from_mode(0o700))
            .expect("make destination private");
        let marker = output.join("marker");
        fs::write(&marker, MARKER_BYTES).expect("write collision marker");
        fs::set_permissions(&marker, fs::Permissions::from_mode(0o600))
            .expect("make collision marker private");
        let marker_inode = fs::symlink_metadata(&marker).unwrap().ino();

        let result = match role {
            ActorRole::Maker => provision_btc_maker_actor_from_config(
                &fixture.config,
                &fixture.agreement_wire,
                fixture.config.accepted_at_unix_seconds,
                &output,
            ),
            ActorRole::Taker => provision_btc_taker_actor_from_config(
                &fixture.config,
                &fixture.agreement_wire,
                fixture.config.accepted_at_unix_seconds,
                &output,
            ),
        };

        assert_eq!(result.unwrap_err(), BtcActorProvisionError::Invalid);
        assert_eq!(fs::read(&marker).unwrap(), MARKER_BYTES);
        assert_eq!(fs::symlink_metadata(&marker).unwrap().ino(), marker_inode);
        assert_eq!(
            fs::symlink_metadata(&marker).unwrap().permissions().mode() & 0o7777,
            0o600
        );
        assert!(!output.join("shared").exists());
        assert!(!output.join("maker").exists());
        assert!(!output.join("taker").exists());
    }
}

#[test]
fn provision_replay_rejects_dangling_opposite_role_symlink() {
    let mut fixture =
        ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    upgrade_fixture_for_supervised_provision(&mut fixture);
    let output = fixture.directory.path().join("published-actor");
    let accepted_at = fixture.config.accepted_at_unix_seconds;
    provision_btc_maker_actor_from_config(
        &fixture.config,
        &fixture.agreement_wire,
        accepted_at,
        &output,
    )
    .expect("first role-fixed publication");
    symlink("missing-counterparty", output.join("taker"))
        .expect("create dangling opposite-role symlink");

    assert_eq!(
        provision_btc_maker_actor_from_config(
            &fixture.config,
            &fixture.agreement_wire,
            accepted_at,
            &output,
        )
        .unwrap_err(),
        BtcActorProvisionError::Invalid
    );
    assert!(
        fs::symlink_metadata(output.join("taker"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

fn custom_token_asset_extension(fixture: &ActorFixture) -> BtcLezAssetExtensionV1 {
    let token = BtcLezCustomTokenTermsV1::new(
        [40; 32],
        [41; 32],
        [42; 32],
        *fixture.agreement.lez_terms().depositor_account(),
        [43; 32],
        *fixture.agreement.lez_terms().claimant_account(),
        [44; 32],
        [45; 32],
        fixture.agreement.lez_terms().amount(),
        fixture.agreement.lez_terms().refund_at_ms(),
        *fixture.agreement.lez_terms().aggregate_authority_account(),
        fixture
            .agreement
            .p2tr_contract()
            .aggregate_internal_key_bytes(),
    );
    let body = BtcLezAssetExtensionBodyV1::new(
        *fixture.agreement.agreement_commitment(),
        BtcLezAssetV1::CustomToken(Box::new(token)),
    );
    let commitment = body.commitment();
    let record = BtcLezAssetExtensionRecordV1::from_parts(
        BTC_LEZ_ASSET_EXTENSION_SCHEMA_V1,
        body,
        commitment,
        agreement_signature(&test_secret(1), commitment),
        agreement_signature(&test_secret(2), commitment),
    );
    BtcLezAssetExtensionV1::validate(record, &fixture.agreement).expect("valid asset extension")
}

fn configure_schema5_asset_extension(fixture: &mut ActorFixture) -> BtcLezAssetExtensionV1 {
    let legacy: PrepareWitnessedClaimResult = serde_json::from_slice(
        &fs::read(&fixture.config.signing.prepared_witnessed_claim_result_file)
            .expect("legacy prepared claim"),
    )
    .expect("legacy prepared claim JSON");
    let extension = custom_token_asset_extension(fixture);
    let commitment = *extension.asset_commitment();
    let extension_path = fixture.directory.path().join("lez-asset-extension.borsh");
    fs::write(
        &extension_path,
        extension.encode_wire().expect("extension wire"),
    )
    .expect("write extension");
    fixture.config.asset_extension = Some(AssetExtensionConfig {
        record_file: extension_path,
        expected_asset_commitment: Hex32::from_bytes(commitment),
    });
    fixture.config.schema_version = ASSET_CONFIG_SCHEMA_VERSION;
    let binding =
        BtcLezAssetBridgeBindingV2::new(&fixture.agreement, &extension, extension.asset())
            .expect("asset claim binding");
    let local_is_claimant = fixture.config.role.sdk() == fixture.agreement.lez_claimant();
    let run_id = if local_is_claimant {
        fixture.config.lez_bridge.run_id.clone()
    } else {
        RunId::new("peer-claimant-run").expect("peer claimant run")
    };
    let prepared = PrepareWitnessedAssetClaimV2Result::new(
        MessageContext::new(
            run_id,
            legacy.claim.preparation_request_id.clone(),
            bridge_participant(fixture.agreement.lez_claimant()),
        ),
        binding.terms().clone(),
        legacy.claim,
    );
    fs::write(
        &fixture.config.signing.prepared_witnessed_claim_result_file,
        serde_json::to_vec(&prepared).expect("asset prepared claim JSON"),
    )
    .expect("write asset prepared claim");
    extension
}

fn configure_schema5_asset_material(fixture: &mut ActorFixture) -> BtcLezAssetExtensionV1 {
    let extension = configure_schema5_asset_extension(fixture);
    assert_eq!(
        fixture.agreement.direction(),
        SwapDirection::TakerSellsForeign
    );
    let binding =
        BtcLezAssetBridgeBindingV2::new(&fixture.agreement, &extension, extension.asset())
            .expect("asset bridge binding");
    let request_path = fixture
        .directory
        .path()
        .join("maker-lez-asset-v2-request.json");
    let result_path = fixture
        .directory
        .path()
        .join("maker-lez-asset-v2-result.json");
    let context = MessageContext::new(
        fixture.config.lez_bridge.run_id.clone(),
        RequestId::new("maker-lock-asset-v2-preparation").expect("request ID"),
        BridgeParticipant::Maker,
    );
    let request = PrepareWitnessedAssetEscrowV2Request::new(
        context.clone(),
        fixture.config.lez_bridge.runtime.clone(),
        binding.terms().clone(),
    );
    let result = PrepareWitnessedAssetEscrowV2Result::new(
        context,
        binding.terms().clone(),
        vec![
            WitnessedAssetPreparedEffectV2::new(
                WitnessedAssetPrepareStepV2::InitializeWitnessed,
                PreparedTransaction::new(
                    TransactionId::from_bytes(TEST_LEZ_INITIALIZATION_ID),
                    ExactTransactionBytes::new(TEST_LEZ_INITIALIZATION_BYTES.to_vec())
                        .expect("initialization bytes"),
                ),
            ),
            WitnessedAssetPreparedEffectV2::new(
                WitnessedAssetPrepareStepV2::CreateCustodyAta,
                PreparedTransaction::new(
                    TransactionId::from_bytes(TEST_LEZ_CUSTODY_CREATION_ID),
                    ExactTransactionBytes::new(TEST_LEZ_CUSTODY_CREATION_BYTES.to_vec())
                        .expect("custody bytes"),
                ),
            ),
            WitnessedAssetPreparedEffectV2::new(
                WitnessedAssetPrepareStepV2::Fund,
                PreparedTransaction::new(
                    TransactionId::from_bytes(TEST_LEZ_FUNDING_ID),
                    ExactTransactionBytes::new(TEST_LEZ_FUNDING_BYTES.to_vec())
                        .expect("funding bytes"),
                ),
            ),
        ],
    )
    .expect("valid asset preparation");
    fs::write(
        &request_path,
        serde_json::to_vec(&request).expect("request JSON"),
    )
    .expect("write request");
    fs::write(
        &result_path,
        serde_json::to_vec(&result).expect("result JSON"),
    )
    .expect("write result");
    fixture.config.maker_lock = Some(MakerLockMaterialConfig::LezAssetV2 {
        preparation_request_file: request_path,
        preparation_result_file: result_path,
    });
    fixture.config.validate().expect("schema-5 maker config");
    extension
}

fn configure_schema5_bitcoin_asset_material(fixture: &mut ActorFixture) -> BtcLezAssetExtensionV1 {
    let extension = configure_schema5_asset_extension(fixture);
    assert_eq!(fixture.agreement.direction(), SwapDirection::TakerSellsLez);
    let binding =
        BtcLezAssetBridgeBindingV2::new(&fixture.agreement, &extension, extension.asset())
            .expect("asset bridge binding");
    let request_path = fixture
        .directory
        .path()
        .join("taker-lez-asset-v2-request.json");
    let result_path = fixture
        .directory
        .path()
        .join("taker-lez-asset-v2-result.json");
    let mut taker_runtime = fixture.config.lez_bridge.runtime.clone();
    taker_runtime.sidecar_role = BridgeParticipant::Taker;
    taker_runtime.signer_account_id = Hex32::from_bytes(
        *fixture
            .agreement
            .participant(Participant::Taker)
            .lez_owner_account(),
    );
    let context = MessageContext::new(
        fixture.config.lez_bridge.run_id.clone(),
        RequestId::new("taker-first-lock-asset-v2-preparation").expect("request ID"),
        BridgeParticipant::Taker,
    );
    let request = PrepareWitnessedAssetEscrowV2Request::new(
        context.clone(),
        taker_runtime,
        binding.terms().clone(),
    );
    let result = PrepareWitnessedAssetEscrowV2Result::new(
        context,
        binding.terms().clone(),
        vec![
            WitnessedAssetPreparedEffectV2::new(
                WitnessedAssetPrepareStepV2::InitializeWitnessed,
                PreparedTransaction::new(
                    TransactionId::from_bytes([50; 32]),
                    ExactTransactionBytes::new(b"exact-taker-token-initialize".to_vec())
                        .expect("initialization bytes"),
                ),
            ),
            WitnessedAssetPreparedEffectV2::new(
                WitnessedAssetPrepareStepV2::CreateCustodyAta,
                PreparedTransaction::new(
                    TransactionId::from_bytes([51; 32]),
                    ExactTransactionBytes::new(b"exact-taker-token-custody".to_vec())
                        .expect("custody bytes"),
                ),
            ),
            WitnessedAssetPreparedEffectV2::new(
                WitnessedAssetPrepareStepV2::Fund,
                PreparedTransaction::new(
                    TransactionId::from_bytes([52; 32]),
                    ExactTransactionBytes::new(b"exact-taker-token-funding".to_vec())
                        .expect("funding bytes"),
                ),
            ),
        ],
    )
    .expect("valid taker asset preparation");
    fs::write(
        &request_path,
        serde_json::to_vec(&request).expect("request JSON"),
    )
    .expect("write request");
    fs::write(
        &result_path,
        serde_json::to_vec(&result).expect("result JSON"),
    )
    .expect("write result");
    fixture.config.taker_first_lock = Some(TakerFirstLockMaterialConfig::LezAssetV2 {
        preparation_request_file: request_path,
        preparation_result_file: result_path,
    });
    let funding_path = fixture
        .directory
        .path()
        .join("maker-bitcoin-asset-funding.hex");
    let mut encoded =
        hex::encode(serialize(&exact_directional_funding(&fixture.agreement))).into_bytes();
    encoded.extend_from_slice(b"\n");
    fs::write(&funding_path, encoded).expect("write exact asset-bound Bitcoin funding");
    fixture.config.maker_lock = Some(MakerLockMaterialConfig::Bitcoin {
        exact_funding_transaction_file: funding_path,
    });
    fixture.config.validate().expect("schema-5 maker config");
    extension
}

fn configure_schema5_asset_claim(fixture: &mut ActorFixture) -> BtcLezAssetExtensionV1 {
    let extension = match (fixture.config.role, fixture.agreement.direction()) {
        (ActorRole::Maker, SwapDirection::TakerSellsForeign) => {
            configure_schema5_asset_material(fixture)
        }
        (ActorRole::Maker, SwapDirection::TakerSellsLez) => {
            configure_schema5_bitcoin_asset_material(fixture)
        }
        (ActorRole::Taker, _) => configure_schema5_asset_extension(fixture),
    };
    fixture.config.validate().expect("schema-5 claim config");
    extension
}

#[allow(clippy::too_many_lines)] // One fixture mirrors both legacy and additive SDK input shapes.
fn fresh_maker_eligibility(fixture: &ActorFixture) -> FreshMakerLockEligibilityV1 {
    let before_cutoff = fixture
        .agreement
        .body()
        .recovery_plan()
        .maker_second_lock_cutoff_unix_seconds()
        - 1;
    let current_maker_chain_time = match fixture
        .agreement
        .coordinator()
        .funded_chain(Participant::Maker)
    {
        Chain::Bitcoin => CanonicalInclusionTimeV1::Bitcoin {
            median_time_unix_seconds: before_cutoff,
        },
        Chain::Lez => CanonicalInclusionTimeV1::Lez {
            timestamp_ms: before_cutoff * 1_000,
        },
        Chain::Zcash | Chain::Monero => unreachable!("Bitcoin agreement chain set"),
    };
    if fixture.config.schema_version == ASSET_CONFIG_SCHEMA_VERSION {
        return match fixture.agreement.direction() {
            SwapDirection::TakerSellsForeign => {
                let funding = exact_directional_funding(&fixture.agreement);
                let exact = serialize(&funding);
                FreshMakerLockEligibilityV1 {
                    prepared_first_lock: PreparedFirstLockMaterialV1::Bitcoin(
                        PreparedBitcoinFundingV1::new(
                            funding.compute_txid().to_string(),
                            exact.clone(),
                        )
                        .expect("prepared Bitcoin first lock"),
                    ),
                    evidence: MakerFirstLockEvidenceV1::Asset(
                        BtcLezAssetFirstLockEvidenceV1::Bitcoin(
                            lez_btc_swap_sdk::BitcoinFirstLockEvidenceV1::new(
                                *fixture.agreement.bitcoin_genesis_hash(),
                                exact,
                                fixture.agreement.required_bitcoin_confirmations(),
                            )
                            .expect("Bitcoin first-lock evidence"),
                        ),
                    ),
                    current_maker_chain_time,
                }
            }
            SwapDirection::TakerSellsLez => {
                let prepared =
                    load_prepared_taker_asset_first_lock(&fixture.config, &fixture.agreement)
                        .expect("prepared taker asset first lock");
                let (extension, _) =
                    validated_asset_extension_material(&fixture.config, &fixture.agreement)
                        .expect("asset extension");
                let (custody, amount) = match extension.asset() {
                    lez_btc_swap_sdk::BtcLezAssetV1::Native => (
                        lez_btc_swap_sdk::LezAssetCustodyEvidenceV1::Native {
                            custody_account: *fixture.agreement.lez_terms().custody_account(),
                        },
                        fixture.agreement.lez_terms().amount(),
                    ),
                    lez_btc_swap_sdk::BtcLezAssetV1::CustomToken(token) => (
                        lez_btc_swap_sdk::LezAssetCustodyEvidenceV1::CustomToken {
                            custody_ata_account: *token.custody_ata_account(),
                            token_definition_account: *token.token_definition_account(),
                        },
                        token.amount(),
                    ),
                };
                FreshMakerLockEligibilityV1 {
                    prepared_first_lock: PreparedFirstLockMaterialV1::LezAsset(
                        prepared.prepared.clone(),
                    ),
                    evidence: MakerFirstLockEvidenceV1::Asset(BtcLezAssetFirstLockEvidenceV1::Lez(
                        lez_btc_swap_sdk::LezAssetFirstLockEvidenceV1::new(
                            *fixture.agreement.lez_terms().genesis_block_hash(),
                            prepared.prepared.plan().clone(),
                            *fixture.agreement.lez_terms().metadata_account(),
                            custody,
                            amount,
                            true,
                        ),
                    )),
                    current_maker_chain_time,
                }
            }
        };
    }
    match fixture.agreement.direction() {
        SwapDirection::TakerSellsForeign => {
            let funding = exact_directional_funding(&fixture.agreement);
            let exact = serialize(&funding);
            FreshMakerLockEligibilityV1 {
                prepared_first_lock: PreparedFirstLockMaterialV1::Bitcoin(
                    PreparedBitcoinFundingV1::new(
                        funding.compute_txid().to_string(),
                        exact.clone(),
                    )
                    .expect("prepared Bitcoin first lock"),
                ),
                evidence: MakerFirstLockEvidenceV1::Legacy(BtcFirstLockEvidenceV1::Bitcoin(
                    lez_btc_swap_sdk::BitcoinFirstLockEvidenceV1::new(
                        *fixture.agreement.bitcoin_genesis_hash(),
                        exact,
                        fixture.agreement.required_bitcoin_confirmations(),
                    )
                    .expect("Bitcoin first-lock evidence"),
                )),
                current_maker_chain_time,
            }
        }
        SwapDirection::TakerSellsLez => FreshMakerLockEligibilityV1 {
            prepared_first_lock: PreparedFirstLockMaterialV1::Lez(
                PreparedLezFundingV1::new(
                    TEST_TAKER_LEZ_INITIALIZATION_ID,
                    TEST_TAKER_LEZ_INITIALIZATION_BYTES.to_vec(),
                    TEST_TAKER_LEZ_FUNDING_ID,
                    TEST_TAKER_LEZ_FUNDING_BYTES.to_vec(),
                )
                .expect("prepared LEZ first lock"),
            ),
            evidence: MakerFirstLockEvidenceV1::Legacy(BtcFirstLockEvidenceV1::Lez(
                lez_btc_swap_sdk::LezFirstLockEvidenceV1::new(
                    *fixture.agreement.lez_terms().genesis_block_hash(),
                    TEST_TAKER_LEZ_INITIALIZATION_ID,
                    TEST_TAKER_LEZ_INITIALIZATION_BYTES.to_vec(),
                    TEST_TAKER_LEZ_FUNDING_ID,
                    TEST_TAKER_LEZ_FUNDING_BYTES.to_vec(),
                    *fixture.agreement.lez_terms().metadata_account(),
                    *fixture.agreement.lez_terms().custody_account(),
                    fixture.agreement.lez_terms().amount(),
                    true,
                )
                .expect("LEZ first-lock evidence"),
            )),
            current_maker_chain_time,
        },
    }
}

fn mismatched_fresh_maker_eligibility(fixture: &ActorFixture) -> FreshMakerLockEligibilityV1 {
    let mut eligibility = fresh_maker_eligibility(fixture);
    eligibility.evidence = match fixture.agreement.direction() {
        SwapDirection::TakerSellsForeign => {
            let mut changed = exact_directional_funding(&fixture.agreement);
            changed.input[0].witness.push([0xaa]);
            MakerFirstLockEvidenceV1::Legacy(BtcFirstLockEvidenceV1::Bitcoin(
                lez_btc_swap_sdk::BitcoinFirstLockEvidenceV1::new(
                    *fixture.agreement.bitcoin_genesis_hash(),
                    serialize(&changed),
                    fixture.agreement.required_bitcoin_confirmations(),
                )
                .expect("changed-witness Bitcoin evidence"),
            ))
        }
        SwapDirection::TakerSellsLez => {
            MakerFirstLockEvidenceV1::Legacy(BtcFirstLockEvidenceV1::Lez(
                lez_btc_swap_sdk::LezFirstLockEvidenceV1::new(
                    *fixture.agreement.lez_terms().genesis_block_hash(),
                    TEST_TAKER_LEZ_INITIALIZATION_ID,
                    b"changed-exact-taker-lez-initialize".to_vec(),
                    TEST_TAKER_LEZ_FUNDING_ID,
                    TEST_TAKER_LEZ_FUNDING_BYTES.to_vec(),
                    *fixture.agreement.lez_terms().metadata_account(),
                    *fixture.agreement.lez_terms().custody_account(),
                    fixture.agreement.lez_terms().amount(),
                    true,
                )
                .expect("changed-byte LEZ evidence"),
            ))
        }
    };
    eligibility
}

fn maker_time_at_cutoff(agreement: &BtcAgreementV1) -> CanonicalInclusionTimeV1 {
    let cutoff = agreement
        .body()
        .recovery_plan()
        .maker_second_lock_cutoff_unix_seconds();
    match agreement.coordinator().funded_chain(Participant::Maker) {
        Chain::Bitcoin => CanonicalInclusionTimeV1::Bitcoin {
            median_time_unix_seconds: cutoff,
        },
        Chain::Lez => CanonicalInclusionTimeV1::Lez {
            timestamp_ms: cutoff * 1_000,
        },
        Chain::Zcash | Chain::Monero => unreachable!("Bitcoin agreement chain set"),
    }
}

struct FixedMakerLockPort {
    observation: MakerLockStepChainObservationV1,
    eligibility: FreshMakerLockEligibilityV1,
    complete: ActorFundingObservation,
    submission_result: BtcMakerLockSubmissionResult,
    submissions: AtomicUsize,
    eligibility_checks: AtomicUsize,
    events: Mutex<Vec<&'static str>>,
    observed_steps: Mutex<Vec<String>>,
}

impl FixedMakerLockPort {
    fn new(
        observation: MakerLockStepChainObservationV1,
        eligibility: FreshMakerLockEligibilityV1,
        complete: ActorFundingObservation,
    ) -> Self {
        Self {
            observation,
            eligibility,
            complete,
            submission_result: BtcMakerLockSubmissionResult::Unknown,
            submissions: AtomicUsize::new(0),
            eligibility_checks: AtomicUsize::new(0),
            events: Mutex::new(Vec::new()),
            observed_steps: Mutex::new(Vec::new()),
        }
    }

    fn with_submission_result(mut self, result: BtcMakerLockSubmissionResult) -> Self {
        self.submission_result = result;
        self
    }

    fn submissions(&self) -> usize {
        self.submissions.load(Ordering::SeqCst)
    }

    fn eligibility_checks(&self) -> usize {
        self.eligibility_checks.load(Ordering::SeqCst)
    }

    fn events(&self) -> Vec<&'static str> {
        self.events.lock().expect("event log").clone()
    }

    fn observed_steps(&self) -> Vec<String> {
        self.observed_steps.lock().expect("step log").clone()
    }
}

#[async_trait]
impl MakerLockExecutionPort for FixedMakerLockPort {
    async fn observe_step(
        &self,
        _agreement: &BtcAgreementV1,
        step: &PublicEffectStepV1,
    ) -> Result<MakerLockStepChainObservationV1, ActorCommandError> {
        self.events.lock().expect("event log").push("observe_step");
        self.observed_steps
            .lock()
            .expect("step log")
            .push(step.step().as_str().to_owned());
        Ok(self.observation.clone())
    }

    async fn fresh_eligibility(
        &self,
        _agreement: &BtcAgreementV1,
    ) -> Result<FreshMakerLockEligibilityV1, ActorCommandError> {
        self.events
            .lock()
            .expect("event log")
            .push("fresh_eligibility");
        self.eligibility_checks.fetch_add(1, Ordering::SeqCst);
        Ok(self.eligibility.clone())
    }

    async fn submit_step(
        &self,
        _agreement: &BtcAgreementV1,
        _step: &PublicEffectStepV1,
    ) -> Result<BtcMakerLockSubmissionResult, ActorCommandError> {
        self.events.lock().expect("event log").push("submit_step");
        self.submissions.fetch_add(1, Ordering::SeqCst);
        Ok(self.submission_result.clone())
    }

    async fn observe_complete(
        &self,
        _agreement: &BtcAgreementV1,
    ) -> Result<ActorFundingObservation, ActorCommandError> {
        self.events
            .lock()
            .expect("event log")
            .push("observe_complete");
        Ok(self.complete.clone())
    }
}

struct FixedObserver {
    observation: ActorFundingObservation,
    calls: AtomicUsize,
    transitions: Mutex<Vec<FundingTransition>>,
}

impl FixedObserver {
    fn new(observation: ActorFundingObservation) -> Self {
        Self {
            observation,
            calls: AtomicUsize::new(0),
            transitions: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn transitions(&self) -> Vec<FundingTransition> {
        self.transitions.lock().expect("transition log").clone()
    }
}

#[async_trait]
impl FundingObservationPort for FixedObserver {
    async fn observe(
        &self,
        _agreement: &BtcAgreementV1,
        transition: FundingTransition,
    ) -> Result<ActorFundingObservation, ActorCommandError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.transitions
            .lock()
            .expect("transition log")
            .push(transition);
        Ok(self.observation.clone())
    }
}

struct BarrierObserver {
    barrier: Arc<Barrier>,
    observation: ActorFundingObservation,
    expected_transition: FundingTransition,
}

#[async_trait]
impl FundingObservationPort for BarrierObserver {
    async fn observe(
        &self,
        _agreement: &BtcAgreementV1,
        transition: FundingTransition,
    ) -> Result<ActorFundingObservation, ActorCommandError> {
        assert_eq!(transition, self.expected_transition);
        self.barrier.wait().await;
        Ok(self.observation.clone())
    }
}

fn output_json(output: impl Serialize) -> Value {
    serde_json::to_value(output).expect("secret-free actor output")
}

async fn activate_and_project_taker_lock(fixture: &ActorFixture) {
    execute_actor_command(&fixture.config, ActorCommand::Activate)
        .await
        .expect("activate actor");
    let mut store = open_existing_store(
        &fixture.config,
        &fixture.agreement,
        fixture.agreement_wire.clone(),
    )
    .expect("open activated actor");
    let chain = fixture
        .agreement
        .coordinator()
        .funded_chain(Participant::Taker);
    let confirmations = if chain == Chain::Bitcoin {
        support::REQUIRED_CONFIRMATIONS
    } else {
        FINALIZED_LEZ_CONFIRMATION_UNITS
    };
    let _ = store
        .project(
            0,
            &BtcLifecycleEvidenceV1::taker_lock(
                chain,
                "canonical-taker-lock",
                confirmations,
                b"canonical-taker-lock-evidence".to_vec(),
            )
            .expect("taker lock evidence"),
        )
        .expect("project taker lock");
}

fn maker_lock_observation(
    agreement: &BtcAgreementV1,
    evidence_suffix: u8,
) -> ActorFundingObservation {
    let cutoff = agreement
        .body()
        .recovery_plan()
        .maker_second_lock_cutoff_unix_seconds();
    maker_lock_observation_at(agreement, evidence_suffix, cutoff)
}

fn exact_maker_lock_complete_observation(
    agreement: &BtcAgreementV1,
    plan: &ExactPublicEffectPlanV1,
    evidence_suffix: u8,
) -> ActorFundingObservation {
    let mut observation = maker_lock_observation(agreement, evidence_suffix);
    let ActorFundingObservation::Ready { transaction_id, .. } = &mut observation else {
        unreachable!("maker-lock helper is ready")
    };
    *transaction_id = plan
        .steps()
        .last()
        .expect("maker plan has a final step")
        .expected_public_id()
        .as_str()
        .into();
    observation
}

fn maker_lock_observation_at(
    agreement: &BtcAgreementV1,
    evidence_suffix: u8,
    inclusion_unix_seconds: u64,
) -> ActorFundingObservation {
    let chain = agreement.coordinator().funded_chain(Participant::Maker);
    ActorFundingObservation::Ready {
        chain,
        transaction_id: "canonical-maker-lock".into(),
        confirmations: if chain == Chain::Bitcoin {
            support::REQUIRED_CONFIRMATIONS
        } else {
            FINALIZED_LEZ_CONFIRMATION_UNITS
        },
        canonical_inclusion_time: match chain {
            Chain::Bitcoin => CanonicalInclusionTimeV1::Bitcoin {
                median_time_unix_seconds: inclusion_unix_seconds,
            },
            Chain::Lez => CanonicalInclusionTimeV1::Lez {
                timestamp_ms: inclusion_unix_seconds
                    .checked_mul(1_000)
                    .expect("fixture LEZ timestamp"),
            },
            Chain::Zcash | Chain::Monero => unreachable!("Bitcoin agreement chain set"),
        },
        chain_evidence: vec![b'm', b'a', b'k', b'e', b'r', evidence_suffix],
    }
}

#[test]
fn maker_lock_timeliness_predicate_covers_live_safety_boundaries_for_both_chains() {
    let cutoff = 1_699_999_800;
    for (chain, before, at, after) in [
        (
            Chain::Bitcoin,
            CanonicalInclusionTimeV1::Bitcoin {
                median_time_unix_seconds: cutoff - 1,
            },
            CanonicalInclusionTimeV1::Bitcoin {
                median_time_unix_seconds: cutoff,
            },
            CanonicalInclusionTimeV1::Bitcoin {
                median_time_unix_seconds: cutoff + 1,
            },
        ),
        (
            Chain::Lez,
            CanonicalInclusionTimeV1::Lez {
                timestamp_ms: cutoff * 1_000 - 1,
            },
            CanonicalInclusionTimeV1::Lez {
                timestamp_ms: cutoff * 1_000,
            },
            CanonicalInclusionTimeV1::Lez {
                timestamp_ms: cutoff * 1_000 + 1,
            },
        ),
    ] {
        assert!(canonical_maker_lock_is_timely(chain, &before, cutoff));
        assert!(canonical_maker_lock_is_timely(chain, &at, cutoff));
        assert!(!canonical_maker_lock_is_timely(chain, &after, cutoff));
    }
    assert!(!canonical_maker_lock_is_timely(
        Chain::Bitcoin,
        &CanonicalInclusionTimeV1::Lez {
            timestamp_ms: cutoff * 1_000,
        },
        cutoff,
    ));
    assert!(!canonical_maker_lock_is_timely(
        Chain::Lez,
        &CanonicalInclusionTimeV1::Bitcoin {
            median_time_unix_seconds: cutoff,
        },
        cutoff,
    ));
}

#[test]
fn schema4_maker_material_is_role_shaped_and_agreement_direction_bound() {
    let mut maker = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    maker.config.schema_version = CONFIG_SCHEMA_VERSION;
    assert_eq!(maker.config.validate(), Err(ActorConfigError::Invalid));

    configure_schema4_maker_material(&mut maker);
    let prepared =
        load_prepared_maker_lock_material(&maker.config, &maker.agreement).expect("maker material");
    assert_eq!(prepared.plan().steps().len(), 2);

    let (request_path, _) = match maker.config.maker_lock.as_ref().expect("maker material") {
        MakerLockMaterialConfig::Lez {
            preparation_request_file,
            preparation_result_file,
        } => (
            preparation_request_file.clone(),
            preparation_result_file.clone(),
        ),
        MakerLockMaterialConfig::Bitcoin { .. } | MakerLockMaterialConfig::LezAssetV2 { .. } => {
            unreachable!("schema-4 forward maker uses native LEZ material")
        }
    };
    let original_request = fs::read(&request_path).expect("original request");
    let mut changed_request: PrepareWitnessedEscrowRequest =
        serde_json::from_slice(&original_request).expect("decode request");
    changed_request.context.request_id =
        RequestId::new("changed-maker-lock-request").expect("changed request ID");
    fs::write(
        &request_path,
        serde_json::to_vec(&changed_request).expect("changed request JSON"),
    )
    .expect("change request");
    assert_eq!(
        load_prepared_maker_lock_material(&maker.config, &maker.agreement),
        Err(ActorCommandError::ActivationMaterialUnavailable)
    );
    fs::write(&request_path, original_request).expect("restore request");

    maker.config.schema_version = LEGACY_CONFIG_SCHEMA_VERSION;
    assert_eq!(maker.config.validate(), Err(ActorConfigError::Invalid));
    maker.config.schema_version = CONFIG_SCHEMA_VERSION;

    let wrong_path = maker.directory.path().join("wrong-bitcoin.raw");
    fs::write(
        &wrong_path,
        serialize(&exact_directional_funding(&maker.agreement)),
    )
    .expect("wrong direction bytes");
    maker.config.maker_lock = Some(MakerLockMaterialConfig::Bitcoin {
        exact_funding_transaction_file: wrong_path,
    });
    maker
        .config
        .validate()
        .expect("role shape is syntactically valid");
    assert_eq!(
        load_prepared_maker_lock_material(&maker.config, &maker.agreement),
        Err(ActorCommandError::ActivationMaterialUnavailable)
    );

    let mut taker = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    taker.config.schema_version = CONFIG_SCHEMA_VERSION;
    taker
        .config
        .validate()
        .expect("schema-4 taker has no maker authority");
    taker.config.maker_lock = maker.config.maker_lock.clone();
    assert_eq!(taker.config.validate(), Err(ActorConfigError::Invalid));
}

#[tokio::test]
async fn supervised_native_maker_owns_the_second_lock_send_path() {
    let mut fixture =
        ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    upgrade_fixture_for_supervised_provision(&mut fixture);
    activate_and_project_taker_lock(&fixture).await;
    let plan = load_prepared_maker_lock_material(&fixture.config, &fixture.agreement)
        .expect("supervised native maker material")
        .plan()
        .clone();
    let first_step = plan.steps().first().expect("maker lock first step");
    let port = FixedMakerLockPort::new(
        MakerLockStepChainObservationV1::Absent,
        fresh_maker_eligibility(&fixture),
        exact_maker_lock_complete_observation(&fixture.agreement, &plan, 0xa2),
    )
    .with_submission_result(BtcMakerLockSubmissionResult::Accepted(
        first_step.expected_public_id().as_str().into(),
    ));

    let output = drive_maker_lock_with_port(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &port,
    )
    .await
    .expect("supervised native Maker owns one exact submission");

    assert_eq!(output.revision, 1);
    assert_eq!(port.submissions(), 1);
    assert_eq!(port.eligibility_checks(), 1);
    let snapshot = SqliteBtcMakerLockJournal::open(&fixture.config.state_db)
        .expect("supervised maker lock journal")
        .load_intent(fixture.agreement.coordinator().id())
        .expect("load supervised maker lock intent")
        .expect("supervised maker lock intent");
    assert_eq!(snapshot.steps().len(), plan.steps().len());
}

#[test]
fn schema5_bridge_timeout_accepts_actor_outer_deadline_and_rejects_above_it() {
    let mut fixture =
        ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    configure_schema5_asset_extension(&mut fixture);

    fixture.config.lez_bridge.request_timeout_millis = 120_000;
    fixture
        .config
        .validate()
        .expect("schema-5 accepts the actor outer request deadline");

    fixture.config.lez_bridge.request_timeout_millis = 120_001;
    assert_eq!(fixture.config.validate(), Err(ActorConfigError::Invalid));
}

#[test]
fn peerless_asset_funding_request_identity_binds_runtime_and_window() {
    let mut fixture =
        ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    let extension = configure_schema5_asset_extension(&mut fixture);
    let binding =
        BtcLezAssetBridgeBindingV2::new(&fixture.agreement, &extension, extension.asset())
            .expect("asset binding");
    let target = FinalizedWitnessedAssetTransactionTargetV2::DiscoverByTerms {};
    let baseline = peerless_asset_funding_request_id(
        &fixture.config,
        &fixture.agreement,
        *extension.asset_commitment(),
        binding.terms(),
        &target,
    )
    .expect("baseline request identity");

    let mut changed_runtime = fixture.config.clone();
    changed_runtime.lez_bridge.runtime.genesis_block_hash = Hex32::from_bytes([0xe0; 32]);
    assert_ne!(
        peerless_asset_funding_request_id(
            &changed_runtime,
            &fixture.agreement,
            *extension.asset_commitment(),
            binding.terms(),
            &target,
        )
        .expect("changed-runtime request identity"),
        baseline
    );

    let mut changed_window = fixture.config.clone();
    changed_window.lez_bridge.discovery_start_height += 1;
    assert_ne!(
        peerless_asset_funding_request_id(
            &changed_window,
            &fixture.agreement,
            *extension.asset_commitment(),
            binding.terms(),
            &target,
        )
        .expect("changed-window request identity"),
        baseline
    );

    let exact_target = FinalizedWitnessedAssetTransactionTargetV2::exact(PreparedTransaction::new(
        TransactionId::from_bytes([0xef; 32]),
        ExactTransactionBytes::new(b"changed-peer-funding-target".to_vec()).expect("target bytes"),
    ));
    assert_ne!(
        peerless_asset_funding_request_id(
            &fixture.config,
            &fixture.agreement,
            *extension.asset_commitment(),
            binding.terms(),
            &exact_target,
        )
        .expect("changed-target request identity"),
        baseline
    );
}

#[derive(Clone)]
struct FixedPeerlessAssetFundingClassifier {
    outcome: FinalizedWitnessedAssetScanOutcomeV2<FinalizedWitnessedAssetFundingFactsV2>,
    calls: Arc<Mutex<Vec<(RequestId, FinalizedWitnessedAssetTransactionTargetV2)>>>,
}

#[async_trait]
impl LezAssetFundingClassifierPort for FixedPeerlessAssetFundingClassifier {
    async fn classify(
        &self,
        binding: &BtcLezAssetBridgeBindingV2,
        request_id: RequestId,
        target: FinalizedWitnessedAssetTransactionTargetV2,
        window: DiscoveryWindow,
    ) -> Result<
        FinalizedWitnessedAssetScanOutcomeV2<FinalizedWitnessedAssetFundingFactsV2>,
        ActorCommandError,
    > {
        assert!(matches!(
            binding.terms().asset(),
            WitnessedLezAssetV2::CustomToken(_)
        ));
        let scanned_window = match &self.outcome {
            FinalizedWitnessedAssetScanOutcomeV2::Found { scanned_window, .. }
            | FinalizedWitnessedAssetScanOutcomeV2::Absent { scanned_window, .. }
            | FinalizedWitnessedAssetScanOutcomeV2::Uncertain { scanned_window, .. } => {
                Some(*scanned_window)
            }
            FinalizedWitnessedAssetScanOutcomeV2::Unavailable { .. } => None,
        };
        if let Some(scanned_window) = scanned_window {
            assert_eq!(window, scanned_window);
        }
        self.calls
            .lock()
            .expect("classifier calls")
            .push((request_id, target));
        Ok(self.outcome.clone())
    }
}

fn exact_peerless_asset_funding_outcome(
    fixture: &ActorFixture,
) -> FinalizedWitnessedAssetScanOutcomeV2<FinalizedWitnessedAssetFundingFactsV2> {
    let (extension, _) = validated_asset_extension_material(&fixture.config, &fixture.agreement)
        .expect("asset extension");
    let binding =
        BtcLezAssetBridgeBindingV2::new(&fixture.agreement, &extension, extension.asset())
            .expect("asset binding");
    let WitnessedLezAssetV2::CustomToken(token) = binding.terms().asset() else {
        panic!("custom-token fixture");
    };
    let window = fixture.config.discovery_window().expect("asset window");
    let height = window.start_height() + 1;
    let block_hash = Hex32::from_bytes([0xe1; 32]);
    let timestamp_ms = fixture
        .agreement
        .body()
        .recovery_plan()
        .maker_second_lock_cutoff_unix_seconds()
        .checked_mul(1_000)
        .and_then(|cutoff| cutoff.checked_sub(1))
        .expect("pre-cutoff LEZ timestamp");
    let metadata_account = Hex32::from_bytes(*binding.metadata_account_id());
    let transaction = ObservedTransactionFacts::new(
        TransactionId::from_bytes([0xe2; 32]),
        ExactTransactionBytes::new(b"peerless-custom-token-funding".to_vec())
            .expect("funding bytes"),
        ChainPosition::new(block_hash, height, 0),
        AccountIds::new(vec![token.depositor_owner_account_id()]).expect("depositor signer"),
        true,
    );
    let facts = FinalizedWitnessedAssetFundingFactsV2::new(
        transaction,
        WitnessedAssetEffectInstructionFactsV2::new(
            WitnessedAssetPrepareStepV2::Fund,
            fixture.config.lez_bridge.runtime.escrow_program_id,
            AccountIds::new(vec![
                metadata_account,
                token.depositor_owner_account_id(),
                token.depositor_ata_account_id(),
                token.custody_ata_account_id(),
            ])
            .expect("funding accounts"),
            token.swap_id(),
        ),
        FinalizedBlockIdentity::new(height, block_hash, timestamp_ms),
        WitnessedEscrowMetadataFacts::from_witnessed_token_terms(
            metadata_account,
            fixture.config.lez_bridge.runtime.escrow_program_id,
            token,
            EscrowState::Funded,
        ),
        WitnessedAssetCustodyFactsV2::CustomToken(TokenHoldingFactsV2::new(
            token.custody_ata_account_id(),
            token.token_program_id(),
            token.token_definition_account_id(),
            75,
        )),
    );
    FinalizedWitnessedAssetScanOutcomeV2::Found {
        finalized_clock: ChainClock::new(block_hash, height, timestamp_ms),
        scanned_window: window,
        facts: Box::new(facts),
    }
}

#[tokio::test]
async fn schema5_taker_projects_custom_token_maker_lock_from_v2_peerless_funding() {
    let mut fixture =
        ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    let _ = configure_schema5_asset_extension(&mut fixture);
    assert_eq!(
        lez_funding_observation_protocol(&fixture.config),
        LezFundingObservationProtocol::AssetV2
    );
    let legacy = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    assert_eq!(
        lez_funding_observation_protocol(&legacy.config),
        LezFundingObservationProtocol::NativeV1
    );
    let outcome = exact_peerless_asset_funding_outcome(&fixture);
    let calls = Arc::new(Mutex::new(Vec::new()));
    let observer = LezAssetFundingObserver {
        config: &fixture.config,
        classifier: FixedPeerlessAssetFundingClassifier {
            outcome,
            calls: Arc::clone(&calls),
        },
    };

    activate_and_project_taker_lock(&fixture).await;
    let output = drive_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &observer,
    )
    .await
    .expect("peerless asset funding projects");

    assert_eq!(output.revision, 2);
    assert_eq!(output.phase, Phase::BothLegsLocked.into());
    let calls = calls.lock().expect("classifier calls");
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].1,
        FinalizedWitnessedAssetTransactionTargetV2::DiscoverByTerms {}
    );
    let (extension, _) = validated_asset_extension_material(&fixture.config, &fixture.agreement)
        .expect("asset extension");
    let binding =
        BtcLezAssetBridgeBindingV2::new(&fixture.agreement, &extension, extension.asset())
            .expect("asset binding");
    assert_eq!(
        calls[0].0,
        peerless_asset_funding_request_id(
            &fixture.config,
            &fixture.agreement,
            *extension.asset_commitment(),
            binding.terms(),
            &calls[0].1,
        )
        .expect("expected request identity")
    );
}

#[tokio::test]
async fn schema5_maker_projects_custom_token_taker_lock_from_v2_peerless_funding() {
    let mut fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Maker);
    let _ = configure_schema5_bitcoin_asset_material(&mut fixture);
    assert_eq!(
        lez_funding_observation_protocol(&fixture.config),
        LezFundingObservationProtocol::AssetV2
    );
    let calls = Arc::new(Mutex::new(Vec::new()));
    let observer = LezAssetFundingObserver {
        config: &fixture.config,
        classifier: FixedPeerlessAssetFundingClassifier {
            outcome: exact_peerless_asset_funding_outcome(&fixture),
            calls: Arc::clone(&calls),
        },
    };

    execute_actor_command(&fixture.config, ActorCommand::Activate)
        .await
        .expect("activate reverse Maker");
    let output = drive_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &observer,
    )
    .await
    .expect("peerless Taker asset funding projects");

    assert_eq!(output.revision, 1);
    assert_eq!(output.phase, Phase::TakerLockConfirmed.into());
    assert_eq!(
        calls.lock().expect("classifier calls")[0].1,
        FinalizedWitnessedAssetTransactionTargetV2::DiscoverByTerms {}
    );
}

#[tokio::test]
async fn schema5_peerless_asset_absence_uncertainty_and_unavailability_remain_pending() {
    let mut fixture =
        ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    let _ = configure_schema5_asset_extension(&mut fixture);
    let window = fixture.config.discovery_window().expect("asset window");
    let clock = ChainClock::new(
        Hex32::from_bytes([0xe3; 32]),
        window.start_height() + u64::from(window.max_blocks()) - 1,
        1_700_000_000_000,
    );
    let outcomes = [
        FinalizedWitnessedAssetScanOutcomeV2::Absent {
            finalized_clock: clock,
            scanned_window: window,
        },
        FinalizedWitnessedAssetScanOutcomeV2::Uncertain {
            finalized_clock: clock,
            scanned_window: window,
        },
        FinalizedWitnessedAssetScanOutcomeV2::Unavailable {
            reason: FinalizedWitnessedAssetUnavailableReasonV2::HistoryUnavailable,
        },
    ];

    for outcome in outcomes {
        let observer = LezAssetFundingObserver {
            config: &fixture.config,
            classifier: FixedPeerlessAssetFundingClassifier {
                outcome,
                calls: Arc::new(Mutex::new(Vec::new())),
            },
        };
        assert!(matches!(
            observer
                .observe(&fixture.agreement, FundingTransition::MakerLock)
                .await
                .expect("classification remains retryable"),
            ActorFundingObservation::Pending { chain: Chain::Lez }
        ));
    }
}

#[test]
fn schema5_asset_extension_maps_and_stages_exact_three_step_f7_plan() {
    let mut fixture =
        ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    let extension = configure_schema5_asset_material(&mut fixture);
    let prepared = load_prepared_maker_lock_material(&fixture.config, &fixture.agreement)
        .expect("asset maker material");
    let plan = prepared.plan();
    assert_eq!(plan.steps().len(), 3);
    assert_eq!(
        plan.steps()
            .iter()
            .map(|step| step.step().as_str())
            .collect::<Vec<_>>(),
        ["lez.initialize", "lez.create_custody_ata", "lez.fund"]
    );
    assert_eq!(
        plan.steps()[1].expected_public_id().as_str(),
        hex::encode(TEST_LEZ_CUSTODY_CREATION_ID)
    );
    assert_eq!(
        plan.steps()[1].exact_bytes().as_slice(),
        TEST_LEZ_CUSTODY_CREATION_BYTES
    );
    let submissions = plan
        .steps()
        .iter()
        .map(|step| {
            maker_lez_submit_request(&fixture.config, &fixture.agreement, step)
                .expect("asset step is an exact submit request")
        })
        .collect::<Vec<_>>();
    assert_eq!(submissions.len(), 3);
    assert_eq!(
        submissions[1].transaction.transaction_id.as_bytes(),
        &TEST_LEZ_CUSTODY_CREATION_ID
    );
    assert_eq!(
        submissions[1].transaction.exact_bytes.as_slice(),
        TEST_LEZ_CUSTODY_CREATION_BYTES
    );
    assert_eq!(
        submissions
            .iter()
            .map(|request| request.context.request_id.as_str())
            .collect::<HashSet<_>>()
            .len(),
        3
    );

    let first = activate(&fixture.config).expect("asset-bound activation");
    assert!(matches!(
        first.outcome,
        ActorEffectOutcomeV1::Activated { was_replay: false }
    ));
    let journal = SqliteBtcMakerLockJournal::open(&fixture.config.state_db)
        .expect("open staged maker journal");
    let snapshot = journal
        .load_intent(fixture.agreement.coordinator().id())
        .expect("load intent")
        .expect("asset intent staged during activation");
    assert_eq!(
        snapshot.intent().agreement_commitment(),
        extension.asset_commitment()
    );
    assert_eq!(snapshot.intent().plan(), plan);
    assert!(snapshot.steps().iter().all(|step| {
        step.state() == BtcMakerLockStepState::Prepared
            && step.attempt_count() == 0
            && step.revision() == 0
            && step.submission_result().is_none()
    }));
    drop(journal);

    let replay = activate(&fixture.config).expect("asset activation replay");
    assert!(matches!(
        replay.outcome,
        ActorEffectOutcomeV1::Activated { was_replay: true }
    ));
    let replay_snapshot = SqliteBtcMakerLockJournal::open(&fixture.config.state_db)
        .expect("reopen staged maker journal")
        .load_intent(fixture.agreement.coordinator().id())
        .expect("load replay intent")
        .expect("replay intent");
    assert_eq!(replay_snapshot, snapshot);

    fixture
        .config
        .asset_extension
        .as_mut()
        .expect("asset config")
        .expected_asset_commitment = Hex32::from_bytes([0x99; 32]);
    assert_eq!(
        load_prepared_maker_lock_material(&fixture.config, &fixture.agreement),
        Err(ActorCommandError::ActivationMaterialUnavailable)
    );
}

#[test]
fn schema5_untrusted_asset_plan_rejects_duplicate_transaction_ids_or_exact_bytes() {
    let mut fixture =
        ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    let _ = configure_schema5_asset_material(&mut fixture);
    let result_path = match fixture.config.maker_lock.as_ref().expect("maker material") {
        MakerLockMaterialConfig::LezAssetV2 {
            preparation_result_file,
            ..
        } => preparation_result_file,
        MakerLockMaterialConfig::Bitcoin { .. } | MakerLockMaterialConfig::Lez { .. } => {
            unreachable!("schema-5 forward maker uses asset-v2 material")
        }
    };
    let result: PrepareWitnessedAssetEscrowV2Result =
        serde_json::from_slice(&fs::read(result_path).expect("read valid preparation result"))
            .expect("decode valid preparation result");

    let mut duplicate_id = result.clone();
    let first_id = duplicate_id.effects[0].transaction.transaction_id;
    duplicate_id.effects[1].transaction.transaction_id = first_id;
    assert_eq!(
        witnessed_asset_effect_plan(&duplicate_id),
        Err(ActorCommandError::ActivationMaterialUnavailable)
    );
    fs::write(
        result_path,
        serde_json::to_vec(&duplicate_id).expect("duplicate-ID result JSON"),
    )
    .expect("persist duplicate-ID result");
    assert_eq!(
        load_prepared_maker_lock_material(&fixture.config, &fixture.agreement),
        Err(ActorCommandError::ActivationMaterialUnavailable)
    );

    let mut duplicate_bytes = result;
    let first_bytes = duplicate_bytes.effects[0].transaction.exact_bytes.clone();
    duplicate_bytes.effects[1].transaction.exact_bytes = first_bytes;
    assert_eq!(
        witnessed_asset_effect_plan(&duplicate_bytes),
        Err(ActorCommandError::ActivationMaterialUnavailable)
    );
    fs::write(
        result_path,
        serde_json::to_vec(&duplicate_bytes).expect("duplicate-bytes result JSON"),
    )
    .expect("persist duplicate-bytes result");
    assert_eq!(
        load_prepared_maker_lock_material(&fixture.config, &fixture.agreement),
        Err(ActorCommandError::ActivationMaterialUnavailable)
    );
}

#[test]
fn schema5_reverse_bitcoin_plan_stages_with_asset_commitment_and_replays() {
    let mut fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Maker);
    let extension = configure_schema5_bitcoin_asset_material(&mut fixture);
    assert_ne!(
        extension.asset_commitment(),
        fixture.agreement.agreement_commitment(),
        "fixture must distinguish the base and extension commitments"
    );
    let first = activate(&fixture.config).expect("reverse asset-bound activation");
    assert!(matches!(
        first.outcome,
        ActorEffectOutcomeV1::Activated { was_replay: false }
    ));
    let journal = SqliteBtcMakerLockJournal::open(&fixture.config.state_db)
        .expect("open reverse staged maker journal");
    let snapshot = journal
        .load_intent(fixture.agreement.coordinator().id())
        .expect("load reverse intent")
        .expect("reverse intent staged during activation");
    assert_eq!(snapshot.intent().plan().steps().len(), 1);
    assert!(snapshot.steps().iter().all(|step| {
        step.state() == BtcMakerLockStepState::Prepared
            && step.attempt_count() == 0
            && step.revision() == 0
            && step.submission_result().is_none()
    }));
    assert_eq!(
        snapshot.intent().agreement_commitment(),
        extension.asset_commitment()
    );
    drop(journal);

    let replay = activate(&fixture.config).expect("reverse asset activation replay");
    assert!(matches!(
        replay.outcome,
        ActorEffectOutcomeV1::Activated { was_replay: true }
    ));
    let replay_snapshot = SqliteBtcMakerLockJournal::open(&fixture.config.state_db)
        .expect("reopen reverse staged maker journal")
        .load_intent(fixture.agreement.coordinator().id())
        .expect("load reverse replay intent")
        .expect("reverse replay intent");
    assert_eq!(replay_snapshot, snapshot);
}

fn schema5_reverse_asset_eligibility(
    fixture: &ActorFixture,
    observed_plan: ExactPublicEffectPlanV1,
    custody_ata: [u8; 32],
    token_definition: [u8; 32],
    amount: u128,
    finalized: bool,
) -> FreshMakerLockEligibilityV1 {
    let retained = load_prepared_taker_asset_first_lock(&fixture.config, &fixture.agreement)
        .expect("retained taker first lock");
    FreshMakerLockEligibilityV1 {
        prepared_first_lock: PreparedFirstLockMaterialV1::LezAsset(retained.prepared),
        evidence: MakerFirstLockEvidenceV1::Asset(BtcLezAssetFirstLockEvidenceV1::Lez(
            lez_btc_swap_sdk::LezAssetFirstLockEvidenceV1::new(
                *fixture.agreement.lez_terms().genesis_block_hash(),
                observed_plan,
                *fixture.agreement.lez_terms().metadata_account(),
                lez_btc_swap_sdk::LezAssetCustodyEvidenceV1::CustomToken {
                    custody_ata_account: custody_ata,
                    token_definition_account: token_definition,
                },
                amount,
                finalized,
            ),
        )),
        current_maker_chain_time: CanonicalInclusionTimeV1::Bitcoin {
            median_time_unix_seconds: fixture
                .agreement
                .body()
                .recovery_plan()
                .maker_second_lock_cutoff_unix_seconds()
                - 1,
        },
    }
}

#[test]
fn schema5_asset_sdk_authorizes_exact_fresh_first_lock_in_both_directions() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let mut fixture = ActorFixture::for_direction(direction, ActorRole::Maker);
        match direction {
            SwapDirection::TakerSellsForeign => {
                let _ = configure_schema5_asset_material(&mut fixture);
            }
            SwapDirection::TakerSellsLez => {
                let _ = configure_schema5_bitcoin_asset_material(&mut fixture);
            }
        }
        let expected = load_prepared_maker_lock_material(&fixture.config, &fixture.agreement)
            .expect("schema-5 maker material")
            .plan()
            .clone();
        assert_eq!(
            validate_fresh_maker_lock_plan(
                &fixture.config,
                &fixture.agreement,
                &fixture.agreement_wire,
                fresh_maker_eligibility(&fixture),
                true,
            )
            .expect("exact schema-5 first lock authorizes Maker plan"),
            expected
        );
    }
}

#[test]
fn schema5_asset_sdk_rejects_finality_bytes_custody_amount_and_asset_drift() {
    let mut fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Maker);
    let extension = configure_schema5_bitcoin_asset_material(&mut fixture);
    let lez_btc_swap_sdk::BtcLezAssetV1::CustomToken(token) = extension.asset() else {
        panic!("custom-token fixture")
    };
    let retained = load_prepared_taker_asset_first_lock(&fixture.config, &fixture.agreement)
        .expect("retained taker first lock");
    let exact_plan = retained.prepared.plan().clone();
    let exact_custody = *token.custody_ata_account();
    let exact_definition = *token.token_definition_account();
    let exact_amount = token.amount();

    let mut changed_steps = exact_plan.steps().to_vec();
    let first = &changed_steps[0];
    changed_steps[0] = PublicEffectStepV1::new(
        first.step().clone(),
        first.expected_public_id().clone(),
        ExactPublicEffectBytes::new(b"different-canonical-initialization".to_vec())
            .expect("changed exact bytes"),
    );
    let changed_plan = ExactPublicEffectPlanV1::new(changed_steps).expect("changed observed plan");
    let cases = [
        schema5_reverse_asset_eligibility(
            &fixture,
            exact_plan.clone(),
            exact_custody,
            exact_definition,
            exact_amount,
            false,
        ),
        schema5_reverse_asset_eligibility(
            &fixture,
            changed_plan,
            exact_custody,
            exact_definition,
            exact_amount,
            true,
        ),
        schema5_reverse_asset_eligibility(
            &fixture,
            exact_plan.clone(),
            [0xa1; 32],
            exact_definition,
            exact_amount,
            true,
        ),
        schema5_reverse_asset_eligibility(
            &fixture,
            exact_plan.clone(),
            exact_custody,
            exact_definition,
            exact_amount + 1,
            true,
        ),
        schema5_reverse_asset_eligibility(
            &fixture,
            exact_plan,
            exact_custody,
            [0xa2; 32],
            exact_amount,
            true,
        ),
    ];
    for eligibility in cases {
        assert_eq!(
            validate_fresh_maker_lock_plan(
                &fixture.config,
                &fixture.agreement,
                &fixture.agreement_wire,
                eligibility,
                true,
            ),
            Err(ActorCommandError::ObservationUnavailable)
        );
    }
}

#[test]
fn schema5_asset_scan_never_turns_uncertainty_unavailability_or_conflict_into_send_authority() {
    let mut fixture =
        ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    let _ = configure_schema5_asset_material(&mut fixture);
    let plan = load_prepared_maker_lock_material(&fixture.config, &fixture.agreement)
        .expect("asset maker material")
        .plan()
        .clone();
    let step = &plan.steps()[0];
    let window = fixture.config.discovery_window().expect("window");
    let height = window.start_height() + u64::from(window.max_blocks()) - 1;
    let clock = ChainClock::new(Hex32::from_bytes([0xb0; 32]), height, 1_800_000_000_000);
    for outcome in [
        FinalizedWitnessedAssetScanOutcomeV2::<()>::Uncertain {
            finalized_clock: clock,
            scanned_window: window,
        },
        FinalizedWitnessedAssetScanOutcomeV2::<()>::Unavailable {
            reason:
                lez_bridge_protocol::FinalizedWitnessedAssetUnavailableReasonV2::HistoryUnavailable,
        },
    ] {
        let observation = asset_scan_step_observation(step, outcome, |()| unreachable!());
        assert_eq!(observation, MakerLockStepChainObservationV1::Uncertain);
        assert!(!observation.can_authorize_submission());
    }

    let conflicting = ObservedTransactionFacts::new(
        TransactionId::from_bytes(
            <[u8; 32]>::try_from(
                hex::decode(step.expected_public_id().as_str()).expect("hex transaction ID"),
            )
            .expect("32-byte transaction ID"),
        ),
        ExactTransactionBytes::new(b"conflicting-canonical-bytes".to_vec())
            .expect("conflicting bytes"),
        ChainPosition::new(Hex32::from_bytes([0xb1; 32]), height, 0),
        AccountIds::new(vec![Hex32::from_bytes([0xb2; 32])]).expect("signer"),
        true,
    );
    let observation = asset_scan_step_observation(
        step,
        FinalizedWitnessedAssetScanOutcomeV2::Found {
            finalized_clock: clock,
            scanned_window: window,
            facts: Box::new(conflicting),
        },
        |facts| facts,
    );
    assert_eq!(
        observation,
        MakerLockStepChainObservationV1::ConflictingPresence
    );
    assert!(!observation.can_authorize_submission());
}

#[tokio::test]
async fn schema5_three_step_asset_lock_is_ordered_and_uncertainty_never_submits() {
    for observation in [
        MakerLockStepChainObservationV1::Uncertain,
        MakerLockStepChainObservationV1::ConflictingPresence,
    ] {
        let mut fixture =
            ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
        let _ = configure_schema5_asset_material(&mut fixture);
        activate_and_project_taker_lock(&fixture).await;
        let plan = load_prepared_maker_lock_material(&fixture.config, &fixture.agreement)
            .expect("asset maker material")
            .plan()
            .clone();
        let port = FixedMakerLockPort::new(
            observation,
            fresh_maker_eligibility(&fixture),
            exact_maker_lock_complete_observation(&fixture.agreement, &plan, 0xa3),
        );
        let output = drive_maker_lock_with_port(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &port,
        )
        .await
        .expect("uncertainty remains observation-only");
        assert_eq!(output.revision, 1);
        assert_eq!(port.submissions(), 0);
        assert_eq!(port.eligibility_checks(), 0);
    }

    let mut fixture =
        ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    let _ = configure_schema5_asset_material(&mut fixture);
    activate_and_project_taker_lock(&fixture).await;
    let plan = load_prepared_maker_lock_material(&fixture.config, &fixture.agreement)
        .expect("asset maker material")
        .plan()
        .clone();
    let eligibility = fresh_maker_eligibility(&fixture);
    let complete = exact_maker_lock_complete_observation(&fixture.agreement, &plan, 0xa4);
    for (index, step) in plan.steps().iter().enumerate() {
        let absent = FixedMakerLockPort::new(
            MakerLockStepChainObservationV1::Absent,
            eligibility.clone(),
            complete.clone(),
        )
        .with_submission_result(BtcMakerLockSubmissionResult::Accepted(
            step.expected_public_id().as_str().into(),
        ));
        let sent = drive_maker_lock_with_port(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &absent,
        )
        .await
        .expect("exact asset step submitted once");
        assert_eq!(sent.revision, 1);
        assert_eq!(absent.submissions(), 1);
        assert_eq!(absent.eligibility_checks(), 1);

        let present = FixedMakerLockPort::new(
            MakerLockStepChainObservationV1::PresentExactCanonical {
                expected_public_id: step.expected_public_id().as_str().into(),
                exact_public_bytes: step.exact_bytes().as_slice().to_vec(),
            },
            eligibility.clone(),
            complete.clone(),
        );
        let observed = drive_maker_lock_with_port(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &present,
        )
        .await
        .unwrap_or_else(|error| {
            panic!(
                "asset step {index} failed: {error:?}; observed {:?}",
                present.observed_steps()
            )
        });
        assert_eq!(
            observed.revision,
            if index + 1 == plan.steps().len() {
                2
            } else {
                1
            }
        );
    }
}

#[tokio::test]
async fn schema4_changed_lez_preparation_result_conflicts_with_durable_intent() {
    let mut fixture =
        ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    configure_schema4_maker_material(&mut fixture);
    activate_and_project_taker_lock(&fixture).await;
    let plan = load_prepared_maker_lock_material(&fixture.config, &fixture.agreement)
        .expect("maker plan")
        .plan()
        .clone();
    let eligibility = fresh_maker_eligibility(&fixture);
    let complete = exact_maker_lock_complete_observation(&fixture.agreement, &plan, 69);
    let first = FixedMakerLockPort::new(
        MakerLockStepChainObservationV1::Absent,
        eligibility.clone(),
        complete.clone(),
    );
    drive_maker_lock_with_port(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &first,
    )
    .await
    .expect("stage immutable intent before first send");
    assert_eq!(first.submissions(), 1);

    let result_path = match fixture.config.maker_lock.as_ref().expect("maker material") {
        MakerLockMaterialConfig::Lez {
            preparation_result_file,
            ..
        } => preparation_result_file,
        MakerLockMaterialConfig::Bitcoin { .. } | MakerLockMaterialConfig::LezAssetV2 { .. } => {
            unreachable!("schema-4 forward maker uses native LEZ material")
        }
    };
    let mut changed: PrepareWitnessedEscrowResult =
        serde_json::from_slice(&fs::read(result_path).expect("result bytes"))
            .expect("decode result");
    changed.funding = PreparedTransaction::new(
        changed.funding.transaction_id,
        ExactTransactionBytes::new(b"changed-exact-lez-funding-result".to_vec())
            .expect("changed bytes"),
    );
    fs::write(
        result_path,
        serde_json::to_vec(&changed).expect("changed result JSON"),
    )
    .expect("change result");

    let retry = FixedMakerLockPort::new(
        MakerLockStepChainObservationV1::Absent,
        eligibility,
        complete,
    );
    assert_eq!(
        drive_maker_lock_with_port(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &retry,
        )
        .await,
        Err(ActorCommandError::ProjectionUnavailable)
    );
    assert_eq!(retry.events(), Vec::<&'static str>::new());
    assert_eq!(retry.submissions(), 0);
}

#[tokio::test]
async fn schema4_maker_send_requires_fresh_exact_first_lock_and_strict_pre_cutoff_time() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let mut fixture = ActorFixture::for_direction(direction, ActorRole::Maker);
        configure_schema4_maker_material(&mut fixture);
        activate_and_project_taker_lock(&fixture).await;
        let plan = load_prepared_maker_lock_material(&fixture.config, &fixture.agreement)
            .expect("maker plan")
            .plan()
            .clone();
        let complete = exact_maker_lock_complete_observation(&fixture.agreement, &plan, 70);

        let mut at_cutoff = fresh_maker_eligibility(&fixture);
        at_cutoff.current_maker_chain_time = maker_time_at_cutoff(&fixture.agreement);
        let cutoff_port = FixedMakerLockPort::new(
            MakerLockStepChainObservationV1::Absent,
            at_cutoff,
            complete.clone(),
        );
        assert_eq!(
            drive_maker_lock_with_port(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &cutoff_port,
            )
            .await,
            Err(ActorCommandError::ObservationUnavailable)
        );
        assert_eq!(cutoff_port.submissions(), 0);
        assert_eq!(
            cutoff_port.events(),
            vec!["observe_step", "fresh_eligibility"]
        );

        let drift_port = FixedMakerLockPort::new(
            MakerLockStepChainObservationV1::Absent,
            mismatched_fresh_maker_eligibility(&fixture),
            complete,
        );
        assert_eq!(
            drive_maker_lock_with_port(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &drift_port,
            )
            .await,
            Err(ActorCommandError::ObservationUnavailable)
        );
        assert_eq!(drift_port.submissions(), 0);

        let journal =
            SqliteBtcMakerLockJournal::open(&fixture.config.state_db).expect("maker journal");
        assert_eq!(
            journal
                .load_intent(fixture.agreement.coordinator().id())
                .expect("load intent"),
            None
        );

        let mut store = open_existing_store(
            &fixture.config,
            &fixture.agreement,
            fixture.agreement_wire.clone(),
        )
        .expect("maker recovery store");
        let maker_chain = fixture
            .agreement
            .coordinator()
            .funded_chain(Participant::Maker);
        let generic = BtcLifecycleEvidenceV1::maker_lock(
            maker_chain,
            "generic-maker-lock",
            if maker_chain == Chain::Bitcoin {
                fixture.agreement.required_bitcoin_confirmations()
            } else {
                FINALIZED_LEZ_CONFIRMATION_UNITS
            },
            b"generic projection is forbidden".to_vec(),
        )
        .expect("generic evidence");
        assert!(matches!(
            store.project(1, &generic),
            Err(BtcRecoveryError::InvalidSequence { revision: 2 })
        ));
    }
}

#[tokio::test]
async fn schema4_exact_idempotent_lez_admission_sends_once_without_claiming_absence() {
    let mut fixture =
        ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    configure_schema4_maker_material(&mut fixture);
    activate_and_project_taker_lock(&fixture).await;
    let plan = load_prepared_maker_lock_material(&fixture.config, &fixture.agreement)
        .expect("maker plan")
        .plan()
        .clone();
    let initialization = &plan.steps()[0];
    let observation = MakerLockStepChainObservationV1::ExactIdempotentSubmissionSafe {
        expected_public_id: initialization.expected_public_id().as_str().into(),
        exact_public_bytes: initialization.exact_bytes().as_slice().to_vec(),
    };
    let first = FixedMakerLockPort::new(
        observation.clone(),
        fresh_maker_eligibility(&fixture),
        exact_maker_lock_complete_observation(&fixture.agreement, &plan, 81),
    );
    let output = drive_maker_lock_with_port(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &first,
    )
    .await
    .expect("exact idempotent admission");
    assert!(matches!(
        output.outcome,
        ActorEffectOutcomeV1::AwaitingObservation {
            chain: ActorChainV1::Lez
        }
    ));
    assert_eq!(first.submissions(), 1);
    assert_eq!(
        first.events(),
        vec!["observe_step", "fresh_eligibility", "submit_step"]
    );

    let restarted = FixedMakerLockPort::new(
        observation,
        fresh_maker_eligibility(&fixture),
        exact_maker_lock_complete_observation(&fixture.agreement, &plan, 82),
    );
    drive_maker_lock_with_port(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &restarted,
    )
    .await
    .expect("exact idempotent restart remains observe-only");
    assert_eq!(restarted.submissions(), 0);
}

#[test]
fn schema4_live_lez_requests_bind_exact_plan_and_distinct_operations() {
    let mut fixture =
        ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    configure_schema4_maker_material(&mut fixture);
    let material = load_prepared_maker_lock_material(&fixture.config, &fixture.agreement)
        .expect("maker material");
    let plan = material.plan();
    let initialization = &plan.steps()[0];
    let funding = &plan.steps()[1];

    let initialization_read =
        maker_lez_initialization_classification_request(&fixture.config, &fixture.agreement, plan)
            .expect("initialization classification request");
    let funding_read =
        maker_lez_funding_classification_request(&fixture.config, &fixture.agreement, funding)
            .expect("funding classification request");
    let initialization_current =
        maker_lez_current_pair_request(&fixture.config, &fixture.agreement, plan, initialization)
            .expect("current initialization request");
    let funding_current =
        maker_lez_current_pair_request(&fixture.config, &fixture.agreement, plan, funding)
            .expect("current funding request");
    let clock_read = maker_lez_current_clock_request(&fixture.config, &fixture.agreement, 1)
        .expect("current clock request");
    let current_funded_read =
        maker_lez_current_funded_request_id(&fixture.config, &fixture.agreement)
            .expect("current funded request ID");
    let initialization_submit =
        maker_lez_submit_request(&fixture.config, &fixture.agreement, initialization)
            .expect("initialization submit request");
    let funding_submit = maker_lez_submit_request(&fixture.config, &fixture.agreement, funding)
        .expect("funding submit request");

    assert_eq!(
        hex::encode(initialization_read.initialization.transaction_id.as_bytes()),
        initialization.expected_public_id().as_str()
    );
    assert_eq!(
        initialization_read.initialization.exact_bytes.as_slice(),
        initialization.exact_bytes().as_slice()
    );
    assert_eq!(
        hex::encode(initialization_read.funding_transaction_id.as_bytes()),
        funding.expected_public_id().as_str()
    );
    assert!(matches!(
        funding_read.target,
        FinalizedWitnessedFundingObservationTarget::Exact {
            funding_transaction_id
        } if hex::encode(funding_transaction_id.as_bytes())
            == funding.expected_public_id().as_str()
    ));
    assert_eq!(clock_read.runtime, fixture.config.lez_bridge.runtime);
    assert_eq!(clock_read.context.sidecar_role, BridgeParticipant::Maker);
    for current in [&initialization_current, &funding_current] {
        assert!(matches!(
            current.target,
            EscrowObservationTarget::Exact {
                initialization_transaction_id,
                funding_transaction_id,
            } if hex::encode(initialization_transaction_id.as_bytes())
                == initialization.expected_public_id().as_str()
                && hex::encode(funding_transaction_id.as_bytes())
                    == funding.expected_public_id().as_str()
        ));
    }
    assert_eq!(
        initialization_submit.transaction,
        initialization_read.initialization
    );
    assert_eq!(
        funding_submit.transaction.exact_bytes.as_slice(),
        funding.exact_bytes().as_slice()
    );

    let request_ids = [
        initialization_read.context.request_id.as_str(),
        funding_read.context.request_id.as_str(),
        initialization_current.context.request_id.as_str(),
        funding_current.context.request_id.as_str(),
        clock_read.context.request_id.as_str(),
        current_funded_read.as_str(),
        initialization_submit.context.request_id.as_str(),
        funding_submit.context.request_id.as_str(),
    ];
    for (index, request_id) in request_ids.iter().enumerate() {
        assert!(!request_ids[index + 1..].contains(request_id));
    }
}

#[test]
fn schema4_bitcoin_maker_material_matches_runner_hex_and_sdk_vocabulary() {
    let mut fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Maker);
    configure_schema4_maker_material(&mut fixture);
    let PreparedMakerLockMaterialV1::Bitcoin(prepared) =
        load_prepared_maker_lock_material(&fixture.config, &fixture.agreement)
            .expect("runner-shaped hex artifact must load")
    else {
        panic!("reverse direction must prepare Bitcoin Maker funding");
    };
    let [step] = prepared.plan().steps() else {
        panic!("Bitcoin Maker plan must have one step");
    };
    assert_eq!(step.step().as_str(), "bitcoin.funding");
    assert!(bitcoin_maker_step_is_supported(step));
}

#[test]
fn live_maker_fresh_read_ordinals_are_bounded_to_one_drive() {
    let counter = AtomicU8::new(0);
    assert_eq!(next_fresh_read_ordinal(&counter).unwrap(), 1);
    assert_eq!(next_fresh_read_ordinal(&counter).unwrap(), 2);
    assert_eq!(
        next_fresh_read_ordinal(&counter),
        Err(ActorCommandError::ObservationUnavailable)
    );
}

#[tokio::test]
async fn schema4_timely_canonical_maker_lock_reconciles_after_current_cutoff() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let mut fixture = ActorFixture::for_direction(direction, ActorRole::Maker);
        configure_schema4_maker_material(&mut fixture);
        activate_and_project_taker_lock(&fixture).await;
        let plan = load_prepared_maker_lock_material(&fixture.config, &fixture.agreement)
            .expect("maker plan")
            .plan()
            .clone();

        for (index, step) in plan.steps().iter().enumerate() {
            let mut after_cutoff = fresh_maker_eligibility(&fixture);
            after_cutoff.current_maker_chain_time = maker_time_at_cutoff(&fixture.agreement);
            let port = FixedMakerLockPort::new(
                MakerLockStepChainObservationV1::PresentExactCanonical {
                    expected_public_id: step.expected_public_id().as_str().into(),
                    exact_public_bytes: step.exact_bytes().as_slice().to_vec(),
                },
                after_cutoff,
                exact_maker_lock_complete_observation(&fixture.agreement, &plan, 91),
            );
            let output = drive_maker_lock_with_port(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &port,
            )
            .await
            .expect("canonical inclusion time, not current clock, admits a found lock");
            assert_eq!(port.submissions(), 0);
            if index + 1 == plan.steps().len() {
                assert_eq!(
                    port.events(),
                    vec!["observe_step", "observe_complete", "fresh_eligibility"]
                );
                assert_eq!(output.phase, ActorPhaseV1::BothLegsLocked);
            } else {
                assert_eq!(port.events(), vec!["observe_step"]);
            }
        }
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One scenario proves the complete ordered crash-safety contract.
async fn schema4_maker_lock_is_ordered_no_rearm_and_atomically_closed_in_both_directions() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let mut fixture = ActorFixture::for_direction(direction, ActorRole::Maker);
        configure_schema4_maker_material(&mut fixture);
        activate_and_project_taker_lock(&fixture).await;

        let plan = load_prepared_maker_lock_material(&fixture.config, &fixture.agreement)
            .expect("maker material")
            .plan()
            .clone();
        let eligibility = fresh_maker_eligibility(&fixture);
        let complete = exact_maker_lock_complete_observation(&fixture.agreement, &plan, 88);

        for (index, step) in plan.steps().iter().enumerate() {
            let submission_result = if index == 0 {
                BtcMakerLockSubmissionResult::Accepted(step.expected_public_id().as_str().into())
            } else {
                BtcMakerLockSubmissionResult::Unknown
            };
            let absent = FixedMakerLockPort::new(
                MakerLockStepChainObservationV1::Absent,
                eligibility.clone(),
                complete.clone(),
            )
            .with_submission_result(submission_result.clone());
            let sent = drive_maker_lock_with_port(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &absent,
            )
            .await
            .expect("one authorized maker send");
            assert_eq!(sent.revision, 1);
            assert_eq!(absent.submissions(), 1);
            assert_eq!(absent.eligibility_checks(), 1);
            assert_eq!(
                absent.events(),
                vec!["observe_step", "fresh_eligibility", "submit_step"]
            );

            let after_send = SqliteBtcMakerLockJournal::open(&fixture.config.state_db)
                .expect("journal after node result")
                .load_intent(fixture.agreement.coordinator().id())
                .expect("load intent")
                .expect("intent after send");
            assert_eq!(
                after_send.steps()[index].state(),
                BtcMakerLockStepState::Unknown
            );
            assert_eq!(
                after_send.steps()[index].submission_result(),
                Some(&submission_result)
            );

            let replay = drive_maker_lock_with_port(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &absent,
            )
            .await
            .expect("absence cannot rearm");
            assert_eq!(replay.revision, 1);
            assert_eq!(absent.submissions(), 1);
            assert_eq!(absent.eligibility_checks(), 1);
            assert_eq!(
                absent.events(),
                vec![
                    "observe_step",
                    "fresh_eligibility",
                    "submit_step",
                    "observe_step",
                ]
            );
            assert_eq!(
                absent.observed_steps(),
                vec![step.step().as_str().to_owned(); 2]
            );

            let pending = FixedMakerLockPort::new(
                MakerLockStepChainObservationV1::PresentExactPending {
                    expected_public_id: step.expected_public_id().as_str().into(),
                    exact_public_bytes: step.exact_bytes().as_slice().to_vec(),
                },
                eligibility.clone(),
                complete.clone(),
            );
            let still_pending = drive_maker_lock_with_port(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &pending,
            )
            .await
            .expect("byte-equal unconfirmed observation remains pending");
            assert_eq!(still_pending.revision, 1);
            assert_eq!(pending.submissions(), 0);

            let conflicting_presence = FixedMakerLockPort::new(
                MakerLockStepChainObservationV1::ConflictingPresence,
                eligibility.clone(),
                complete.clone(),
            );
            let conflict_pending = drive_maker_lock_with_port(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &conflicting_presence,
            )
            .await
            .expect("unknown step stays observation-only on conflicting presence");
            assert_eq!(conflict_pending.revision, 1);
            assert_eq!(conflicting_presence.submissions(), 0);

            let mut wrong_bytes = step.exact_bytes().as_slice().to_vec();
            wrong_bytes.push(0xff);
            let conflict = FixedMakerLockPort::new(
                MakerLockStepChainObservationV1::PresentExactCanonical {
                    expected_public_id: step.expected_public_id().as_str().into(),
                    exact_public_bytes: wrong_bytes,
                },
                eligibility.clone(),
                complete.clone(),
            );
            assert_eq!(
                drive_maker_lock_with_port(
                    &fixture.config,
                    fixture.agreement.clone(),
                    fixture.agreement_wire.clone(),
                    &conflict,
                )
                .await,
                Err(ActorCommandError::ProjectionUnavailable)
            );

            let is_final = index + 1 == plan.steps().len();
            let final_pending = ActorFundingObservation::Pending {
                chain: fixture
                    .agreement
                    .coordinator()
                    .funded_chain(Participant::Maker),
            };
            let present = FixedMakerLockPort::new(
                MakerLockStepChainObservationV1::PresentExactCanonical {
                    expected_public_id: step.expected_public_id().as_str().into(),
                    exact_public_bytes: step.exact_bytes().as_slice().to_vec(),
                },
                eligibility.clone(),
                if is_final {
                    final_pending
                } else {
                    complete.clone()
                },
            );
            let observed = drive_maker_lock_with_port(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &present,
            )
            .await
            .expect("canonical exact observation advances ordered maker plan");
            assert_eq!(present.submissions(), 0);
            assert_eq!(observed.revision, 1);

            if is_final {
                let mut wrong_complete = complete.clone();
                let ActorFundingObservation::Ready { transaction_id, .. } = &mut wrong_complete
                else {
                    unreachable!("complete observation")
                };
                *transaction_id = "different-plan-final-id".into();
                let wrong_final = FixedMakerLockPort::new(
                    MakerLockStepChainObservationV1::Uncertain,
                    eligibility.clone(),
                    wrong_complete,
                );
                assert_eq!(
                    drive_maker_lock_with_port(
                        &fixture.config,
                        fixture.agreement.clone(),
                        fixture.agreement_wire.clone(),
                        &wrong_final,
                    )
                    .await,
                    Err(ActorCommandError::AgreementBindingInvalid)
                );
                assert_eq!(wrong_final.events(), vec!["observe_complete"]);

                let stale_first = FixedMakerLockPort::new(
                    MakerLockStepChainObservationV1::Uncertain,
                    mismatched_fresh_maker_eligibility(&fixture),
                    complete.clone(),
                );
                assert_eq!(
                    drive_maker_lock_with_port(
                        &fixture.config,
                        fixture.agreement.clone(),
                        fixture.agreement_wire.clone(),
                        &stale_first,
                    )
                    .await,
                    Err(ActorCommandError::ObservationUnavailable)
                );
                assert_eq!(
                    stale_first.events(),
                    vec!["observe_complete", "fresh_eligibility"]
                );

                let finalize = FixedMakerLockPort::new(
                    MakerLockStepChainObservationV1::Uncertain,
                    eligibility.clone(),
                    complete.clone(),
                );
                let closed = drive_maker_lock_with_port(
                    &fixture.config,
                    fixture.agreement.clone(),
                    fixture.agreement_wire.clone(),
                    &finalize,
                )
                .await
                .expect("fresh first lock permits atomic final projection");
                assert_eq!(closed.revision, 2);
                assert_eq!(finalize.observed_steps(), Vec::<String>::new());
                assert_eq!(finalize.eligibility_checks(), 1);
                assert_eq!(
                    finalize.events(),
                    vec!["observe_complete", "fresh_eligibility"]
                );
            }
        }

        let journal =
            SqliteBtcMakerLockJournal::open(&fixture.config.state_db).expect("maker journal");
        let snapshot = journal
            .load_intent(fixture.agreement.coordinator().id())
            .expect("load intent")
            .expect("durable intent");
        assert_eq!(snapshot.closed_revision(), Some(2));
        assert!(
            snapshot
                .steps()
                .iter()
                .all(|step| step.state() == BtcMakerLockStepState::Accepted)
        );
        let store = open_existing_store(
            &fixture.config,
            &fixture.agreement,
            fixture.agreement_wire.clone(),
        )
        .expect("reopen actor store");
        assert_eq!(store.status().expect("status").revision(), 2);
    }
}

#[test]
fn claim_recovery_config_requires_distinct_sessions_and_paths() {
    let fixture = ActorFixture::new();
    let mut config = fixture.config.clone();
    let valid = config.signing.clone();
    config
        .validate()
        .expect("distinct claim recovery authority");

    let mut zero = valid.clone();
    zero.bitcoin.session_id = Hex32::from_bytes([0; 32]);
    config.signing = zero;
    assert_eq!(config.validate(), Err(ActorConfigError::Invalid));

    let mut reused_id = valid.clone();
    reused_id.lez.session_id = reused_id.bitcoin.session_id;
    config.signing = reused_id;
    assert_eq!(config.validate(), Err(ActorConfigError::Invalid));

    let mut missing_taker_secret = valid.clone();
    missing_taker_secret.adaptor_secret_file = None;
    config.signing = missing_taker_secret;
    assert_eq!(config.validate(), Err(ActorConfigError::Invalid));

    let mut aliased_path = valid.clone();
    aliased_path.lez.journal_db = aliased_path.bitcoin.journal_db.clone();
    config.signing = aliased_path;
    assert_eq!(config.validate(), Err(ActorConfigError::Invalid));

    let mut aliased_secret = valid;
    aliased_secret.adaptor_secret_file = Some(aliased_secret.bitcoin.journal_db.clone());
    config.signing = aliased_secret;
    assert_eq!(config.validate(), Err(ActorConfigError::Invalid));

    let mut maker = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    assert_eq!(maker.config.signing.adaptor_secret_file, None);
    maker.config.signing.adaptor_secret_file =
        Some(maker.directory.path().join("unexpected-maker-secret"));
    assert_eq!(maker.config.validate(), Err(ActorConfigError::Invalid));

    let debug = format!("{:?}", fixture.config);
    assert!(!debug.contains("adaptor-secret.key"));
    assert!(!debug.contains("bitcoin-refund.key"));
    assert!(!debug.contains(&hex::encode(support::ADAPTOR_SECRET)));
    assert!(!debug.contains(&hex::encode(support::REFUND_SECRET)));
}

#[tokio::test(flavor = "current_thread")]
async fn activation_requires_a_private_point_matching_taker_adaptor_secret() {
    for mutation in [
        "missing",
        "short",
        "wrong",
        "world_readable",
        "symlink",
        "hardlink",
    ] {
        let fixture = ActorFixture::new();
        let secret_path = fixture
            .config
            .signing
            .adaptor_secret_file
            .as_ref()
            .expect("taker secret path");
        match mutation {
            "missing" => fs::remove_file(secret_path).expect("remove secret"),
            "short" => {
                fs::write(secret_path, b"53\n").expect("write short secret");
            }
            "wrong" => write_private_secret(secret_path, [0x54; 32]),
            "world_readable" => {
                fs::set_permissions(secret_path, fs::Permissions::from_mode(0o644))
                    .expect("loosen secret permissions");
            }
            "symlink" => {
                let target = fixture.directory.path().join("secret-target");
                write_private_secret(&target, support::ADAPTOR_SECRET);
                fs::remove_file(secret_path).expect("remove original secret");
                symlink(&target, secret_path).expect("replace with symlink");
            }
            "hardlink" => {
                let alias = fixture.directory.path().join("secret-hardlink");
                fs::hard_link(secret_path, alias).expect("create hard-link alias");
            }
            _ => unreachable!("fixed mutation"),
        }
        let error = execute_actor_command(&fixture.config, ActorCommand::Activate)
            .await
            .expect_err("unsafe or mismatched secret must fail closed");
        assert_eq!(
            error,
            ActorCommandError::ActivationMaterialUnavailable,
            "mutation: {mutation}"
        );
        assert!(!fixture.config.state_db.exists(), "mutation: {mutation}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn activation_requires_role_shaped_private_bitcoin_refund_authority() {
    for mutation in [
        "missing",
        "short",
        "cross_role",
        "world_readable",
        "symlink",
        "hardlink",
    ] {
        let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Maker);
        assert_eq!(fixture.agreement.bitcoin_funder(), Participant::Maker);
        let secret_path = fixture
            .config
            .refund
            .bitcoin_refund_key_file
            .as_ref()
            .expect("Bitcoin funder refund key path");
        match mutation {
            "missing" => fs::remove_file(secret_path).expect("remove refund key"),
            "short" => fs::write(secret_path, b"53\n").expect("write short refund key"),
            "cross_role" => write_private_secret(secret_path, [4; 32]),
            "world_readable" => {
                fs::set_permissions(secret_path, fs::Permissions::from_mode(0o644))
                    .expect("loosen refund-key permissions");
            }
            "symlink" => {
                let target = fixture.directory.path().join("refund-key-target");
                write_private_secret(&target, [3; 32]);
                fs::remove_file(secret_path).expect("remove original refund key");
                symlink(&target, secret_path).expect("replace refund key with symlink");
            }
            "hardlink" => {
                let alias = fixture.directory.path().join("refund-key-hardlink");
                fs::hard_link(secret_path, alias).expect("create refund-key hard link");
            }
            _ => unreachable!("fixed mutation"),
        }

        let error = execute_actor_command(&fixture.config, ActorCommand::Activate)
            .await
            .expect_err("missing, mismatched, or unsafe refund key must fail closed");
        assert_eq!(
            error,
            ActorCommandError::ActivationMaterialUnavailable,
            "mutation: {mutation}"
        );
        assert!(!fixture.config.state_db.exists(), "mutation: {mutation}");
    }

    let mut nonowner = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    assert_ne!(
        nonowner.config.role.sdk(),
        nonowner.agreement.bitcoin_funder()
    );
    assert_eq!(nonowner.config.refund.bitcoin_refund_key_file, None);
    let forbidden_path = nonowner.directory.path().join("forbidden-refund-key");
    write_private_secret(&forbidden_path, [3; 32]);
    nonowner.config.refund.bitcoin_refund_key_file = Some(forbidden_path);
    nonowner
        .config
        .validate()
        .expect("the agreement-bound role check occurs at activation");
    let error = execute_actor_command(&nonowner.config, ActorCommand::Activate)
        .await
        .expect_err("non-funder must not carry Bitcoin refund authority");
    assert_eq!(error, ActorCommandError::ActivationMaterialUnavailable);
    assert!(!nonowner.config.state_db.exists());
}

#[tokio::test(flavor = "current_thread")]
async fn activation_requires_existing_signing_material_before_state_creation() {
    let fixture = ActorFixture::without_activation_material();
    assert!(!fixture.config.state_db.exists());

    let error = execute_actor_command(&fixture.config, ActorCommand::Activate)
        .await
        .expect_err("missing signer journals and prepared claim must fail closed");
    assert_eq!(error, ActorCommandError::ActivationMaterialUnavailable);
    assert!(
        !fixture.config.state_db.exists(),
        "failed pre-lock validation must not create actor state"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn activation_rejects_crosswired_journals_and_prepared_result_drift() {
    for mutation in [
        "journal_domain",
        "run",
        "claimant",
        "request",
        "invalid_hash",
        "valid_other_message",
    ] {
        let mut fixture = ActorFixture::new();
        match mutation {
            "journal_domain" => {
                std::mem::swap(
                    &mut fixture.config.signing.bitcoin.journal_db,
                    &mut fixture.config.signing.lez.journal_db,
                );
            }
            "run" | "claimant" | "request" | "invalid_hash" | "valid_other_message" => {
                let bytes = fs::read(&fixture.config.signing.prepared_witnessed_claim_result_file)
                    .expect("prepared");
                let mut prepared: PrepareWitnessedClaimResult =
                    serde_json::from_slice(&bytes).expect("prepared result");
                match mutation {
                    "run" => {
                        prepared.context.run_id =
                            RunId::new("different-run").expect("different run");
                    }
                    "claimant" => {
                        prepared.context.sidecar_role = match prepared.context.sidecar_role {
                            BridgeParticipant::Maker => BridgeParticipant::Taker,
                            BridgeParticipant::Taker => BridgeParticipant::Maker,
                        };
                    }
                    "request" => {
                        prepared.context.request_id =
                            RequestId::new("different-request").expect("different request");
                    }
                    "invalid_hash" => {
                        prepared.claim.message_hash = Hex32::from_bytes([1; 32]);
                    }
                    "valid_other_message" => {
                        let other = b"internally-valid-but-not-agreement-bound".to_vec();
                        prepared.claim.message_hash =
                            Hex32::from_bytes(support::lez_message_hash(&other));
                        prepared.claim.exact_message_bytes =
                            ExactMessageBytes::new(other).expect("other message");
                    }
                    _ => unreachable!("fixed mutation"),
                }
                fs::write(
                    &fixture.config.signing.prepared_witnessed_claim_result_file,
                    serde_json::to_vec(&prepared).expect("mutated result"),
                )
                .expect("rewrite prepared result");
            }
            _ => unreachable!("fixed mutation"),
        }
        fixture
            .config
            .validate()
            .expect("structurally valid config");
        let error = execute_actor_command(&fixture.config, ActorCommand::Activate)
            .await
            .expect_err("crosswired activation material must fail closed");
        assert_eq!(
            error,
            ActorCommandError::ActivationMaterialUnavailable,
            "mutation: {mutation}"
        );
        assert!(!fixture.config.state_db.exists(), "mutation: {mutation}");
    }
}

#[test]
fn finalized_lez_request_identity_binds_the_complete_discovery_window() {
    let fixture = ActorFixture::new();
    let original = finalized_lez_funding_request(&fixture.config, &fixture.agreement)
        .expect("original request");
    let exact_retry =
        finalized_lez_funding_request(&fixture.config, &fixture.agreement).expect("exact retry");
    assert_eq!(exact_retry, original);
    assert_eq!(original.context.request_id.as_str().len(), 64);

    let mut changed_start_config = fixture.config.clone();
    changed_start_config.lez_bridge.discovery_start_height += 1;
    let changed_start = finalized_lez_funding_request(&changed_start_config, &fixture.agreement)
        .expect("changed-start request");
    assert_ne!(
        changed_start.context.request_id,
        original.context.request_id
    );
    assert_eq!(
        changed_start.window.start_height(),
        original.window.start_height() + 1
    );
    assert_eq!(
        changed_start.window.max_blocks(),
        original.window.max_blocks()
    );
    assert_eq!(changed_start.terms, original.terms);
    assert_eq!(changed_start.runtime, original.runtime);

    let mut changed_max_config = fixture.config.clone();
    changed_max_config.lez_bridge.discovery_max_blocks += 1;
    let changed_max = finalized_lez_funding_request(&changed_max_config, &fixture.agreement)
        .expect("changed-max request");
    assert_ne!(changed_max.context.request_id, original.context.request_id);
    assert_ne!(
        changed_max.context.request_id,
        changed_start.context.request_id
    );
    assert_eq!(
        changed_max.window.start_height(),
        original.window.start_height()
    );
    assert_eq!(
        changed_max.window.max_blocks(),
        original.window.max_blocks() + 1
    );
    assert_eq!(changed_max.terms, original.terms);
    assert_eq!(changed_max.runtime, original.runtime);
}

fn finalized_funding_facts(
    request: &ObserveFinalizedWitnessedFundingRequest,
    agreement: &BtcAgreementV1,
) -> FinalizedWitnessedFundingFacts {
    let block_hash = Hex32::from_bytes([93; 32]);
    let metadata_id = Hex32::from_bytes(*agreement.lez_terms().metadata_account());
    let custody_id = Hex32::from_bytes(*agreement.lez_terms().custody_account());
    FinalizedWitnessedFundingFacts::new(
        ObservedTransactionFacts::new(
            TransactionId::from_bytes([90; 32]),
            ExactTransactionBytes::new(vec![90; 128]).expect("exact transaction bytes"),
            ChainPosition::new(block_hash, 4, 0),
            AccountIds::new(vec![request.terms.depositor_account_id()])
                .expect("single funding signer"),
            true,
        ),
        NativeFundInstructionFacts::new(
            request.runtime.escrow_program_id,
            AccountIds::new(vec![
                metadata_id,
                custody_id,
                request.terms.depositor_account_id(),
            ])
            .expect("fund account order"),
            request.terms.swap_id(),
        ),
        FinalizedBlockIdentity::new(4, block_hash, 1_850_000_000_050),
        WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
            metadata_id,
            request.runtime.escrow_program_id,
            custody_id,
            &request.terms,
            EscrowState::Funded,
        ),
        NativeCustodyFacts::new(
            custody_id,
            request.terms.authenticated_transfer_program_id(),
            request.terms.amount().as_u128(),
        ),
    )
}

#[test]
fn finalized_lez_evidence_retains_the_authenticated_prefix() {
    let mut fixture = ActorFixture::new();
    fixture.config.lez_bridge.discovery_max_blocks = 20;
    let request = finalized_lez_funding_request(&fixture.config, &fixture.agreement)
        .expect("signed witnessed terms");
    let funding = finalized_funding_facts(&request, &fixture.agreement);
    let finalized_clock = ChainClock::new(Hex32::from_bytes([95; 32]), 11, 1_850_000_000_110);
    let scanned_window = DiscoveryWindow::new(1, 11).expect("same-start finalized prefix");

    let encoded = encode_finalized_lez_funding_evidence(
        &fixture.config,
        &fixture.agreement,
        &request,
        finalized_clock,
        scanned_window,
        &funding,
    )
    .expect("durable LEZ evidence");
    let decoded =
        decode_finalized_lez_funding_evidence(&fixture.config, &fixture.agreement, &encoded)
            .expect("offline evidence audit");
    assert_eq!(decoded.request, request);
    let value: Value = serde_json::from_slice(&encoded).expect("evidence JSON");
    let keys: std::collections::BTreeSet<_> = value
        .as_object()
        .expect("evidence object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        std::collections::BTreeSet::from([
            "agreement_commitment",
            "finalized_clock",
            "funding",
            "request",
            "scanned_window",
            "schema_version",
        ])
    );
    assert_eq!(
        value["request"]["context"]["run_id"],
        "m3-internal-actor-test"
    );
    assert_eq!(value["request"]["runtime"]["compatibility"], "lee_v0_2_0");
    assert_eq!(value["request"]["window"]["start_height"], 1);
    assert_eq!(
        value["request"]["terms"]["terms_hash"],
        hex::encode(fixture.agreement.agreement_commitment())
    );
    assert_eq!(value["schema_version"], 2);
    assert_eq!(value["finalized_clock"]["height"], 11);
    assert_eq!(
        value["finalized_clock"]["block_hash"],
        hex::encode([95; 32])
    );
    assert_eq!(value["scanned_window"]["max_blocks"], 11);
    assert_eq!(
        value["agreement_commitment"],
        hex::encode(fixture.agreement.agreement_commitment())
    );
    assert_eq!(value["funding"]["containing_block"]["block_id"], 4);

    for mutation in ["unknown", "missing", "changed_terms", "shifted_prefix"] {
        let mut changed = value.clone();
        match mutation {
            "unknown" => {
                changed
                    .as_object_mut()
                    .expect("evidence object")
                    .insert("unexpected".to_owned(), Value::Bool(true));
            }
            "missing" => {
                changed
                    .as_object_mut()
                    .expect("evidence object")
                    .remove("request");
            }
            "changed_terms" => {
                changed["request"]["terms"]["terms_hash"] = Value::String("00".repeat(32));
            }
            "shifted_prefix" => {
                changed["scanned_window"]["start_height"] = Value::from(2);
            }
            _ => unreachable!("fixed mutation"),
        }
        assert!(
            decode_finalized_lez_funding_evidence(
                &fixture.config,
                &fixture.agreement,
                &serde_json::to_vec(&changed).expect("mutated JSON"),
            )
            .is_err(),
            "mutation must fail: {mutation}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn drive_requires_explicit_activation_before_observation() {
    let fixture = ActorFixture::new();
    let observer = FixedObserver::new(ActorFundingObservation::Pending {
        chain: Chain::Bitcoin,
    });

    let error = drive_with_observer(
        &fixture.config,
        fixture.agreement,
        fixture.agreement_wire,
        &observer,
    )
    .await
    .expect_err("drive must not implicitly activate");
    assert_eq!(error, ActorCommandError::NotActivated);
    assert_eq!(observer.calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn private_empty_or_interrupted_database_is_not_activation() {
    let fixture = ActorFixture::new();
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&fixture.config.state_db)
        .expect("precreate private empty database");
    let observer = FixedObserver::new(ActorFundingObservation::Pending {
        chain: Chain::Bitcoin,
    });

    for _ in 0..2 {
        let status = output_json(
            execute_actor_command(&fixture.config, ActorCommand::Status)
                .await
                .expect("empty or migrated no-acceptance status"),
        );
        assert_eq!(status["state"], "not_activated");
        let error = drive_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &observer,
        )
        .await
        .expect_err("no-acceptance database cannot drive");
        assert_eq!(error, ActorCommandError::NotActivated);
    }
    assert_eq!(observer.calls(), 0);

    let first = output_json(
        execute_actor_command(&fixture.config, ActorCommand::Activate)
            .await
            .expect("explicit first activation"),
    );
    assert_eq!(first["was_replay"], false);
    let replay = output_json(
        execute_actor_command(&fixture.config, ActorCommand::Activate)
            .await
            .expect("exact activation replay"),
    );
    assert_eq!(replay["was_replay"], true);
}

#[tokio::test(flavor = "current_thread")]
async fn ready_first_lock_is_observed_then_projected_once() {
    let fixture = ActorFixture::new();
    execute_actor_command(&fixture.config, ActorCommand::Activate)
        .await
        .expect("activate actor");
    let observer = FixedObserver::new(ActorFundingObservation::Ready {
        chain: Chain::Bitcoin,
        transaction_id: support::swap_fixture()
            .funding
            .compute_txid()
            .to_string()
            .into_boxed_str(),
        confirmations: support::REQUIRED_CONFIRMATIONS,
        canonical_inclusion_time: CanonicalInclusionTimeV1::Bitcoin {
            median_time_unix_seconds: fixture
                .agreement
                .body()
                .recovery_plan()
                .maker_second_lock_cutoff_unix_seconds(),
        },
        chain_evidence: b"canonical-adapter-evidence".to_vec(),
    });

    let projected = output_json(
        drive_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &observer,
        )
        .await
        .expect("project first lock"),
    );
    assert_eq!(projected["outcome"], "observed_then_projected");
    assert_eq!(projected["chain"], "bitcoin");
    assert_eq!(projected["revision"], 1);
    assert_eq!(projected["phase"], "taker_lock_confirmed");
    assert_eq!(observer.calls(), 1);
    let status = output_json(
        execute_actor_command(&fixture.config, ActorCommand::Status)
            .await
            .expect("offline revision-one status"),
    );
    assert_eq!(
        status["next_action"],
        "observe_maker_second_lock_or_recover_taker_leg"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn maker_lock_projects_in_both_directions_for_both_roles() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        for role in [ActorRole::Maker, ActorRole::Taker] {
            let fixture = ActorFixture::for_direction(direction, role);
            activate_and_project_taker_lock(&fixture).await;
            let expected_chain = fixture
                .agreement
                .coordinator()
                .funded_chain(Participant::Maker);
            let observer = FixedObserver::new(maker_lock_observation(&fixture.agreement, 1));

            let projected = output_json(
                drive_with_observer(
                    &fixture.config,
                    fixture.agreement.clone(),
                    fixture.agreement_wire.clone(),
                    &observer,
                )
                .await
                .expect("project maker lock"),
            );
            assert_eq!(projected["outcome"], "observed_then_projected");
            assert_eq!(
                projected["chain"],
                match expected_chain {
                    Chain::Bitcoin => "bitcoin",
                    Chain::Lez => "lez",
                    Chain::Zcash | Chain::Monero => unreachable!("Bitcoin agreement chain set"),
                }
            );
            assert_eq!(projected["revision"], 2);
            assert_eq!(projected["phase"], "both_legs_locked");
            if role == ActorRole::Maker {
                let journal = SqliteBtcMakerLockJournal::open(&fixture.config.state_db)
                    .expect("legacy observation-only journal");
                let snapshot = journal
                    .load_intent(fixture.agreement.coordinator().id())
                    .expect("load legacy intent")
                    .expect("legacy observed intent");
                assert_eq!(snapshot.steps().len(), 1);
                assert_eq!(snapshot.steps()[0].state(), BtcMakerLockStepState::Accepted);
                assert_eq!(snapshot.steps()[0].attempt_count(), 0);
                assert_eq!(snapshot.steps()[0].submission_result(), None);
            }
            assert_eq!(observer.calls(), 1);
            assert_eq!(observer.transitions(), vec![FundingTransition::MakerLock]);

            let later = output_json(
                drive_with_observer(
                    &fixture.config,
                    fixture.agreement.clone(),
                    fixture.agreement_wire.clone(),
                    &observer,
                )
                .await
                .expect("revision two is outside this slice"),
            );
            assert_eq!(later["outcome"], "not_yet_composed");
            assert_eq!(later["durable_revision"], 2);
            assert_eq!(observer.calls(), 1, "revision two must not observe");

            let status = output_json(
                execute_actor_command(&fixture.config, ActorCommand::Status)
                    .await
                    .expect("offline revision-two status"),
            );
            assert_eq!(status["revision"], 2);
            assert_eq!(status["phase"], "both_legs_locked");
            assert_eq!(status["next_action"], "observe_revealing_claim");
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn maker_lock_cutoff_accepts_before_and_at_but_rejects_after_on_both_chains() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        for offset in [-1_i64, 0] {
            let fixture = ActorFixture::for_direction(direction, ActorRole::Taker);
            activate_and_project_taker_lock(&fixture).await;
            let cutoff = fixture
                .agreement
                .body()
                .recovery_plan()
                .maker_second_lock_cutoff_unix_seconds();
            let inclusion = if offset < 0 {
                cutoff - offset.unsigned_abs()
            } else {
                cutoff
            };
            let observer = FixedObserver::new(maker_lock_observation_at(
                &fixture.agreement,
                u8::try_from(offset + 2).expect("fixture suffix"),
                inclusion,
            ));
            let projected = drive_with_observer(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &observer,
            )
            .await
            .expect("timely maker lock projects");
            assert_eq!(projected.revision, 2);
            assert_eq!(projected.phase, Phase::BothLegsLocked.into());
        }

        let fixture = ActorFixture::for_direction(direction, ActorRole::Taker);
        activate_and_project_taker_lock(&fixture).await;
        let cutoff = fixture
            .agreement
            .body()
            .recovery_plan()
            .maker_second_lock_cutoff_unix_seconds();
        let observer =
            FixedObserver::new(maker_lock_observation_at(&fixture.agreement, 9, cutoff + 1));
        let error = drive_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &observer,
        )
        .await
        .expect_err("late maker lock must fail closed");
        assert_eq!(error, ActorCommandError::AgreementBindingInvalid);
        let status = output_json(
            execute_actor_command(&fixture.config, ActorCommand::Status)
                .await
                .expect("late maker lock leaves revision one"),
        );
        assert_eq!(status["revision"], 1);
        assert_eq!(status["phase"], "taker_lock_confirmed");

        let fixture = ActorFixture::for_direction(direction, ActorRole::Taker);
        activate_and_project_taker_lock(&fixture).await;
        let mut mismatched = maker_lock_observation(&fixture.agreement, 10);
        let ActorFundingObservation::Ready {
            chain,
            canonical_inclusion_time,
            ..
        } = &mut mismatched
        else {
            unreachable!("maker fixture is ready")
        };
        *canonical_inclusion_time = match chain {
            Chain::Bitcoin => CanonicalInclusionTimeV1::Lez { timestamp_ms: 0 },
            Chain::Lez => CanonicalInclusionTimeV1::Bitcoin {
                median_time_unix_seconds: 0,
            },
            Chain::Zcash | Chain::Monero => unreachable!("Bitcoin agreement chain set"),
        };
        let error = drive_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &FixedObserver::new(mismatched),
        )
        .await
        .expect_err("chain/time discriminator mismatch fails closed");
        assert_eq!(error, ActorCommandError::AgreementBindingInvalid);
    }
}

#[test]
fn maker_lock_cutoff_wrapper_is_canonical_and_rejects_mutation() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let fixture = ActorFixture::for_direction(direction, ActorRole::Taker);
        let chain = fixture
            .agreement
            .coordinator()
            .funded_chain(Participant::Maker);
        let cutoff = fixture
            .agreement
            .body()
            .recovery_plan()
            .maker_second_lock_cutoff_unix_seconds();
        let inclusion = match chain {
            Chain::Bitcoin => CanonicalInclusionTimeV1::Bitcoin {
                median_time_unix_seconds: cutoff,
            },
            Chain::Lez => CanonicalInclusionTimeV1::Lez {
                timestamp_ms: cutoff * 1_000,
            },
            Chain::Zcash | Chain::Monero => unreachable!("Bitcoin agreement chain set"),
        };
        let encoded = encode_maker_lock_cutoff_evidence(
            &fixture.agreement,
            chain,
            inclusion,
            b"exact-chain-evidence",
        )
        .expect("canonical cutoff wrapper");
        decode_maker_lock_cutoff_evidence(&fixture.agreement, &encoded)
            .expect("canonical wrapper decodes");
        let value: Value = serde_json::from_slice(&encoded).expect("wrapper JSON");
        for mutation in [
            "unknown",
            "commitment",
            "chain",
            "cutoff",
            "time",
            "payload",
        ] {
            let mut changed = value.clone();
            match mutation {
                "unknown" => {
                    changed
                        .as_object_mut()
                        .expect("wrapper object")
                        .insert("unexpected".to_owned(), Value::Bool(true));
                }
                "commitment" => changed["agreement_commitment"] = Value::String("00".repeat(32)),
                "chain" => {
                    changed["maker_chain"] = Value::String(
                        match chain {
                            Chain::Bitcoin => "lez",
                            Chain::Lez => "bitcoin",
                            Chain::Zcash | Chain::Monero => {
                                unreachable!("Bitcoin agreement chain set")
                            }
                        }
                        .to_owned(),
                    );
                }
                "cutoff" => changed["cutoff_unix_seconds"] = Value::from(cutoff + 1),
                "time" => match chain {
                    Chain::Bitcoin => {
                        changed["canonical_inclusion_time"]["median_time_unix_seconds"] =
                            Value::from(cutoff + 1);
                    }
                    Chain::Lez => {
                        changed["canonical_inclusion_time"]["timestamp_ms"] =
                            Value::from(cutoff * 1_000 + 1);
                    }
                    Chain::Zcash | Chain::Monero => unreachable!("Bitcoin agreement chain set"),
                },
                "payload" => changed["chain_evidence_hex"] = Value::String(String::new()),
                _ => unreachable!("fixed mutation"),
            }
            let mutated = serde_json::to_vec(&changed).expect("mutated wrapper JSON");
            assert!(
                decode_maker_lock_cutoff_evidence(&fixture.agreement, &mutated).is_err(),
                "mutation must fail: {mutation}"
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn pending_maker_lock_preserves_revision_one_in_both_directions() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let fixture = ActorFixture::for_direction(direction, ActorRole::Taker);
        activate_and_project_taker_lock(&fixture).await;
        let expected_chain = fixture
            .agreement
            .coordinator()
            .funded_chain(Participant::Maker);
        let observer = FixedObserver::new(ActorFundingObservation::Pending {
            chain: expected_chain,
        });

        let pending = output_json(
            drive_with_observer(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &observer,
            )
            .await
            .expect("maker lock remains pending"),
        );
        assert_eq!(pending["outcome"], "awaiting_observation");
        assert_eq!(pending["revision"], 1);
        assert_eq!(pending["phase"], "taker_lock_confirmed");
        assert_eq!(observer.calls(), 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn contradictory_maker_lock_chain_fails_before_projection() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    activate_and_project_taker_lock(&fixture).await;
    let expected_chain = fixture
        .agreement
        .coordinator()
        .funded_chain(Participant::Maker);
    let observer = FixedObserver::new(ActorFundingObservation::Pending {
        chain: match expected_chain {
            Chain::Bitcoin => Chain::Lez,
            Chain::Lez => Chain::Bitcoin,
            Chain::Zcash | Chain::Monero => unreachable!("Bitcoin agreement chain set"),
        },
    });

    let error = drive_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &observer,
    )
    .await
    .expect_err("wrong maker-lock chain must fail closed");
    assert_eq!(error, ActorCommandError::AgreementBindingInvalid);
    let status = output_json(
        execute_actor_command(&fixture.config, ActorCommand::Status)
            .await
            .expect("offline status after contradiction"),
    );
    assert_eq!(status["revision"], 1);
    assert_eq!(status["phase"], "taker_lock_confirmed");
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_maker_lock_drives_converge_only_on_revision_two_winner() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_taker_lock(&fixture).await;
    let barrier = Arc::new(Barrier::new(2));
    let first_observer = BarrierObserver {
        barrier: Arc::clone(&barrier),
        observation: maker_lock_observation(&fixture.agreement, 1),
        expected_transition: FundingTransition::MakerLock,
    };
    let second_observer = BarrierObserver {
        barrier,
        observation: maker_lock_observation(&fixture.agreement, 2),
        expected_transition: FundingTransition::MakerLock,
    };
    let first = drive_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &first_observer,
    );
    let second = drive_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &second_observer,
    );

    let (first, second) = tokio::join!(first, second);
    let outputs = [
        output_json(first.expect("first concurrent maker-lock drive")),
        output_json(second.expect("second concurrent maker-lock drive")),
    ];
    let outcomes: std::collections::BTreeSet<_> = outputs
        .iter()
        .map(|output| output["outcome"].as_str().expect("typed outcome"))
        .collect();
    assert_eq!(
        outcomes,
        std::collections::BTreeSet::from([
            "converged_on_existing_projection",
            "observed_then_projected",
        ])
    );
    let converged = outputs
        .iter()
        .find(|output| output["outcome"] == "converged_on_existing_projection")
        .expect("truthful maker-lock winner convergence");
    assert_eq!(converged["durable_revision"], 2);
    assert_eq!(converged["phase"], "both_legs_locked");
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_exact_maker_lock_observation_replays_revision_two_evidence() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    activate_and_project_taker_lock(&fixture).await;
    let barrier = Arc::new(Barrier::new(2));
    let observation = maker_lock_observation(&fixture.agreement, 1);
    let first_observer = BarrierObserver {
        barrier: Arc::clone(&barrier),
        observation: observation.clone(),
        expected_transition: FundingTransition::MakerLock,
    };
    let second_observer = BarrierObserver {
        barrier,
        observation,
        expected_transition: FundingTransition::MakerLock,
    };
    let first = drive_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &first_observer,
    );
    let second = drive_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &second_observer,
    );

    let (first, second) = tokio::join!(first, second);
    let outputs = [
        output_json(first.expect("first exact maker-lock drive")),
        output_json(second.expect("second exact maker-lock drive")),
    ];
    assert!(outputs.iter().all(|output| {
        output["outcome"] == "observed_then_projected" && output["revision"] == 2
    }));
    let replay_values: std::collections::BTreeSet<_> = outputs
        .iter()
        .map(|output| output["was_replay"].as_bool().expect("replay flag"))
        .collect();
    assert_eq!(
        replay_values,
        std::collections::BTreeSet::from([false, true])
    );
}

#[tokio::test(flavor = "current_thread")]
async fn revision_two_gate_prevents_any_later_observer_call() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_taker_lock(&fixture).await;
    let first = FixedObserver::new(maker_lock_observation(&fixture.agreement, 1));
    drive_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &first,
    )
    .await
    .expect("project revision two");
    let forbidden = FixedObserver::new(maker_lock_observation(&fixture.agreement, 2));

    let later = output_json(
        drive_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &forbidden,
        )
        .await
        .expect("later revision remains uncomposed"),
    );
    assert_eq!(later["outcome"], "not_yet_composed");
    assert_eq!(later["durable_revision"], 2);
    assert_eq!(forbidden.calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_revision_zero_drives_converge_on_a_valid_winner() {
    let fixture = ActorFixture::new();
    execute_actor_command(&fixture.config, ActorCommand::Activate)
        .await
        .expect("activate actor");
    let barrier = Arc::new(Barrier::new(2));
    let transaction_id = support::swap_fixture().funding.compute_txid().to_string();
    let observer = |tip_height: u64| BarrierObserver {
        barrier: Arc::clone(&barrier),
        observation: ActorFundingObservation::Ready {
            chain: Chain::Bitcoin,
            transaction_id: transaction_id.clone().into_boxed_str(),
            confirmations: support::REQUIRED_CONFIRMATIONS,
            canonical_inclusion_time: CanonicalInclusionTimeV1::Bitcoin {
                median_time_unix_seconds: fixture
                    .agreement
                    .body()
                    .recovery_plan()
                    .maker_second_lock_cutoff_unix_seconds(),
            },
            chain_evidence: serde_json::to_vec(&serde_json::json!({
                "immutable_funding": "same",
                "finalized_tip": { "height": tip_height }
            }))
            .expect("moving-tip evidence"),
        },
        expected_transition: FundingTransition::TakerLock,
    };
    let first_observer = observer(100);
    let second_observer = observer(101);
    let first = drive_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &first_observer,
    );
    let second = drive_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &second_observer,
    );

    let (first, second) = tokio::join!(first, second);
    let outputs = [
        output_json(first.expect("first concurrent drive")),
        output_json(second.expect("second concurrent drive")),
    ];
    let outcomes: std::collections::BTreeSet<_> = outputs
        .iter()
        .map(|output| output["outcome"].as_str().expect("typed outcome"))
        .collect();
    assert_eq!(
        outcomes,
        std::collections::BTreeSet::from([
            "converged_on_existing_projection",
            "observed_then_projected",
        ])
    );
    let converged = outputs
        .iter()
        .find(|output| output["outcome"] == "converged_on_existing_projection")
        .expect("truthful non-identical winner outcome");
    assert_eq!(converged["durable_revision"], 1);
    assert!(converged.get("was_replay").is_none());
    let status = output_json(
        execute_actor_command(&fixture.config, ActorCommand::Status)
            .await
            .expect("reconstruct concurrent winner"),
    );
    assert_eq!(status["revision"], 1);
    assert_eq!(status["phase"], "taker_lock_confirmed");
}

#[tokio::test(flavor = "current_thread")]
async fn contradictory_pending_chain_is_rejected_before_projection() {
    let fixture = ActorFixture::new();
    execute_actor_command(&fixture.config, ActorCommand::Activate)
        .await
        .expect("activate actor");
    let observer = FixedObserver::new(ActorFundingObservation::Pending { chain: Chain::Lez });

    let error = drive_with_observer(
        &fixture.config,
        fixture.agreement,
        fixture.agreement_wire,
        &observer,
    )
    .await
    .expect_err("wrong pending chain must fail closed");
    assert_eq!(error, ActorCommandError::AgreementBindingInvalid);
    let status = output_json(
        execute_actor_command(&fixture.config, ActorCommand::Status)
            .await
            .expect("offline status after contradiction"),
    );
    assert_eq!(status["revision"], 0);
}

struct FixedClaimObserver {
    observation: ActorClaimObservation,
    calls: AtomicUsize,
}

impl FixedClaimObserver {
    fn new(observation: ActorClaimObservation) -> Self {
        Self {
            observation,
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ClaimObservationPort for FixedClaimObserver {
    async fn observe(
        &self,
        _agreement: &BtcAgreementV1,
        _transition: ClaimTransition,
    ) -> Result<ActorClaimObservation, ActorCommandError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.observation.clone())
    }
}

struct FixedRefundObserver {
    observation: ActorRefundObservation,
    transitions: Mutex<Vec<RefundTransition>>,
}

impl FixedRefundObserver {
    fn new(observation: ActorRefundObservation) -> Self {
        Self {
            observation,
            transitions: Mutex::new(Vec::new()),
        }
    }

    fn transitions(&self) -> Vec<RefundTransition> {
        self.transitions.lock().unwrap().clone()
    }
}

#[async_trait]
impl RefundObservationPort for FixedRefundObserver {
    async fn observe(
        &self,
        _agreement: &BtcAgreementV1,
        transition: RefundTransition,
    ) -> Result<ActorRefundObservation, ActorCommandError> {
        self.transitions.lock().unwrap().push(transition);
        Ok(self.observation.clone())
    }
}

struct FixedFirstLockRecoverySafety {
    observations: Mutex<Vec<FirstLockRecoverySafetyObservation>>,
    calls: AtomicUsize,
}

impl FixedFirstLockRecoverySafety {
    fn ready(agreement: &BtcAgreementV1) -> Self {
        let observation = FirstLockRecoverySafetyObservation::ReadyToRefund {
            maker_chain: agreement.coordinator().funded_chain(Participant::Maker),
            cutoff_unix_seconds: agreement
                .body()
                .recovery_plan()
                .maker_second_lock_cutoff_unix_seconds(),
            observed_unix_seconds: agreement
                .body()
                .recovery_plan()
                .earlier_refund_latest_unix_seconds(),
            absence_evidence: b"canonical-maker-lock-absence-read-1".to_vec(),
        };
        let second = FirstLockRecoverySafetyObservation::ReadyToRefund {
            maker_chain: agreement.coordinator().funded_chain(Participant::Maker),
            cutoff_unix_seconds: agreement
                .body()
                .recovery_plan()
                .maker_second_lock_cutoff_unix_seconds(),
            observed_unix_seconds: agreement
                .body()
                .recovery_plan()
                .earlier_refund_latest_unix_seconds(),
            absence_evidence: b"canonical-maker-lock-absence-read-2".to_vec(),
        };
        Self {
            observations: Mutex::new(vec![observation, second]),
            calls: AtomicUsize::new(0),
        }
    }

    fn from_observations(observations: Vec<FirstLockRecoverySafetyObservation>) -> Self {
        Self {
            observations: Mutex::new(observations),
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl FirstLockRecoverySafetyPort for FixedFirstLockRecoverySafety {
    async fn observe(
        &self,
        _agreement: &BtcAgreementV1,
        _read_ordinal: u8,
    ) -> Result<FirstLockRecoverySafetyObservation, ActorCommandError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut observations = self.observations.lock().unwrap();
        if observations.is_empty() {
            return Err(ActorCommandError::ObservationUnavailable);
        }
        Ok(observations.remove(0))
    }
}

async fn drive_admitted_first_lock_refund(
    fixture: &ActorFixture,
    observer: &dyn RefundObservationPort,
) -> Result<ActorEffectOutputV1, ActorCommandError> {
    let safety = FixedFirstLockRecoverySafety::ready(&fixture.agreement);
    let result = drive_first_lock_refund_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &safety,
        observer,
    )
    .await;
    assert_eq!(safety.calls(), 2);
    result
}

fn ready_refund_observation(
    agreement: &BtcAgreementV1,
    transition: RefundTransition,
) -> ActorRefundObservation {
    let participant = transition.funded_participant();
    let chain = agreement.coordinator().funded_chain(participant);
    let position = agreement
        .coordinator()
        .recovery_schedule()
        .deadline_for_chain(chain)
        .expect("typed refund deadline");
    let confirmations = match chain {
        Chain::Bitcoin => agreement.required_bitcoin_confirmations(),
        Chain::Lez => FINALIZED_LEZ_CONFIRMATION_UNITS,
        Chain::Zcash | Chain::Monero => unreachable!("Bitcoin agreement chain set"),
    };
    ActorRefundObservation::Ready {
        chain,
        transaction_id: format!("{participant:?}-{chain:?}-refund")
            .to_lowercase()
            .into_boxed_str(),
        confirmations,
        chain_evidence: format!("canonical-{participant:?}-{chain:?}-refund-evidence").into_bytes(),
        position,
    }
}

async fn activate_and_project_both_locks(fixture: &ActorFixture) {
    activate_and_project_taker_lock(fixture).await;
    let observer = FixedObserver::new(maker_lock_observation(&fixture.agreement, 1));
    drive_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &observer,
    )
    .await
    .expect("project maker lock");
}

async fn activate_and_project_schema5_both_locks(fixture: &ActorFixture) {
    assert_eq!(fixture.config.schema_version, ASSET_CONFIG_SCHEMA_VERSION);
    if fixture.config.role == ActorRole::Taker {
        activate_and_project_both_locks(fixture).await;
        return;
    }

    activate_and_project_taker_lock(fixture).await;
    let plan = load_prepared_maker_lock_material(&fixture.config, &fixture.agreement)
        .expect("schema-5 maker material")
        .plan()
        .clone();
    let eligibility = fresh_maker_eligibility(fixture);
    let complete = exact_maker_lock_complete_observation(&fixture.agreement, &plan, 0xc1);
    for (index, step) in plan.steps().iter().enumerate() {
        let present = FixedMakerLockPort::new(
            MakerLockStepChainObservationV1::PresentExactCanonical {
                expected_public_id: step.expected_public_id().as_str().into(),
                exact_public_bytes: step.exact_bytes().as_slice().to_vec(),
            },
            eligibility.clone(),
            complete.clone(),
        );
        let output = drive_maker_lock_with_port(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &present,
        )
        .await
        .unwrap_or_else(|error| panic!("schema-5 maker step {index} failed: {error:?}"));
        assert_eq!(
            output.revision,
            if index + 1 == plan.steps().len() {
                2
            } else {
                1
            }
        );
    }
    assert_eq!(durable_status(fixture).phase(), Phase::BothLegsLocked);
}

fn revealing_signature(fixture: &ActorFixture) -> [u8; 64] {
    let chain = fixture
        .agreement
        .coordinator()
        .funded_chain(Participant::Maker);
    let (domain, signing) = match chain {
        Chain::Bitcoin => (
            BtcAdaptorSessionDomain::Bitcoin,
            &fixture.config.signing.bitcoin,
        ),
        Chain::Lez => (BtcAdaptorSessionDomain::Lez, &fixture.config.signing.lez),
        Chain::Zcash | Chain::Monero => unreachable!("Bitcoin agreement chain set"),
    };
    let context = fixture
        .agreement
        .adaptor_session_context(domain, *signing.session_id.as_bytes())
        .expect("agreement claim context");
    let journal = SqliteAdaptorSessionJournal::open_existing(&signing.journal_db)
        .expect("open signer journal");
    let snapshot = journal
        .load(signing.session_id.as_bytes())
        .expect("load signer snapshot")
        .expect("signer snapshot");
    let presignature = snapshot.presignature().expect("durable presignature");
    lez_btc_swap_sdk::adapt_presignature(&context, *presignature.bytes(), Zeroizing::new([7; 32]))
        .expect("adapt revealing signature")
}

#[tokio::test(flavor = "current_thread")]
async fn refund_projector_reaches_terminal_for_both_roles_and_directions() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        for role in [ActorRole::Maker, ActorRole::Taker] {
            let fixture = ActorFixture::for_direction(direction, role);
            activate_and_project_both_locks(&fixture).await;

            let maker = FixedRefundObserver::new(ready_refund_observation(
                &fixture.agreement,
                RefundTransition::MakerLeg,
            ));
            let maker_output = drive_refund_with_observer(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &maker,
            )
            .await
            .expect("project maker-funded refund");
            let maker_json = output_json(ActorCommandOutputV1::Effect(maker_output));
            assert_eq!(maker_json["command"], "recover");
            assert_eq!(maker_json["phase"], "maker_leg_refunded");
            assert_eq!(maker_json["revision"], 3);
            assert_eq!(maker_json["next_action"], "recover_taker_leg");
            assert_eq!(maker.transitions(), vec![RefundTransition::MakerLeg]);

            let taker = FixedRefundObserver::new(ready_refund_observation(
                &fixture.agreement,
                RefundTransition::TakerLeg,
            ));
            let taker_output = drive_refund_with_observer(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &taker,
            )
            .await
            .expect("project taker-funded refund");
            let taker_json = output_json(ActorCommandOutputV1::Effect(taker_output));
            assert_eq!(taker_json["command"], "recover");
            assert_eq!(taker_json["phase"], "refunded");
            assert_eq!(taker_json["revision"], 4);
            assert_eq!(taker_json["next_action"], "complete");
            assert_eq!(taker.transitions(), vec![RefundTransition::TakerLeg]);

            let durable = durable_status(&fixture);
            assert_eq!(durable.revision(), 4);
            assert_eq!(durable.phase(), Phase::Refunded);
            assert_eq!(durable.terminal(), Some(BtcTerminalOutcome::Refunded));
            let persisted = persisted_lifecycle_evidence(&fixture, 4);
            let taker_chain = fixture
                .agreement
                .coordinator()
                .funded_chain(Participant::Taker);
            assert_eq!(
                persisted.chain_evidence(),
                format!(
                    "canonical-{:?}-{:?}-refund-evidence",
                    Participant::Taker,
                    taker_chain
                )
                .as_bytes()
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn first_lock_only_refund_projector_reaches_terminal_for_both_roles_and_directions() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        for role in [ActorRole::Maker, ActorRole::Taker] {
            let fixture = ActorFixture::for_direction(direction, role);
            activate_and_project_taker_lock(&fixture).await;
            let taker_chain = fixture
                .agreement
                .coordinator()
                .funded_chain(Participant::Taker);
            let position = fixture
                .agreement
                .coordinator()
                .recovery_schedule()
                .deadline_for_chain(taker_chain)
                .expect("typed taker refund deadline");
            let confirmations = match taker_chain {
                Chain::Bitcoin => fixture.agreement.required_bitcoin_confirmations(),
                Chain::Lez => FINALIZED_LEZ_CONFIRMATION_UNITS,
                Chain::Zcash | Chain::Monero => unreachable!("Bitcoin agreement chain set"),
            };
            let observer = FixedRefundObserver::new(ActorRefundObservation::Ready {
                chain: taker_chain,
                transaction_id: "first-lock-only-refund".into(),
                confirmations,
                chain_evidence: b"canonical-first-lock-only-refund".to_vec(),
                position,
            });

            let safety = FixedFirstLockRecoverySafety::ready(&fixture.agreement);
            let output = drive_first_lock_refund_with_observer(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &safety,
                &observer,
            )
            .await
            .expect("project first-lock-only refund");
            assert_eq!(safety.calls(), 2);
            let json = output_json(ActorCommandOutputV1::Effect(output));
            assert_eq!(json["phase"], "refunded");
            assert_eq!(json["revision"], 2);
            let transitions = observer.transitions();
            assert_eq!(transitions.len(), 1);
            assert_eq!(transitions[0].funded_participant(), Participant::Taker);
            let durable = durable_status(&fixture);
            assert_eq!(durable.phase(), Phase::Refunded);
            assert_eq!(durable.revision(), 2);
            assert_eq!(durable.terminal(), Some(BtcTerminalOutcome::Refunded));
            let persisted = persisted_lifecycle_evidence(&fixture, 2);
            let envelope: FirstLockRecoveryChainEvidenceV1 =
                serde_json::from_slice(persisted.chain_evidence())
                    .expect("decode persisted first-lock admission envelope");
            assert_eq!(envelope.schema_version, 1);
            assert_eq!(
                envelope.agreement_commitment,
                hex::encode(fixture.agreement.agreement_commitment())
            );
            assert_eq!(envelope.first_read.read_ordinal, 1);
            assert_eq!(envelope.second_read.read_ordinal, 2);
            assert_ne!(
                envelope.first_read.absence_evidence_hex,
                envelope.second_read.absence_evidence_hex
            );
            assert_eq!(
                hex::decode(&envelope.first_read.absence_evidence_hex)
                    .expect("first safety evidence hex"),
                b"canonical-maker-lock-absence-read-1"
            );
            assert_eq!(
                hex::decode(&envelope.second_read.absence_evidence_hex)
                    .expect("second safety evidence hex"),
                b"canonical-maker-lock-absence-read-2"
            );
            assert_eq!(
                hex::decode(&envelope.refund_chain_evidence_hex).expect("refund evidence hex"),
                b"canonical-first-lock-only-refund"
            );
        }
    }
}

#[test]
fn lez_first_lock_absence_requires_tip_ending_full_window_and_distinct_requests() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    let first = first_lock_lez_funding_request(&fixture.config, &fixture.agreement, 1)
        .expect("first safety request");
    let second = first_lock_lez_funding_request(&fixture.config, &fixture.agreement, 2)
        .expect("second safety request");
    assert_ne!(first.context, second.context);
    let window_end = first
        .window
        .start_height()
        .checked_add(u64::from(first.window.max_blocks() - 1))
        .expect("bounded test window");
    let cutoff_ms = fixture
        .agreement
        .body()
        .recovery_plan()
        .maker_second_lock_cutoff_unix_seconds()
        .checked_mul(1_000)
        .expect("test cutoff milliseconds");
    let exact_clock = ChainClock::new(Hex32::from_bytes([0xa1; 32]), window_end, cutoff_ms);
    let encoded = encode_lez_maker_lock_absence_evidence(
        &fixture.config,
        &fixture.agreement,
        1,
        &first,
        &first.context,
        exact_clock,
        first.window,
    )
    .expect("full scan ending at cutoff-authorizing tip");
    let value: Value = serde_json::from_slice(&encoded).expect("absence evidence JSON");
    assert_eq!(value["read_ordinal"], 1);
    assert_eq!(
        value["agreement_commitment"],
        hex::encode(fixture.agreement.agreement_commitment())
    );
    assert!(
        !String::from_utf8(encoded.clone())
            .expect("UTF-8 evidence")
            .contains("capability_file")
    );

    let stale_tip = ChainClock::new(
        Hex32::from_bytes([0xa2; 32]),
        window_end + 1,
        cutoff_ms + 1_000,
    );
    assert_eq!(
        encode_lez_maker_lock_absence_evidence(
            &fixture.config,
            &fixture.agreement,
            1,
            &first,
            &first.context,
            stale_tip,
            first.window,
        )
        .expect_err("a stale window cannot borrow a newer tip clock"),
        ActorCommandError::AgreementBindingInvalid
    );

    let mut truncated = first.clone();
    truncated.window =
        DiscoveryWindow::new(first.window.start_height(), first.window.max_blocks() - 1)
            .expect("truncated test window");
    assert_eq!(
        encode_lez_maker_lock_absence_evidence(
            &fixture.config,
            &fixture.agreement,
            1,
            &truncated,
            &truncated.context,
            exact_clock,
            truncated.window,
        )
        .expect_err("a request narrower than the configured baseline window fails closed"),
        ActorCommandError::AgreementBindingInvalid
    );
}

#[tokio::test(flavor = "current_thread")]
async fn canonical_maker_second_lock_wins_revision_one_refund_boundary_race() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let fixture = ActorFixture::for_direction(direction, ActorRole::Taker);
        activate_and_project_taker_lock(&fixture).await;
        let refund_observer = FixedRefundObserver::new(ready_refund_observation(
            &fixture.agreement,
            RefundTransition::FirstLockRecovery,
        ));
        let first = FirstLockRecoverySafetyObservation::ReadyToRefund {
            maker_chain: fixture
                .agreement
                .coordinator()
                .funded_chain(Participant::Maker),
            cutoff_unix_seconds: fixture
                .agreement
                .body()
                .recovery_plan()
                .maker_second_lock_cutoff_unix_seconds(),
            observed_unix_seconds: fixture
                .agreement
                .body()
                .recovery_plan()
                .earlier_refund_latest_unix_seconds(),
            absence_evidence: b"first-stable-absence".to_vec(),
        };
        let maker_observation = maker_lock_observation(&fixture.agreement, 1);
        let ActorFundingObservation::Ready {
            chain,
            transaction_id,
            confirmations,
            canonical_inclusion_time,
            chain_evidence,
        } = maker_observation.clone()
        else {
            unreachable!("maker fixture is canonical")
        };
        let cutoff = fixture
            .agreement
            .body()
            .recovery_plan()
            .maker_second_lock_cutoff_unix_seconds();
        assert!(canonical_maker_lock_is_timely(
            chain,
            &canonical_inclusion_time,
            cutoff,
        ));
        assert!(match &canonical_inclusion_time {
            CanonicalInclusionTimeV1::Bitcoin {
                median_time_unix_seconds,
            } => *median_time_unix_seconds == cutoff,
            CanonicalInclusionTimeV1::Lez { timestamp_ms } => *timestamp_ms == cutoff * 1_000,
        });
        let safety = FixedFirstLockRecoverySafety::from_observations(vec![
            first,
            FirstLockRecoverySafetyObservation::MakerLockReady {
                chain,
                transaction_id,
                confirmations,
                chain_evidence,
            },
        ]);

        let withheld = drive_first_lock_refund_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &safety,
            &refund_observer,
        )
        .await
        .expect("maker lock appearing on the safety recheck withholds refund");
        assert_eq!(withheld.revision, 1);
        assert_eq!(refund_observer.transitions(), Vec::new());
        assert_eq!(safety.calls(), 2);

        let maker = FixedObserver::new(maker_observation);
        let projected = drive_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &maker,
        )
        .await
        .expect("canonical maker lock projects after the refund branch is withheld");
        assert_eq!(projected.revision, 2);
        assert_eq!(projected.phase, ActorPhaseV1::BothLegsLocked);
        let durable = durable_status(&fixture);
        assert_eq!(durable.revision(), 2);
        assert_eq!(durable.phase(), Phase::BothLegsLocked);
        assert_eq!(durable.terminal(), None);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn first_lock_cutoff_and_uncertain_recheck_fail_closed_without_observer_io() {
    let before = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_taker_lock(&before).await;
    let maker_chain = before
        .agreement
        .coordinator()
        .funded_chain(Participant::Maker);
    let cutoff = before
        .agreement
        .body()
        .recovery_plan()
        .maker_second_lock_cutoff_unix_seconds();
    let refund = FixedRefundObserver::new(ready_refund_observation(
        &before.agreement,
        RefundTransition::FirstLockRecovery,
    ));
    let before_cutoff = FirstLockRecoverySafetyObservation::ReadyToRefund {
        maker_chain,
        cutoff_unix_seconds: cutoff,
        observed_unix_seconds: cutoff - 1,
        absence_evidence: b"stable-absence-before-cutoff".to_vec(),
    };
    let safety = FixedFirstLockRecoverySafety::from_observations(vec![before_cutoff]);
    assert_eq!(
        drive_first_lock_refund_with_observer(
            &before.config,
            before.agreement.clone(),
            before.agreement_wire.clone(),
            &safety,
            &refund,
        )
        .await
        .expect_err("pre-cutoff absence cannot grant refund authority"),
        ActorCommandError::AgreementBindingInvalid
    );
    assert_eq!(refund.transitions(), Vec::new());

    let flip = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_taker_lock(&flip).await;
    let maker_chain = flip
        .agreement
        .coordinator()
        .funded_chain(Participant::Maker);
    let cutoff = flip
        .agreement
        .body()
        .recovery_plan()
        .maker_second_lock_cutoff_unix_seconds();
    let refund = FixedRefundObserver::new(ready_refund_observation(
        &flip.agreement,
        RefundTransition::FirstLockRecovery,
    ));
    let safety = FixedFirstLockRecoverySafety::from_observations(vec![
        FirstLockRecoverySafetyObservation::ReadyToRefund {
            maker_chain,
            cutoff_unix_seconds: cutoff,
            observed_unix_seconds: cutoff,
            absence_evidence: b"stable-absence-at-cutoff".to_vec(),
        },
        FirstLockRecoverySafetyObservation::Uncertain { maker_chain },
    ]);
    let pending = drive_first_lock_refund_with_observer(
        &flip.config,
        flip.agreement.clone(),
        flip.agreement_wire.clone(),
        &safety,
        &refund,
    )
    .await
    .expect("an uncertain second read remains observation-only");
    assert_eq!(pending.revision, 1);
    assert_eq!(safety.calls(), 2);
    assert_eq!(refund.transitions(), Vec::new());
}

#[tokio::test(flavor = "current_thread")]
async fn first_lock_regressing_second_clock_fails_closed_without_refund_io() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_taker_lock(&fixture).await;
    let maker_chain = fixture
        .agreement
        .coordinator()
        .funded_chain(Participant::Maker);
    let cutoff = fixture
        .agreement
        .body()
        .recovery_plan()
        .maker_second_lock_cutoff_unix_seconds();
    let safety = FixedFirstLockRecoverySafety::from_observations(vec![
        FirstLockRecoverySafetyObservation::ReadyToRefund {
            maker_chain,
            cutoff_unix_seconds: cutoff,
            observed_unix_seconds: cutoff + 1,
            absence_evidence: b"later-clock-read-1".to_vec(),
        },
        FirstLockRecoverySafetyObservation::ReadyToRefund {
            maker_chain,
            cutoff_unix_seconds: cutoff,
            observed_unix_seconds: cutoff,
            absence_evidence: b"regressed-clock-read-2".to_vec(),
        },
    ]);
    let refund = FixedRefundObserver::new(ready_refund_observation(
        &fixture.agreement,
        RefundTransition::FirstLockRecovery,
    ));
    let output = drive_first_lock_refund_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &safety,
        &refund,
    )
    .await
    .expect("clock regression remains observation-only");
    assert_eq!(output.revision, 1);
    assert_eq!(safety.calls(), 2);
    assert_eq!(refund.transitions(), Vec::new());
}

#[tokio::test(flavor = "current_thread")]
async fn refund_projector_keeps_pending_and_wrong_chain_observations_at_revision_two() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    activate_and_project_both_locks(&fixture).await;
    let expected_chain = fixture
        .agreement
        .coordinator()
        .funded_chain(Participant::Maker);
    let pending = FixedRefundObserver::new(ActorRefundObservation::Pending {
        chain: expected_chain,
    });
    let output = drive_refund_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &pending,
    )
    .await
    .expect("pending refund observation");
    let value = output_json(ActorCommandOutputV1::Effect(output));
    assert_eq!(value["outcome"], "awaiting_observation");
    assert_eq!(value["revision"], 2);
    assert_eq!(pending.transitions(), vec![RefundTransition::MakerLeg]);

    let wrong_chain = match expected_chain {
        Chain::Bitcoin => Chain::Lez,
        Chain::Lez => Chain::Bitcoin,
        Chain::Zcash | Chain::Monero => unreachable!("Bitcoin agreement chain set"),
    };
    let contradictory =
        FixedRefundObserver::new(ActorRefundObservation::Pending { chain: wrong_chain });
    assert_eq!(
        drive_refund_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &contradictory,
        )
        .await
        .expect_err("wrong refund chain must fail closed"),
        ActorCommandError::AgreementBindingInvalid
    );
    assert_eq!(durable_status(&fixture).revision(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn revealing_claim_projects_revision_three_for_both_roles_and_directions() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        for role in [ActorRole::Maker, ActorRole::Taker] {
            let fixture = ActorFixture::for_direction(direction, role);
            activate_and_project_both_locks(&fixture).await;
            let chain = fixture
                .agreement
                .coordinator()
                .funded_chain(Participant::Maker);
            let observer = FixedClaimObserver::new(ActorClaimObservation::Ready {
                chain,
                transaction_id: "canonical-revealing-claim".into(),
                confirmations: if chain == Chain::Bitcoin {
                    support::REQUIRED_CONFIRMATIONS
                } else {
                    FINALIZED_LEZ_CONFIRMATION_UNITS
                },
                chain_evidence: b"canonical-revealing-claim-evidence".to_vec(),
                revealing_public_signature: Some(revealing_signature(&fixture)),
            });

            let projected = output_json(
                drive_claim_with_observer(
                    &fixture.config,
                    fixture.agreement.clone(),
                    fixture.agreement_wire.clone(),
                    &observer,
                )
                .await
                .expect("project revealing claim"),
            );
            assert_eq!(projected["outcome"], "observed_then_projected");
            assert_eq!(projected["revision"], 3);
            assert_eq!(projected["phase"], "claim_evidence_available");
            assert_eq!(observer.calls(), 1);

            let status = output_json(
                execute_actor_command(&fixture.config, ActorCommand::Status)
                    .await
                    .expect("offline revision-three status"),
            );
            assert_eq!(status["next_action"], "observe_followup_claim");
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn revealing_claim_requires_the_exact_related_public_signature() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Maker);
    activate_and_project_both_locks(&fixture).await;
    let chain = fixture
        .agreement
        .coordinator()
        .funded_chain(Participant::Maker);
    let mut signature = revealing_signature(&fixture);
    signature[63] ^= 1;
    let observer = FixedClaimObserver::new(ActorClaimObservation::Ready {
        chain,
        transaction_id: "canonical-revealing-claim".into(),
        confirmations: support::REQUIRED_CONFIRMATIONS,
        chain_evidence: b"canonical-revealing-claim-evidence".to_vec(),
        revealing_public_signature: Some(signature),
    });

    let error = drive_claim_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &observer,
    )
    .await
    .expect_err("unrelated public signature must fail closed");
    assert_eq!(error, ActorCommandError::ObservationUnavailable);
    let status = execute_actor_command(&fixture.config, ActorCommand::Status)
        .await
        .expect("offline status remains readable");
    assert_eq!(output_json(status)["revision"], 2);
}

async fn project_revealing_claim(fixture: &ActorFixture) {
    let chain = fixture
        .agreement
        .coordinator()
        .funded_chain(Participant::Maker);
    let observer = FixedClaimObserver::new(ActorClaimObservation::Ready {
        chain,
        transaction_id: "canonical-revealing-claim".into(),
        confirmations: if chain == Chain::Bitcoin {
            support::REQUIRED_CONFIRMATIONS
        } else {
            FINALIZED_LEZ_CONFIRMATION_UNITS
        },
        chain_evidence: b"canonical-revealing-claim-evidence".to_vec(),
        revealing_public_signature: Some(revealing_signature(fixture)),
    });
    drive_claim_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &observer,
    )
    .await
    .expect("project revealing claim");
}

#[tokio::test(flavor = "current_thread")]
async fn pending_revealing_claim_preserves_both_locks_without_secret_use() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_both_locks(&fixture).await;
    let chain = fixture
        .agreement
        .coordinator()
        .funded_chain(Participant::Maker);
    let observer = FixedClaimObserver::new(ActorClaimObservation::Pending { chain });

    let pending = output_json(
        drive_claim_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &observer,
        )
        .await
        .expect("revealing claim remains pending"),
    );
    assert_eq!(pending["outcome"], "awaiting_observation");
    assert_eq!(pending["revision"], 2);
    assert_eq!(pending["phase"], "both_legs_locked");
    assert_eq!(observer.calls(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn followup_claim_completes_both_roles_and_directions() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        for role in [ActorRole::Maker, ActorRole::Taker] {
            let fixture = ActorFixture::for_direction(direction, role);
            activate_and_project_both_locks(&fixture).await;
            project_revealing_claim(&fixture).await;
            let chain = fixture
                .agreement
                .coordinator()
                .funded_chain(Participant::Taker);
            let observer = FixedClaimObserver::new(ActorClaimObservation::Ready {
                chain,
                transaction_id: "canonical-followup-claim".into(),
                confirmations: if chain == Chain::Bitcoin {
                    support::REQUIRED_CONFIRMATIONS
                } else {
                    FINALIZED_LEZ_CONFIRMATION_UNITS
                },
                chain_evidence: b"canonical-followup-claim-evidence".to_vec(),
                revealing_public_signature: None,
            });

            let completed = output_json(
                drive_claim_with_observer(
                    &fixture.config,
                    fixture.agreement.clone(),
                    fixture.agreement_wire.clone(),
                    &observer,
                )
                .await
                .expect("project followup claim"),
            );
            assert_eq!(completed["outcome"], "observed_then_projected");
            assert_eq!(completed["revision"], 4);
            assert_eq!(completed["phase"], "completed");
            assert_eq!(completed["next_action"], "complete");
            assert_eq!(observer.calls(), 1);

            let status = output_json(
                execute_actor_command(&fixture.config, ActorCommand::Status)
                    .await
                    .expect("offline completed status"),
            );
            assert_eq!(status["revision"], 4);
            assert_eq!(status["phase"], "completed");
            assert_eq!(status["next_action"], "complete");
        }
    }
}

#[derive(Clone)]
struct FixedBitcoinClaimPort {
    scan: BitcoinClaimScan,
    submission: Result<AuthorizedClaimSubmission, ActorCommandError>,
    observe_calls: Arc<AtomicUsize>,
    submit_calls: Arc<AtomicUsize>,
}

#[derive(Clone)]
struct FixedLezClaimPort {
    completed: PreparedTransaction,
    presence: lez_bridge_client::FinalizedWitnessedClaimPresence,
    submission: Result<SubmissionOutcome, ActorCommandError>,
    complete_calls: Arc<AtomicUsize>,
    classify_calls: Arc<AtomicUsize>,
    submit_calls: Arc<AtomicUsize>,
    complete_requests: Arc<Mutex<Vec<CompleteWitnessedClaimRequest>>>,
    classify_requests: Arc<Mutex<Vec<ObserveFinalizedWitnessedClaimRequest>>>,
    submit_requests: Arc<Mutex<Vec<SubmitTransactionRequest>>>,
}

#[async_trait]
impl LezClaimChainPort for FixedLezClaimPort {
    async fn complete_witnessed_claim(
        &self,
        request: CompleteWitnessedClaimRequest,
    ) -> Result<CompleteWitnessedClaimResult, ActorCommandError> {
        self.complete_calls.fetch_add(1, Ordering::SeqCst);
        self.complete_requests
            .lock()
            .expect("complete request lock")
            .push(request.clone());
        Ok(CompleteWitnessedClaimResult::new(
            request.context,
            self.completed.clone(),
        ))
    }

    async fn classify_finalized_witnessed_claim(
        &self,
        request: ObserveFinalizedWitnessedClaimRequest,
    ) -> Result<lez_bridge_client::FinalizedWitnessedClaimPresence, ActorCommandError> {
        self.classify_calls.fetch_add(1, Ordering::SeqCst);
        self.classify_requests
            .lock()
            .expect("classify request lock")
            .push(request);
        Ok(self.presence.clone())
    }

    async fn submit_transaction(
        &self,
        request: SubmitTransactionRequest,
    ) -> Result<SubmitTransactionResult, ActorCommandError> {
        self.submit_calls.fetch_add(1, Ordering::SeqCst);
        self.submit_requests
            .lock()
            .expect("submit request lock")
            .push(request.clone());
        self.submission.map(|outcome| {
            SubmitTransactionResult::new(
                request.context,
                request.transaction.transaction_id,
                outcome,
            )
        })
    }
}

impl FixedLezClaimPort {
    fn with_presence(
        completed: PreparedTransaction,
        presence: lez_bridge_client::FinalizedWitnessedClaimPresence,
        submission: Result<SubmissionOutcome, ActorCommandError>,
    ) -> Self {
        Self {
            completed,
            presence,
            submission,
            complete_calls: Arc::new(AtomicUsize::new(0)),
            classify_calls: Arc::new(AtomicUsize::new(0)),
            submit_calls: Arc::new(AtomicUsize::new(0)),
            complete_requests: Arc::new(Mutex::new(Vec::new())),
            classify_requests: Arc::new(Mutex::new(Vec::new())),
            submit_requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn submit_calls(&self) -> usize {
        self.submit_calls.load(Ordering::SeqCst)
    }
}

fn placeholder_lez_port() -> FixedLezClaimPort {
    let transaction = PreparedTransaction::new(
        TransactionId::from_bytes([91; 32]),
        ExactTransactionBytes::new(vec![92; 128]).expect("exact LEZ transaction bytes"),
    );
    FixedLezClaimPort::with_presence(
        transaction,
        lez_bridge_client::FinalizedWitnessedClaimPresence::Uncertain(
            lez_bridge_client::FinalizedWitnessedClaimUncertain::Transport,
        ),
        Ok(SubmissionOutcome::Accepted),
    )
}

fn prepared_lez_claim(config: &ActorConfig, agreement: &BtcAgreementV1) -> PreparedWitnessedClaim {
    load_prepared_witnessed_claim(config, agreement)
        .expect("load prepared LEZ claim")
        .claim
}

fn finalized_lez_tip(window: DiscoveryWindow) -> ChainTip {
    let end = window.start_height() + u64::from(window.max_blocks() - 1);
    ChainTip::new(Hex32::from_bytes([94; 32]), end)
}

fn not_found_lez_claim_presence(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    transition: ClaimTransition,
    effect: Option<&PreparedLezClaimEffect>,
) -> lez_bridge_client::FinalizedWitnessedClaimPresence {
    let request = finalized_lez_claim_request(
        config,
        agreement,
        transition,
        &prepared_lez_claim(config, agreement),
        effect,
    )
    .expect("finalized LEZ claim request");
    lez_bridge_client::FinalizedWitnessedClaimPresence::NotFound {
        context: request.context,
        finalized_tip: finalized_lez_tip(request.window),
        scanned_window: request.window,
    }
}

fn prefix_uncertain_lez_claim_presence(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    transition: ClaimTransition,
    effect: Option<&PreparedLezClaimEffect>,
) -> lez_bridge_client::FinalizedWitnessedClaimPresence {
    let request = finalized_lez_claim_request(
        config,
        agreement,
        transition,
        &prepared_lez_claim(config, agreement),
        effect,
    )
    .expect("finalized LEZ claim request");
    let prefix =
        DiscoveryWindow::new(request.window.start_height(), 2).expect("strict finalized prefix");
    lez_bridge_client::FinalizedWitnessedClaimPresence::PrefixUncertain {
        context: request.context,
        finalized_tip: finalized_lez_tip(prefix),
        scanned_window: prefix,
    }
}

fn exact_lez_claim_presence(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    transition: ClaimTransition,
    effect: Option<&PreparedLezClaimEffect>,
    transaction: PreparedTransaction,
    aggregate_signature: [u8; 64],
) -> lez_bridge_client::FinalizedWitnessedClaimPresence {
    let prepared = prepared_lez_claim(config, agreement);
    let request = finalized_lez_claim_request(config, agreement, transition, &prepared, effect)
        .expect("finalized LEZ claim request");
    let block_height = request.window.start_height() + 1;
    let block_hash = Hex32::from_bytes([93; 32]);
    let authority = request.terms.aggregate_authority_account_id();
    let metadata_account = Hex32::from_bytes(*agreement.lez_terms().metadata_account());
    let custody_account = Hex32::from_bytes(*agreement.lez_terms().custody_account());
    let transaction_facts = ObservedTransactionFacts::new(
        transaction.transaction_id,
        transaction.exact_bytes,
        ChainPosition::new(block_hash, block_height, 0),
        AccountIds::new(vec![authority]).expect("one signer"),
        true,
    );
    let instruction = WitnessedClaimInstructionFacts::new(
        request.runtime.escrow_program_id,
        AccountIds::new(vec![
            metadata_account,
            custody_account,
            request.terms.claimant_account_id(),
            authority,
        ])
        .expect("claim account list"),
        request.terms.swap_id(),
        request.terms.claimant_account_id(),
        authority,
        prepared,
    );
    let metadata = WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
        metadata_account,
        request.runtime.escrow_program_id,
        custody_account,
        &request.terms,
        EscrowState::Claimed,
    );
    let custody = NativeCustodyFacts::new(
        custody_account,
        request.terms.authenticated_transfer_program_id(),
        0,
    );
    let claim = FinalizedWitnessedClaimFacts::new(
        transaction_facts,
        instruction,
        AggregateBip340Signature::from_bytes(aggregate_signature),
        FinalizedBlockIdentity::new(block_height, block_hash, 1_700_000_001_000),
        metadata,
        custody,
    );
    lez_bridge_client::FinalizedWitnessedClaimPresence::PresentExact {
        context: request.context,
        finalized_tip: finalized_lez_tip(request.window),
        scanned_window: request.window,
        claim: Box::new(claim),
    }
}

fn valid_lez_claim_signature(fixture: &ActorFixture, transition: ClaimTransition) -> [u8; 64] {
    if transition == ClaimTransition::RevealingClaim {
        return revealing_signature(fixture);
    }
    let revealing_public_signature = revealing_signature(fixture);
    let (bitcoin_context, bitcoin_presignature) =
        verified_chain_presignature(&fixture.config, &fixture.agreement, Chain::Bitcoin)
            .expect("verified Bitcoin presignature");
    let secret = extract_adaptor_secret(
        &bitcoin_context,
        bitcoin_presignature,
        revealing_public_signature,
    )
    .expect("extract followup adaptor secret");
    let (lez_context, lez_presignature) =
        verified_chain_presignature(&fixture.config, &fixture.agreement, Chain::Lez)
            .expect("verified LEZ presignature");
    adapt_presignature(&lez_context, lez_presignature, secret)
        .expect("adapt LEZ followup signature")
}

#[derive(Clone)]
struct FixedLezAssetClaimPort {
    run_id: RunId,
    role: BridgeParticipant,
    completed: PreparedTransaction,
    outcome: FinalizedWitnessedAssetScanOutcomeV2<FinalizedWitnessedAssetClaimFactsV2>,
    submission: Result<SubmissionOutcome, ActorCommandError>,
    complete_calls: Arc<AtomicUsize>,
    classify_calls: Arc<AtomicUsize>,
    submit_calls: Arc<AtomicUsize>,
    targets: Arc<Mutex<Vec<FinalizedWitnessedAssetTransactionTargetV2>>>,
}

impl FixedLezAssetClaimPort {
    fn new(
        config: &ActorConfig,
        completed: PreparedTransaction,
        outcome: FinalizedWitnessedAssetScanOutcomeV2<FinalizedWitnessedAssetClaimFactsV2>,
        submission: Result<SubmissionOutcome, ActorCommandError>,
    ) -> Self {
        Self {
            run_id: config.lez_bridge.run_id.clone(),
            role: config.role.bridge(),
            completed,
            outcome,
            submission,
            complete_calls: Arc::new(AtomicUsize::new(0)),
            classify_calls: Arc::new(AtomicUsize::new(0)),
            submit_calls: Arc::new(AtomicUsize::new(0)),
            targets: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn complete_calls(&self) -> usize {
        self.complete_calls.load(Ordering::SeqCst)
    }

    fn classify_calls(&self) -> usize {
        self.classify_calls.load(Ordering::SeqCst)
    }

    fn submit_calls(&self) -> usize {
        self.submit_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl LezAssetClaimChainPort for FixedLezAssetClaimPort {
    async fn complete_asset_claim(
        &self,
        binding: &BtcLezAssetBridgeBindingV2,
        request_id: RequestId,
        _claim: PreparedWitnessedClaim,
        _aggregate_signature: AggregateBip340Signature,
    ) -> Result<CompleteWitnessedAssetClaimV2Result, ActorCommandError> {
        self.complete_calls.fetch_add(1, Ordering::SeqCst);
        Ok(CompleteWitnessedAssetClaimV2Result::new(
            MessageContext::new(self.run_id.clone(), request_id, self.role),
            binding.terms().clone(),
            self.completed.clone(),
        ))
    }

    async fn classify_finalized_asset_claim(
        &self,
        _binding: &BtcLezAssetBridgeBindingV2,
        _request_id: RequestId,
        _claim: PreparedWitnessedClaim,
        target: FinalizedWitnessedAssetTransactionTargetV2,
        _window: DiscoveryWindow,
    ) -> Result<
        FinalizedWitnessedAssetScanOutcomeV2<FinalizedWitnessedAssetClaimFactsV2>,
        ActorCommandError,
    > {
        self.classify_calls.fetch_add(1, Ordering::SeqCst);
        self.targets.lock().expect("asset target lock").push(target);
        Ok(self.outcome.clone())
    }

    async fn submit_transaction(
        &self,
        request: SubmitTransactionRequest,
    ) -> Result<SubmitTransactionResult, ActorCommandError> {
        self.submit_calls.fetch_add(1, Ordering::SeqCst);
        self.submission.map(|outcome| {
            SubmitTransactionResult::new(
                request.context,
                request.transaction.transaction_id,
                outcome,
            )
        })
    }
}

fn prepared_lez_asset_claim(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
) -> PreparedWitnessedClaim {
    load_prepared_asset_witnessed_claim(config, agreement)
        .expect("load prepared LEZ asset claim")
        .claim
}

fn asset_claim_transaction() -> PreparedTransaction {
    PreparedTransaction::new(
        TransactionId::from_bytes([0xd1; 32]),
        ExactTransactionBytes::new(b"exact-schema-5-token-claim".to_vec())
            .expect("asset claim bytes"),
    )
}

fn finalized_asset_clock(window: DiscoveryWindow) -> ChainClock {
    ChainClock::new(
        Hex32::from_bytes([0xd2; 32]),
        window.start_height() + u64::from(window.max_blocks() - 1),
        1_800_000_000_000,
    )
}

fn absent_asset_claim_outcome(
    config: &ActorConfig,
) -> FinalizedWitnessedAssetScanOutcomeV2<FinalizedWitnessedAssetClaimFactsV2> {
    let window = config.discovery_window().expect("asset window");
    FinalizedWitnessedAssetScanOutcomeV2::Absent {
        finalized_clock: finalized_asset_clock(window),
        scanned_window: window,
    }
}

fn uncertain_asset_claim_outcome(
    config: &ActorConfig,
) -> FinalizedWitnessedAssetScanOutcomeV2<FinalizedWitnessedAssetClaimFactsV2> {
    let window = config.discovery_window().expect("asset window");
    FinalizedWitnessedAssetScanOutcomeV2::Uncertain {
        finalized_clock: finalized_asset_clock(window),
        scanned_window: window,
    }
}

fn unavailable_asset_claim_outcome()
-> FinalizedWitnessedAssetScanOutcomeV2<FinalizedWitnessedAssetClaimFactsV2> {
    FinalizedWitnessedAssetScanOutcomeV2::Unavailable {
        reason: FinalizedWitnessedAssetUnavailableReasonV2::HistoryUnavailable,
    }
}

fn exact_asset_claim_outcome(
    fixture: &ActorFixture,
    transaction: PreparedTransaction,
    aggregate_signature: [u8; 64],
    exact_target: bool,
) -> FinalizedWitnessedAssetScanOutcomeV2<FinalizedWitnessedAssetClaimFactsV2> {
    let (extension, _) = validated_asset_extension_material(&fixture.config, &fixture.agreement)
        .expect("asset extension");
    let binding =
        BtcLezAssetBridgeBindingV2::new(&fixture.agreement, &extension, extension.asset())
            .expect("asset binding");
    let WitnessedLezAssetV2::CustomToken(token) = binding.terms().asset() else {
        panic!("custom-token claim fixture");
    };
    let prepared = prepared_lez_asset_claim(&fixture.config, &fixture.agreement);
    let window = fixture.config.discovery_window().expect("asset window");
    let height = window.start_height() + 1;
    let block_hash = Hex32::from_bytes([0xd3; 32]);
    let metadata_account = Hex32::from_bytes(*binding.metadata_account_id());
    let authority = token.aggregate_authority_account_id();
    let facts = FinalizedWitnessedAssetClaimFactsV2::new(
        ObservedTransactionFacts::new(
            transaction.transaction_id,
            transaction.exact_bytes.clone(),
            ChainPosition::new(block_hash, height, 0),
            AccountIds::new(vec![authority]).expect("asset claim signer"),
            true,
        ),
        WitnessedAssetClaimInstructionFactsV2::new(
            fixture.config.lez_bridge.runtime.escrow_program_id,
            AccountIds::new(vec![
                metadata_account,
                token.custody_ata_account_id(),
                token.claimant_owner_account_id(),
                token.claimant_ata_account_id(),
                authority,
            ])
            .expect("asset claim accounts"),
            token.swap_id(),
            prepared.clone(),
        ),
        AggregateBip340Signature::from_bytes(aggregate_signature),
        FinalizedBlockIdentity::new(height, block_hash, 1_700_000_001_000),
        WitnessedEscrowMetadataFacts::from_witnessed_token_terms(
            metadata_account,
            fixture.config.lez_bridge.runtime.escrow_program_id,
            token,
            EscrowState::Claimed,
        ),
        WitnessedAssetCustodyFactsV2::CustomToken(TokenHoldingFactsV2::new(
            token.custody_ata_account_id(),
            token.token_program_id(),
            token.token_definition_account_id(),
            0,
        )),
    );
    let target = if exact_target {
        FinalizedWitnessedAssetTransactionTargetV2::exact(transaction)
    } else {
        FinalizedWitnessedAssetTransactionTargetV2::DiscoverByTerms {}
    };
    ClassifyFinalizedWitnessedAssetClaimV2Result::found(
        MessageContext::new(
            fixture.config.lez_bridge.run_id.clone(),
            RequestId::new("asset-claim-fixture").expect("asset fixture request"),
            fixture.config.role.bridge(),
        ),
        binding.terms().clone(),
        prepared,
        target,
        finalized_asset_clock(window),
        window,
        facts,
    )
    .expect("valid finalized asset claim")
    .outcome
}

#[test]
fn schema5_claim_material_is_strict_and_bound_to_context_terms_and_transcript() {
    let mut fixture =
        ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    let _ = configure_schema5_asset_claim(&mut fixture);
    let exact = load_prepared_asset_witnessed_claim(&fixture.config, &fixture.agreement)
        .expect("exact schema-5 claim material");
    assert_eq!(
        exact.context.run_id, fixture.config.lez_bridge.run_id,
        "claim owner must use its local isolated sidecar run"
    );

    let path = &fixture.config.signing.prepared_witnessed_claim_result_file;
    let mut changed_context = exact.clone();
    changed_context.context.request_id =
        RequestId::new("changed-asset-claim-request").expect("changed request ID");
    fs::write(
        path,
        serde_json::to_vec(&changed_context).expect("changed context JSON"),
    )
    .expect("write changed context");
    assert_eq!(
        load_prepared_asset_witnessed_claim(&fixture.config, &fixture.agreement),
        Err(ActorCommandError::ActivationMaterialUnavailable)
    );

    let mut changed_terms = serde_json::to_value(&exact).expect("asset claim value");
    changed_terms["terms"]["asset"]["terms"]["token_definition_account_id"] =
        Value::String(hex::encode([0x2e; 32]));
    fs::write(
        path,
        serde_json::to_vec(&changed_terms).expect("changed terms JSON"),
    )
    .expect("write changed terms");
    assert_eq!(
        load_prepared_asset_witnessed_claim(&fixture.config, &fixture.agreement),
        Err(ActorCommandError::ActivationMaterialUnavailable)
    );

    let mut unknown = serde_json::to_value(exact).expect("strict asset claim value");
    unknown["unexpected_private_material"] = Value::String("forbidden".to_owned());
    fs::write(
        path,
        serde_json::to_vec(&unknown).expect("unknown-field JSON"),
    )
    .expect("write unknown field");
    assert_eq!(
        load_prepared_asset_witnessed_claim(&fixture.config, &fixture.agreement),
        Err(ActorCommandError::ActivationMaterialUnavailable)
    );
}

#[tokio::test(flavor = "current_thread")]
#[allow(clippy::too_many_lines)]
async fn schema5_asset_claim_owner_sends_once_restart_never_rearms_and_exact_finality_projects() {
    for (direction, role, transition) in [
        (
            SwapDirection::TakerSellsForeign,
            ActorRole::Taker,
            ClaimTransition::RevealingClaim,
        ),
        (
            SwapDirection::TakerSellsLez,
            ActorRole::Maker,
            ClaimTransition::FollowupClaim,
        ),
    ] {
        let mut fixture = ActorFixture::for_direction(direction, role);
        let extension = configure_schema5_asset_claim(&mut fixture);
        activate_and_project_schema5_both_locks(&fixture).await;
        if transition == ClaimTransition::FollowupClaim {
            project_revealing_claim(&fixture).await;
        }
        let transaction = asset_claim_transaction();
        let preparation = FixedLezAssetClaimPort::new(
            &fixture.config,
            transaction.clone(),
            unavailable_asset_claim_outcome(),
            Ok(SubmissionOutcome::Accepted),
        );
        let effect = prepare_lez_asset_claim_effect(
            &fixture.config,
            &fixture.agreement,
            transition,
            &durable_status(&fixture),
            &preparation,
        )
        .await
        .expect("prepare exact asset claim")
        .expect("claimant owns exact asset claim");
        let repeated = prepare_lez_asset_claim_effect(
            &fixture.config,
            &fixture.agreement,
            transition,
            &durable_status(&fixture),
            &preparation,
        )
        .await
        .expect("repeat exact asset completion")
        .expect("claimant remains owner");
        assert_eq!(effect, repeated);
        assert_eq!(preparation.complete_calls(), 2);
        assert_eq!(
            effect.effect.agreement_commitment(),
            *extension.asset_commitment()
        );

        let absent = FixedLezAssetClaimPort::new(
            &fixture.config,
            transaction.clone(),
            absent_asset_claim_outcome(&fixture.config),
            Ok(SubmissionOutcome::Accepted),
        );
        let observer = LezAssetClaimObserver {
            config: &fixture.config,
            chain: absent.clone(),
            effect: Some(effect.clone()),
            prepared_claim: prepared_lez_asset_claim(&fixture.config, &fixture.agreement),
            state_db: fixture.config.state_db.clone(),
        };
        let pending = output_json(
            drive_claim_with_observer(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &observer,
            )
            .await
            .expect("affirmative absence grants one asset claim send"),
        );
        assert_eq!(pending["outcome"], "awaiting_observation");
        assert_eq!(pending["revision"], transition.predecessor_revision());
        assert_eq!(absent.submit_calls(), 1);

        let restarted = FixedLezAssetClaimPort::new(
            &fixture.config,
            transaction.clone(),
            absent_asset_claim_outcome(&fixture.config),
            Ok(SubmissionOutcome::Accepted),
        );
        let restarted_observer = LezAssetClaimObserver {
            config: &fixture.config,
            chain: restarted.clone(),
            effect: Some(effect.clone()),
            prepared_claim: prepared_lez_asset_claim(&fixture.config, &fixture.agreement),
            state_db: fixture.config.state_db.clone(),
        };
        drive_claim_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &restarted_observer,
        )
        .await
        .expect("accepted asset claim restart is observe-only");
        assert_eq!(restarted.submit_calls(), 0);

        let found = FixedLezAssetClaimPort::new(
            &fixture.config,
            transaction.clone(),
            exact_asset_claim_outcome(&fixture, transaction, effect.aggregate_signature, true),
            Err(ActorCommandError::ObservationUnavailable),
        );
        let found_observer = LezAssetClaimObserver {
            config: &fixture.config,
            chain: found.clone(),
            effect: Some(effect),
            prepared_claim: prepared_lez_asset_claim(&fixture.config, &fixture.agreement),
            state_db: fixture.config.state_db.clone(),
        };
        let projected = output_json(
            drive_claim_with_observer(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &found_observer,
            )
            .await
            .expect("exact finalized asset claim projects"),
        );
        assert_eq!(projected["outcome"], "observed_then_projected");
        assert_eq!(projected["revision"], transition.revision());
        assert_eq!(found.submit_calls(), 0);
        assert_eq!(found.classify_calls(), 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn schema5_asset_claim_nonowner_discovers_without_completion_send_or_peer_private_material() {
    for (direction, role, transition) in [
        (
            SwapDirection::TakerSellsForeign,
            ActorRole::Maker,
            ClaimTransition::RevealingClaim,
        ),
        (
            SwapDirection::TakerSellsLez,
            ActorRole::Taker,
            ClaimTransition::FollowupClaim,
        ),
    ] {
        let mut fixture = ActorFixture::for_direction(direction, role);
        let _ = configure_schema5_asset_claim(&mut fixture);
        let prepared = load_prepared_asset_witnessed_claim(&fixture.config, &fixture.agreement)
            .expect("peer-public prepared claim");
        assert_ne!(prepared.context.run_id, fixture.config.lez_bridge.run_id);
        activate_and_project_schema5_both_locks(&fixture).await;
        if transition == ClaimTransition::FollowupClaim {
            project_revealing_claim(&fixture).await;
        }
        let transaction = asset_claim_transaction();
        let port = FixedLezAssetClaimPort::new(
            &fixture.config,
            transaction.clone(),
            exact_asset_claim_outcome(
                &fixture,
                transaction,
                valid_lez_claim_signature(&fixture, transition),
                false,
            ),
            Err(ActorCommandError::ObservationUnavailable),
        );
        let effect = prepare_lez_asset_claim_effect(
            &fixture.config,
            &fixture.agreement,
            transition,
            &durable_status(&fixture),
            &port,
        )
        .await
        .expect("nonowner remains observation-only");
        assert!(effect.is_none());
        let observer = LezAssetClaimObserver {
            config: &fixture.config,
            chain: port.clone(),
            effect: None,
            prepared_claim: prepared.claim,
            state_db: fixture.config.state_db.clone(),
        };
        let projected = output_json(
            drive_claim_with_observer(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &observer,
            )
            .await
            .expect("peerless asset claim discovery projects"),
        );
        assert_eq!(projected["revision"], transition.revision());
        assert_eq!(port.complete_calls(), 0);
        assert_eq!(port.submit_calls(), 0);
        assert!(matches!(
            port.targets.lock().expect("asset targets").as_slice(),
            [FinalizedWitnessedAssetTransactionTargetV2::DiscoverByTerms {}]
        ));
    }
}

#[tokio::test(flavor = "current_thread")]
#[allow(clippy::too_many_lines)]
async fn schema5_asset_claim_uncertainty_unavailability_and_conflict_never_authorize_send() {
    for outcome in ["uncertain", "unavailable"] {
        let mut fixture =
            ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
        let _ = configure_schema5_asset_claim(&mut fixture);
        activate_and_project_schema5_both_locks(&fixture).await;
        let transaction = asset_claim_transaction();
        let preparation = FixedLezAssetClaimPort::new(
            &fixture.config,
            transaction.clone(),
            unavailable_asset_claim_outcome(),
            Ok(SubmissionOutcome::Accepted),
        );
        let effect = prepare_lez_asset_claim_effect(
            &fixture.config,
            &fixture.agreement,
            ClaimTransition::RevealingClaim,
            &durable_status(&fixture),
            &preparation,
        )
        .await
        .expect("prepare asset claim")
        .expect("taker owns revealing claim");
        let scan = match outcome {
            "uncertain" => uncertain_asset_claim_outcome(&fixture.config),
            "unavailable" => unavailable_asset_claim_outcome(),
            _ => unreachable!("fixed outcomes"),
        };
        let port = FixedLezAssetClaimPort::new(
            &fixture.config,
            transaction,
            scan,
            Ok(SubmissionOutcome::Accepted),
        );
        let observer = LezAssetClaimObserver {
            config: &fixture.config,
            chain: port.clone(),
            effect: Some(effect),
            prepared_claim: prepared_lez_asset_claim(&fixture.config, &fixture.agreement),
            state_db: fixture.config.state_db.clone(),
        };
        let pending = output_json(
            drive_claim_with_observer(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &observer,
            )
            .await
            .expect("non-authoritative asset scan remains pending"),
        );
        assert_eq!(pending["revision"], 2, "outcome: {outcome}");
        assert_eq!(port.submit_calls(), 0, "outcome: {outcome}");
    }

    let mut fixture =
        ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    let _ = configure_schema5_asset_claim(&mut fixture);
    activate_and_project_schema5_both_locks(&fixture).await;
    let transaction = asset_claim_transaction();
    let preparation = FixedLezAssetClaimPort::new(
        &fixture.config,
        transaction.clone(),
        unavailable_asset_claim_outcome(),
        Ok(SubmissionOutcome::Accepted),
    );
    let effect = prepare_lez_asset_claim_effect(
        &fixture.config,
        &fixture.agreement,
        ClaimTransition::RevealingClaim,
        &durable_status(&fixture),
        &preparation,
    )
    .await
    .expect("prepare conflicting asset claim")
    .expect("taker owns revealing claim");
    let mut conflicting = exact_asset_claim_outcome(
        &fixture,
        transaction.clone(),
        effect.aggregate_signature,
        true,
    );
    let FinalizedWitnessedAssetScanOutcomeV2::Found { facts, .. } = &mut conflicting else {
        unreachable!("exact found fixture")
    };
    facts.transaction.exact_bytes =
        ExactTransactionBytes::new(b"conflicting-canonical-asset-claim".to_vec())
            .expect("conflicting bytes");
    let conflict_port = FixedLezAssetClaimPort::new(
        &fixture.config,
        transaction.clone(),
        conflicting,
        Ok(SubmissionOutcome::Accepted),
    );
    let conflict_observer = LezAssetClaimObserver {
        config: &fixture.config,
        chain: conflict_port.clone(),
        effect: Some(effect.clone()),
        prepared_claim: prepared_lez_asset_claim(&fixture.config, &fixture.agreement),
        state_db: fixture.config.state_db.clone(),
    };
    assert_eq!(
        drive_claim_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &conflict_observer,
        )
        .await,
        Err(ActorCommandError::AgreementBindingInvalid)
    );
    assert_eq!(conflict_port.submit_calls(), 0);

    let absent_restart = FixedLezAssetClaimPort::new(
        &fixture.config,
        transaction,
        absent_asset_claim_outcome(&fixture.config),
        Ok(SubmissionOutcome::Accepted),
    );
    let restarted_observer = LezAssetClaimObserver {
        config: &fixture.config,
        chain: absent_restart.clone(),
        effect: Some(effect),
        prepared_claim: prepared_lez_asset_claim(&fixture.config, &fixture.agreement),
        state_db: fixture.config.state_db.clone(),
    };
    drive_claim_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &restarted_observer,
    )
    .await
    .expect("conflict-burned authority remains observe-only");
    assert_eq!(absent_restart.submit_calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn lez_claim_effect_is_role_and_revision_ordered() {
    let taker = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_both_locks(&taker).await;
    assert!(
        prepare_lez_claim_effect(
            &taker.config,
            &taker.agreement,
            ClaimTransition::RevealingClaim,
            &durable_status(&taker),
            &placeholder_lez_port(),
        )
        .await
        .expect("taker owns maker-funded LEZ claim")
        .is_some()
    );

    let maker = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    activate_and_project_both_locks(&maker).await;
    assert!(
        prepare_lez_claim_effect(
            &maker.config,
            &maker.agreement,
            ClaimTransition::RevealingClaim,
            &durable_status(&maker),
            &placeholder_lez_port(),
        )
        .await
        .expect("maker only observes its LEZ claim")
        .is_none()
    );

    let maker_owner = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Maker);
    activate_and_project_both_locks(&maker_owner).await;
    project_revealing_claim(&maker_owner).await;
    assert!(
        prepare_lez_claim_effect(
            &maker_owner.config,
            &maker_owner.agreement,
            ClaimTransition::FollowupClaim,
            &durable_status(&maker_owner),
            &placeholder_lez_port(),
        )
        .await
        .expect("maker owns taker-funded LEZ followup claim")
        .is_some()
    );

    let taker_observer =
        ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_both_locks(&taker_observer).await;
    project_revealing_claim(&taker_observer).await;
    assert!(
        prepare_lez_claim_effect(
            &taker_observer.config,
            &taker_observer.agreement,
            ClaimTransition::FollowupClaim,
            &durable_status(&taker_observer),
            &placeholder_lez_port(),
        )
        .await
        .expect("taker only observes its LEZ followup claim")
        .is_none()
    );
}

#[tokio::test(flavor = "current_thread")]
#[allow(clippy::too_many_lines)] // One scenario proves completion, send, restart window, and projection.
async fn lez_claim_owner_submits_once_then_projects_exact_finalized_presence() {
    for (direction, role, transition) in [
        (
            SwapDirection::TakerSellsForeign,
            ActorRole::Taker,
            ClaimTransition::RevealingClaim,
        ),
        (
            SwapDirection::TakerSellsLez,
            ActorRole::Maker,
            ClaimTransition::FollowupClaim,
        ),
    ] {
        let fixture = ActorFixture::for_direction(direction, role);
        activate_and_project_both_locks(&fixture).await;
        if transition == ClaimTransition::FollowupClaim {
            project_revealing_claim(&fixture).await;
        }
        let preparation_port = placeholder_lez_port();
        let effect = prepare_lez_claim_effect(
            &fixture.config,
            &fixture.agreement,
            transition,
            &durable_status(&fixture),
            &preparation_port,
        )
        .await
        .expect("prepare owned LEZ claim")
        .expect("role owns LEZ claim");
        let repeated = prepare_lez_claim_effect(
            &fixture.config,
            &fixture.agreement,
            transition,
            &durable_status(&fixture),
            &preparation_port,
        )
        .await
        .expect("repeat deterministic completion")
        .expect("role still owns LEZ claim");
        assert_eq!(effect, repeated);
        {
            let completion_requests = preparation_port
                .complete_requests
                .lock()
                .expect("completion request lock");
            assert_eq!(completion_requests.len(), 2);
            assert_eq!(completion_requests[0], completion_requests[1]);
        }

        let absence_port = FixedLezClaimPort::with_presence(
            effect.transaction.clone(),
            not_found_lez_claim_presence(
                &fixture.config,
                &fixture.agreement,
                transition,
                Some(&effect),
            ),
            Ok(SubmissionOutcome::Accepted),
        );
        let absence_observer = LezClaimObserver {
            config: &fixture.config,
            chain: absence_port.clone(),
            effect: Some(effect.clone()),
            prepared_claim: prepared_lez_claim(&fixture.config, &fixture.agreement),
            state_db: fixture.config.state_db.clone(),
        };
        let awaiting = output_json(
            drive_claim_with_observer(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &absence_observer,
            )
            .await
            .expect("one exact LEZ submission"),
        );
        assert_eq!(awaiting["outcome"], "awaiting_observation");
        assert_eq!(awaiting["revision"], transition.predecessor_revision());
        assert_eq!(absence_port.submit_calls(), 1);
        let first_request_id = {
            let absence_requests = absence_port
                .classify_requests
                .lock()
                .expect("absence request lock");
            assert!(matches!(
                absence_requests[0].target,
                FinalizedWitnessedClaimObservationTarget::Exact {
                    claim_transaction_id
                } if claim_transaction_id == effect.transaction.transaction_id
            ));
            absence_requests[0].context.request_id.clone()
        };

        let mut later_config = fixture.config.clone();
        later_config.lez_bridge.discovery_start_height = 21;
        later_config
            .validate()
            .expect("later valid discovery window");
        let exact_port = FixedLezClaimPort::with_presence(
            effect.transaction.clone(),
            exact_lez_claim_presence(
                &later_config,
                &fixture.agreement,
                transition,
                Some(&effect),
                effect.transaction.clone(),
                effect.aggregate_signature,
            ),
            Err(ActorCommandError::ObservationUnavailable),
        );
        let exact_observer = LezClaimObserver {
            config: &later_config,
            chain: exact_port.clone(),
            effect: Some(effect),
            prepared_claim: prepared_lez_claim(&later_config, &fixture.agreement),
            state_db: later_config.state_db.clone(),
        };
        let projected = output_json(
            drive_claim_with_observer(
                &later_config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &exact_observer,
            )
            .await
            .expect("project finalized exact LEZ claim"),
        );
        assert_eq!(projected["outcome"], "observed_then_projected");
        assert_eq!(projected["revision"], transition.revision());
        assert_eq!(exact_port.submit_calls(), 0);
        let exact_requests = exact_port
            .classify_requests
            .lock()
            .expect("exact request lock");
        assert_ne!(exact_requests[0].context.request_id, first_request_id);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn lez_claim_non_submitter_discovers_peerlessly_without_completion_or_send() {
    for (direction, role, transition) in [
        (
            SwapDirection::TakerSellsForeign,
            ActorRole::Maker,
            ClaimTransition::RevealingClaim,
        ),
        (
            SwapDirection::TakerSellsLez,
            ActorRole::Taker,
            ClaimTransition::FollowupClaim,
        ),
    ] {
        let fixture = ActorFixture::for_direction(direction, role);
        activate_and_project_both_locks(&fixture).await;
        if transition == ClaimTransition::FollowupClaim {
            project_revealing_claim(&fixture).await;
        }
        let transaction = PreparedTransaction::new(
            TransactionId::from_bytes([101; 32]),
            ExactTransactionBytes::new(vec![102; 128]).expect("observer transaction bytes"),
        );
        let port = FixedLezClaimPort::with_presence(
            transaction.clone(),
            exact_lez_claim_presence(
                &fixture.config,
                &fixture.agreement,
                transition,
                None,
                transaction,
                valid_lez_claim_signature(&fixture, transition),
            ),
            Err(ActorCommandError::ObservationUnavailable),
        );
        let observer = LezClaimObserver {
            config: &fixture.config,
            chain: port.clone(),
            effect: None,
            prepared_claim: prepared_lez_claim(&fixture.config, &fixture.agreement),
            state_db: fixture.config.state_db.clone(),
        };
        let projected = output_json(
            drive_claim_with_observer(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &observer,
            )
            .await
            .expect("peerless finalized LEZ discovery"),
        );
        assert_eq!(projected["outcome"], "observed_then_projected");
        assert_eq!(projected["revision"], transition.revision());
        assert_eq!(port.complete_calls.load(Ordering::SeqCst), 0);
        assert_eq!(port.submit_calls(), 0);
        let requests = port
            .classify_requests
            .lock()
            .expect("peerless classify request lock");
        assert!(matches!(
            requests[0].target,
            FinalizedWitnessedClaimObservationTarget::DiscoverByTerms
        ));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn lez_claim_unavailable_and_uncertain_are_retryable_observe_only() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_both_locks(&fixture).await;
    let transition = ClaimTransition::RevealingClaim;
    let effect = prepare_lez_claim_effect(
        &fixture.config,
        &fixture.agreement,
        transition,
        &durable_status(&fixture),
        &placeholder_lez_port(),
    )
    .await
    .expect("prepare LEZ claim")
    .expect("taker owns LEZ claim");
    for presence in [
        lez_bridge_client::FinalizedWitnessedClaimPresence::Unavailable(
            lez_bridge_client::FinalizedWitnessedClaimUnavailable::NodeFinalityOrHistory,
        ),
        lez_bridge_client::FinalizedWitnessedClaimPresence::Unavailable(
            lez_bridge_client::FinalizedWitnessedClaimUnavailable::MovingTip,
        ),
        lez_bridge_client::FinalizedWitnessedClaimPresence::Uncertain(
            lez_bridge_client::FinalizedWitnessedClaimUncertain::Timeout,
        ),
        lez_bridge_client::FinalizedWitnessedClaimPresence::Uncertain(
            lez_bridge_client::FinalizedWitnessedClaimUncertain::Transport,
        ),
    ] {
        let port = FixedLezClaimPort::with_presence(
            effect.transaction.clone(),
            presence,
            Ok(SubmissionOutcome::Accepted),
        );
        let observer = LezClaimObserver {
            config: &fixture.config,
            chain: port.clone(),
            effect: Some(effect.clone()),
            prepared_claim: prepared_lez_claim(&fixture.config, &fixture.agreement),
            state_db: fixture.config.state_db.clone(),
        };
        let awaiting = output_json(
            drive_claim_with_observer(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &observer,
            )
            .await
            .expect("uncertain observation stays pending"),
        );
        assert_eq!(awaiting["revision"], 2);
        assert_eq!(port.submit_calls(), 0);
    }

    let stable_absence_port = FixedLezClaimPort::with_presence(
        effect.transaction.clone(),
        not_found_lez_claim_presence(
            &fixture.config,
            &fixture.agreement,
            transition,
            Some(&effect),
        ),
        Ok(SubmissionOutcome::Accepted),
    );
    let stable_absence_observer = LezClaimObserver {
        config: &fixture.config,
        chain: stable_absence_port.clone(),
        effect: Some(effect),
        prepared_claim: prepared_lez_claim(&fixture.config, &fixture.agreement),
        state_db: fixture.config.state_db.clone(),
    };
    drive_claim_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &stable_absence_observer,
    )
    .await
    .expect("later stable absence authorizes one send");
    assert_eq!(stable_absence_port.submit_calls(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn lez_claim_prefix_uncertainty_authorizes_only_the_owned_exact_effect_once() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_both_locks(&fixture).await;
    let transition = ClaimTransition::RevealingClaim;
    let effect = prepare_lez_claim_effect(
        &fixture.config,
        &fixture.agreement,
        transition,
        &durable_status(&fixture),
        &placeholder_lez_port(),
    )
    .await
    .expect("prepare LEZ claim")
    .expect("taker owns LEZ claim");
    let port = FixedLezClaimPort::with_presence(
        effect.transaction.clone(),
        prefix_uncertain_lez_claim_presence(
            &fixture.config,
            &fixture.agreement,
            transition,
            Some(&effect),
        ),
        Ok(SubmissionOutcome::Accepted),
    );
    let observer = LezClaimObserver {
        config: &fixture.config,
        chain: port.clone(),
        effect: Some(effect.clone()),
        prepared_claim: prepared_lez_claim(&fixture.config, &fixture.agreement),
        state_db: fixture.config.state_db.clone(),
    };

    for expected_calls in [1, 1] {
        let awaiting = output_json(
            drive_claim_with_observer(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &observer,
            )
            .await
            .expect("prefix uncertainty remains pending"),
        );
        assert_eq!(awaiting["outcome"], "awaiting_observation");
        assert_eq!(awaiting["revision"], transition.predecessor_revision());
        assert_eq!(port.submit_calls(), expected_calls);
    }

    let submitted_transaction = {
        let requests = port.submit_requests.lock().expect("submit request lock");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].transaction, effect.transaction);
        requests[0].transaction.clone()
    };

    let peerless = FixedLezClaimPort::with_presence(
        submitted_transaction,
        prefix_uncertain_lez_claim_presence(&fixture.config, &fixture.agreement, transition, None),
        Ok(SubmissionOutcome::Accepted),
    );
    let peerless_observer = LezClaimObserver {
        config: &fixture.config,
        chain: peerless.clone(),
        effect: None,
        prepared_claim: prepared_lez_claim(&fixture.config, &fixture.agreement),
        state_db: fixture.config.state_db.clone(),
    };
    let _ = peerless_observer
        .observe(&fixture.agreement, transition)
        .await
        .expect("peerless prefix uncertainty remains pending");
    assert_eq!(peerless.submit_calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn lez_claim_post_authority_error_is_unknown_and_never_rearms() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_both_locks(&fixture).await;
    let transition = ClaimTransition::RevealingClaim;
    let effect = prepare_lez_claim_effect(
        &fixture.config,
        &fixture.agreement,
        transition,
        &durable_status(&fixture),
        &placeholder_lez_port(),
    )
    .await
    .expect("prepare LEZ claim")
    .expect("taker owns LEZ claim");
    let failing_port = FixedLezClaimPort::with_presence(
        effect.transaction.clone(),
        not_found_lez_claim_presence(
            &fixture.config,
            &fixture.agreement,
            transition,
            Some(&effect),
        ),
        Err(ActorCommandError::ObservationUnavailable),
    );
    let failing_observer = LezClaimObserver {
        config: &fixture.config,
        chain: failing_port.clone(),
        effect: Some(effect.clone()),
        prepared_claim: prepared_lez_claim(&fixture.config, &fixture.agreement),
        state_db: fixture.config.state_db.clone(),
    };
    assert_eq!(
        drive_claim_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &failing_observer,
        )
        .await
        .expect_err("post-authority failure is surfaced after Unknown"),
        ActorCommandError::ObservationUnavailable
    );
    assert_eq!(failing_port.submit_calls(), 1);

    let restarted_port = FixedLezClaimPort::with_presence(
        effect.transaction.clone(),
        not_found_lez_claim_presence(
            &fixture.config,
            &fixture.agreement,
            transition,
            Some(&effect),
        ),
        Ok(SubmissionOutcome::Accepted),
    );
    let restarted_observer = LezClaimObserver {
        config: &fixture.config,
        chain: restarted_port.clone(),
        effect: Some(effect),
        prepared_claim: prepared_lez_claim(&fixture.config, &fixture.agreement),
        state_db: fixture.config.state_db.clone(),
    };
    drive_claim_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &restarted_observer,
    )
    .await
    .expect("Unknown restart remains observe-only");
    assert_eq!(restarted_port.submit_calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
#[allow(clippy::too_many_lines)] // Crash and activation gates share one exact owned-effect setup.
async fn lez_claim_started_restart_and_missing_activation_never_send() {
    let started_fixture =
        ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_both_locks(&started_fixture).await;
    let transition = ClaimTransition::RevealingClaim;
    let started_effect = prepare_lez_claim_effect(
        &started_fixture.config,
        &started_fixture.agreement,
        transition,
        &durable_status(&started_fixture),
        &placeholder_lez_port(),
    )
    .await
    .expect("prepare LEZ claim")
    .expect("taker owns LEZ claim");
    let mut journal = SqlitePublicEffectJournal::open(&started_fixture.config.state_db)
        .expect("open effect journal");
    let _ = journal
        .record_prepared(&started_effect.effect)
        .expect("persist exact public bytes");
    assert!(matches!(
        journal
            .reconcile(started_effect.effect.key(), PublicEffectObservation::Absent,)
            .expect("consume send authority before simulated crash"),
        PublicEffectDecision::SubmitOnce(_)
    ));
    drop(journal);
    let restarted_port = FixedLezClaimPort::with_presence(
        started_effect.transaction.clone(),
        not_found_lez_claim_presence(
            &started_fixture.config,
            &started_fixture.agreement,
            transition,
            Some(&started_effect),
        ),
        Ok(SubmissionOutcome::Accepted),
    );
    let restarted_observer = LezClaimObserver {
        config: &started_fixture.config,
        chain: restarted_port.clone(),
        effect: Some(started_effect),
        prepared_claim: prepared_lez_claim(&started_fixture.config, &started_fixture.agreement),
        state_db: started_fixture.config.state_db.clone(),
    };
    drive_claim_with_observer(
        &started_fixture.config,
        started_fixture.agreement.clone(),
        started_fixture.agreement_wire.clone(),
        &restarted_observer,
    )
    .await
    .expect("Started restart remains observe-only");
    assert_eq!(restarted_port.submit_calls(), 0);

    let gated_fixture =
        ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_both_locks(&gated_fixture).await;
    let gated_effect = prepare_lez_claim_effect(
        &gated_fixture.config,
        &gated_fixture.agreement,
        transition,
        &durable_status(&gated_fixture),
        &placeholder_lez_port(),
    )
    .await
    .expect("prepare gated LEZ claim")
    .expect("taker owns gated LEZ claim");
    let gated_port = FixedLezClaimPort::with_presence(
        gated_effect.transaction.clone(),
        not_found_lez_claim_presence(
            &gated_fixture.config,
            &gated_fixture.agreement,
            transition,
            Some(&gated_effect),
        ),
        Ok(SubmissionOutcome::Accepted),
    );
    let gated_observer = LezClaimObserver {
        config: &gated_fixture.config,
        chain: gated_port.clone(),
        effect: Some(gated_effect),
        prepared_claim: prepared_lez_claim(&gated_fixture.config, &gated_fixture.agreement),
        state_db: gated_fixture.config.state_db.clone(),
    };
    fs::remove_file(
        &gated_fixture
            .config
            .signing
            .prepared_witnessed_claim_result_file,
    )
    .expect("remove activation material");
    assert_eq!(
        drive_claim_with_observer(
            &gated_fixture.config,
            gated_fixture.agreement.clone(),
            gated_fixture.agreement_wire.clone(),
            &gated_observer,
        )
        .await
        .expect_err("activation gate reruns before chain observation"),
        ActorCommandError::ActivationMaterialUnavailable
    );
    assert_eq!(gated_port.classify_calls.load(Ordering::SeqCst), 0);
    assert_eq!(gated_port.submit_calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn lez_claim_conflicting_exact_presence_never_sends_or_rearms() {
    for conflict_signature in [false, true] {
        let fixture =
            ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
        activate_and_project_both_locks(&fixture).await;
        let transition = ClaimTransition::RevealingClaim;
        let effect = prepare_lez_claim_effect(
            &fixture.config,
            &fixture.agreement,
            transition,
            &durable_status(&fixture),
            &placeholder_lez_port(),
        )
        .await
        .expect("prepare LEZ claim")
        .expect("taker owns LEZ claim");
        let transaction = if conflict_signature {
            effect.transaction.clone()
        } else {
            PreparedTransaction::new(
                effect.transaction.transaction_id,
                ExactTransactionBytes::new(vec![103; 128]).expect("conflicting exact bytes"),
            )
        };
        let mut signature = effect.aggregate_signature;
        if conflict_signature {
            signature[63] ^= 1;
        }
        let conflicting_port = FixedLezClaimPort::with_presence(
            effect.transaction.clone(),
            exact_lez_claim_presence(
                &fixture.config,
                &fixture.agreement,
                transition,
                Some(&effect),
                transaction,
                signature,
            ),
            Ok(SubmissionOutcome::Accepted),
        );
        let conflicting_observer = LezClaimObserver {
            config: &fixture.config,
            chain: conflicting_port.clone(),
            effect: Some(effect.clone()),
            prepared_claim: prepared_lez_claim(&fixture.config, &fixture.agreement),
            state_db: fixture.config.state_db.clone(),
        };
        assert_eq!(
            drive_claim_with_observer(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &conflicting_observer,
            )
            .await
            .expect_err("conflicting canonical facts fail closed"),
            ActorCommandError::AgreementBindingInvalid
        );
        assert_eq!(conflicting_port.submit_calls(), 0);
        assert_eq!(durable_status(&fixture).revision(), 2);

        let restarted_port = FixedLezClaimPort::with_presence(
            effect.transaction.clone(),
            not_found_lez_claim_presence(
                &fixture.config,
                &fixture.agreement,
                transition,
                Some(&effect),
            ),
            Ok(SubmissionOutcome::Accepted),
        );
        let restarted_observer = LezClaimObserver {
            config: &fixture.config,
            chain: restarted_port.clone(),
            effect: Some(effect),
            prepared_claim: prepared_lez_claim(&fixture.config, &fixture.agreement),
            state_db: fixture.config.state_db.clone(),
        };
        drive_claim_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &restarted_observer,
        )
        .await
        .expect("conflicting-presence restart remains observe-only");
        assert_eq!(restarted_port.submit_calls(), 0);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn lez_claim_rejects_containing_block_outside_scanned_finalized_window() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    activate_and_project_both_locks(&fixture).await;
    let transition = ClaimTransition::RevealingClaim;
    let transaction = PreparedTransaction::new(
        TransactionId::from_bytes([104; 32]),
        ExactTransactionBytes::new(vec![105; 128]).expect("observer transaction bytes"),
    );
    let mut presence = exact_lez_claim_presence(
        &fixture.config,
        &fixture.agreement,
        transition,
        None,
        transaction.clone(),
        valid_lez_claim_signature(&fixture, transition),
    );
    let lez_bridge_client::FinalizedWitnessedClaimPresence::PresentExact {
        finalized_tip,
        claim,
        ..
    } = &mut presence
    else {
        unreachable!("exact presence fixture")
    };
    let outside_height = finalized_tip.height + 1;
    claim.containing_block.block_id = outside_height;
    claim.transaction.position.height = outside_height;
    let port = FixedLezClaimPort::with_presence(
        transaction,
        presence,
        Err(ActorCommandError::ObservationUnavailable),
    );
    let observer = LezClaimObserver {
        config: &fixture.config,
        chain: port.clone(),
        effect: None,
        prepared_claim: prepared_lez_claim(&fixture.config, &fixture.agreement),
        state_db: fixture.config.state_db.clone(),
    };
    assert_eq!(
        drive_claim_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &observer,
        )
        .await
        .expect_err("claim outside scanned finalized window fails closed"),
        ActorCommandError::AgreementBindingInvalid
    );
    assert_eq!(durable_status(&fixture).revision(), 2);
    assert_eq!(port.submit_calls(), 0);
}

impl FixedBitcoinClaimPort {
    fn new(
        scan: BitcoinClaimScan,
        submission: Result<AuthorizedClaimSubmission, ActorCommandError>,
    ) -> Self {
        Self {
            scan,
            submission,
            observe_calls: Arc::new(AtomicUsize::new(0)),
            submit_calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn submit_calls(&self) -> usize {
        self.submit_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl BitcoinClaimChainPort for FixedBitcoinClaimPort {
    async fn observe_claim(&self, _agreement: &BtcAgreementV1) -> BitcoinClaimScan {
        self.observe_calls.fetch_add(1, Ordering::SeqCst);
        self.scan.clone()
    }

    async fn submit_authorized_claim(
        &self,
        _agreement: &BtcAgreementV1,
        _transaction_bytes: &[u8],
        _expected_transaction_id: bitcoin::Txid,
    ) -> Result<AuthorizedClaimSubmission, ActorCommandError> {
        self.submit_calls.fetch_add(1, Ordering::SeqCst);
        self.submission.clone()
    }
}

fn durable_status(fixture: &ActorFixture) -> BtcOfflineStatus {
    open_existing_store(
        &fixture.config,
        &fixture.agreement,
        fixture.agreement_wire.clone(),
    )
    .expect("open actor state")
    .status()
    .expect("replay actor state")
}

fn persisted_lifecycle_evidence(fixture: &ActorFixture, revision: u64) -> BtcLifecycleEvidenceV1 {
    let connection =
        rusqlite::Connection::open(&fixture.config.state_db).expect("open actor evidence database");
    let payload: String = connection
        .query_row(
            "SELECT payload_json FROM btc_actor_evidence WHERE aggregate_revision = ?1",
            [i64::try_from(revision).expect("test revision")],
            |row| row.get(0),
        )
        .expect("load persisted lifecycle evidence");
    serde_json::from_str(&payload).expect("decode persisted lifecycle evidence")
}

fn exact_finalized_scan(
    effect: &PreparedBitcoinClaimEffect,
    public_signature: [u8; 64],
) -> BitcoinClaimScan {
    BitcoinClaimScan::Exact(BitcoinExactClaim {
        transaction_bytes: effect.effect.exact_public_bytes().to_vec(),
        transaction_id: effect.expected_transaction_id.to_string().into_boxed_str(),
        confirmations: support::REQUIRED_CONFIRMATIONS,
        chain_evidence: b"exact-finalized-bitcoin-claim".to_vec(),
        public_signature,
        finalized: true,
    })
}

#[tokio::test(flavor = "current_thread")]
#[allow(clippy::similar_names)] // Parallel role names make the authority matrix explicit.
async fn bitcoin_claim_effect_is_role_and_revision_ordered() {
    let revealing_taker =
        ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_both_locks(&revealing_taker).await;
    assert!(
        prepare_bitcoin_claim_effect(
            &revealing_taker.config,
            &revealing_taker.agreement,
            ClaimTransition::RevealingClaim,
            &durable_status(&revealing_taker),
        )
        .expect("prepare taker revealing claim")
        .is_some()
    );

    let revealing_maker =
        ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Maker);
    activate_and_project_both_locks(&revealing_maker).await;
    assert!(
        prepare_bitcoin_claim_effect(
            &revealing_maker.config,
            &revealing_maker.agreement,
            ClaimTransition::RevealingClaim,
            &durable_status(&revealing_maker),
        )
        .expect("maker only observes revealing claim")
        .is_none()
    );

    let followup_maker =
        ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    activate_and_project_both_locks(&followup_maker).await;
    project_revealing_claim(&followup_maker).await;
    assert!(
        prepare_bitcoin_claim_effect(
            &followup_maker.config,
            &followup_maker.agreement,
            ClaimTransition::FollowupClaim,
            &durable_status(&followup_maker),
        )
        .expect("prepare maker followup claim")
        .is_some()
    );

    let followup_taker =
        ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_both_locks(&followup_taker).await;
    project_revealing_claim(&followup_taker).await;
    assert!(
        prepare_bitcoin_claim_effect(
            &followup_taker.config,
            &followup_taker.agreement,
            ClaimTransition::FollowupClaim,
            &durable_status(&followup_taker),
        )
        .expect("taker only observes followup claim")
        .is_none()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn bitcoin_claim_effect_persists_before_one_send_then_projects_only_when_finalized() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_both_locks(&fixture).await;
    let transition = ClaimTransition::RevealingClaim;
    let effect = prepare_bitcoin_claim_effect(
        &fixture.config,
        &fixture.agreement,
        transition,
        &durable_status(&fixture),
    )
    .expect("prepare revealing effect")
    .expect("taker owns revealing effect");
    let first_port = FixedBitcoinClaimPort::new(
        BitcoinClaimScan::Unspent,
        Ok(AuthorizedClaimSubmission::Accepted {
            transaction_id: effect.expected_transaction_id,
        }),
    );
    let first_observer = BitcoinClaimObserver {
        chain: first_port.clone(),
        effect: Some(effect.clone()),
        state_db: fixture.config.state_db.clone(),
    };

    let first = output_json(
        drive_claim_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &first_observer,
        )
        .await
        .expect("submit exact revealing claim"),
    );
    assert_eq!(first["outcome"], "awaiting_observation");
    assert_eq!(first["revision"], 2);
    assert_eq!(first_port.submit_calls(), 1);

    let finalized_port = FixedBitcoinClaimPort::new(
        exact_finalized_scan(&effect, revealing_signature(&fixture)),
        Err(ActorCommandError::ObservationUnavailable),
    );
    let finalized_observer = BitcoinClaimObserver {
        chain: finalized_port.clone(),
        effect: Some(effect),
        state_db: fixture.config.state_db.clone(),
    };
    let finalized = output_json(
        drive_claim_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &finalized_observer,
        )
        .await
        .expect("project only finalized exact claim"),
    );
    assert_eq!(finalized["outcome"], "observed_then_projected");
    assert_eq!(finalized["revision"], 3);
    assert_eq!(finalized_port.submit_calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn bitcoin_claim_uncertain_scan_stays_prepared_then_stable_scan_submits_once() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_both_locks(&fixture).await;
    let effect = prepare_bitcoin_claim_effect(
        &fixture.config,
        &fixture.agreement,
        ClaimTransition::RevealingClaim,
        &durable_status(&fixture),
    )
    .expect("prepare revealing effect")
    .expect("taker owns revealing effect");

    let uncertain_port = FixedBitcoinClaimPort::new(
        BitcoinClaimScan::Uncertain,
        Ok(AuthorizedClaimSubmission::Accepted {
            transaction_id: effect.expected_transaction_id,
        }),
    );
    let uncertain_observer = BitcoinClaimObserver {
        chain: uncertain_port.clone(),
        effect: Some(effect.clone()),
        state_db: fixture.config.state_db.clone(),
    };
    let pending = output_json(
        drive_claim_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &uncertain_observer,
        )
        .await
        .expect("uncertain scan remains observation-only"),
    );
    assert_eq!(pending["outcome"], "awaiting_observation");
    assert_eq!(pending["revision"], 2);
    assert_eq!(uncertain_port.submit_calls(), 0);

    let stable_port = FixedBitcoinClaimPort::new(
        BitcoinClaimScan::Unspent,
        Ok(AuthorizedClaimSubmission::Accepted {
            transaction_id: effect.expected_transaction_id,
        }),
    );
    let stable_observer = BitcoinClaimObserver {
        chain: stable_port.clone(),
        effect: Some(effect.clone()),
        state_db: fixture.config.state_db.clone(),
    };
    drive_claim_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &stable_observer,
    )
    .await
    .expect("stable absence consumes one send authority");
    assert_eq!(stable_port.submit_calls(), 1);

    let replay_port = FixedBitcoinClaimPort::new(
        BitcoinClaimScan::Unspent,
        Ok(AuthorizedClaimSubmission::Accepted {
            transaction_id: effect.expected_transaction_id,
        }),
    );
    let replay_observer = BitcoinClaimObserver {
        chain: replay_port.clone(),
        effect: Some(effect),
        state_db: fixture.config.state_db.clone(),
    };
    drive_claim_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &replay_observer,
    )
    .await
    .expect("accepted send authority remains consumed after restart");
    assert_eq!(replay_port.submit_calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn bitcoin_claim_unknown_never_rearms_after_restart() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_both_locks(&fixture).await;
    let effect = prepare_bitcoin_claim_effect(
        &fixture.config,
        &fixture.agreement,
        ClaimTransition::RevealingClaim,
        &durable_status(&fixture),
    )
    .expect("prepare revealing effect")
    .expect("taker owns revealing effect");
    let failing_port = FixedBitcoinClaimPort::new(
        BitcoinClaimScan::Unspent,
        Err(ActorCommandError::ObservationUnavailable),
    );
    let failing_observer = BitcoinClaimObserver {
        chain: failing_port.clone(),
        effect: Some(effect.clone()),
        state_db: fixture.config.state_db.clone(),
    };
    assert_eq!(
        drive_claim_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &failing_observer,
        )
        .await
        .expect_err("post-authority error is surfaced after recording Unknown"),
        ActorCommandError::ObservationUnavailable
    );
    assert_eq!(failing_port.submit_calls(), 1);

    let restarted_port = FixedBitcoinClaimPort::new(
        BitcoinClaimScan::Unspent,
        Ok(AuthorizedClaimSubmission::Accepted {
            transaction_id: effect.expected_transaction_id,
        }),
    );
    let restarted_observer = BitcoinClaimObserver {
        chain: restarted_port.clone(),
        effect: Some(effect),
        state_db: fixture.config.state_db.clone(),
    };
    drive_claim_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &restarted_observer,
    )
    .await
    .expect("Unknown restart remains observe-only");
    assert_eq!(restarted_port.submit_calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn bitcoin_claim_started_never_rearms_after_restart() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_both_locks(&fixture).await;
    let effect = prepare_bitcoin_claim_effect(
        &fixture.config,
        &fixture.agreement,
        ClaimTransition::RevealingClaim,
        &durable_status(&fixture),
    )
    .expect("prepare revealing effect")
    .expect("taker owns revealing effect");
    let mut journal =
        SqlitePublicEffectJournal::open(&fixture.config.state_db).expect("open effect journal");
    let _ = journal
        .record_prepared(&effect.effect)
        .expect("persist exact public bytes");
    assert!(matches!(
        journal
            .reconcile(effect.effect.key(), PublicEffectObservation::Absent)
            .expect("consume send authority before simulated crash"),
        PublicEffectDecision::SubmitOnce(_)
    ));
    drop(journal);

    let restarted_port = FixedBitcoinClaimPort::new(
        BitcoinClaimScan::Unspent,
        Ok(AuthorizedClaimSubmission::Accepted {
            transaction_id: effect.expected_transaction_id,
        }),
    );
    let restarted_observer = BitcoinClaimObserver {
        chain: restarted_port.clone(),
        effect: Some(effect),
        state_db: fixture.config.state_db.clone(),
    };
    drive_claim_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &restarted_observer,
    )
    .await
    .expect("Started restart remains observe-only");
    assert_eq!(restarted_port.submit_calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn bitcoin_claim_wrong_bytes_and_missing_secret_never_gain_send_authority() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_both_locks(&fixture).await;
    let effect = prepare_bitcoin_claim_effect(
        &fixture.config,
        &fixture.agreement,
        ClaimTransition::RevealingClaim,
        &durable_status(&fixture),
    )
    .expect("prepare revealing effect")
    .expect("taker owns revealing effect");
    let mut wrong = exact_finalized_scan(&effect, revealing_signature(&fixture));
    let BitcoinClaimScan::Exact(exact) = &mut wrong else {
        unreachable!("exact scan")
    };
    exact.transaction_bytes.push(0);
    let wrong_port = FixedBitcoinClaimPort::new(
        wrong,
        Ok(AuthorizedClaimSubmission::Accepted {
            transaction_id: effect.expected_transaction_id,
        }),
    );
    let wrong_observer = BitcoinClaimObserver {
        chain: wrong_port.clone(),
        effect: Some(effect.clone()),
        state_db: fixture.config.state_db.clone(),
    };
    let pending = output_json(
        drive_claim_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &wrong_observer,
        )
        .await
        .expect("wrong exact bytes become a durable no-send conflict"),
    );
    assert_eq!(pending["revision"], 2);
    assert_eq!(wrong_port.submit_calls(), 0);

    let restarted_port = FixedBitcoinClaimPort::new(
        BitcoinClaimScan::Unspent,
        Ok(AuthorizedClaimSubmission::Accepted {
            transaction_id: effect.expected_transaction_id,
        }),
    );
    let restarted_observer = BitcoinClaimObserver {
        chain: restarted_port.clone(),
        effect: Some(effect.clone()),
        state_db: fixture.config.state_db.clone(),
    };
    drive_claim_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &restarted_observer,
    )
    .await
    .expect("conflicting-presence restart remains observe-only");
    assert_eq!(restarted_port.submit_calls(), 0);

    fs::remove_file(
        fixture
            .config
            .signing
            .adaptor_secret_file
            .as_ref()
            .expect("taker secret path"),
    )
    .expect("remove owner secret");
    assert_eq!(
        prepare_bitcoin_claim_effect(
            &fixture.config,
            &fixture.agreement,
            ClaimTransition::RevealingClaim,
            &durable_status(&fixture),
        )
        .expect_err("missing secret fails before effect preparation"),
        ActorCommandError::ActivationMaterialUnavailable
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FixedLezRefundLookup {
    Unknown,
    Found,
}

#[derive(Clone)]
struct FixedLezRefundPort {
    transaction: PreparedTransaction,
    metadata_account: Hex32,
    custody_account: Hex32,
    state_only_state: EscrowState,
    clock_ms: u64,
    lookup: FixedLezRefundLookup,
    submission: Result<SubmissionOutcome, ActorCommandError>,
    prepare_calls: Arc<AtomicUsize>,
    observe_calls: Arc<AtomicUsize>,
    submit_calls: Arc<AtomicUsize>,
    calls: Arc<Mutex<Vec<String>>>,
}

impl FixedLezRefundPort {
    fn new(
        fixture: &ActorFixture,
        state_only_state: EscrowState,
        clock_ms: u64,
        lookup: FixedLezRefundLookup,
        submission: Result<SubmissionOutcome, ActorCommandError>,
    ) -> Self {
        Self {
            transaction: PreparedTransaction::new(
                TransactionId::from_bytes([121; 32]),
                ExactTransactionBytes::new(vec![122; 128])
                    .expect("exact deterministic LEZ refund bytes"),
            ),
            metadata_account: Hex32::from_bytes(*fixture.agreement.lez_terms().metadata_account()),
            custody_account: Hex32::from_bytes(*fixture.agreement.lez_terms().custody_account()),
            state_only_state,
            clock_ms,
            lookup,
            submission,
            prepare_calls: Arc::new(AtomicUsize::new(0)),
            observe_calls: Arc::new(AtomicUsize::new(0)),
            submit_calls: Arc::new(AtomicUsize::new(0)),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn prepare_calls(&self) -> usize {
        self.prepare_calls.load(Ordering::SeqCst)
    }

    fn submit_calls(&self) -> usize {
        self.submit_calls.load(Ordering::SeqCst)
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("LEZ refund call log").clone()
    }

    fn record(&self, call: &str) {
        self.calls
            .lock()
            .expect("LEZ refund call log")
            .push(call.to_owned());
    }

    fn accounts(
        &self,
        request: &ObserveNativeRefundRequest,
        state: EscrowState,
    ) -> NativeEscrowAccountObservation {
        let terms = request
            .terms
            .witnessed()
            .expect("actor always requests witnessed refund terms");
        NativeEscrowAccountObservation::found(NativeEscrowAccountFacts::new_witnessed(
            WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
                self.metadata_account,
                request.runtime.escrow_program_id,
                self.custody_account,
                terms,
                state,
            ),
            NativeCustodyFacts::new(
                self.custody_account,
                terms.authenticated_transfer_program_id(),
                if state == EscrowState::Funded {
                    terms.amount().as_u128()
                } else {
                    0
                },
            ),
        ))
    }

    fn found_refund(&self, request: &ObserveNativeRefundRequest) -> NativeRefundObservation {
        let terms = request
            .terms
            .witnessed()
            .expect("actor always requests witnessed refund terms");
        let window = match request.target {
            NativeRefundObservationTarget::Exact { window, .. }
            | NativeRefundObservationTarget::DiscoverByTerms { window } => window,
            NativeRefundObservationTarget::StateOnly => {
                unreachable!("found refund requires a transaction lookup")
            }
        };
        let block_height = window.start_height() + 1;
        let block_hash = Hex32::from_bytes([123; 32]);
        NativeRefundObservation::found(NativeRefundFoundFacts::new(
            ObservedTransactionFacts::new(
                self.transaction.transaction_id,
                self.transaction.exact_bytes.clone(),
                ChainPosition::new(block_hash, block_height, 0),
                AccountIds::new(Vec::new()).expect("permissionless refund has no signers"),
                true,
            ),
            NativeRefundInstructionFacts::new(
                request.runtime.escrow_program_id,
                AccountIds::new(vec![
                    self.metadata_account,
                    self.custody_account,
                    terms.depositor_account_id(),
                ])
                .expect("refund account order"),
                terms.swap_id(),
            ),
        ))
    }
}

#[async_trait]
impl LezRefundChainPort for FixedLezRefundPort {
    async fn prepare_native_refund(
        &self,
        request: PrepareNativeRefundRequest,
    ) -> Result<PrepareNativeRefundResult, ActorCommandError> {
        self.prepare_calls.fetch_add(1, Ordering::SeqCst);
        self.record("prepare");
        Ok(PrepareNativeRefundResult::new(
            request.context,
            self.transaction.clone(),
        ))
    }

    async fn observe_native_refund(
        &self,
        request: ObserveNativeRefundRequest,
    ) -> Result<ObserveNativeRefundResult, ActorCommandError> {
        self.observe_calls.fetch_add(1, Ordering::SeqCst);
        let (state, refund) = match request.target {
            NativeRefundObservationTarget::StateOnly => {
                self.record("state_only");
                (self.state_only_state, NativeRefundObservation::NotRequested)
            }
            NativeRefundObservationTarget::Exact { .. } => {
                self.record("exact");
                match self.lookup {
                    FixedLezRefundLookup::Unknown => (
                        EscrowState::Funded,
                        NativeRefundObservation::UnknownOrPending,
                    ),
                    FixedLezRefundLookup::Found => {
                        (EscrowState::Refunded, self.found_refund(&request))
                    }
                }
            }
            NativeRefundObservationTarget::DiscoverByTerms { .. } => {
                self.record("discover_by_terms");
                match self.lookup {
                    FixedLezRefundLookup::Unknown => (
                        EscrowState::Funded,
                        NativeRefundObservation::UnknownOrPending,
                    ),
                    FixedLezRefundLookup::Found => {
                        (EscrowState::Refunded, self.found_refund(&request))
                    }
                }
            }
        };
        let clock = ChainClock::new(
            Hex32::from_bytes([124; 32]),
            match request.target {
                NativeRefundObservationTarget::StateOnly => 3,
                NativeRefundObservationTarget::Exact { window, .. }
                | NativeRefundObservationTarget::DiscoverByTerms { window } => {
                    window.start_height() + u64::from(window.max_blocks() - 1)
                }
            },
            self.clock_ms,
        );
        let accounts = self.accounts(&request, state);
        Ok(ObserveNativeRefundResult::new(
            request.context,
            clock,
            accounts,
            refund,
            clock,
        ))
    }

    async fn submit_transaction(
        &self,
        request: SubmitTransactionRequest,
    ) -> Result<SubmitTransactionResult, ActorCommandError> {
        self.submit_calls.fetch_add(1, Ordering::SeqCst);
        self.record("submit");
        assert_eq!(request.transaction, self.transaction);
        self.submission.map(|outcome| {
            SubmitTransactionResult::new(
                request.context,
                request.transaction.transaction_id,
                outcome,
            )
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FixedBitcoinRefundSubmission {
    transaction_bytes: Vec<u8>,
    expected_transaction_id: Txid,
}

#[derive(Clone)]
struct FixedBitcoinRefundPort {
    scan: BitcoinRefundScan,
    submission: Result<AuthorizedRefundSubmission, ActorCommandError>,
    observe_calls: Arc<AtomicUsize>,
    submit_calls: Arc<AtomicUsize>,
    submitted: Arc<Mutex<Vec<FixedBitcoinRefundSubmission>>>,
}

impl FixedBitcoinRefundPort {
    fn new(
        scan: BitcoinRefundScan,
        submission: Result<AuthorizedRefundSubmission, ActorCommandError>,
    ) -> Self {
        Self {
            scan,
            submission,
            observe_calls: Arc::new(AtomicUsize::new(0)),
            submit_calls: Arc::new(AtomicUsize::new(0)),
            submitted: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn submit_calls(&self) -> usize {
        self.submit_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl BitcoinRefundChainPort for FixedBitcoinRefundPort {
    async fn observe_refund(&self, _agreement: &BtcAgreementV1) -> BitcoinRefundScan {
        self.observe_calls.fetch_add(1, Ordering::SeqCst);
        self.scan.clone()
    }

    async fn submit_authorized_refund(
        &self,
        _agreement: &BtcAgreementV1,
        transaction_bytes: &[u8],
        expected_transaction_id: Txid,
    ) -> Result<AuthorizedRefundSubmission, ActorCommandError> {
        self.submit_calls.fetch_add(1, Ordering::SeqCst);
        self.submitted
            .lock()
            .expect("Bitcoin refund submission log")
            .push(FixedBitcoinRefundSubmission {
                transaction_bytes: transaction_bytes.to_vec(),
                expected_transaction_id,
            });
        self.submission.clone()
    }
}

fn exact_finalized_bitcoin_refund_scan(effect: &PreparedBitcoinRefundEffect) -> BitcoinRefundScan {
    BitcoinRefundScan::Exact(BitcoinExactRefund {
        transaction_bytes: effect.effect.exact_public_bytes().to_vec(),
        transaction_id: effect.expected_transaction_id.to_string().into_boxed_str(),
        witness_transaction_id: effect
            .expected_witness_transaction_id
            .to_string()
            .into_boxed_str(),
        confirmations: support::REQUIRED_CONFIRMATIONS,
        block_height: Some(1_145),
        chain_evidence: b"canonical-core-refund-evidence".to_vec(),
        finalized: true,
    })
}

fn prepared_first_lock_bitcoin_refund(fixture: &ActorFixture) -> PreparedBitcoinRefundEffect {
    prepare_bitcoin_refund_effect(
        &fixture.config,
        &fixture.agreement,
        RefundTransition::FirstLockRecovery,
        &durable_status(fixture),
    )
    .expect("prepare first-lock Bitcoin refund")
    .expect("the taker is the sole first-lock refund owner")
}

#[tokio::test(flavor = "current_thread")]
async fn first_lock_owner_does_not_send_without_fresh_maker_lock_absence_proof() {
    let bitcoin = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_taker_lock(&bitcoin).await;
    assert_eq!(
        bitcoin
            .agreement
            .coordinator()
            .funded_chain(Participant::Taker),
        Chain::Bitcoin
    );
    let bitcoin_effect = prepared_first_lock_bitcoin_refund(&bitcoin);
    let bitcoin_port = FixedBitcoinRefundPort::new(
        BitcoinRefundScan::Eligible,
        Ok(AuthorizedRefundSubmission::Accepted {
            transaction_id: bitcoin_effect.expected_transaction_id,
            witness_transaction_id: bitcoin_effect.expected_witness_transaction_id,
        }),
    );
    let bitcoin_observer = BitcoinRefundObserver {
        chain: bitcoin_port.clone(),
        effect: Some(bitcoin_effect),
        state_db: bitcoin.config.state_db.clone(),
    };
    let bitcoin_output = drive_refund_with_observer(
        &bitcoin.config,
        bitcoin.agreement.clone(),
        bitcoin.agreement_wire.clone(),
        &bitcoin_observer,
    )
    .await
    .expect("same-chain eligibility alone remains observe-only");

    let lez = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_taker_lock(&lez).await;
    assert_eq!(
        lez.agreement.coordinator().funded_chain(Participant::Taker),
        Chain::Lez
    );
    let lez_port = FixedLezRefundPort::new(
        &lez,
        EscrowState::Funded,
        lez.agreement.lez_terms().refund_at_ms(),
        FixedLezRefundLookup::Unknown,
        Ok(SubmissionOutcome::Accepted),
    );
    let lez_observer = LezRefundObserver {
        config: lez.config.clone(),
        chain: lez_port.clone(),
        state_db: lez.config.state_db.clone(),
    };
    let lez_output = drive_refund_with_observer(
        &lez.config,
        lez.agreement.clone(),
        lez.agreement_wire.clone(),
        &lez_observer,
    )
    .await
    .expect("same-chain deadline alone remains observe-only");

    // Both ports intentionally supply only the first-lock chain state. Neither
    // supplies a signed last-safe maker-lock cutoff nor a fresh canonical
    // absence observation from the opposite chain. Send authority must remain
    // untouched until a composed admission proof binds both observations.
    assert_eq!(
        (
            bitcoin_port.submit_calls(),
            lez_port.prepare_calls(),
            lez_port.submit_calls(),
        ),
        (0, 0, 0),
    );
    assert_eq!(bitcoin_output.revision, 1);
    assert_eq!(lez_output.revision, 1);
}

#[allow(clippy::too_many_lines)] // One first-lock contract covers owner, crash states, and both role projections.
#[tokio::test(flavor = "current_thread")]
async fn first_lock_bitcoin_refund_is_taker_owned_crash_safe_and_observable() {
    let accepted = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_taker_lock(&accepted).await;
    let accepted_effect = prepared_first_lock_bitcoin_refund(&accepted);
    let accepted_submission = AuthorizedRefundSubmission::Accepted {
        transaction_id: accepted_effect.expected_transaction_id,
        witness_transaction_id: accepted_effect.expected_witness_transaction_id,
    };
    let accepted_port =
        FixedBitcoinRefundPort::new(BitcoinRefundScan::Eligible, Ok(accepted_submission.clone()));
    let accepted_observer = BitcoinRefundObserver {
        chain: accepted_port.clone(),
        effect: Some(accepted_effect.clone()),
        state_db: accepted.config.state_db.clone(),
    };
    let pending = drive_admitted_first_lock_refund(&accepted, &accepted_observer)
        .await
        .expect("admitted first-lock Bitcoin refund sends once");
    assert_eq!(pending.revision, 1);
    assert_eq!(accepted_port.submit_calls(), 1);

    let accepted_restart =
        FixedBitcoinRefundPort::new(BitcoinRefundScan::Eligible, Ok(accepted_submission));
    let accepted_restart_observer = BitcoinRefundObserver {
        chain: accepted_restart.clone(),
        effect: Some(accepted_effect.clone()),
        state_db: accepted.config.state_db.clone(),
    };
    drive_admitted_first_lock_refund(&accepted, &accepted_restart_observer)
        .await
        .expect("Accepted first-lock Bitcoin restart observes only");
    assert_eq!(accepted_restart.submit_calls(), 0);

    let exact = exact_finalized_bitcoin_refund_scan(&accepted_effect);
    let finalized_port = FixedBitcoinRefundPort::new(
        exact.clone(),
        Err(ActorCommandError::ObservationUnavailable),
    );
    let finalized_observer = BitcoinRefundObserver {
        chain: finalized_port.clone(),
        effect: Some(accepted_effect),
        state_db: accepted.config.state_db.clone(),
    };
    let finalized = drive_admitted_first_lock_refund(&accepted, &finalized_observer)
        .await
        .expect("exact finalized owner refund projects terminal revision two");
    assert_eq!(finalized.revision, 2);
    assert_eq!(finalized.phase, ActorPhaseV1::Refunded);
    assert_eq!(finalized_port.submit_calls(), 0);

    let nonowner = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    activate_and_project_taker_lock(&nonowner).await;
    assert!(nonowner.config.refund.bitcoin_refund_key_file.is_none());
    assert!(
        prepare_bitcoin_refund_effect(
            &nonowner.config,
            &nonowner.agreement,
            RefundTransition::FirstLockRecovery,
            &durable_status(&nonowner),
        )
        .expect("nonowner preparation is an observation-only result")
        .is_none()
    );
    let nonowner_port =
        FixedBitcoinRefundPort::new(exact, Err(ActorCommandError::ObservationUnavailable));
    let nonowner_observer = BitcoinRefundObserver {
        chain: nonowner_port.clone(),
        effect: None,
        state_db: nonowner.config.state_db.clone(),
    };
    let observed = drive_admitted_first_lock_refund(&nonowner, &nonowner_observer)
        .await
        .expect("maker only observes the taker-owned exact refund");
    assert_eq!(observed.revision, 2);
    assert_eq!(observed.phase, ActorPhaseV1::Refunded);
    assert_eq!(nonowner_port.submit_calls(), 0);

    let unknown = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_taker_lock(&unknown).await;
    let unknown_effect = prepared_first_lock_bitcoin_refund(&unknown);
    let unknown_port = FixedBitcoinRefundPort::new(
        BitcoinRefundScan::Eligible,
        Ok(AuthorizedRefundSubmission::Unknown),
    );
    let unknown_observer = BitcoinRefundObserver {
        chain: unknown_port.clone(),
        effect: Some(unknown_effect.clone()),
        state_db: unknown.config.state_db.clone(),
    };
    drive_admitted_first_lock_refund(&unknown, &unknown_observer)
        .await
        .expect("ambiguous first-lock Bitcoin send records Unknown");
    assert_eq!(unknown_port.submit_calls(), 1);
    let unknown_restart = FixedBitcoinRefundPort::new(
        BitcoinRefundScan::Eligible,
        Ok(AuthorizedRefundSubmission::Accepted {
            transaction_id: unknown_effect.expected_transaction_id,
            witness_transaction_id: unknown_effect.expected_witness_transaction_id,
        }),
    );
    let unknown_restart_observer = BitcoinRefundObserver {
        chain: unknown_restart.clone(),
        effect: Some(unknown_effect),
        state_db: unknown.config.state_db.clone(),
    };
    drive_admitted_first_lock_refund(&unknown, &unknown_restart_observer)
        .await
        .expect("Unknown first-lock Bitcoin restart observes only");
    assert_eq!(unknown_restart.submit_calls(), 0);

    let started = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_taker_lock(&started).await;
    let started_effect = prepared_first_lock_bitcoin_refund(&started);
    let mut journal = SqlitePublicEffectJournal::open(&started.config.state_db)
        .expect("open first-lock Bitcoin effect journal");
    let _ = journal
        .record_prepared(&started_effect.effect)
        .expect("persist exact first-lock Bitcoin refund");
    assert!(matches!(
        journal
            .reconcile(
                started_effect.effect.key(),
                PublicEffectObservation::EligibleToAttempt,
            )
            .expect("consume first-lock Bitcoin authority before simulated crash"),
        PublicEffectDecision::SubmitOnce(_)
    ));
    drop(journal);
    let started_restart = FixedBitcoinRefundPort::new(
        BitcoinRefundScan::Eligible,
        Ok(AuthorizedRefundSubmission::Accepted {
            transaction_id: started_effect.expected_transaction_id,
            witness_transaction_id: started_effect.expected_witness_transaction_id,
        }),
    );
    let started_restart_observer = BitcoinRefundObserver {
        chain: started_restart.clone(),
        effect: Some(started_effect),
        state_db: started.config.state_db.clone(),
    };
    drive_admitted_first_lock_refund(&started, &started_restart_observer)
        .await
        .expect("Started first-lock Bitcoin restart observes only");
    assert_eq!(started_restart.submit_calls(), 0);
}

#[allow(clippy::too_many_lines)] // One first-lock contract covers owner, crash states, and both role projections.
#[tokio::test(flavor = "current_thread")]
async fn first_lock_lez_refund_is_taker_owned_crash_safe_and_observable() {
    let accepted = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_taker_lock(&accepted).await;
    let deadline = accepted.agreement.lez_terms().refund_at_ms();
    let accepted_port = FixedLezRefundPort::new(
        &accepted,
        EscrowState::Funded,
        deadline,
        FixedLezRefundLookup::Unknown,
        Ok(SubmissionOutcome::Accepted),
    );
    let accepted_observer = LezRefundObserver {
        config: accepted.config.clone(),
        chain: accepted_port.clone(),
        state_db: accepted.config.state_db.clone(),
    };
    let pending = drive_admitted_first_lock_refund(&accepted, &accepted_observer)
        .await
        .expect("admitted first-lock LEZ refund sends once");
    assert_eq!(pending.revision, 1);
    assert_eq!(accepted_port.prepare_calls(), 1);
    assert_eq!(accepted_port.submit_calls(), 1);

    let accepted_restart = FixedLezRefundPort::new(
        &accepted,
        EscrowState::Funded,
        deadline,
        FixedLezRefundLookup::Unknown,
        Ok(SubmissionOutcome::Accepted),
    );
    let accepted_restart_observer = LezRefundObserver {
        config: accepted.config.clone(),
        chain: accepted_restart.clone(),
        state_db: accepted.config.state_db.clone(),
    };
    drive_admitted_first_lock_refund(&accepted, &accepted_restart_observer)
        .await
        .expect("Accepted first-lock LEZ restart observes only");
    assert_eq!(accepted_restart.submit_calls(), 0);

    let finalized_port = FixedLezRefundPort::new(
        &accepted,
        EscrowState::Refunded,
        deadline + 1,
        FixedLezRefundLookup::Found,
        Err(ActorCommandError::ObservationUnavailable),
    );
    let finalized_observer = LezRefundObserver {
        config: accepted.config.clone(),
        chain: finalized_port.clone(),
        state_db: accepted.config.state_db.clone(),
    };
    let finalized = drive_admitted_first_lock_refund(&accepted, &finalized_observer)
        .await
        .expect("exact finalized owner LEZ refund projects terminal revision two");
    assert_eq!(finalized.revision, 2);
    assert_eq!(finalized.phase, ActorPhaseV1::Refunded);
    assert_eq!(finalized_port.submit_calls(), 0);

    let nonowner = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Maker);
    activate_and_project_taker_lock(&nonowner).await;
    assert_eq!(
        prepare_lez_refund_effect(
            &nonowner.config,
            &nonowner.agreement,
            RefundTransition::FirstLockRecovery,
            &FixedLezRefundPort::new(
                &nonowner,
                EscrowState::Refunded,
                nonowner.agreement.lez_terms().refund_at_ms() + 1,
                FixedLezRefundLookup::Found,
                Err(ActorCommandError::ObservationUnavailable),
            ),
        )
        .await
        .expect_err("maker cannot prepare the taker-owned first-lock LEZ refund"),
        ActorCommandError::AgreementBindingInvalid
    );
    let nonowner_port = FixedLezRefundPort::new(
        &nonowner,
        EscrowState::Refunded,
        nonowner.agreement.lez_terms().refund_at_ms() + 1,
        FixedLezRefundLookup::Found,
        Err(ActorCommandError::ObservationUnavailable),
    );
    let nonowner_observer = LezRefundObserver {
        config: nonowner.config.clone(),
        chain: nonowner_port.clone(),
        state_db: nonowner.config.state_db.clone(),
    };
    let observed = drive_admitted_first_lock_refund(&nonowner, &nonowner_observer)
        .await
        .expect("maker only observes the taker-owned exact LEZ refund");
    assert_eq!(observed.revision, 2);
    assert_eq!(observed.phase, ActorPhaseV1::Refunded);
    assert_eq!(nonowner_port.prepare_calls(), 0);
    assert_eq!(nonowner_port.submit_calls(), 0);

    let unknown = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_taker_lock(&unknown).await;
    let unknown_deadline = unknown.agreement.lez_terms().refund_at_ms();
    let unknown_port = FixedLezRefundPort::new(
        &unknown,
        EscrowState::Funded,
        unknown_deadline,
        FixedLezRefundLookup::Unknown,
        Err(ActorCommandError::ObservationUnavailable),
    );
    let unknown_observer = LezRefundObserver {
        config: unknown.config.clone(),
        chain: unknown_port.clone(),
        state_db: unknown.config.state_db.clone(),
    };
    assert_eq!(
        drive_admitted_first_lock_refund(&unknown, &unknown_observer)
            .await
            .expect_err("ambiguous first-lock LEZ send records Unknown"),
        ActorCommandError::ObservationUnavailable
    );
    assert_eq!(unknown_port.submit_calls(), 1);
    let unknown_restart = FixedLezRefundPort::new(
        &unknown,
        EscrowState::Funded,
        unknown_deadline,
        FixedLezRefundLookup::Unknown,
        Ok(SubmissionOutcome::Accepted),
    );
    let unknown_restart_observer = LezRefundObserver {
        config: unknown.config.clone(),
        chain: unknown_restart.clone(),
        state_db: unknown.config.state_db.clone(),
    };
    drive_admitted_first_lock_refund(&unknown, &unknown_restart_observer)
        .await
        .expect("Unknown first-lock LEZ restart observes only");
    assert_eq!(unknown_restart.submit_calls(), 0);

    let started = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_taker_lock(&started).await;
    let started_deadline = started.agreement.lez_terms().refund_at_ms();
    let prepare_port = FixedLezRefundPort::new(
        &started,
        EscrowState::Funded,
        started_deadline,
        FixedLezRefundLookup::Unknown,
        Ok(SubmissionOutcome::Accepted),
    );
    let started_effect = prepare_lez_refund_effect(
        &started.config,
        &started.agreement,
        RefundTransition::FirstLockRecovery,
        &prepare_port,
    )
    .await
    .expect("prepare first-lock LEZ refund before simulated crash");
    let mut journal = SqlitePublicEffectJournal::open(&started.config.state_db)
        .expect("open first-lock LEZ effect journal");
    let _ = journal
        .record_prepared(&started_effect.effect)
        .expect("persist exact first-lock LEZ refund");
    assert!(matches!(
        journal
            .reconcile(
                started_effect.effect.key(),
                PublicEffectObservation::EligibleToAttempt,
            )
            .expect("consume first-lock LEZ authority before simulated crash"),
        PublicEffectDecision::SubmitOnce(_)
    ));
    drop(journal);
    let started_restart = FixedLezRefundPort::new(
        &started,
        EscrowState::Funded,
        started_deadline,
        FixedLezRefundLookup::Unknown,
        Ok(SubmissionOutcome::Accepted),
    );
    let started_restart_observer = LezRefundObserver {
        config: started.config.clone(),
        chain: started_restart.clone(),
        state_db: started.config.state_db.clone(),
    };
    drive_admitted_first_lock_refund(&started, &started_restart_observer)
        .await
        .expect("Started first-lock LEZ restart observes only");
    assert_eq!(started_restart.submit_calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn lez_refund_owner_before_deadline_only_reads_state() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    activate_and_project_both_locks(&fixture).await;
    assert_eq!(fixture.agreement.lez_depositor(), Participant::Maker);
    assert_eq!(
        fixture
            .agreement
            .coordinator()
            .funded_chain(Participant::Maker),
        Chain::Lez
    );
    let deadline = fixture.agreement.lez_terms().refund_at_ms();
    let port = FixedLezRefundPort::new(
        &fixture,
        EscrowState::Funded,
        deadline - 1,
        FixedLezRefundLookup::Unknown,
        Ok(SubmissionOutcome::Accepted),
    );
    let observer = LezRefundObserver {
        config: fixture.config.clone(),
        chain: port.clone(),
        state_db: fixture.config.state_db.clone(),
    };

    let output = output_json(ActorCommandOutputV1::Effect(
        drive_refund_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &observer,
        )
        .await
        .expect("pre-deadline recovery remains pending"),
    ));

    assert_eq!(output["outcome"], "awaiting_observation");
    assert_eq!(output["revision"], 2);
    assert_eq!(port.prepare_calls(), 0);
    assert_eq!(port.submit_calls(), 0);
    assert_eq!(port.calls(), vec!["state_only".to_owned()]);
}

#[tokio::test(flavor = "current_thread")]
async fn lez_refund_owner_submits_once_and_accepted_restart_never_rearms() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    activate_and_project_both_locks(&fixture).await;
    let deadline = fixture.agreement.lez_terms().refund_at_ms();
    let port = FixedLezRefundPort::new(
        &fixture,
        EscrowState::Funded,
        deadline,
        FixedLezRefundLookup::Unknown,
        Ok(SubmissionOutcome::Accepted),
    );
    let observer = LezRefundObserver {
        config: fixture.config.clone(),
        chain: port.clone(),
        state_db: fixture.config.state_db.clone(),
    };

    let awaiting = output_json(ActorCommandOutputV1::Effect(
        drive_refund_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &observer,
        )
        .await
        .expect("eligible owner submits one exact LEZ refund"),
    ));
    assert_eq!(awaiting["revision"], 2);
    assert_eq!(port.prepare_calls(), 1);
    assert_eq!(port.submit_calls(), 1);
    assert_eq!(
        port.calls(),
        vec![
            "state_only".to_owned(),
            "prepare".to_owned(),
            "exact".to_owned(),
            "submit".to_owned(),
        ]
    );

    let restarted_port = FixedLezRefundPort::new(
        &fixture,
        EscrowState::Funded,
        deadline,
        FixedLezRefundLookup::Unknown,
        Ok(SubmissionOutcome::Accepted),
    );
    let restarted_observer = LezRefundObserver {
        config: fixture.config.clone(),
        chain: restarted_port.clone(),
        state_db: fixture.config.state_db.clone(),
    };
    drive_refund_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &restarted_observer,
    )
    .await
    .expect("accepted LEZ restart remains observe-only");
    assert_eq!(restarted_port.submit_calls(), 0);
    assert_eq!(
        restarted_port.calls(),
        vec![
            "state_only".to_owned(),
            "prepare".to_owned(),
            "exact".to_owned(),
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn lez_refund_unknown_submission_restart_never_rearms() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    activate_and_project_both_locks(&fixture).await;
    let deadline = fixture.agreement.lez_terms().refund_at_ms();
    let failing_port = FixedLezRefundPort::new(
        &fixture,
        EscrowState::Funded,
        deadline,
        FixedLezRefundLookup::Unknown,
        Err(ActorCommandError::ObservationUnavailable),
    );
    let failing_observer = LezRefundObserver {
        config: fixture.config.clone(),
        chain: failing_port.clone(),
        state_db: fixture.config.state_db.clone(),
    };
    assert_eq!(
        drive_refund_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &failing_observer,
        )
        .await
        .expect_err("post-authority transport error is recorded as Unknown"),
        ActorCommandError::ObservationUnavailable
    );
    assert_eq!(failing_port.submit_calls(), 1);

    let restarted_port = FixedLezRefundPort::new(
        &fixture,
        EscrowState::Funded,
        deadline,
        FixedLezRefundLookup::Unknown,
        Ok(SubmissionOutcome::Accepted),
    );
    let restarted_observer = LezRefundObserver {
        config: fixture.config.clone(),
        chain: restarted_port.clone(),
        state_db: fixture.config.state_db.clone(),
    };
    drive_refund_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &restarted_observer,
    )
    .await
    .expect("Unknown LEZ restart remains observe-only");
    assert_eq!(restarted_port.submit_calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn lez_refund_exact_finality_projects_owner_and_nonowner_without_resubmission() {
    let owner = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    activate_and_project_both_locks(&owner).await;
    let owner_port = FixedLezRefundPort::new(
        &owner,
        EscrowState::Refunded,
        owner.agreement.lez_terms().refund_at_ms() + 1,
        FixedLezRefundLookup::Found,
        Ok(SubmissionOutcome::Accepted),
    );
    let owner_observer = LezRefundObserver {
        config: owner.config.clone(),
        chain: owner_port.clone(),
        state_db: owner.config.state_db.clone(),
    };
    let owner_output = output_json(ActorCommandOutputV1::Effect(
        drive_refund_with_observer(
            &owner.config,
            owner.agreement.clone(),
            owner.agreement_wire.clone(),
            &owner_observer,
        )
        .await
        .expect("owner projects exact finalized LEZ refund"),
    ));
    assert_eq!(owner_output["phase"], "maker_leg_refunded");
    assert_eq!(owner_output["revision"], 3);
    assert_eq!(owner_port.submit_calls(), 0);
    assert_eq!(
        owner_port.calls(),
        vec![
            "state_only".to_owned(),
            "prepare".to_owned(),
            "exact".to_owned(),
        ]
    );

    let nonowner = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_both_locks(&nonowner).await;
    assert_eq!(nonowner.agreement.lez_claimant(), Participant::Taker);
    let nonowner_port = FixedLezRefundPort::new(
        &nonowner,
        EscrowState::Refunded,
        nonowner.agreement.lez_terms().refund_at_ms() + 1,
        FixedLezRefundLookup::Found,
        Err(ActorCommandError::ObservationUnavailable),
    );
    let nonowner_observer = LezRefundObserver {
        config: nonowner.config.clone(),
        chain: nonowner_port.clone(),
        state_db: nonowner.config.state_db.clone(),
    };
    let nonowner_output = output_json(ActorCommandOutputV1::Effect(
        drive_refund_with_observer(
            &nonowner.config,
            nonowner.agreement.clone(),
            nonowner.agreement_wire.clone(),
            &nonowner_observer,
        )
        .await
        .expect("nonowner discovers and projects finalized LEZ refund"),
    ));
    assert_eq!(nonowner_output["phase"], "maker_leg_refunded");
    assert_eq!(nonowner_output["revision"], 3);
    assert_eq!(nonowner_port.prepare_calls(), 0);
    assert_eq!(nonowner_port.submit_calls(), 0);
    assert_eq!(nonowner_port.calls(), vec!["discover_by_terms".to_owned()]);
}

#[tokio::test(flavor = "current_thread")]
async fn bitcoin_refund_owner_sends_once_and_accepted_restart_never_rearms() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Maker);
    activate_and_project_both_locks(&fixture).await;
    assert_eq!(fixture.agreement.bitcoin_funder(), Participant::Maker);
    assert_eq!(
        fixture
            .agreement
            .coordinator()
            .funded_chain(Participant::Maker),
        Chain::Bitcoin
    );
    let effect = prepare_bitcoin_refund_effect(
        &fixture.config,
        &fixture.agreement,
        RefundTransition::MakerLeg,
        &durable_status(&fixture),
    )
    .expect("prepare Bitcoin refund")
    .expect("maker owns Bitcoin refund");
    let accepted = AuthorizedRefundSubmission::Accepted {
        transaction_id: effect.expected_transaction_id,
        witness_transaction_id: effect.expected_witness_transaction_id,
    };
    let port = FixedBitcoinRefundPort::new(BitcoinRefundScan::Eligible, Ok(accepted.clone()));
    let observer = BitcoinRefundObserver {
        chain: port.clone(),
        effect: Some(effect.clone()),
        state_db: fixture.config.state_db.clone(),
    };
    let awaiting = output_json(ActorCommandOutputV1::Effect(
        drive_refund_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &observer,
        )
        .await
        .expect("eligible Bitcoin owner submits exact refund once"),
    ));
    assert_eq!(awaiting["revision"], 2);
    assert_eq!(port.submit_calls(), 1);
    assert_eq!(
        port.submitted
            .lock()
            .expect("Bitcoin submission log")
            .as_slice(),
        &[FixedBitcoinRefundSubmission {
            transaction_bytes: effect.effect.exact_public_bytes().to_vec(),
            expected_transaction_id: effect.expected_transaction_id,
        }]
    );

    let restarted_port = FixedBitcoinRefundPort::new(BitcoinRefundScan::Eligible, Ok(accepted));
    let restarted_observer = BitcoinRefundObserver {
        chain: restarted_port.clone(),
        effect: Some(effect),
        state_db: fixture.config.state_db.clone(),
    };
    drive_refund_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &restarted_observer,
    )
    .await
    .expect("accepted Bitcoin restart remains observe-only");
    assert_eq!(restarted_port.submit_calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn bitcoin_refund_started_and_unknown_restarts_never_rearm() {
    let unknown_fixture =
        ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Maker);
    activate_and_project_both_locks(&unknown_fixture).await;
    let unknown_effect = prepare_bitcoin_refund_effect(
        &unknown_fixture.config,
        &unknown_fixture.agreement,
        RefundTransition::MakerLeg,
        &durable_status(&unknown_fixture),
    )
    .expect("prepare unknown-path Bitcoin refund")
    .expect("maker owns Bitcoin refund");
    let unknown_port = FixedBitcoinRefundPort::new(
        BitcoinRefundScan::Eligible,
        Ok(AuthorizedRefundSubmission::Unknown),
    );
    let unknown_observer = BitcoinRefundObserver {
        chain: unknown_port.clone(),
        effect: Some(unknown_effect.clone()),
        state_db: unknown_fixture.config.state_db.clone(),
    };
    drive_refund_with_observer(
        &unknown_fixture.config,
        unknown_fixture.agreement.clone(),
        unknown_fixture.agreement_wire.clone(),
        &unknown_observer,
    )
    .await
    .expect("ambiguous Bitcoin send is durably Unknown");
    assert_eq!(unknown_port.submit_calls(), 1);

    let unknown_restart = FixedBitcoinRefundPort::new(
        BitcoinRefundScan::Eligible,
        Ok(AuthorizedRefundSubmission::Accepted {
            transaction_id: unknown_effect.expected_transaction_id,
            witness_transaction_id: unknown_effect.expected_witness_transaction_id,
        }),
    );
    let unknown_restart_observer = BitcoinRefundObserver {
        chain: unknown_restart.clone(),
        effect: Some(unknown_effect),
        state_db: unknown_fixture.config.state_db.clone(),
    };
    drive_refund_with_observer(
        &unknown_fixture.config,
        unknown_fixture.agreement.clone(),
        unknown_fixture.agreement_wire.clone(),
        &unknown_restart_observer,
    )
    .await
    .expect("Unknown Bitcoin restart remains observe-only");
    assert_eq!(unknown_restart.submit_calls(), 0);

    let started_fixture =
        ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Maker);
    activate_and_project_both_locks(&started_fixture).await;
    let started_effect = prepare_bitcoin_refund_effect(
        &started_fixture.config,
        &started_fixture.agreement,
        RefundTransition::MakerLeg,
        &durable_status(&started_fixture),
    )
    .expect("prepare started-path Bitcoin refund")
    .expect("maker owns Bitcoin refund");
    let mut journal = SqlitePublicEffectJournal::open(&started_fixture.config.state_db)
        .expect("open Bitcoin refund effect journal");
    let _ = journal
        .record_prepared(&started_effect.effect)
        .expect("persist exact Bitcoin refund");
    assert!(matches!(
        journal
            .reconcile(
                started_effect.effect.key(),
                PublicEffectObservation::EligibleToAttempt,
            )
            .expect("consume authority before simulated crash"),
        PublicEffectDecision::SubmitOnce(_)
    ));
    drop(journal);

    let started_restart = FixedBitcoinRefundPort::new(
        BitcoinRefundScan::Eligible,
        Ok(AuthorizedRefundSubmission::Accepted {
            transaction_id: started_effect.expected_transaction_id,
            witness_transaction_id: started_effect.expected_witness_transaction_id,
        }),
    );
    let started_restart_observer = BitcoinRefundObserver {
        chain: started_restart.clone(),
        effect: Some(started_effect),
        state_db: started_fixture.config.state_db.clone(),
    };
    drive_refund_with_observer(
        &started_fixture.config,
        started_fixture.agreement.clone(),
        started_fixture.agreement_wire.clone(),
        &started_restart_observer,
    )
    .await
    .expect("Started Bitcoin restart remains observe-only");
    assert_eq!(started_restart.submit_calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn bitcoin_refund_exact_finality_projects_owner_and_nonowner_without_send() {
    let owner = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Maker);
    activate_and_project_both_locks(&owner).await;
    let effect = prepare_bitcoin_refund_effect(
        &owner.config,
        &owner.agreement,
        RefundTransition::MakerLeg,
        &durable_status(&owner),
    )
    .expect("prepare finalized Bitcoin refund")
    .expect("maker owns Bitcoin refund");
    let exact = exact_finalized_bitcoin_refund_scan(&effect);
    let owner_port = FixedBitcoinRefundPort::new(
        exact.clone(),
        Err(ActorCommandError::ObservationUnavailable),
    );
    let owner_observer = BitcoinRefundObserver {
        chain: owner_port.clone(),
        effect: Some(effect),
        state_db: owner.config.state_db.clone(),
    };
    let owner_output = output_json(ActorCommandOutputV1::Effect(
        drive_refund_with_observer(
            &owner.config,
            owner.agreement.clone(),
            owner.agreement_wire.clone(),
            &owner_observer,
        )
        .await
        .expect("owner projects finalized Bitcoin refund"),
    ));
    assert_eq!(owner_output["phase"], "maker_leg_refunded");
    assert_eq!(owner_output["revision"], 3);
    assert_eq!(owner_port.submit_calls(), 0);

    let nonowner = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_both_locks(&nonowner).await;
    assert_ne!(
        nonowner.config.role.sdk(),
        RefundTransition::MakerLeg.funded_participant()
    );
    let nonowner_port =
        FixedBitcoinRefundPort::new(exact, Err(ActorCommandError::ObservationUnavailable));
    let nonowner_observer = BitcoinRefundObserver {
        chain: nonowner_port.clone(),
        effect: None,
        state_db: nonowner.config.state_db.clone(),
    };
    let nonowner_output = output_json(ActorCommandOutputV1::Effect(
        drive_refund_with_observer(
            &nonowner.config,
            nonowner.agreement.clone(),
            nonowner.agreement_wire.clone(),
            &nonowner_observer,
        )
        .await
        .expect("nonowner projects finalized Bitcoin refund"),
    ));
    assert_eq!(nonowner_output["phase"], "maker_leg_refunded");
    assert_eq!(nonowner_output["revision"], 3);
    assert_eq!(nonowner_port.submit_calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn bitcoin_refund_mismatched_exact_evidence_burns_authority_without_projection() {
    for mutation in [
        "transaction_id",
        "witness_transaction_id",
        "noncanonical_bytes",
    ] {
        let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Maker);
        activate_and_project_both_locks(&fixture).await;
        let effect = prepare_bitcoin_refund_effect(
            &fixture.config,
            &fixture.agreement,
            RefundTransition::MakerLeg,
            &durable_status(&fixture),
        )
        .expect("prepare Bitcoin refund")
        .expect("maker owns Bitcoin refund");
        let mut mismatched = exact_finalized_bitcoin_refund_scan(&effect);
        let BitcoinRefundScan::Exact(exact) = &mut mismatched else {
            unreachable!("fixture is exact");
        };
        match mutation {
            "transaction_id" => exact.transaction_id = "00".repeat(32).into_boxed_str(),
            "witness_transaction_id" => {
                exact.witness_transaction_id = "11".repeat(32).into_boxed_str();
            }
            "noncanonical_bytes" => exact.transaction_bytes.push(0),
            _ => unreachable!("fixed mutation"),
        }
        let conflicting_port =
            FixedBitcoinRefundPort::new(mismatched, Err(ActorCommandError::ObservationUnavailable));
        let conflicting_observer = BitcoinRefundObserver {
            chain: conflicting_port.clone(),
            effect: Some(effect.clone()),
            state_db: fixture.config.state_db.clone(),
        };
        let pending = output_json(ActorCommandOutputV1::Effect(
            drive_refund_with_observer(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &conflicting_observer,
            )
            .await
            .expect("mismatched exact observation remains pending"),
        ));
        assert_eq!(pending["phase"], "both_legs_locked", "mutation: {mutation}");
        assert_eq!(pending["revision"], 2, "mutation: {mutation}");
        assert_eq!(conflicting_port.submit_calls(), 0, "mutation: {mutation}");
        assert_eq!(
            durable_status(&fixture).phase(),
            Phase::BothLegsLocked,
            "mutation: {mutation}"
        );

        let eligible_restart = FixedBitcoinRefundPort::new(
            BitcoinRefundScan::Eligible,
            Ok(AuthorizedRefundSubmission::Accepted {
                transaction_id: effect.expected_transaction_id,
                witness_transaction_id: effect.expected_witness_transaction_id,
            }),
        );
        let restarted_observer = BitcoinRefundObserver {
            chain: eligible_restart.clone(),
            effect: Some(effect),
            state_db: fixture.config.state_db.clone(),
        };
        let restarted = output_json(ActorCommandOutputV1::Effect(
            drive_refund_with_observer(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &restarted_observer,
            )
            .await
            .expect("conflicting exact evidence keeps restart observe-only"),
        ));
        assert_eq!(restarted["revision"], 2, "mutation: {mutation}");
        assert_eq!(eligible_restart.submit_calls(), 0, "mutation: {mutation}");
    }
}

async fn project_maker_leg_refund_for_live_test(fixture: &ActorFixture) {
    let observer = FixedRefundObserver::new(ready_refund_observation(
        &fixture.agreement,
        RefundTransition::MakerLeg,
    ));
    let output = drive_refund_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &observer,
    )
    .await
    .expect("seed revision three with canonical maker-leg refund");
    assert_eq!(output.revision, 3);
    assert_eq!(durable_status(fixture).phase(), Phase::MakerLegRefunded);
    let status = output_json(
        execute_actor_command(&fixture.config, ActorCommand::Status)
            .await
            .expect("offline maker-leg-refunded status"),
    );
    assert_eq!(status["phase"], "maker_leg_refunded");
    assert_eq!(status["revision"], 3);
    assert_eq!(status["next_action"], "recover_taker_leg");
}

#[tokio::test(flavor = "current_thread")]
async fn lez_taker_leg_owner_sends_once_then_projects_terminal_refund() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_both_locks(&fixture).await;
    project_maker_leg_refund_for_live_test(&fixture).await;
    assert_eq!(fixture.agreement.lez_depositor(), Participant::Taker);
    assert_eq!(
        fixture
            .agreement
            .coordinator()
            .funded_chain(Participant::Taker),
        Chain::Lez
    );
    let deadline = fixture.agreement.lez_terms().refund_at_ms();
    let eligible_port = FixedLezRefundPort::new(
        &fixture,
        EscrowState::Funded,
        deadline,
        FixedLezRefundLookup::Unknown,
        Ok(SubmissionOutcome::Accepted),
    );
    let eligible_observer = LezRefundObserver {
        config: fixture.config.clone(),
        chain: eligible_port.clone(),
        state_db: fixture.config.state_db.clone(),
    };
    let pending = output_json(ActorCommandOutputV1::Effect(
        drive_refund_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &eligible_observer,
        )
        .await
        .expect("TakerLeg LEZ owner submits exactly once"),
    ));
    assert_eq!(pending["revision"], 3);
    assert_eq!(eligible_port.submit_calls(), 1);

    let finalized_port = FixedLezRefundPort::new(
        &fixture,
        EscrowState::Refunded,
        deadline + 1,
        FixedLezRefundLookup::Found,
        Err(ActorCommandError::ObservationUnavailable),
    );
    let finalized_observer = LezRefundObserver {
        config: fixture.config.clone(),
        chain: finalized_port.clone(),
        state_db: fixture.config.state_db.clone(),
    };
    let finalized = output_json(ActorCommandOutputV1::Effect(
        drive_refund_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &finalized_observer,
        )
        .await
        .expect("finalized TakerLeg LEZ refund projects terminal state"),
    ));
    assert_eq!(finalized["phase"], "refunded");
    assert_eq!(finalized["revision"], 4);
    assert_eq!(finalized_port.submit_calls(), 0);
    assert_eq!(
        durable_status(&fixture).terminal(),
        Some(BtcTerminalOutcome::Refunded)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn bitcoin_taker_leg_owner_sends_once_then_projects_terminal_refund() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_both_locks(&fixture).await;
    project_maker_leg_refund_for_live_test(&fixture).await;
    assert_eq!(fixture.agreement.bitcoin_funder(), Participant::Taker);
    assert_eq!(
        fixture
            .agreement
            .coordinator()
            .funded_chain(Participant::Taker),
        Chain::Bitcoin
    );
    let effect = prepare_bitcoin_refund_effect(
        &fixture.config,
        &fixture.agreement,
        RefundTransition::TakerLeg,
        &durable_status(&fixture),
    )
    .expect("prepare TakerLeg Bitcoin refund")
    .expect("taker owns TakerLeg Bitcoin refund");
    let eligible_port = FixedBitcoinRefundPort::new(
        BitcoinRefundScan::Eligible,
        Ok(AuthorizedRefundSubmission::Accepted {
            transaction_id: effect.expected_transaction_id,
            witness_transaction_id: effect.expected_witness_transaction_id,
        }),
    );
    let eligible_observer = BitcoinRefundObserver {
        chain: eligible_port.clone(),
        effect: Some(effect.clone()),
        state_db: fixture.config.state_db.clone(),
    };
    let pending = output_json(ActorCommandOutputV1::Effect(
        drive_refund_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &eligible_observer,
        )
        .await
        .expect("TakerLeg Bitcoin owner submits exactly once"),
    ));
    assert_eq!(pending["revision"], 3);
    assert_eq!(eligible_port.submit_calls(), 1);

    let finalized_port = FixedBitcoinRefundPort::new(
        exact_finalized_bitcoin_refund_scan(&effect),
        Err(ActorCommandError::ObservationUnavailable),
    );
    let finalized_observer = BitcoinRefundObserver {
        chain: finalized_port.clone(),
        effect: Some(effect),
        state_db: fixture.config.state_db.clone(),
    };
    let finalized = output_json(ActorCommandOutputV1::Effect(
        drive_refund_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &finalized_observer,
        )
        .await
        .expect("finalized TakerLeg Bitcoin refund projects terminal state"),
    ));
    assert_eq!(finalized["phase"], "refunded");
    assert_eq!(finalized["revision"], 4);
    assert_eq!(finalized_port.submit_calls(), 0);
    assert_eq!(
        durable_status(&fixture).terminal(),
        Some(BtcTerminalOutcome::Refunded)
    );
}
