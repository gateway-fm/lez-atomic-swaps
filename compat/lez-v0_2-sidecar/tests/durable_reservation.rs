#![cfg(unix)]

use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use lez_bridge_protocol::{
    Hex32, MessageContext, NativeEscrowTerms, NativeEscrowTermsInput, Participant,
    PrepareNativeEscrowRequest, RequestId, RunId, RuntimeCompatibility, RuntimeDescriptor,
};
use lez_v0_2_sidecar::{
    DurableReservationError, NativeEscrowPlanner, NativePrepareError, NonceSource,
    PrepareVaultClaimRequest, VaultClaimAllocation, VaultClaimNonceSource, VaultClaimPlanner,
    VaultClaimPrepareError,
};
use nssa::{AccountId, PrivateKey, PublicKey};

static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct TestDirectory(PathBuf);

impl TestDirectory {
    fn secure(label: &str) -> Self {
        let sequence = TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "lez-v02-durable-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn only_regular_file(&self) -> PathBuf {
        let mut files = fs::read_dir(&self.0)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        assert_eq!(files.len(), 1, "expected exactly one durable reservation");
        files.pop().unwrap()
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug)]
struct FixedNativeNonce {
    value: u128,
    calls: AtomicUsize,
}

#[async_trait]
impl NonceSource for FixedNativeNonce {
    async fn account_nonce(&self, _account_id: AccountId) -> Result<u128, NativePrepareError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.value)
    }
}

#[derive(Debug)]
struct FixedVaultNonce {
    value: u128,
    calls: AtomicUsize,
}

#[async_trait]
impl VaultClaimNonceSource for FixedVaultNonce {
    async fn account_nonce(&self, _account_id: AccountId) -> Result<u128, VaultClaimPrepareError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.value)
    }
}

fn keyed_account(byte: u8) -> (AccountId, PrivateKey) {
    let key = PrivateKey::try_new([byte; 32]).unwrap();
    let account = AccountId::from(&PublicKey::new_from_private_key(&key));
    (account, key)
}

const fn h(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}

fn program_hex(program_id: [u32; 8]) -> Hex32 {
    let mut bytes = [0_u8; 32];
    for (chunk, word) in bytes.chunks_exact_mut(4).zip(program_id) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    Hex32::from_bytes(bytes)
}

fn runtime(role: Participant, signer: AccountId, escrow_program: [u32; 8]) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        role,
        RuntimeCompatibility::LeeV0_2_0,
        h(1),
        h(2),
        h(3),
        program_hex(escrow_program),
        Hex32::from_bytes(signer.into_value()),
    )
}

fn native_request(
    role: Participant,
    signer: AccountId,
    claimant: AccountId,
    escrow_program: [u32; 8],
    transfer_program: [u32; 8],
    run_id: &str,
    request_id: &str,
) -> PrepareNativeEscrowRequest {
    PrepareNativeEscrowRequest::new(
        MessageContext::new(
            RunId::new(run_id).unwrap(),
            RequestId::new(request_id).unwrap(),
            role,
        ),
        runtime(role, signer, escrow_program),
        NativeEscrowTerms::new(NativeEscrowTermsInput {
            swap_id: h(4),
            terms_hash: h(5),
            secret_digest: h(6),
            depositor: role,
            depositor_account_id: Hex32::from_bytes(signer.into_value()),
            claimant: match role {
                Participant::Maker => Participant::Taker,
                Participant::Taker => Participant::Maker,
            },
            claimant_account_id: Hex32::from_bytes(claimant.into_value()),
            amount: 75_000,
            refund_at_ms: 1_850_000_000_123,
            authenticated_transfer_program_id: program_hex(transfer_program),
        })
        .unwrap(),
    )
}

fn durable_native_planner(
    role: Participant,
    key_byte: u8,
    escrow_program: [u32; 8],
    transfer_program: [u32; 8],
    nonce: Arc<FixedNativeNonce>,
    directory: &Path,
) -> NativeEscrowPlanner {
    let (owner, key) = keyed_account(key_byte);
    NativeEscrowPlanner::new_durable(
        role,
        key,
        escrow_program,
        transfer_program,
        runtime(role, owner, escrow_program),
        nonce,
        directory,
    )
    .unwrap()
}

fn vault_request(
    role: Participant,
    owner: AccountId,
    amount: u128,
    run_id: &str,
    request_id: &str,
) -> PrepareVaultClaimRequest {
    PrepareVaultClaimRequest::new(
        MessageContext::new(
            RunId::new(run_id).unwrap(),
            RequestId::new(request_id).unwrap(),
            role,
        ),
        runtime(role, owner, [77; 8]),
        VaultClaimAllocation::new(role, Hex32::from_bytes(owner.into_value()), amount).unwrap(),
        0,
    )
}

fn durable_vault_planner(
    role: Participant,
    key_byte: u8,
    amount: u128,
    nonce: Arc<FixedVaultNonce>,
    directory: &Path,
) -> VaultClaimPlanner {
    let (owner, key) = keyed_account(key_byte);
    VaultClaimPlanner::new_durable(
        role,
        key,
        runtime(role, owner, [77; 8]),
        VaultClaimAllocation::new(role, Hex32::from_bytes(owner.into_value()), amount).unwrap(),
        nonce,
        directory,
    )
    .unwrap()
}

#[tokio::test]
async fn native_restart_restores_exact_bytes_without_nonce_reuse_and_rejects_request_drift() {
    let directory = TestDirectory::secure("native-restart");
    let (owner, _) = keyed_account(21);
    let (claimant, _) = keyed_account(22);
    let escrow_program = [31; 8];
    let transfer_program = [32; 8];
    let request = native_request(
        Participant::Maker,
        owner,
        claimant,
        escrow_program,
        transfer_program,
        "durable-native-run-0001",
        "durable-native-request-0001",
    );
    let first_nonce = Arc::new(FixedNativeNonce {
        value: 41,
        calls: AtomicUsize::new(0),
    });
    let first = durable_native_planner(
        Participant::Maker,
        21,
        escrow_program,
        transfer_program,
        Arc::clone(&first_nonce),
        directory.path(),
    );
    let prepared = first.prepare(request.clone()).await.unwrap();
    assert_eq!(first_nonce.calls.load(Ordering::SeqCst), 1);
    drop(first);

    let state = directory.only_regular_file();
    let metadata = fs::symlink_metadata(&state).unwrap();
    assert!(metadata.file_type().is_file());
    assert_eq!(metadata.mode() & 0o777, 0o600);
    assert_eq!(metadata.nlink(), 1);

    let restart_nonce = Arc::new(FixedNativeNonce {
        value: 999,
        calls: AtomicUsize::new(0),
    });
    let restarted = durable_native_planner(
        Participant::Maker,
        21,
        escrow_program,
        transfer_program,
        Arc::clone(&restart_nonce),
        directory.path(),
    );
    let recovered = restarted.prepare(request.clone()).await.unwrap();
    assert_eq!(recovered, prepared);
    assert_eq!(restart_nonce.calls.load(Ordering::SeqCst), 0);

    let mut wrong_request = request.clone();
    wrong_request.context.request_id = RequestId::new("durable-native-request-0002").unwrap();
    assert_eq!(
        restarted.prepare(wrong_request).await.unwrap_err(),
        NativePrepareError::ActivePrepare
    );
    drop(restarted);

    let wrong_run = native_request(
        Participant::Maker,
        owner,
        claimant,
        escrow_program,
        transfer_program,
        "durable-native-run-0002",
        "durable-native-request-0001",
    );
    let wrong_run_planner = durable_native_planner(
        Participant::Maker,
        21,
        escrow_program,
        transfer_program,
        Arc::new(FixedNativeNonce {
            value: 41,
            calls: AtomicUsize::new(0),
        }),
        directory.path(),
    );
    assert_eq!(
        wrong_run_planner.prepare(wrong_run).await.unwrap_err(),
        NativePrepareError::ActivePrepare
    );
}

#[tokio::test]
async fn vault_restart_is_exact_and_maker_taker_stores_are_independent() {
    let maker_directory = TestDirectory::secure("maker-vault");
    let taker_directory = TestDirectory::secure("taker-vault");
    let actors = [
        (
            Participant::Maker,
            1,
            100_000,
            &maker_directory,
            "durable-maker-vault-request-0001",
        ),
        (
            Participant::Taker,
            2,
            200_000,
            &taker_directory,
            "durable-taker-vault-request-0001",
        ),
    ];
    let mut state_paths = Vec::new();

    for (role, key_byte, amount, directory, request_id) in actors {
        let (owner, _) = keyed_account(key_byte);
        let request = vault_request(role, owner, amount, "durable-vault-run-0001", request_id);
        let first = durable_vault_planner(
            role,
            key_byte,
            amount,
            Arc::new(FixedVaultNonce {
                value: 0,
                calls: AtomicUsize::new(0),
            }),
            directory.path(),
        );
        let prepared = first.prepare(request.clone()).await.unwrap();
        drop(first);

        let restart_nonce = Arc::new(FixedVaultNonce {
            value: 77,
            calls: AtomicUsize::new(0),
        });
        let restarted = durable_vault_planner(
            role,
            key_byte,
            amount,
            Arc::clone(&restart_nonce),
            directory.path(),
        );
        assert_eq!(restarted.prepare(request).await.unwrap(), prepared);
        assert_eq!(restart_nonce.calls.load(Ordering::SeqCst), 0);
        state_paths.push(directory.only_regular_file());
    }

    assert_ne!(state_paths[0].parent(), state_paths[1].parent());
    assert_ne!(
        fs::read(&state_paths[0]).unwrap(),
        fs::read(&state_paths[1]).unwrap()
    );
}

type NativeTamperCase = (Box<dyn Fn(&mut serde_json::Value)>, NativePrepareError);

fn mutate_json(path: &Path, mutate: impl FnOnce(&mut serde_json::Value)) {
    let mut value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    mutate(&mut value);
    let bytes = serde_json::to_vec(&value).unwrap();
    let mut file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .unwrap();
    file.write_all(&bytes).unwrap();
    file.sync_all().unwrap();
}

#[tokio::test]
async fn recovery_revalidates_role_runtime_signer_allocation_program_and_exact_bytes() {
    let directory = TestDirectory::secure("tamper-native");
    let (owner, _) = keyed_account(21);
    let (claimant, _) = keyed_account(22);
    let escrow_program = [31; 8];
    let transfer_program = [32; 8];
    let request = native_request(
        Participant::Maker,
        owner,
        claimant,
        escrow_program,
        transfer_program,
        "durable-native-run-0003",
        "durable-native-request-0003",
    );
    let make_planner = || {
        durable_native_planner(
            Participant::Maker,
            21,
            escrow_program,
            transfer_program,
            Arc::new(FixedNativeNonce {
                value: 41,
                calls: AtomicUsize::new(0),
            }),
            directory.path(),
        )
    };
    let _ = make_planner().prepare(request.clone()).await.unwrap();
    let path = directory.only_regular_file();
    let original = fs::read(&path).unwrap();

    let cases: Vec<NativeTamperCase> = vec![
        (
            Box::new(|value| value["request"]["context"]["sidecar_role"] = "taker".into()),
            NativePrepareError::WrongRole,
        ),
        (
            Box::new(|value| value["request"]["runtime"]["chain_id"] = "63".repeat(32).into()),
            NativePrepareError::WrongRuntime,
        ),
        (
            Box::new(|value| {
                value["request"]["terms"]["depositor_account_id"] = "63".repeat(32).into();
            }),
            NativePrepareError::WrongSigner,
        ),
        (
            Box::new(|value| {
                value["request"]["terms"]["authenticated_transfer_program_id"] =
                    "63".repeat(32).into();
            }),
            NativePrepareError::WrongAuthenticatedTransferProgram,
        ),
        (
            Box::new(|value| {
                value["result"]["initialization"]["transaction_id"] = "63".repeat(32).into();
            }),
            NativePrepareError::WrongTransactionId,
        ),
    ];
    for (mutator, expected) in cases {
        fs::write(&path, &original).unwrap();
        mutate_json(&path, mutator);
        assert_eq!(
            make_planner().prepare(request.clone()).await.unwrap_err(),
            expected
        );
    }
}

#[tokio::test]
async fn vault_recovery_revalidates_stored_allocation() {
    let vault_directory = TestDirectory::secure("tamper-vault");
    let (vault_owner, _) = keyed_account(1);
    let vault_request = vault_request(
        Participant::Maker,
        vault_owner,
        100_000,
        "durable-vault-run-0002",
        "durable-maker-vault-request-0002",
    );
    let make_vault_planner = || {
        durable_vault_planner(
            Participant::Maker,
            1,
            100_000,
            Arc::new(FixedVaultNonce {
                value: 0,
                calls: AtomicUsize::new(0),
            }),
            vault_directory.path(),
        )
    };
    let _ = make_vault_planner()
        .prepare(vault_request.clone())
        .await
        .unwrap();
    let vault_path = vault_directory.only_regular_file();
    mutate_json(&vault_path, |value| {
        value["request"]["allocation"]["amount"] = 99.into();
    });
    assert_eq!(
        make_vault_planner()
            .prepare(vault_request)
            .await
            .unwrap_err(),
        VaultClaimPrepareError::WrongAllocation
    );
}

#[tokio::test]
async fn rejects_crash_partials_corruption_future_schema_and_unknown_fields() {
    let directory = TestDirectory::secure("invalid-state");
    let (owner, _) = keyed_account(21);
    let (claimant, _) = keyed_account(22);
    let escrow_program = [31; 8];
    let transfer_program = [32; 8];
    let request = native_request(
        Participant::Maker,
        owner,
        claimant,
        escrow_program,
        transfer_program,
        "durable-native-run-0004",
        "durable-native-request-0004",
    );
    let make_planner = || {
        durable_native_planner(
            Participant::Maker,
            21,
            escrow_program,
            transfer_program,
            Arc::new(FixedNativeNonce {
                value: 41,
                calls: AtomicUsize::new(0),
            }),
            directory.path(),
        )
    };

    let partial = directory
        .path()
        .join(".native-escrow-reservation.v1.json.partial.crashed");
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&partial)
        .unwrap()
        .write_all(b"{\"schema_version\":")
        .unwrap();
    assert_eq!(
        make_planner().prepare(request.clone()).await.unwrap_err(),
        NativePrepareError::DurableReservation(DurableReservationError::PartialReservation)
    );
    fs::remove_file(partial).unwrap();

    let _ = make_planner().prepare(request.clone()).await.unwrap();
    let path = directory.only_regular_file();
    let original = fs::read(&path).unwrap();
    fs::write(&path, b"{\"schema_version\":").unwrap();
    assert_eq!(
        make_planner().prepare(request.clone()).await.unwrap_err(),
        NativePrepareError::DurableReservation(DurableReservationError::CorruptReservation)
    );

    fs::write(&path, &original).unwrap();
    mutate_json(&path, |value| value["schema_version"] = 2.into());
    assert_eq!(
        make_planner().prepare(request.clone()).await.unwrap_err(),
        NativePrepareError::DurableReservation(DurableReservationError::FutureSchema)
    );

    fs::write(&path, &original).unwrap();
    mutate_json(&path, |value| value["unexpected"] = true.into());
    assert_eq!(
        make_planner().prepare(request).await.unwrap_err(),
        NativePrepareError::DurableReservation(DurableReservationError::CorruptReservation)
    );
}

#[tokio::test]
async fn rejects_insecure_directories_symlinks_permissions_and_hardlink_aliases() {
    let insecure = TestDirectory::secure("path-must-stay-redacted");
    fs::set_permissions(insecure.path(), fs::Permissions::from_mode(0o755)).unwrap();
    let error = NativeEscrowPlanner::new_durable(
        Participant::Maker,
        keyed_account(21).1,
        [31; 8],
        [32; 8],
        runtime(Participant::Maker, keyed_account(21).0, [31; 8]),
        Arc::new(FixedNativeNonce {
            value: 41,
            calls: AtomicUsize::new(0),
        }),
        insecure.path(),
    )
    .unwrap_err();
    assert_eq!(
        error,
        NativePrepareError::DurableReservation(DurableReservationError::InsecureDirectory)
    );
    assert!(!error.to_string().contains("path-must-stay-redacted"));

    let target = TestDirectory::secure("symlink-target");
    let link = target.path().with_extension("link");
    symlink(target.path(), &link).unwrap();
    let symlink_error = NativeEscrowPlanner::new_durable(
        Participant::Maker,
        keyed_account(21).1,
        [31; 8],
        [32; 8],
        runtime(Participant::Maker, keyed_account(21).0, [31; 8]),
        Arc::new(FixedNativeNonce {
            value: 41,
            calls: AtomicUsize::new(0),
        }),
        &link,
    )
    .unwrap_err();
    assert_eq!(
        symlink_error,
        NativePrepareError::DurableReservation(DurableReservationError::InsecureDirectory)
    );
    fs::remove_file(link).unwrap();

    let directory = TestDirectory::secure("state-security");
    let (owner, _) = keyed_account(21);
    let (claimant, _) = keyed_account(22);
    let request = native_request(
        Participant::Maker,
        owner,
        claimant,
        [31; 8],
        [32; 8],
        "durable-native-run-0005",
        "durable-native-request-0005",
    );
    let make_planner = || {
        durable_native_planner(
            Participant::Maker,
            21,
            [31; 8],
            [32; 8],
            Arc::new(FixedNativeNonce {
                value: 41,
                calls: AtomicUsize::new(0),
            }),
            directory.path(),
        )
    };
    let _ = make_planner().prepare(request.clone()).await.unwrap();
    let state = directory.only_regular_file();

    fs::set_permissions(&state, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        make_planner().prepare(request.clone()).await.unwrap_err(),
        NativePrepareError::DurableReservation(DurableReservationError::InsecureStateFile)
    );
    fs::set_permissions(&state, fs::Permissions::from_mode(0o600)).unwrap();

    let alias = directory.path().join("hardlink-alias");
    fs::hard_link(&state, &alias).unwrap();
    assert_eq!(
        make_planner().prepare(request.clone()).await.unwrap_err(),
        NativePrepareError::DurableReservation(DurableReservationError::InsecureStateFile)
    );
    fs::remove_file(alias).unwrap();

    let original = fs::read(&state).unwrap();
    fs::remove_file(&state).unwrap();
    let symlink_target = directory.path().join("symlink-target-file");
    fs::write(&symlink_target, original).unwrap();
    fs::set_permissions(&symlink_target, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&symlink_target, &state).unwrap();
    assert_eq!(
        make_planner().prepare(request).await.unwrap_err(),
        NativePrepareError::DurableReservation(DurableReservationError::InsecureStateFile)
    );
}

#[test]
fn durable_planner_diagnostics_redact_key_and_store_path() {
    let directory = TestDirectory::secure("never-print-store-path");
    let planner = durable_native_planner(
        Participant::Maker,
        21,
        [31; 8],
        [32; 8],
        Arc::new(FixedNativeNonce {
            value: 41,
            calls: AtomicUsize::new(0),
        }),
        directory.path(),
    );
    let rendered = format!("{planner:?}");
    assert!(rendered.contains("[REDACTED]"));
    assert!(!rendered.contains("never-print-store-path"));
    assert!(!rendered.contains(&"15".repeat(32)));
}
