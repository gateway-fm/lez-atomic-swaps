#![cfg(feature = "sessions")]

use std::{
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::Write as _,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{Arc, Mutex},
};

use command_fds::{CommandFdExt as _, FdMapping};
use jsonrpsee::RpcModule;
use lez_adaptor_role_runner::{Role, ValidatedSession};
use lez_adaptor_signature::{
    AdaptorSessionContext, AdaptorSigner, SigningRole, adapt_presignature,
};
use lez_bridge_adapter::XmrLezBridgeBindingV3;
use lez_bridge_client::{
    METHOD_COMPLETE_NATIVE_XMR_REFUND_V3, METHOD_PREPARE_NATIVE_XMR_PUNISH_V3,
    METHOD_PREPARE_NATIVE_XMR_REFUND_V3, RUN_ID_HEADER, SIDECAR_ROLE_HEADER,
};
use lez_bridge_protocol::{
    CompleteNativeXmrRefundV3Request, CompleteNativeXmrRefundV3Result, ExactMessageBytes,
    ExactTransactionBytes, Hex32, METHOD_SUBMIT_TRANSACTION, Participant,
    PrepareNativeXmrPunishV3Request, PrepareNativeXmrPunishV3Result,
    PrepareNativeXmrRefundV3Request, PrepareNativeXmrRefundV3Result, PreparedTransaction,
    PreparedWitnessedClaim, RuntimeCompatibility, RuntimeDescriptor, SubmissionOutcome,
    SubmitTransactionRequest, SubmitTransactionResult, TransactionId,
};
use lez_swap_store::{
    AdaptorNonceCommitment, AdaptorPartialSignature, AdaptorPresignature, AdaptorPublicNonce,
    AdaptorSessionReservation, SecretNonceBytes, SqliteAdaptorSessionJournal,
};
use lez_xmr_swap_sdk::{
    CrossCurveDleqProofV1, CrossCurveScalar, MoneroAddressNetworkV1, MoneroPrivateViewKey,
    MoneroSharedAddressV1, XMR_ACTIVATION_SCHEMA_V1, XMR_AGREEMENT_SCHEMA_V1,
    XmrActivatedAgreementV1, XmrActivationBodyV1, XmrActivationRecordV1, XmrAgreementBodyV1,
    XmrAgreementRecordV1, XmrAgreementV1, XmrLezTermsV1, XmrMessagesV1, XmrMoneroTermsV1,
    XmrNamedProfileV1, XmrParticipantIdentityV1, XmrParticipantsV1, XmrRoleV1,
    XmrSessionTranscriptV1, XmrSwapDirectionV1, XmrWindowsV1,
};
use monero::util::key::{PrivateKey as MoneroPrivateKey, PublicKey as MoneroPublicKey};
use monero::{Address as MoneroAddress, Network as MoneroNetwork};
use rand_chacha::{ChaCha20Rng, rand_core::SeedableRng as _};
use rustix::fs::{MemfdFlags, Mode, SealFlags, fchmod, fcntl_add_seals, memfd_create};
use secp256k1::{Keypair, Message as SecpMessage, PublicKey, Secp256k1, SecretKey};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;
use tower::ServiceBuilder;
use tower_http::validate_request::ValidateRequestHeaderLayer;
use xmr_reference_actor::{
    ActorRole, XMR_EFFECT_CAPABILITY_FD, XMR_EFFECT_CHILD_PLAN_FD, XMR_EFFECT_DAEMON_PASSWORD_FD,
    XMR_EFFECT_DAEMON_USERNAME_FD, XMR_EFFECT_FINALIZED_REFUND_SIGNATURE_FD,
    XMR_EFFECT_PRIVATE_VIEW_KEY_FD, XMR_EFFECT_PRIVATE_XMR_SHARE_FD, XMR_EFFECT_ROLE_PASSWORD_FD,
    XMR_EFFECT_ROLE_USERNAME_FD, XMR_EFFECT_RUNTIME_FD, XMR_EFFECT_SHARED_PASSWORD_FD,
    XMR_EFFECT_SHARED_USERNAME_FD, XMR_EFFECT_SHARED_WALLET_FILE_PASSWORD_FD,
    XMR_EFFECT_STAGE_A_FD, XMR_EFFECT_STAGE_B_FD, parse_xmr_effect_child_plan_v1,
};

const CAPABILITY: &str = "m5-xmr-tag16-process-capability-00000001";
const RUN: &str = "m5-xmr-tag16-process-run";
const PREPARE: &str = "m5-xmr-tag16-prepare-001";
const COMPLETE: &str = "m5-xmr-tag16-complete-001";
const MAKER_AGREEMENT_SECRET: [u8; 32] = [7; 32];
const TAKER_AGREEMENT_SECRET: [u8; 32] = [8; 32];
const MAKER_CLAIM_SECRET: [u8; 32] = [9; 32];
const TAKER_CLAIM_SECRET: [u8; 32] = [10; 32];
const MAKER_REFUND_SECRET: [u8; 32] = [11; 32];
const TAKER_REFUND_SECRET: [u8; 32] = [12; 32];
const VIEW_KEY_BYTES: [u8; 32] = {
    let mut bytes = [0; 32];
    bytes[0] = 17;
    bytes
};
const RESTORE_HEIGHT_FOR_ASSERTION: u64 = 0;
const SESSION_DOMAIN: &[u8] = b"logos.gateway.lez-xmr.adaptor-session.v1\0";
const REFUND_MESSAGE_BYTES: [u8; 128] = [0xd1; 128];
const TAKER_XMR_SHARE_BYTES: [u8; 32] = {
    let mut bytes = [0; 32];
    bytes[0] = 13;
    bytes
};
const MAKER_XMR_SHARE_BYTES: [u8; 32] = {
    let mut bytes = [0; 32];
    bytes[0] = 11;
    bytes
};

struct StageFixture {
    agreement: XmrAgreementV1,
    activation: XmrActivatedAgreementV1,
    binding: XmrLezBridgeBindingV3,
    runtime: RuntimeDescriptor,
    final_signature: [u8; 64],
}

#[derive(Clone, Copy, Debug)]
enum Behavior {
    Happy,
    RejectPrepare,
    RejectSubmit,
}

#[derive(Clone, Debug, Default)]
struct Calls {
    prepare: Vec<PrepareNativeXmrRefundV3Request>,
    punish: Vec<PrepareNativeXmrPunishV3Request>,
    complete: Vec<CompleteNativeXmrRefundV3Request>,
    submit: Vec<SubmitTransactionRequest>,
}

#[derive(Clone, Debug)]
struct ServerFixture {
    behavior: Behavior,
    calls: Arc<Mutex<Calls>>,
}

struct MockSidecar {
    endpoint: String,
    calls: Arc<Mutex<Calls>>,
    _handle: jsonrpsee::server::ServerHandle,
}

#[derive(Clone)]
struct WalletFixture {
    address: String,
    restored_address: String,
    amount_piconero: u64,
    calls: Arc<Mutex<Vec<String>>>,
}

struct MockWallet {
    endpoint: String,
    calls: Arc<Mutex<Vec<String>>>,
    _handle: jsonrpsee::server::ServerHandle,
}

#[allow(
    clippy::too_many_lines,
    reason = "one exact typed wallet-RPC fixture keeps the recorded call contract together"
)]
async fn spawn_wallet(
    address: String,
    restored_address: String,
    amount_piconero: u64,
) -> MockWallet {
    let fixture = WalletFixture {
        address,
        restored_address,
        amount_piconero,
        calls: Arc::default(),
    };
    let server = jsonrpsee::server::ServerBuilder::default()
        .build("127.0.0.1:0")
        .await
        .expect("mock wallet binds");
    let address = server.local_addr().expect("mock wallet address");
    let mut module = RpcModule::new(fixture.clone());
    module
        .register_method("get_address", |_params, fixture, _| {
            fixture
                .calls
                .lock()
                .expect("wallet calls")
                .push("get_address".to_owned());
            json_value(serde_json::json!({
                "address": fixture.address,
                "addresses": []
            }))
        })
        .expect("register get_address");
    module
        .register_method("close_wallet", |_params, fixture, _| {
            fixture
                .calls
                .lock()
                .expect("wallet calls")
                .push("close_wallet".to_owned());
            json_value(serde_json::json!({}))
        })
        .expect("register close_wallet");
    module
        .register_method("generate_from_keys", |_params, fixture, _| {
            fixture
                .calls
                .lock()
                .expect("wallet calls")
                .push("generate_from_keys".to_owned());
            json_value(serde_json::json!({
                "address": fixture.restored_address,
                "info": "restored"
            }))
        })
        .expect("register generate_from_keys");
    module
        .register_method("refresh", |_params, fixture, _| {
            fixture
                .calls
                .lock()
                .expect("wallet calls")
                .push("refresh".to_owned());
            json_value(serde_json::json!({
                "blocks_fetched": 1,
                "received_money": true
            }))
        })
        .expect("register refresh");
    module
        .register_method("get_balance", |_params, fixture, _| {
            fixture
                .calls
                .lock()
                .expect("wallet calls")
                .push("get_balance".to_owned());
            json_value(serde_json::json!({
                "balance": fixture.amount_piconero,
                "unlocked_balance": fixture.amount_piconero,
                "multisig_import_needed": false,
                "per_subaddress": []
            }))
        })
        .expect("register get_balance");
    module
        .register_method("sweep_all", |_params, fixture, _| {
            fixture
                .calls
                .lock()
                .expect("wallet calls")
                .push("sweep_all".to_owned());
            json_value(serde_json::json!({
                "tx_hash_list": ["77".repeat(32)],
                "tx_key_list": null,
                "amount_list": [fixture.amount_piconero - 2_000_000_000],
                "fee_list": [2_000_000_000_u64],
                "tx_blob_list": null,
                "tx_metadata_list": null,
                "multisig_txset": "",
                "unsigned_txset": ""
            }))
        })
        .expect("register sweep_all");
    let handle = server.start(module);
    MockWallet {
        endpoint: format!("http://{address}"),
        calls: fixture.calls,
        _handle: handle,
    }
}

fn json_value(value: impl Serialize) -> Result<Value, jsonrpsee::types::ErrorObjectOwned> {
    serde_json::to_value(value).map_err(|error| {
        jsonrpsee::types::ErrorObjectOwned::owned(-32_000, error.to_string(), None::<Value>)
    })
}

#[allow(
    clippy::too_many_lines,
    reason = "three exact authenticated RPC registrations form one process-contract fixture"
)]
async fn spawn_sidecar(behavior: Behavior) -> MockSidecar {
    let fixture = ServerFixture {
        behavior,
        calls: Arc::default(),
    };
    let middleware = ServiceBuilder::new()
        .layer(
            ValidateRequestHeaderLayer::has_header_value(
                "authorization",
                &format!("Bearer {CAPABILITY}"),
            )
            .expect("authorization header"),
        )
        .layer(
            ValidateRequestHeaderLayer::has_header_value(RUN_ID_HEADER, RUN).expect("run header"),
        )
        .layer(
            ValidateRequestHeaderLayer::has_header_value(SIDECAR_ROLE_HEADER, "taker")
                .expect("role header"),
        );
    let server = jsonrpsee::server::ServerBuilder::default()
        .set_http_middleware(middleware)
        .build("127.0.0.1:0")
        .await
        .expect("mock sidecar binds");
    let address = server.local_addr().expect("mock sidecar address");
    let mut module = RpcModule::new(fixture.clone());
    module
        .register_async_method(
            METHOD_PREPARE_NATIVE_XMR_REFUND_V3,
            |params, fixture, _| async move {
                let request: PrepareNativeXmrRefundV3Request = params.one()?;
                fixture
                    .calls
                    .lock()
                    .expect("call recorder")
                    .prepare
                    .push(request.clone());
                if matches!(fixture.behavior, Behavior::RejectPrepare) {
                    return Err(jsonrpsee::types::ErrorObjectOwned::owned(
                        -32_002,
                        "injected preparation failure",
                        None::<Value>,
                    ));
                }
                json_value(
                    PrepareNativeXmrRefundV3Result::new(
                        request.context.clone(),
                        request.terms,
                        PreparedWitnessedClaim::new(
                            request.context.request_id.clone(),
                            request.terms.to_input().refund_message_hash,
                            ExactMessageBytes::new(REFUND_MESSAGE_BYTES.to_vec())
                                .expect("refund message bytes"),
                        ),
                    )
                    .expect("exact Stage-A refund hash"),
                )
            },
        )
        .expect("register refund preparation");
    module
        .register_async_method(
            METHOD_COMPLETE_NATIVE_XMR_REFUND_V3,
            |params, fixture, _| async move {
                let request: CompleteNativeXmrRefundV3Request = params.one()?;
                fixture
                    .calls
                    .lock()
                    .expect("call recorder")
                    .complete
                    .push(request.clone());
                json_value(CompleteNativeXmrRefundV3Result::new(
                    request.context,
                    request.terms,
                    completed_transaction(),
                ))
            },
        )
        .expect("register refund completion");
    module
        .register_async_method(METHOD_SUBMIT_TRANSACTION, |params, fixture, _| async move {
            let request: SubmitTransactionRequest = params.one()?;
            fixture
                .calls
                .lock()
                .expect("call recorder")
                .submit
                .push(request.clone());
            if matches!(fixture.behavior, Behavior::RejectSubmit) {
                return Err(jsonrpsee::types::ErrorObjectOwned::owned(
                    -32_001,
                    "injected submission failure",
                    None::<Value>,
                ));
            }
            json_value(SubmitTransactionResult::new(
                request.context,
                request.transaction.transaction_id,
                SubmissionOutcome::Accepted,
            ))
        })
        .expect("register exact submission");
    let handle = server.start(module);
    MockSidecar {
        endpoint: format!("http://{address}"),
        calls: fixture.calls,
        _handle: handle,
    }
}

async fn spawn_tag17_sidecar(behavior: Behavior) -> MockSidecar {
    let fixture = ServerFixture {
        behavior,
        calls: Arc::default(),
    };
    let middleware = ServiceBuilder::new()
        .layer(
            ValidateRequestHeaderLayer::has_header_value(
                "authorization",
                &format!("Bearer {CAPABILITY}"),
            )
            .expect("authorization header"),
        )
        .layer(
            ValidateRequestHeaderLayer::has_header_value(RUN_ID_HEADER, RUN).expect("run header"),
        )
        .layer(
            ValidateRequestHeaderLayer::has_header_value(SIDECAR_ROLE_HEADER, "maker")
                .expect("Maker role header"),
        );
    let server = jsonrpsee::server::ServerBuilder::default()
        .set_http_middleware(middleware)
        .build("127.0.0.1:0")
        .await
        .expect("Tag17 mock sidecar binds");
    let address = server.local_addr().expect("Tag17 sidecar address");
    let mut module = RpcModule::new(fixture.clone());
    module
        .register_async_method(
            METHOD_PREPARE_NATIVE_XMR_PUNISH_V3,
            |params, fixture, _| async move {
                let request: PrepareNativeXmrPunishV3Request = params.one()?;
                fixture
                    .calls
                    .lock()
                    .expect("call recorder")
                    .punish
                    .push(request.clone());
                if matches!(fixture.behavior, Behavior::RejectPrepare) {
                    return Err(jsonrpsee::types::ErrorObjectOwned::owned(
                        -32_002,
                        "injected Tag17 preparation failure",
                        None::<Value>,
                    ));
                }
                json_value(PrepareNativeXmrPunishV3Result::new(
                    request.context,
                    request.terms,
                    completed_transaction(),
                ))
            },
        )
        .expect("register Tag17 preparation");
    module
        .register_async_method(METHOD_SUBMIT_TRANSACTION, |params, fixture, _| async move {
            let request: SubmitTransactionRequest = params.one()?;
            fixture
                .calls
                .lock()
                .expect("call recorder")
                .submit
                .push(request.clone());
            if matches!(fixture.behavior, Behavior::RejectSubmit) {
                return Err(jsonrpsee::types::ErrorObjectOwned::owned(
                    -32_001,
                    "injected Tag17 submission failure",
                    None::<Value>,
                ));
            }
            json_value(SubmitTransactionResult::new(
                request.context,
                request.transaction.transaction_id,
                SubmissionOutcome::Accepted,
            ))
        })
        .expect("register Tag17 submission");
    let handle = server.start(module);
    MockSidecar {
        endpoint: format!("http://{address}"),
        calls: fixture.calls,
        _handle: handle,
    }
}

struct Inputs {
    _directory: TempDir,
    runtime: PathBuf,
    agreement: PathBuf,
    activation: PathBuf,
    view_key: PathBuf,
    capability: PathBuf,
    final_signature: PathBuf,
}

#[derive(Serialize)]
struct EffectChildPlanFixture<'a> {
    schema_version: u16,
    pair: &'static str,
    role: ActorRole,
    mode: &'static str,
    step: &'static str,
    run_id: &'static str,
    swap_id: String,
    agreement_commitment: String,
    activation_commitment: String,
    executable_abi: &'static str,
    sending_tool_plan_sha256: String,
    adaptor_journal: &'a Path,
    evidence_root: &'a Path,
    lez_sidecar_url: &'a str,
    monero_daemon_url: &'a str,
    monero_funding_wallet_url: &'a str,
    monero_shared_wallet_url: &'a str,
    monero_role_wallet_url: &'a str,
}

impl Inputs {
    fn new(stage: &StageFixture) -> Self {
        let directory = TempDir::new().expect("temporary process root");
        let runtime = directory.path().join("runtime.json");
        let agreement = directory.path().join("agreement.bin");
        let activation = directory.path().join("activation.bin");
        let view_key = directory.path().join("view.key");
        let capability = directory.path().join("capability");
        let final_signature = directory.path().join("refund-final.json");
        fs::write(
            &runtime,
            serde_json::to_vec(&stage.runtime).expect("runtime JSON"),
        )
        .expect("write runtime");
        fs::write(
            &agreement,
            stage.agreement.encode_wire().expect("Stage-A wire"),
        )
        .expect("write Stage A");
        fs::write(
            &activation,
            stage.activation.encode_wire().expect("Stage-B wire"),
        )
        .expect("write Stage B");
        write_private(&view_key, hex::encode(VIEW_KEY_BYTES).as_bytes());
        write_private(&capability, CAPABILITY.as_bytes());
        write_final_signature_packet(&final_signature, &stage.agreement, stage.final_signature);
        Self {
            _directory: directory,
            runtime,
            agreement,
            activation,
            view_key,
            capability,
            final_signature,
        }
    }

    fn output(&self, name: &str) -> PathBuf {
        self.runtime.parent().expect("fixture parent").join(name)
    }
}

fn command(inputs: &Inputs, endpoint: &str, output: &Path) -> Command {
    command_with_request_ids(inputs, endpoint, output, PREPARE, COMPLETE)
}

fn command_with_request_ids(
    inputs: &Inputs,
    endpoint: &str,
    output: &Path,
    prepare_request_id: &str,
    complete_request_id: &str,
) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xmr-reference-tag16"));
    command
        .arg("--sidecar-endpoint")
        .arg(endpoint)
        .arg("--capability-file")
        .arg(&inputs.capability)
        .arg("--runtime-file")
        .arg(&inputs.runtime)
        .arg("--agreement-wire-file")
        .arg(&inputs.agreement)
        .arg("--activation-wire-file")
        .arg(&inputs.activation)
        .arg("--monero-view-key-file")
        .arg(&inputs.view_key)
        .arg("--final-signature-file")
        .arg(&inputs.final_signature)
        .arg("--run-id")
        .arg(RUN)
        .arg("--prepare-request-id")
        .arg(prepare_request_id)
        .arg("--complete-request-id")
        .arg(complete_request_id)
        .arg("--output-evidence")
        .arg(output);
    command
}

fn sealed_memfd(label: &str, bytes: &[u8]) -> File {
    let name = CString::new(label).expect("memfd label");
    let descriptor = memfd_create(
        name.as_c_str(),
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
    )
    .expect("create sealed descriptor");
    let mut file = File::from(descriptor);
    fchmod(&file, Mode::RUSR | Mode::WUSR).expect("make descriptor writable");
    file.write_all(bytes).expect("write sealed descriptor");
    fchmod(&file, Mode::RUSR).expect("make descriptor read-only");
    fcntl_add_seals(
        &file,
        SealFlags::SEAL | SealFlags::SHRINK | SealFlags::GROW | SealFlags::WRITE,
    )
    .expect("seal descriptor");
    file
}

fn taker_refund_journal(stage: &StageFixture, path: &Path, presignature: [u8; 65]) {
    let session = ValidatedSession::from_untweaked_context(
        stage
            .agreement
            .refund_session_descriptor()
            .context()
            .expect("refund context"),
    )
    .expect("validated refund session");
    let identity = session.identity(Role::Taker);
    let transcript = stage.activation.body().refund_transcript();
    let mut journal = SqliteAdaptorSessionJournal::open(path).expect("create Taker journal");
    let _ = journal
        .reserve(AdaptorSessionReservation::new(
            identity.clone(),
            SecretNonceBytes::new([0x91; 97]),
            AdaptorPublicNonce::new(transcript.taker_public_nonce()),
            AdaptorNonceCommitment::new(transcript.taker_nonce_commitment()),
        ))
        .expect("reserve exact Taker refund transcript");
    let _ = journal
        .record_peer_commitment(
            &identity,
            AdaptorNonceCommitment::new(transcript.maker_nonce_commitment()),
        )
        .expect("record Maker commitment");
    let _ = journal
        .record_verified_peer_public_nonce(
            &identity,
            AdaptorPublicNonce::new(transcript.maker_public_nonce()),
        )
        .expect("record Maker nonce");
    let _ = journal
        .sign_and_persist_partial(&identity, |_| {
            Ok(AdaptorPartialSignature::new(
                stage.activation.body().taker_refund_partial(),
            ))
        })
        .expect("persist exact Taker partial");
    let _ = journal
        .record_verified_peer_partial(
            &identity,
            AdaptorPartialSignature::new(stage.activation.body().maker_refund_partial()),
        )
        .expect("record exact Maker partial");
    let _ = journal
        .record_verified_presignature(&identity, AdaptorPresignature::new(presignature))
        .expect("record exact Stage-B presignature");
}

fn maker_refund_journal(stage: &StageFixture, path: &Path) {
    let session = ValidatedSession::from_untweaked_context(
        stage
            .agreement
            .refund_session_descriptor()
            .context()
            .expect("refund context"),
    )
    .expect("validated refund session");
    let identity = session.identity(Role::Maker);
    let transcript = stage.activation.body().refund_transcript();
    let mut journal = SqliteAdaptorSessionJournal::open(path).expect("create Maker journal");
    let _ = journal
        .reserve(AdaptorSessionReservation::new(
            identity.clone(),
            SecretNonceBytes::new([0x92; 97]),
            AdaptorPublicNonce::new(transcript.maker_public_nonce()),
            AdaptorNonceCommitment::new(transcript.maker_nonce_commitment()),
        ))
        .expect("reserve exact Maker refund transcript");
    let _ = journal
        .record_peer_commitment(
            &identity,
            AdaptorNonceCommitment::new(transcript.taker_nonce_commitment()),
        )
        .expect("record Taker commitment");
    let _ = journal
        .record_verified_peer_public_nonce(
            &identity,
            AdaptorPublicNonce::new(transcript.taker_public_nonce()),
        )
        .expect("record Taker nonce");
    let _ = journal
        .sign_and_persist_partial(&identity, |_| {
            Ok(AdaptorPartialSignature::new(
                stage.activation.body().maker_refund_partial(),
            ))
        })
        .expect("persist exact Maker partial");
    let _ = journal
        .record_verified_peer_partial(
            &identity,
            AdaptorPartialSignature::new(stage.activation.body().taker_refund_partial()),
        )
        .expect("record exact Taker partial");
    let _ = journal
        .record_verified_presignature(
            &identity,
            AdaptorPresignature::new(stage.activation.body().refund_presignature()),
        )
        .expect("record exact Stage-B presignature");
}

fn effect_child_command(
    stage: &StageFixture,
    inputs: &Inputs,
    endpoint: &str,
) -> (Command, PathBuf) {
    effect_child_command_with_mode_and_presignature(
        stage,
        inputs,
        endpoint,
        "invoke",
        stage.activation.body().refund_presignature(),
    )
}

fn effect_child_command_with_presignature(
    stage: &StageFixture,
    inputs: &Inputs,
    endpoint: &str,
    presignature: [u8; 65],
) -> (Command, PathBuf) {
    effect_child_command_with_mode_and_presignature(stage, inputs, endpoint, "invoke", presignature)
}

fn effect_child_command_with_mode(
    stage: &StageFixture,
    inputs: &Inputs,
    endpoint: &str,
    mode: &'static str,
) -> (Command, PathBuf) {
    effect_child_command_with_mode_and_presignature(
        stage,
        inputs,
        endpoint,
        mode,
        stage.activation.body().refund_presignature(),
    )
}

fn effect_child_command_with_mode_and_presignature(
    stage: &StageFixture,
    inputs: &Inputs,
    endpoint: &str,
    mode: &'static str,
    presignature: [u8; 65],
) -> (Command, PathBuf) {
    let root = inputs.runtime.parent().expect("fixture root");
    let journal = root.join("taker-adaptor.sqlite");
    taker_refund_journal(stage, &journal, presignature);
    let evidence_root = root.join("evidence");
    fs::create_dir(&evidence_root).expect("create evidence root");
    fs::set_permissions(&evidence_root, fs::Permissions::from_mode(0o700))
        .expect("protect evidence root");
    let plan = EffectChildPlanFixture {
        schema_version: 1,
        pair: "monero",
        role: ActorRole::Taker,
        mode,
        step: "refund_lez_tag16",
        run_id: RUN,
        swap_id: hex::encode(stage.agreement.body().swap_id()),
        agreement_commitment: hex::encode(stage.agreement.agreement_commitment()),
        activation_commitment: hex::encode(stage.activation.activation_commitment()),
        executable_abi: "lez_xmr_tag16_refund_v1",
        sending_tool_plan_sha256: hex::encode([0xa7; 32]),
        adaptor_journal: &journal,
        evidence_root: &evidence_root,
        lez_sidecar_url: endpoint,
        monero_daemon_url: "http://127.0.0.1:32974/",
        monero_funding_wallet_url: "http://127.0.0.1:32975/",
        monero_shared_wallet_url: "http://127.0.0.1:32976/",
        monero_role_wallet_url: "http://127.0.0.1:32977/",
    };
    let mut plan_bytes = serde_json::to_vec(&plan).expect("effect-child plan JSON");
    plan_bytes.push(b'\n');
    let _ = parse_xmr_effect_child_plan_v1(&plan_bytes).expect("canonical effect-child plan");
    let descriptors = vec![
        (
            sealed_memfd(
                "tag16-runtime",
                &serde_json::to_vec(&stage.runtime).expect("runtime JSON"),
            ),
            XMR_EFFECT_RUNTIME_FD,
        ),
        (
            sealed_memfd("tag16-capability", CAPABILITY.as_bytes()),
            XMR_EFFECT_CAPABILITY_FD,
        ),
        (
            sealed_memfd(
                "tag16-stage-a",
                &stage.agreement.encode_wire().expect("Stage-A wire"),
            ),
            XMR_EFFECT_STAGE_A_FD,
        ),
        (
            sealed_memfd(
                "tag16-stage-b",
                &stage.activation.encode_wire().expect("Stage-B wire"),
            ),
            XMR_EFFECT_STAGE_B_FD,
        ),
        (
            sealed_memfd("tag16-view-key", hex::encode(VIEW_KEY_BYTES).as_bytes()),
            XMR_EFFECT_PRIVATE_VIEW_KEY_FD,
        ),
        (
            sealed_memfd("tag16-child-plan", &plan_bytes),
            XMR_EFFECT_CHILD_PLAN_FD,
        ),
        (
            sealed_memfd("tag16-xmr-share", &TAKER_XMR_SHARE_BYTES),
            XMR_EFFECT_PRIVATE_XMR_SHARE_FD,
        ),
    ];
    let mut command = Command::new(env!("CARGO_BIN_EXE_xmr-reference-tag16"));
    command
        .fd_mappings(
            descriptors
                .into_iter()
                .map(|(file, child_fd)| FdMapping {
                    parent_fd: file.into(),
                    child_fd,
                })
                .collect(),
        )
        .expect("map sealed effect-child descriptors");
    (command, evidence_root.join("tag16-refund-submission.json"))
}

fn tag17_effect_child_command(
    stage: &StageFixture,
    inputs: &Inputs,
    endpoint: &str,
    mode: &'static str,
    include_private_share: bool,
) -> (Command, PathBuf) {
    let root = inputs.runtime.parent().expect("fixture root");
    let evidence_root = root.join("tag17-evidence");
    fs::create_dir_all(&evidence_root).expect("create Tag17 evidence root");
    fs::set_permissions(&evidence_root, fs::Permissions::from_mode(0o700))
        .expect("protect Tag17 evidence root");
    let mut runtime = stage.runtime.clone();
    runtime.sidecar_role = Participant::Maker;
    runtime.signer_account_id = Hex32::from_bytes([21; 32]);
    let adaptor_journal = root.join("maker-adaptor.sqlite");
    let plan = EffectChildPlanFixture {
        schema_version: 1,
        pair: "monero",
        role: ActorRole::Maker,
        mode,
        step: "punish_lez_tag17",
        run_id: RUN,
        swap_id: hex::encode(stage.agreement.body().swap_id()),
        agreement_commitment: hex::encode(stage.agreement.agreement_commitment()),
        activation_commitment: hex::encode(stage.activation.activation_commitment()),
        executable_abi: "lez_xmr_tag17_punish_v1",
        sending_tool_plan_sha256: hex::encode([0xb7; 32]),
        adaptor_journal: &adaptor_journal,
        evidence_root: &evidence_root,
        lez_sidecar_url: endpoint,
        monero_daemon_url: "http://127.0.0.1:32974/",
        monero_funding_wallet_url: "http://127.0.0.1:32975/",
        monero_shared_wallet_url: "http://127.0.0.1:32976/",
        monero_role_wallet_url: "http://127.0.0.1:32977/",
    };
    let mut plan_bytes = serde_json::to_vec(&plan).expect("Tag17 child plan JSON");
    plan_bytes.push(b'\n');
    let _ = parse_xmr_effect_child_plan_v1(&plan_bytes).expect("canonical Tag17 child plan");
    let mut descriptors = vec![
        (
            sealed_memfd(
                "tag17-runtime",
                &serde_json::to_vec(&runtime).expect("Maker runtime JSON"),
            ),
            XMR_EFFECT_RUNTIME_FD,
        ),
        (
            sealed_memfd("tag17-capability", CAPABILITY.as_bytes()),
            XMR_EFFECT_CAPABILITY_FD,
        ),
        (
            sealed_memfd(
                "tag17-stage-a",
                &stage.agreement.encode_wire().expect("Stage-A wire"),
            ),
            XMR_EFFECT_STAGE_A_FD,
        ),
        (
            sealed_memfd(
                "tag17-stage-b",
                &stage.activation.encode_wire().expect("Stage-B wire"),
            ),
            XMR_EFFECT_STAGE_B_FD,
        ),
        (
            sealed_memfd("tag17-view-key", hex::encode(VIEW_KEY_BYTES).as_bytes()),
            XMR_EFFECT_PRIVATE_VIEW_KEY_FD,
        ),
        (
            sealed_memfd("tag17-child-plan", &plan_bytes),
            XMR_EFFECT_CHILD_PLAN_FD,
        ),
    ];
    if include_private_share {
        descriptors.push((
            sealed_memfd("forbidden-tag17-xmr-share", &TAKER_XMR_SHARE_BYTES),
            XMR_EFFECT_PRIVATE_XMR_SHARE_FD,
        ));
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_xmr-reference-tag17"));
    command
        .fd_mappings(
            descriptors
                .into_iter()
                .map(|(file, child_fd)| FdMapping {
                    parent_fd: file.into(),
                    child_fd,
                })
                .collect(),
        )
        .expect("map sealed Tag17 descriptors");
    (
        command,
        evidence_root.join("tag17-punishment-submission.json"),
    )
}

#[allow(
    clippy::too_many_lines,
    reason = "the fixed-descriptor process contract is clearer as one complete mapping"
)]
fn maker_refund_effect_child_command(
    stage: &StageFixture,
    inputs: &Inputs,
    daemon_endpoint: &str,
    shared_endpoint: &str,
    role_endpoint: &str,
) -> (Command, PathBuf) {
    let root = inputs.runtime.parent().expect("fixture root");
    let evidence_root = root.join("maker-refund-evidence");
    fs::create_dir(&evidence_root).expect("create Maker refund evidence root");
    fs::set_permissions(&evidence_root, fs::Permissions::from_mode(0o700))
        .expect("protect Maker refund evidence root");
    let journal = root.join("maker-refund-adaptor.sqlite");
    maker_refund_journal(stage, &journal);
    let mut runtime = stage.runtime.clone();
    runtime.sidecar_role = Participant::Maker;
    runtime.signer_account_id = Hex32::from_bytes([21; 32]);
    let plan = EffectChildPlanFixture {
        schema_version: 1,
        pair: "monero",
        role: ActorRole::Maker,
        mode: "invoke",
        step: "sweep_monero_refund",
        run_id: RUN,
        swap_id: hex::encode(stage.agreement.body().swap_id()),
        agreement_commitment: hex::encode(stage.agreement.agreement_commitment()),
        activation_commitment: hex::encode(stage.activation.activation_commitment()),
        executable_abi: "lez_xmr_monero_refund_sweep_v3",
        sending_tool_plan_sha256: hex::encode([0xc7; 32]),
        adaptor_journal: &journal,
        evidence_root: &evidence_root,
        lez_sidecar_url: "http://127.0.0.1:32973/",
        monero_daemon_url: daemon_endpoint,
        monero_funding_wallet_url: "http://127.0.0.1:32975/",
        monero_shared_wallet_url: shared_endpoint,
        monero_role_wallet_url: role_endpoint,
    };
    let mut plan_bytes = serde_json::to_vec(&plan).expect("refund child plan JSON");
    plan_bytes.push(b'\n');
    let _ = parse_xmr_effect_child_plan_v1(&plan_bytes).expect("canonical refund child plan");
    let descriptors = vec![
        (
            sealed_memfd(
                "refund-runtime",
                &serde_json::to_vec(&runtime).expect("Maker runtime JSON"),
            ),
            XMR_EFFECT_RUNTIME_FD,
        ),
        (
            sealed_memfd("refund-daemon-user", b"daemon-user"),
            XMR_EFFECT_DAEMON_USERNAME_FD,
        ),
        (
            sealed_memfd("refund-daemon-password", b"daemon-password"),
            XMR_EFFECT_DAEMON_PASSWORD_FD,
        ),
        (
            sealed_memfd("refund-shared-user", b"shared-user"),
            XMR_EFFECT_SHARED_USERNAME_FD,
        ),
        (
            sealed_memfd("refund-shared-password", b"shared-password"),
            XMR_EFFECT_SHARED_PASSWORD_FD,
        ),
        (
            sealed_memfd("refund-role-user", b"role-user"),
            XMR_EFFECT_ROLE_USERNAME_FD,
        ),
        (
            sealed_memfd("refund-role-password", b"role-password"),
            XMR_EFFECT_ROLE_PASSWORD_FD,
        ),
        (
            sealed_memfd("refund-wallet-file-password", b"wallet-file-password"),
            XMR_EFFECT_SHARED_WALLET_FILE_PASSWORD_FD,
        ),
        (
            sealed_memfd(
                "refund-stage-a",
                &stage.agreement.encode_wire().expect("Stage-A wire"),
            ),
            XMR_EFFECT_STAGE_A_FD,
        ),
        (
            sealed_memfd(
                "refund-stage-b",
                &stage.activation.encode_wire().expect("Stage-B wire"),
            ),
            XMR_EFFECT_STAGE_B_FD,
        ),
        (
            sealed_memfd("refund-view-key", hex::encode(VIEW_KEY_BYTES).as_bytes()),
            XMR_EFFECT_PRIVATE_VIEW_KEY_FD,
        ),
        (
            sealed_memfd("refund-child-plan", &plan_bytes),
            XMR_EFFECT_CHILD_PLAN_FD,
        ),
        (
            sealed_memfd("refund-maker-share", &MAKER_XMR_SHARE_BYTES),
            XMR_EFFECT_PRIVATE_XMR_SHARE_FD,
        ),
        (
            sealed_memfd(
                "refund-final-signature",
                &fs::read(&inputs.final_signature).expect("canonical final-signature packet"),
            ),
            XMR_EFFECT_FINALIZED_REFUND_SIGNATURE_FD,
        ),
    ];
    let mut command = Command::new(env!("CARGO_BIN_EXE_xmr-reference-monero-refund"));
    command
        .fd_mappings(
            descriptors
                .into_iter()
                .map(|(file, child_fd)| FdMapping {
                    parent_fd: file.into(),
                    child_fd,
                })
                .collect(),
        )
        .expect("map sealed refund descriptors");
    (command, evidence_root.join("monero-refund-submission.json"))
}

#[allow(
    clippy::too_many_arguments,
    reason = "the process fixture makes every sealed authority input explicit"
)]
fn maker_refund_observer_command(
    stage: &StageFixture,
    evidence_root: &Path,
    daemon_endpoint: &str,
    role_endpoint: &str,
    include_private_share: bool,
) -> Command {
    let root = evidence_root.parent().expect("fixture root");
    let journal = root.join("maker-refund-adaptor.sqlite");
    let plan = EffectChildPlanFixture {
        schema_version: 1,
        pair: "monero",
        role: ActorRole::Maker,
        mode: "observe",
        step: "sweep_monero_refund",
        run_id: RUN,
        swap_id: hex::encode(stage.agreement.body().swap_id()),
        agreement_commitment: hex::encode(stage.agreement.agreement_commitment()),
        activation_commitment: hex::encode(stage.activation.activation_commitment()),
        executable_abi: "lez_xmr_monero_verify_v2",
        sending_tool_plan_sha256: hex::encode([0xc7; 32]),
        adaptor_journal: &journal,
        evidence_root,
        lez_sidecar_url: "http://127.0.0.1:32973/",
        monero_daemon_url: daemon_endpoint,
        monero_funding_wallet_url: "http://127.0.0.1:32975/",
        monero_shared_wallet_url: "http://127.0.0.1:32976/",
        monero_role_wallet_url: role_endpoint,
    };
    let mut plan_bytes = serde_json::to_vec(&plan).expect("observer child plan JSON");
    plan_bytes.push(b'\n');
    let _ = parse_xmr_effect_child_plan_v1(&plan_bytes).expect("canonical observer child plan");
    let mut descriptors = vec![
        (
            sealed_memfd("observer-daemon-user", b"daemon-user"),
            XMR_EFFECT_DAEMON_USERNAME_FD,
        ),
        (
            sealed_memfd("observer-daemon-password", b"daemon-password"),
            XMR_EFFECT_DAEMON_PASSWORD_FD,
        ),
        (
            sealed_memfd("observer-role-user", b"role-user"),
            XMR_EFFECT_ROLE_USERNAME_FD,
        ),
        (
            sealed_memfd("observer-role-password", b"role-password"),
            XMR_EFFECT_ROLE_PASSWORD_FD,
        ),
        (
            sealed_memfd(
                "observer-stage-a",
                &stage.agreement.encode_wire().expect("Stage-A wire"),
            ),
            XMR_EFFECT_STAGE_A_FD,
        ),
        (
            sealed_memfd(
                "observer-stage-b",
                &stage.activation.encode_wire().expect("Stage-B wire"),
            ),
            XMR_EFFECT_STAGE_B_FD,
        ),
        (
            sealed_memfd("observer-view-key", hex::encode(VIEW_KEY_BYTES).as_bytes()),
            XMR_EFFECT_PRIVATE_VIEW_KEY_FD,
        ),
        (
            sealed_memfd("observer-child-plan", &plan_bytes),
            XMR_EFFECT_CHILD_PLAN_FD,
        ),
    ];
    if include_private_share {
        descriptors.push((
            sealed_memfd("forbidden-observer-xmr-share", &MAKER_XMR_SHARE_BYTES),
            XMR_EFFECT_PRIVATE_XMR_SHARE_FD,
        ));
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_xmr-reference-monero-verify"));
    command
        .arg("--xmr-workflow-step")
        .arg("sweep_monero_refund")
        .fd_mappings(
            descriptors
                .into_iter()
                .map(|(file, child_fd)| FdMapping {
                    parent_fd: file.into(),
                    child_fd,
                })
                .collect(),
        )
        .expect("map sealed observer descriptors");
    command
}

fn standard_monero_address(seed: u8) -> String {
    let mut spend = [0_u8; 32];
    spend[0] = seed;
    let mut view = [0_u8; 32];
    view[0] = seed.saturating_add(1);
    let spend = MoneroPrivateKey::from_slice(&spend).expect("destination spend scalar");
    let view = MoneroPrivateKey::from_slice(&view).expect("destination view scalar");
    MoneroAddress::standard(
        MoneroNetwork::Mainnet,
        MoneroPublicKey::from_private_key(&spend),
        MoneroPublicKey::from_private_key(&view),
    )
    .to_string()
}

fn assert_failure(output: &Output, label: &str) {
    assert!(!output.status.success(), "{label} unexpectedly succeeded");
    assert!(output.stdout.is_empty(), "{label} leaked stdout");
}

#[tokio::test(flavor = "multi_thread")]
async fn sealed_tag17_preflight_prepares_only_and_rejected_preflight_has_no_effect() {
    let stage = build_stage_b();
    let inputs = Inputs::new(&stage);
    let sidecar = spawn_tag17_sidecar(Behavior::Happy).await;
    let (mut command, evidence) =
        tag17_effect_child_command(&stage, &inputs, &sidecar.endpoint, "preflight", false);
    let output = command.output().expect("spawn sealed Tag17 preflight");
    assert!(
        output.status.success(),
        "Tag17 preflight failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert!(!evidence.exists());
    {
        let calls = sidecar.calls.lock().expect("Tag17 calls");
        assert_eq!(calls.punish.len(), 1);
        assert!(calls.submit.is_empty());
        assert_eq!(calls.punish[0].context.sidecar_role, Participant::Maker);
        assert_eq!(calls.punish[0].terms, stage.binding.terms());
    }

    let rejected = spawn_tag17_sidecar(Behavior::RejectPrepare).await;
    let (mut command, rejected_evidence) =
        tag17_effect_child_command(&stage, &inputs, &rejected.endpoint, "preflight", false);
    let output = command.output().expect("spawn rejected Tag17 preflight");
    assert_failure(&output, "rejected Tag17 preflight");
    assert!(!rejected_evidence.exists());
    let calls = rejected.calls.lock().expect("rejected Tag17 calls");
    assert_eq!(calls.punish.len(), 1);
    assert!(calls.submit.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn sealed_tag17_rejects_forbidden_private_share_before_sidecar_use() {
    let stage = build_stage_b();
    let inputs = Inputs::new(&stage);
    let sidecar = spawn_tag17_sidecar(Behavior::Happy).await;
    let (mut command, evidence) =
        tag17_effect_child_command(&stage, &inputs, &sidecar.endpoint, "invoke", true);
    let output = command
        .output()
        .expect("spawn Tag17 with forbidden private share");
    assert_failure(&output, "Tag17 forbidden private share");
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("received forbidden Monero private-share FD")
    );
    assert!(!evidence.exists());
    let calls = sidecar.calls.lock().expect("Tag17 calls");
    assert!(calls.punish.is_empty());
    assert!(calls.submit.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn sealed_tag17_invocation_submits_exactly_once_and_writes_bounded_evidence() {
    let stage = build_stage_b();
    let inputs = Inputs::new(&stage);
    let sidecar = spawn_tag17_sidecar(Behavior::Happy).await;
    let (mut command, evidence) =
        tag17_effect_child_command(&stage, &inputs, &sidecar.endpoint, "invoke", false);
    let output = command.output().expect("spawn sealed Tag17 invocation");
    assert!(
        output.status.success(),
        "Tag17 invocation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    let calls = sidecar.calls.lock().expect("Tag17 calls");
    assert_eq!(calls.punish.len(), 1);
    assert_eq!(calls.submit.len(), 1);
    assert_eq!(calls.submit[0].transaction, completed_transaction());
    assert_eq!(
        calls.submit[0].context.request_id,
        calls.submit[0]
            .transaction
            .transaction_id
            .submission_request_id()
    );
    drop(calls);

    let report: Value = serde_json::from_slice(&fs::read(evidence).expect("Tag17 evidence"))
        .expect("Tag17 evidence JSON");
    assert_eq!(report["schema"], "lez_v02_m7_actual_local_tag17_worker_v1");
    assert_eq!(report["role"], "maker");
    assert_eq!(report["submission_outcome"], "accepted");
    assert_eq!(
        report["prepared_message_hash"],
        hex::encode(stage.agreement.body().messages().punish())
    );
    assert_eq!(report["automatic_submission_retry"], false);
    assert_eq!(report["public_rpc_used"], false);
}

#[tokio::test(flavor = "multi_thread")]
async fn sealed_maker_refund_reconstructs_and_submits_once_without_mining_or_finality_wait() {
    let stage = build_stage_b();
    let inputs = Inputs::new(&stage);
    let shared_address = stage.agreement.shared_address().address_string();
    let amount = stage.agreement.body().monero().amount_piconero();
    let daemon = spawn_wallet(standard_monero_address(3), shared_address.clone(), amount).await;
    let shared = spawn_wallet(standard_monero_address(5), shared_address, amount).await;
    let maker_destination = standard_monero_address(7);
    let role = spawn_wallet(maker_destination.clone(), maker_destination.clone(), amount).await;
    let (mut command, evidence) = maker_refund_effect_child_command(
        &stage,
        &inputs,
        &daemon.endpoint,
        &shared.endpoint,
        &role.endpoint,
    );
    let output = command.output().expect("spawn sealed Maker refund worker");
    assert!(
        output.status.success(),
        "Maker refund worker failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert!(daemon.calls.lock().expect("daemon calls").is_empty());
    assert_eq!(
        *role.calls.lock().expect("role wallet calls"),
        ["get_address"]
    );
    assert_eq!(
        *shared.calls.lock().expect("shared wallet calls"),
        [
            "close_wallet",
            "generate_from_keys",
            "refresh",
            "get_balance",
            "sweep_all"
        ]
    );
    let report: Value = serde_json::from_slice(&fs::read(evidence).expect("refund evidence"))
        .expect("refund evidence JSON");
    assert_eq!(report["schema"], "lez_v02_m7_monero_refund_submission_v1");
    assert_eq!(report["role"], "maker");
    assert_eq!(report["destination_address"], maker_destination);
    assert_eq!(report["funded_amount_piconero"], amount);
    assert_eq!(report["received_amount_piconero"], amount - 2_000_000_000);
    assert_eq!(report["fee_piconero"], 2_000_000_000_u64);
    assert_eq!(report["transaction_id"], "77".repeat(32));
    assert_eq!(report["restore_height"], RESTORE_HEIGHT_FOR_ASSERTION);
    assert_eq!(report["finality_observer_required"], true);
    assert_eq!(report["automatic_submission_retry"], false);
}

#[tokio::test(flavor = "multi_thread")]
async fn sealed_maker_refund_rejects_invalid_final_signature_before_any_rpc() {
    let stage = build_stage_b();
    let inputs = Inputs::new(&stage);
    fs::remove_file(&inputs.final_signature).expect("remove valid signature fixture");
    let mut invalid_signature = stage.final_signature;
    invalid_signature[63] ^= 1;
    write_final_signature_packet(&inputs.final_signature, &stage.agreement, invalid_signature);
    let shared_address = stage.agreement.shared_address().address_string();
    let amount = stage.agreement.body().monero().amount_piconero();
    let daemon = spawn_wallet(standard_monero_address(3), shared_address.clone(), amount).await;
    let shared = spawn_wallet(standard_monero_address(5), shared_address, amount).await;
    let destination = standard_monero_address(7);
    let role = spawn_wallet(destination.clone(), destination, amount).await;
    let (mut command, evidence) = maker_refund_effect_child_command(
        &stage,
        &inputs,
        &daemon.endpoint,
        &shared.endpoint,
        &role.endpoint,
    );
    let output = command
        .output()
        .expect("spawn invalid-signature Maker refund worker");
    assert_failure(&output, "invalid-signature Maker refund worker");
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("extract refund adaptor scalar from durable Maker transcript")
    );
    assert!(!evidence.exists());
    assert!(daemon.calls.lock().expect("daemon calls").is_empty());
    assert!(shared.calls.lock().expect("shared calls").is_empty());
    assert!(role.calls.lock().expect("role calls").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn sealed_maker_refund_observer_rejects_invocation_secret_before_all_rpc() {
    let stage = build_stage_b();
    let root = tempfile::tempdir().expect("observer fixture root");
    let evidence_root = root.path().join("observer-evidence");
    fs::create_dir(&evidence_root).expect("create observer evidence root");
    fs::set_permissions(&evidence_root, fs::Permissions::from_mode(0o700))
        .expect("protect observer evidence root");
    let daemon = spawn_wallet(
        standard_monero_address(3),
        stage.agreement.shared_address().address_string(),
        stage.agreement.body().monero().amount_piconero(),
    )
    .await;
    let role_address = standard_monero_address(7);
    let role = spawn_wallet(
        role_address.clone(),
        role_address,
        stage.agreement.body().monero().amount_piconero(),
    )
    .await;
    let mut command = maker_refund_observer_command(
        &stage,
        &evidence_root,
        &daemon.endpoint,
        &role.endpoint,
        true,
    );
    let output = command.output().expect("spawn forbidden-input observer");
    assert_failure(&output, "forbidden-input observer");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("received forbidden private XMR share FD")
    );
    assert!(output.stdout.is_empty());
    assert!(daemon.calls.lock().expect("daemon calls").is_empty());
    assert!(role.calls.lock().expect("role calls").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn sealed_maker_refund_observer_rejects_changed_submission_before_all_rpc() {
    let stage = build_stage_b();
    let inputs = Inputs::new(&stage);
    let shared_address = stage.agreement.shared_address().address_string();
    let amount = stage.agreement.body().monero().amount_piconero();
    let daemon = spawn_wallet(standard_monero_address(3), shared_address.clone(), amount).await;
    let shared = spawn_wallet(standard_monero_address(5), shared_address, amount).await;
    let role_address = standard_monero_address(7);
    let role = spawn_wallet(role_address.clone(), role_address, amount).await;
    let (mut sender, submission) = maker_refund_effect_child_command(
        &stage,
        &inputs,
        &daemon.endpoint,
        &shared.endpoint,
        &role.endpoint,
    );
    let output = sender.output().expect("spawn sealed Maker refund worker");
    assert!(
        output.status.success(),
        "Maker refund worker failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    daemon.calls.lock().expect("daemon calls").clear();
    role.calls.lock().expect("role calls").clear();
    let mut valid_observer = maker_refund_observer_command(
        &stage,
        submission.parent().expect("evidence root"),
        &daemon.endpoint,
        &role.endpoint,
        false,
    );
    let output = valid_observer
        .output()
        .expect("spawn valid-input observer against incomplete daemon fixture");
    assert_failure(
        &output,
        "valid-input observer against incomplete daemon fixture",
    );
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("typed Monero RPC operation `on_get_block_hash(0)` failed"),
        "valid sender evidence did not reach the typed observer RPC boundary: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    let mut changed: Value =
        serde_json::from_slice(&fs::read(&submission).expect("submission evidence"))
            .expect("submission JSON");
    changed["sending_tool_plan_sha256"] = Value::String(hex::encode([0xd8; 32]));
    let mut changed_bytes = serde_json::to_vec(&changed).expect("changed submission JSON");
    changed_bytes.push(b'\n');
    fs::write(&submission, changed_bytes).expect("replace changed submission");
    let mut observer = maker_refund_observer_command(
        &stage,
        submission.parent().expect("evidence root"),
        &daemon.endpoint,
        &role.endpoint,
        false,
    );
    let output = observer.output().expect("spawn changed-input observer");
    assert_failure(&output, "changed-input observer");
    assert!(output.stdout.is_empty());
    assert!(daemon.calls.lock().expect("daemon calls").is_empty());
    assert!(role.calls.lock().expect("role calls").is_empty());
    assert!(
        !submission
            .parent()
            .expect("evidence root")
            .join("monero-refund-finalized.json")
            .exists()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn taker_process_binds_stage_a_refund_and_submits_canonical_transaction_once() {
    let stage = build_stage_b();
    let inputs = Inputs::new(&stage);
    let sidecar = spawn_sidecar(Behavior::Happy).await;
    let evidence = inputs.output("happy.json");
    let output = command(&inputs, &sidecar.endpoint, &evidence)
        .output()
        .expect("spawn tag-16 process");
    assert!(
        output.status.success(),
        "tag 16 failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    {
        let calls = sidecar.calls.lock().expect("call recorder");
        assert_eq!(calls.prepare.len(), 1);
        assert_eq!(calls.complete.len(), 1);
        assert_eq!(calls.submit.len(), 1);
        let prepared = &calls.prepare[0];
        let completed = &calls.complete[0];
        let submitted = &calls.submit[0];
        assert_eq!(prepared.context.sidecar_role, Participant::Taker);
        assert_eq!(prepared.runtime, stage.runtime);
        assert_eq!(prepared.terms, stage.binding.terms());
        assert_eq!(
            prepared.terms.to_input().refund_message_hash.as_bytes(),
            &stage.agreement.body().messages().refund()
        );
        assert_eq!(completed.refund.preparation_request_id.as_str(), PREPARE);
        assert_eq!(
            completed.refund.message_hash,
            prepared.terms.to_input().refund_message_hash
        );
        assert_eq!(
            completed.aggregate_signature.as_bytes(),
            &stage.final_signature
        );
        assert_eq!(submitted.transaction, completed_transaction());
        assert_eq!(
            submitted.context.request_id,
            submitted.transaction.transaction_id.submission_request_id()
        );
    }

    let report: Value =
        serde_json::from_slice(&fs::read(&evidence).expect("evidence")).expect("evidence JSON");
    assert_eq!(report["schema"], "lez_v02_m5_actual_local_tag16_v1");
    assert_eq!(report["role"], "taker");
    assert_eq!(report["submission_outcome"], "accepted");
    assert_eq!(
        report["prepared_message_hash"],
        hex::encode(stage.agreement.body().messages().refund())
    );
    assert_eq!(report["automatic_submission_retry"], false);

    let rejecting = spawn_sidecar(Behavior::RejectSubmit).await;
    let failed = command(&inputs, &rejecting.endpoint, &inputs.output("reject.json"))
        .output()
        .expect("spawn rejected submission");
    assert_failure(&failed, "rejected submission");
    assert_eq!(rejecting.calls.lock().expect("calls").submit.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn sealed_effect_child_derives_stage_b_tag16_from_the_live_journal_and_submits_once() {
    let stage = build_stage_b();
    let inputs = Inputs::new(&stage);
    let sidecar = spawn_sidecar(Behavior::Happy).await;
    let (mut command, evidence) = effect_child_command(&stage, &inputs, &sidecar.endpoint);
    let output = command.output().expect("spawn sealed Tag16 effect child");
    assert!(
        output.status.success(),
        "sealed Tag16 child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());

    let calls = sidecar.calls.lock().expect("call recorder");
    assert_eq!(calls.prepare.len(), 1);
    assert_eq!(calls.complete.len(), 1);
    assert_eq!(calls.submit.len(), 1);
    assert_eq!(
        calls.complete[0].aggregate_signature.as_bytes(),
        &stage.final_signature
    );
    assert_eq!(calls.submit[0].transaction, completed_transaction());
    drop(calls);

    let report: Value = serde_json::from_slice(&fs::read(evidence).expect("effect evidence"))
        .expect("effect evidence JSON");
    assert_eq!(report["schema"], "lez_v02_m5_actual_local_tag16_v1");
    assert_eq!(report["submission_outcome"], "accepted");
}

#[tokio::test(flavor = "multi_thread")]
async fn sealed_effect_child_preflight_prepares_only_and_never_publishes_evidence() {
    let stage = build_stage_b();
    let inputs = Inputs::new(&stage);
    let sidecar = spawn_sidecar(Behavior::Happy).await;
    let (mut command, evidence) =
        effect_child_command_with_mode(&stage, &inputs, &sidecar.endpoint, "preflight");
    let output = command
        .output()
        .expect("spawn sealed Tag16 preflight child");
    assert!(
        output.status.success(),
        "sealed Tag16 preflight failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert!(!evidence.exists());
    let calls = sidecar.calls.lock().expect("call recorder");
    assert_eq!(calls.prepare.len(), 1);
    assert!(calls.complete.is_empty());
    assert!(calls.submit.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn sealed_effect_child_rejected_preflight_never_completes_submits_or_publishes() {
    let stage = build_stage_b();
    let inputs = Inputs::new(&stage);
    let sidecar = spawn_sidecar(Behavior::RejectPrepare).await;
    let (mut command, evidence) =
        effect_child_command_with_mode(&stage, &inputs, &sidecar.endpoint, "preflight");
    let output = command
        .output()
        .expect("spawn rejected Tag16 preflight child");
    assert_failure(&output, "rejected Tag16 preflight");
    assert!(!evidence.exists());
    let calls = sidecar.calls.lock().expect("call recorder");
    assert_eq!(calls.prepare.len(), 1);
    assert!(calls.complete.is_empty());
    assert!(calls.submit.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn sealed_effect_child_rejects_live_journal_drift_before_sidecar_use() {
    let stage = build_stage_b();
    let inputs = Inputs::new(&stage);
    let sidecar = spawn_sidecar(Behavior::Happy).await;
    let mut changed = stage.activation.body().refund_presignature();
    changed[0] ^= 1;
    let (mut command, evidence) =
        effect_child_command_with_presignature(&stage, &inputs, &sidecar.endpoint, changed);
    let output = command.output().expect("spawn drifted Tag16 effect child");
    assert_failure(&output, "drifted live journal");
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("durable Taker refund presignature differs from Stage B")
    );
    assert!(!evidence.exists());
    let calls = sidecar.calls.lock().expect("call recorder");
    assert!(calls.prepare.is_empty());
    assert!(calls.complete.is_empty());
    assert!(calls.submit.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_role_session_signature_and_crossed_request_fail_before_submission() {
    let stage = build_stage_b();
    let inputs = Inputs::new(&stage);
    let sidecar = spawn_sidecar(Behavior::Happy).await;

    let mut wrong_runtime = stage.runtime.clone();
    wrong_runtime.sidecar_role = Participant::Maker;
    fs::write(
        &inputs.runtime,
        serde_json::to_vec(&wrong_runtime).expect("wrong runtime JSON"),
    )
    .expect("write wrong runtime");
    let wrong_role = command(
        &inputs,
        &sidecar.endpoint,
        &inputs.output("wrong-role.json"),
    )
    .output()
    .expect("spawn wrong role");
    assert_failure(&wrong_role, "wrong role");
    fs::write(
        &inputs.runtime,
        serde_json::to_vec(&stage.runtime).expect("runtime JSON"),
    )
    .expect("restore runtime");

    write_final_signature_packet_for_descriptor(
        &inputs.final_signature,
        stage.agreement.claim_session_descriptor(),
        stage.final_signature,
    );
    let wrong_session = command(
        &inputs,
        &sidecar.endpoint,
        &inputs.output("wrong-session.json"),
    )
    .output()
    .expect("spawn wrong session");
    assert_failure(&wrong_session, "wrong session");

    write_final_signature_packet(&inputs.final_signature, &stage.agreement, [0x55; 64]);
    let wrong_signature = command(
        &inputs,
        &sidecar.endpoint,
        &inputs.output("wrong-signature.json"),
    )
    .output()
    .expect("spawn wrong signature");
    assert_failure(&wrong_signature, "wrong signature");

    write_final_signature_packet(
        &inputs.final_signature,
        &stage.agreement,
        stage.final_signature,
    );
    let mut crossed = command_with_request_ids(
        &inputs,
        &sidecar.endpoint,
        &inputs.output("crossed-request.json"),
        PREPARE,
        PREPARE,
    );
    let crossed = crossed.output().expect("spawn crossed request");
    assert_failure(&crossed, "crossed request");
    assert!(String::from_utf8_lossy(&crossed.stderr).contains("must be distinct"));

    let calls = sidecar.calls.lock().expect("call recorder");
    assert!(calls.prepare.is_empty());
    assert!(calls.complete.is_empty());
    assert!(calls.submit.is_empty());
}

fn completed_transaction() -> PreparedTransaction {
    PreparedTransaction::new(
        TransactionId::from_bytes([32; 32]),
        ExactTransactionBytes::new(vec![32; 128]).expect("transaction bytes"),
    )
}

fn write_private(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .expect("create private fixture");
    file.write_all(bytes).expect("write private fixture");
}

fn write_final_signature_packet(path: &Path, agreement: &XmrAgreementV1, signature: [u8; 64]) {
    write_final_signature_packet_for_descriptor(
        path,
        agreement.refund_session_descriptor(),
        signature,
    );
}

fn write_final_signature_packet_for_descriptor(
    path: &Path,
    descriptor: lez_xmr_swap_sdk::XmrAdaptorSessionDescriptorV1,
    signature: [u8; 64],
) {
    #[derive(Serialize)]
    struct Packet {
        schema_version: u16,
        kind: &'static str,
        session_id: String,
        sender_role: &'static str,
        context_binding: String,
        payload: String,
    }
    if path.exists() {
        fs::remove_file(path).expect("replace test packet");
    }
    let mut bytes = serde_json::to_vec(&Packet {
        schema_version: 1,
        kind: "final_signature",
        session_id: hex::encode(descriptor.session_id()),
        sender_role: "aggregate",
        context_binding: hex::encode(descriptor.context_binding()),
        payload: hex::encode(signature),
    })
    .expect("final-signature JSON");
    bytes.push(b'\n');
    write_private(path, &bytes);
}

#[allow(clippy::too_many_lines)]
fn build_stage_b() -> StageFixture {
    let maker_scalar = scalar(11);
    let taker_scalar = scalar(13);
    let maker_proof =
        CrossCurveDleqProofV1::prove(&maker_scalar, &mut ChaCha20Rng::from_seed([71; 32]))
            .expect("Maker proof");
    let taker_proof =
        CrossCurveDleqProofV1::prove(&taker_scalar, &mut ChaCha20Rng::from_seed([72; 32]))
            .expect("Taker proof");
    let view = view_key();
    let shared = MoneroSharedAddressV1::derive(
        MoneroAddressNetworkV1::Regtest,
        &maker_proof,
        &taker_proof,
        &view,
    )
    .expect("shared address");
    let participants = participants();
    let claim_key = participants
        .claim_aggregate_x_only_key()
        .expect("claim aggregate");
    let refund_key = participants
        .refund_aggregate_x_only_key()
        .expect("refund aggregate");
    let profile = XmrNamedProfileV1::AcceleratedRegtest;
    let refund_message_hash = official_message_hash(&REFUND_MESSAGE_BYTES);
    let body = XmrAgreementBodyV1::new(
        XmrSwapDirectionV1::TakerSellsLez,
        profile,
        [19; 32],
        participants,
        XmrMoneroTermsV1::new(
            MoneroAddressNetworkV1::Regtest,
            [31; 32],
            1_000_000_000_000,
            profile.required_monero_confirmations(),
            maker_proof.to_wire_bytes().expect("Maker proof wire"),
            taker_proof.to_wire_bytes().expect("Taker proof wire"),
            shared.public_view_key(),
            shared.public_spend_key(),
            shared.address_string(),
        ),
        XmrLezTermsV1::new(
            [40; 32],
            [41; 32],
            [42; 8],
            [43; 8],
            profile.required_lez_finality_units(),
            [45; 32],
            [47; 32],
            [22; 32],
            [21; 32],
            claim_key,
            XmrLezTermsV1::authority_account_for_key(claim_key),
            refund_key,
            XmrLezTermsV1::authority_account_for_key(refund_key),
            maker_proof.transcript_commitment(),
            taker_proof.transcript_commitment(),
            501,
        ),
        XmrMessagesV1::new(
            [51; 32],
            refund_message_hash.as_bytes().to_owned(),
            [53; 32],
        ),
        XmrWindowsV1::new(10_000, 20_000, 30_000),
    );
    let agreement_commitment = body.commitment();
    let agreement = XmrAgreementV1::from_wire(
        &XmrAgreementRecordV1::from_parts(
            XMR_AGREEMENT_SCHEMA_V1,
            body,
            agreement_commitment,
            sign(MAKER_AGREEMENT_SECRET, agreement_commitment),
            sign(TAKER_AGREEMENT_SECRET, agreement_commitment),
        )
        .encode_wire()
        .expect("agreement wire"),
    )
    .expect("validated Stage A");

    let claim_context = session_context(
        &agreement,
        b"claim",
        agreement.body().messages().claim(),
        maker_proof.secp256k1_public_key(),
        true,
    );
    let refund_context = session_context(
        &agreement,
        b"refund",
        agreement.body().messages().refund(),
        taker_proof.secp256k1_public_key(),
        false,
    );
    let (claim_transcript, maker_claim_partial, taker_claim_partial, _) =
        signer_round(&claim_context, MAKER_CLAIM_SECRET, TAKER_CLAIM_SECRET);
    let (refund_transcript, maker_refund_partial, taker_refund_partial, refund_presignature) =
        signer_round(&refund_context, MAKER_REFUND_SECRET, TAKER_REFUND_SECRET);
    let partial_context = agreement
        .claim_partial_context_binding(&claim_transcript, maker_claim_partial)
        .expect("claim partial context");
    let partial_commitment = agreement
        .commit_taker_claim_partial(&claim_transcript, maker_claim_partial, taker_claim_partial)
        .expect("Taker partial commitment");
    let activation_body = XmrActivationBodyV1::new(
        agreement.agreement_commitment(),
        agreement.claim_context_binding(),
        claim_transcript,
        maker_claim_partial,
        partial_context,
        partial_commitment,
        agreement.refund_context_binding(),
        refund_transcript,
        maker_refund_partial,
        taker_refund_partial,
        refund_presignature,
    );
    let activation_commitment = activation_body.commitment();
    let activation = XmrActivatedAgreementV1::validate(
        &agreement,
        XmrActivationRecordV1::from_parts(
            XMR_ACTIVATION_SCHEMA_V1,
            activation_body,
            activation_commitment,
            sign(MAKER_AGREEMENT_SECRET, activation_commitment),
            sign(TAKER_AGREEMENT_SECRET, activation_commitment),
        ),
        &view,
    )
    .expect("validated Stage B");
    let binding = XmrLezBridgeBindingV3::new(&agreement, &activation).expect("bridge binding");
    let plan = activation
        .lez_initialize_plan(&agreement)
        .expect("initialize plan");
    let runtime = RuntimeDescriptor::new(
        Participant::Taker,
        RuntimeCompatibility::LeeV0_2_0,
        h(39),
        Hex32::from_bytes(plan.channel_id()),
        Hex32::from_bytes(plan.genesis_hash()),
        Hex32::from_bytes(program_bytes(plan.escrow_program_id())),
        Hex32::from_bytes(plan.depositor_account()),
    );
    let final_signature = adapt_presignature(
        &refund_context,
        refund_presignature,
        taker_scalar.adaptor_scalar_big_endian(),
    )
    .expect("refund final signature");
    StageFixture {
        agreement,
        activation,
        binding,
        runtime,
        final_signature,
    }
}

fn session_context(
    agreement: &XmrAgreementV1,
    purpose: &[u8],
    message: [u8; 32],
    adaptor_point: [u8; 33],
    claim: bool,
) -> AdaptorSessionContext {
    let participants = agreement.body().participants();
    let maker = participants.for_role(XmrRoleV1::Maker);
    let taker = participants.for_role(XmrRoleV1::Taker);
    let keys = if claim {
        [
            maker.claim_session_public_key(),
            taker.claim_session_public_key(),
        ]
    } else {
        [
            maker.refund_session_public_key(),
            taker.refund_session_public_key(),
        ]
    };
    AdaptorSessionContext::untweaked(
        keys,
        message,
        adaptor_point,
        session_id(agreement.agreement_commitment(), purpose),
    )
    .expect("adaptor session")
}

fn signer_round(
    context: &AdaptorSessionContext,
    maker_secret: [u8; 32],
    taker_secret: [u8; 32],
) -> (XmrSessionTranscriptV1, [u8; 32], [u8; 32], [u8; 65]) {
    let mut maker = AdaptorSigner::new(context.clone(), SigningRole::Maker, maker_secret)
        .expect("Maker signer");
    let mut taker = AdaptorSigner::new(context.clone(), SigningRole::Taker, taker_secret)
        .expect("Taker signer");
    let maker_commitment = maker.nonce_commitment();
    let taker_commitment = taker.nonce_commitment();
    maker
        .accept_peer_commitment(taker_commitment)
        .expect("Maker commitment");
    taker
        .accept_peer_commitment(maker_commitment)
        .expect("Taker commitment");
    let maker_nonce = maker.public_nonce().expect("Maker nonce");
    let taker_nonce = taker.public_nonce().expect("Taker nonce");
    maker.accept_peer_nonce(taker_nonce).expect("Maker opening");
    taker.accept_peer_nonce(maker_nonce).expect("Taker opening");
    let maker_partial = maker.create_partial_signature().expect("Maker partial");
    let taker_partial = taker.create_partial_signature().expect("Taker partial");
    maker
        .accept_peer_partial_signature(taker_partial)
        .expect("Maker verifies partial");
    taker
        .accept_peer_partial_signature(maker_partial)
        .expect("Taker verifies partial");
    (
        XmrSessionTranscriptV1::new(maker_commitment, taker_commitment, maker_nonce, taker_nonce),
        maker_partial,
        taker_partial,
        maker.presignature().expect("presignature"),
    )
}

fn participants() -> XmrParticipantsV1 {
    XmrParticipantsV1::new(
        XmrParticipantIdentityV1::new(
            [21; 32],
            public_key(MAKER_AGREEMENT_SECRET),
            public_key(MAKER_CLAIM_SECRET),
            public_key(MAKER_REFUND_SECRET),
        ),
        XmrParticipantIdentityV1::new(
            [22; 32],
            public_key(TAKER_AGREEMENT_SECRET),
            public_key(TAKER_CLAIM_SECRET),
            public_key(TAKER_REFUND_SECRET),
        ),
    )
}

fn scalar(value: u8) -> CrossCurveScalar {
    let mut bytes = [0; 32];
    bytes[0] = value;
    CrossCurveScalar::from_monero_little_endian(bytes).expect("fixture scalar")
}

fn view_key() -> MoneroPrivateViewKey {
    MoneroPrivateViewKey::from_monero_little_endian(VIEW_KEY_BYTES).expect("private view key")
}

fn public_key(secret: [u8; 32]) -> [u8; 33] {
    let secret = SecretKey::from_slice(&secret).expect("fixture secret");
    PublicKey::from_secret_key(&Secp256k1::new(), &secret).serialize()
}

fn sign(secret: [u8; 32], commitment: [u8; 32]) -> [u8; 64] {
    let secret = SecretKey::from_slice(&secret).expect("fixture secret");
    let secp = Secp256k1::new();
    secp.sign_schnorr_no_aux_rand(
        &SecpMessage::from_digest(commitment),
        &Keypair::from_secret_key(&secp, &secret),
    )
    .serialize()
}

fn session_id(commitment: [u8; 32], purpose: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(SESSION_DOMAIN);
    hasher.update(commitment);
    hasher.update(purpose);
    hasher.finalize().into()
}

fn official_message_hash(bytes: &[u8]) -> Hex32 {
    let mut hasher = Sha256::new();
    hasher.update(b"/LEE/v0.3/Message/Public/\x00\x00\x00\x00\x00\x00\x00");
    hasher.update(bytes);
    Hex32::from_bytes(hasher.finalize().into())
}

fn program_bytes(words: [u32; 8]) -> [u8; 32] {
    let mut bytes = [0; 32];
    for (index, word) in words.into_iter().enumerate() {
        bytes[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    bytes
}

const fn h(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}
