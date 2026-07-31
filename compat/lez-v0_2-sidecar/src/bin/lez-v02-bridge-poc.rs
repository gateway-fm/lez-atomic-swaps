#![forbid(unsafe_code)]

use std::{
    fs,
    io::{self, Write as _},
    net::SocketAddr,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    str::FromStr as _,
    sync::Arc,
};

use anyhow::{Context as _, Result, bail, ensure};
use clap::{Parser, ValueEnum};
use lez_bridge_protocol::{
    Hex32, MessageContext, RequestId, RunId, RuntimeDescriptor, XmrNativeEscrowTermsV3,
};
use lez_v0_2_sidecar::{
    BridgeRuntime, BridgeServerCapability, BridgeServerConfig, NativeEscrowPlanner,
    OfficialIndexerRpc, OfficialNodeRpc, StateDirectoryLease, m4_tag13_state_present,
    program_id_from_hex, read_genesis_bound_finalized_clock, start_bridge_server,
    validate_loopback_http_endpoint, verify_m4_tag13_bridge_handoff,
};
use nssa::{AccountId, PrivateKey, PublicKey};
use serde::Serialize;
use tokio::io::{AsyncBufReadExt as _, BufReader};
use zeroize::{Zeroize as _, Zeroizing};

const MAX_PUBLIC_CONFIG_BYTES: u64 = 16 * 1024;
const MAX_SECRET_FILE_BYTES: u64 = 256;

/// Selects one complete outbound node route without weakening the local listener.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum NodeRouteProfile {
    /// Explicit literal-loopback HTTP sequencer and indexer endpoints.
    Local,
    /// Exact allowlisted official Testnet HTTPS origin for both clients.
    #[value(name = "official_public")]
    OfficialPublic,
}

impl NodeRouteProfile {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::OfficialPublic => "official_public",
        }
    }

    fn validate_endpoint(self, endpoint: &str) -> Result<()> {
        match self {
            Self::Local => validate_loopback_http_endpoint(endpoint),
            Self::OfficialPublic => OfficialNodeRpc::validate_official_public_endpoint(endpoint),
        }
        .map_err(anyhow::Error::from)
    }

    fn connect_sequencer(self, endpoint: &str) -> Result<OfficialNodeRpc> {
        match self {
            Self::Local => OfficialNodeRpc::connect_local(endpoint),
            Self::OfficialPublic => OfficialNodeRpc::connect_official_public(endpoint),
        }
        .map_err(anyhow::Error::from)
    }

    fn connect_indexer(self, endpoint: &str) -> Result<OfficialIndexerRpc> {
        match self {
            Self::Local => OfficialIndexerRpc::connect_local(endpoint),
            Self::OfficialPublic => OfficialIndexerRpc::connect_official_public(endpoint),
        }
        .map_err(anyhow::Error::from)
    }
}

/// Run one role-isolated exact-LEZ-v0.2 bridge against a selected node route.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Arguments {
    /// Run-allocated literal-loopback nonzero listen address.
    #[arg(long)]
    listen_address: SocketAddr,
    /// Outbound sequencer URL accepted by the selected node profile.
    #[arg(long)]
    sequencer_url: String,
    /// Outbound indexer URL accepted by the selected node profile.
    #[arg(long)]
    indexer_url: String,
    /// Complete outbound node route; the actor-facing listener stays loopback-only.
    #[arg(long, value_enum, default_value_t = NodeRouteProfile::Local)]
    node_profile: NodeRouteProfile,
    /// Composed run identity shared with this actor's bridge client.
    #[arg(long)]
    run_id: String,
    /// Public JSON file containing the exact immutable runtime descriptor.
    #[arg(long)]
    runtime_file: PathBuf,
    /// Owner-private JSON file containing exact activated XMR terms.
    #[arg(long)]
    terms_file: Option<PathBuf>,
    /// Owner-only file containing the bridge bearer capability.
    #[arg(long)]
    capability_file: PathBuf,
    /// Owner-only file containing one lowercase-hex 32-byte LEZ private key.
    #[arg(long)]
    private_key_file: PathBuf,
    /// Existing owner-only 0700 directory dedicated to this actor and run.
    #[arg(long)]
    state_directory: PathBuf,
    /// Optional owner-private receipt required when this is adopted tag-13 state.
    #[arg(long)]
    tag13_handoff_receipt: Option<PathBuf>,
    /// Checked authenticated-transfer program ID as 64 lowercase hex characters.
    #[arg(long)]
    authenticated_transfer_program_id: String,
    /// Stop after one line is read from standard input (process-harness mode).
    #[arg(long)]
    shutdown_on_stdin: bool,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct Readiness<'a> {
    event: &'static str,
    endpoint: &'a str,
    run_id: &'a str,
    runtime: &'a RuntimeDescriptor,
    node_profile: &'static str,
    sequencer_observation: &'static str,
    indexer_health: &'static str,
    finality: &'static str,
}

#[tokio::main]
async fn main() {
    if let Err(error) = execute(Arguments::parse()).await {
        eprintln!("LEZ v0.2 bridge PoC failed: {error:#}");
        std::process::exit(1);
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "ordered startup keeps state lease, public config, secrets, health, and bind checks auditable"
)]
async fn execute(arguments: Arguments) -> Result<()> {
    validate_arguments(&arguments)?;
    let state_lease = acquire_state_directory_lease(&arguments.state_directory)?;
    let run_id = RunId::new(arguments.run_id.clone()).context("invalid run ID")?;
    let authenticated_transfer_program = parse_nonzero_hex(
        &arguments.authenticated_transfer_program_id,
        "authenticated-transfer program ID",
    )?;
    let runtime = load_runtime_before_secrets(
        &arguments,
        &state_lease,
        &run_id,
        authenticated_transfer_program,
    )?;
    let activated_xmr_terms =
        load_activated_xmr_terms(arguments.terms_file.as_deref(), &run_id, &runtime)?;
    let mut capability = read_secret_text(&arguments.capability_file)?;
    let capability = BridgeServerCapability::new(std::mem::take(&mut *capability))
        .context("invalid bridge capability")?;
    let signer_key = read_private_key(&arguments.private_key_file)?;
    let signer_account_id = AccountId::from(&PublicKey::new_from_private_key(&signer_key));
    ensure!(
        runtime.signer_account_id == Hex32::from_bytes(signer_account_id.into_value()),
        "runtime signer does not match the isolated private key"
    );
    ensure!(
        authenticated_transfer_program != runtime.escrow_program_id,
        "escrow and authenticated-transfer programs must be distinct"
    );
    let node = Arc::new(
        arguments
            .node_profile
            .connect_sequencer(&arguments.sequencer_url)
            .context("invalid official sequencer endpoint")?,
    );
    let indexer = Arc::new(
        arguments
            .node_profile
            .connect_indexer(&arguments.indexer_url)
            .context("connect official indexer")?,
    );
    let finalized_clock =
        read_genesis_bound_finalized_clock(indexer.as_ref(), runtime.genesis_block_hash)
            .await
            .context("official indexer finalized clock is not bound to the runtime genesis")?;
    ensure!(
        finalized_clock.height >= 2,
        "official indexer has not finalized a non-genesis block"
    );
    let planner = Arc::new(NativeEscrowPlanner::new_durable(
        runtime.sidecar_role,
        signer_key,
        program_id_from_hex(runtime.escrow_program_id),
        program_id_from_hex(authenticated_transfer_program),
        runtime.clone(),
        Arc::clone(&node),
        &arguments.state_directory,
    )?);
    let bridge_runtime = Arc::new(bind_activated_xmr_terms(
        BridgeRuntime::new(runtime.clone(), planner, node, indexer),
        activated_xmr_terms.as_ref(),
    ));
    let server = start_bridge_server(
        BridgeServerConfig::new(
            run_id.clone(),
            capability,
            arguments.state_directory.join("bridge-requests.v1.json"),
            arguments.listen_address,
        ),
        bridge_runtime,
    )
    .await?;
    serde_json::to_writer(
        io::stdout().lock(),
        &Readiness {
            event: "ready",
            endpoint: server.endpoint(),
            run_id: run_id.as_str(),
            runtime: &runtime,
            node_profile: arguments.node_profile.as_str(),
            sequencer_observation: "bounded_canonical_inclusion_and_same_tip_accounts",
            indexer_health: "stable_finalized_tip_bound_to_runtime_genesis",
            finality: "exact_genesis_bound_finalized_indexer_clock_available",
        },
    )
    .context("encode readiness")?;
    println!();
    io::stdout().flush().context("flush readiness")?;

    if arguments.shutdown_on_stdin {
        let mut line = String::new();
        BufReader::new(tokio::io::stdin())
            .read_line(&mut line)
            .await
            .context("read shutdown signal")?;
        line.zeroize();
    } else {
        tokio::signal::ctrl_c()
            .await
            .context("wait for interrupt")?;
    }
    server.stop().await?;
    drop(state_lease);
    Ok(())
}

fn bind_activated_xmr_terms(
    runtime: BridgeRuntime,
    terms: Option<&XmrNativeEscrowTermsV3>,
) -> BridgeRuntime {
    match terms {
        Some(terms) => runtime.with_activated_xmr_terms(*terms),
        None => runtime,
    }
}

fn load_activated_xmr_terms(
    path: Option<&Path>,
    run_id: &RunId,
    runtime: &RuntimeDescriptor,
) -> Result<Option<XmrNativeEscrowTermsV3>> {
    path.map(|path| -> Result<XmrNativeEscrowTermsV3> {
        let terms: XmrNativeEscrowTermsV3 =
            serde_json::from_slice(&read_public_file(path, MAX_PUBLIC_CONFIG_BYTES)?)
                .context("invalid activated XMR terms")?;
        terms
            .validate_runtime_binding(
                &MessageContext::new(
                    run_id.clone(),
                    RequestId::new("startup-activated-terms-0001")?,
                    runtime.sidecar_role,
                ),
                runtime,
            )
            .context("activated XMR terms do not match runtime")?;
        Ok(terms)
    })
    .transpose()
}
fn load_runtime_before_secrets(
    arguments: &Arguments,
    state_lease: &StateDirectoryLease,
    run_id: &RunId,
    authenticated_transfer_program_id: Hex32,
) -> Result<RuntimeDescriptor> {
    if let Some(receipt) = arguments.tag13_handoff_receipt.as_ref() {
        return verify_m4_tag13_bridge_handoff(
            state_lease,
            receipt,
            &arguments.runtime_file,
            run_id,
            authenticated_transfer_program_id,
        )
        .map(|verified| verified.runtime().clone())
        .context("revalidate exact tag-13 receipt and state before secrets or RPCs");
    }
    ensure!(
        !m4_tag13_state_present(state_lease)
            .context("inspect fixed tag-13 state before secrets or RPCs")?,
        "tag-13 handoff receipt is required for fixed tag-13 state"
    );
    serde_json::from_slice(&read_public_file(
        &arguments.runtime_file,
        MAX_PUBLIC_CONFIG_BYTES,
    )?)
    .context("invalid runtime descriptor")
}

fn acquire_state_directory_lease(path: &Path) -> Result<StateDirectoryLease> {
    StateDirectoryLease::acquire(path)
        .context("acquire exclusive bridge sidecar state-directory lease")
}

fn validate_arguments(arguments: &Arguments) -> Result<()> {
    ensure!(
        arguments.listen_address.ip().is_loopback() && arguments.listen_address.port() != 0,
        "listen address must be a literal loopback IP and nonzero port"
    );
    validate_node_routes(
        arguments.node_profile,
        &arguments.sequencer_url,
        &arguments.indexer_url,
    )?;
    ensure!(
        arguments.capability_file != arguments.private_key_file
            && arguments.terms_file.as_ref().is_none_or(|terms| {
                terms != &arguments.runtime_file
                    && terms != &arguments.capability_file
                    && terms != &arguments.private_key_file
            })
            && arguments.runtime_file != arguments.capability_file
            && arguments.runtime_file != arguments.private_key_file
            && arguments
                .tag13_handoff_receipt
                .as_ref()
                .is_none_or(|receipt| {
                    receipt != &arguments.runtime_file
                        && receipt != &arguments.capability_file
                        && receipt != &arguments.private_key_file
                }),
        "configuration and secret files must be distinct"
    );
    let state = fs::symlink_metadata(&arguments.state_directory)
        .context("state directory is unavailable")?;
    ensure!(
        state.is_dir() && state.permissions().mode() & 0o7777 == 0o700,
        "state directory must be a real owner-only 0700 directory"
    );
    Ok(())
}

fn validate_node_routes(
    profile: NodeRouteProfile,
    sequencer_url: &str,
    indexer_url: &str,
) -> Result<()> {
    profile
        .validate_endpoint(sequencer_url)
        .context("invalid sequencer URL")?;
    profile
        .validate_endpoint(indexer_url)
        .context("invalid indexer URL")?;
    Ok(())
}

fn read_public_file(path: &Path, maximum: u64) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).context("public config is unavailable")?;
    ensure!(
        metadata.file_type().is_file() && metadata.len() != 0 && metadata.len() <= maximum,
        "public config must be one bounded regular file"
    );
    fs::read(path).context("read public config")
}

fn read_secret_text(path: &Path) -> Result<Zeroizing<String>> {
    let metadata = fs::symlink_metadata(path).context("secret file is unavailable")?;
    ensure!(
        metadata.file_type().is_file()
            && metadata.len() != 0
            && metadata.len() <= MAX_SECRET_FILE_BYTES
            && metadata.permissions().mode().trailing_zeros() >= 6
            && metadata.nlink() == 1,
        "secret must be one bounded owner-only regular file"
    );
    let mut bytes = Zeroizing::new(fs::read(path).context("read secret file")?);
    while bytes
        .last()
        .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
    {
        bytes.pop();
    }
    let value = String::from_utf8(bytes.to_vec()).context("secret file is not UTF-8")?;
    ensure!(
        !value.is_empty() && !value.contains(['\n', '\r']),
        "secret file contains invalid line breaks"
    );
    Ok(Zeroizing::new(value))
}

fn read_private_key(path: &Path) -> Result<PrivateKey> {
    let encoded = read_secret_text(path)?;
    ensure!(
        encoded.len() == 64
            && encoded
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "private key must be exactly 64 lowercase hex characters"
    );
    PrivateKey::from_str(encoded.as_str()).context("invalid private key")
}

fn parse_nonzero_hex(value: &str, name: &str) -> Result<Hex32> {
    let value = Hex32::from_hex(value).with_context(|| format!("invalid {name}"))?;
    if value.as_bytes() == &[0; 32] {
        bail!("{name} must be nonzero");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt as _};

    use super::{
        Arguments, NodeRouteProfile, acquire_state_directory_lease, load_runtime_before_secrets,
        validate_node_routes,
    };
    use lez_bridge_protocol::{Hex32, RunId};

    const LOCAL_SEQUENCER: &str = "http://127.0.0.1:3040/";
    const LOCAL_INDEXER: &str = "http://127.0.0.1:8779/";
    const OFFICIAL_PUBLIC: &str = "https://testnet.lez.logos.co/";

    #[test]
    fn typed_node_profile_accepts_only_complete_local_or_official_public_routes() {
        assert!(
            validate_node_routes(NodeRouteProfile::Local, LOCAL_SEQUENCER, LOCAL_INDEXER).is_ok()
        );
        assert!(
            validate_node_routes(
                NodeRouteProfile::OfficialPublic,
                OFFICIAL_PUBLIC,
                OFFICIAL_PUBLIC,
            )
            .is_ok()
        );

        for (profile, sequencer, indexer) in [
            (NodeRouteProfile::Local, LOCAL_SEQUENCER, OFFICIAL_PUBLIC),
            (
                NodeRouteProfile::OfficialPublic,
                OFFICIAL_PUBLIC,
                LOCAL_INDEXER,
            ),
            (
                NodeRouteProfile::OfficialPublic,
                "https://example.com/",
                OFFICIAL_PUBLIC,
            ),
            (
                NodeRouteProfile::OfficialPublic,
                OFFICIAL_PUBLIC,
                "https://testnet.lez.logos.co/path",
            ),
        ] {
            assert!(validate_node_routes(profile, sequencer, indexer).is_err());
        }
    }

    #[test]
    fn bridge_binary_holds_one_exclusive_state_directory_lease() {
        let directory = tempfile::tempdir().expect("temporary state directory");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .expect("owner-only state directory");
        let first =
            acquire_state_directory_lease(directory.path()).expect("first binary state lease");
        let error = acquire_state_directory_lease(directory.path())
            .expect_err("second binary state lease must fail closed");
        assert!(
            format!("{error:#}").contains("already owned by another process"),
            "concurrent ownership must report a clear error"
        );

        drop(first);
        acquire_state_directory_lease(directory.path()).expect("binary lease after release");
    }

    #[test]
    fn direct_cli_omission_is_rejected_before_runtime_or_secret_reads() {
        let directory = tempfile::tempdir().expect("temporary state directory");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .expect("owner-only state directory");
        let evidence = directory
            .path()
            .join("m4-xmr-stage-a-tag13-evidence.v2.json");
        fs::write(&evidence, b"{}").expect("fixed tag-13 marker");
        fs::set_permissions(&evidence, fs::Permissions::from_mode(0o600))
            .expect("owner-only marker");
        let lease = acquire_state_directory_lease(directory.path()).expect("state lease");
        let arguments = Arguments {
            listen_address: "127.0.0.1:39001".parse().expect("listen address"),
            sequencer_url: LOCAL_SEQUENCER.to_owned(),
            indexer_url: LOCAL_INDEXER.to_owned(),
            node_profile: NodeRouteProfile::Local,
            run_id: "m4-tag13-direct-cli-test".to_owned(),
            runtime_file: directory.path().join("does-not-exist-runtime.json"),
            terms_file: None,
            capability_file: directory.path().join("does-not-exist-capability"),
            private_key_file: directory.path().join("does-not-exist-key"),
            state_directory: directory.path().to_path_buf(),
            tag13_handoff_receipt: None,
            authenticated_transfer_program_id:
                "0101010101010101010101010101010101010101010101010101010101010101".to_owned(),
            shutdown_on_stdin: true,
        };
        let error = load_runtime_before_secrets(
            &arguments,
            &lease,
            &RunId::new("m4-tag13-direct-cli-test").expect("run"),
            Hex32::from_bytes([1; 32]),
        )
        .expect_err("receipt omission must fail before missing runtime or secrets");
        assert!(format!("{error:#}").contains("receipt is required"));
    }
}
