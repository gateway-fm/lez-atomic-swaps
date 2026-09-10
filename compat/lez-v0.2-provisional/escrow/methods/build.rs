use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

const RISC0_GUEST_BUILDER_TAG: &str =
    "r0.1.94.1@sha256:c2f63fdd720337c0727e05c5e1733083baba04c00a864a89b0e3f4f8d92617be";
const GUEST_PACKAGE: &str = "lez-zec-escrow-v02-guest";
const GUEST_BINARY: &str = "zec_escrow_v02";
const RISC0_TARGET_TRIPLE: &str = "riscv32im-risc0-zkvm-elf";

fn main() {
    if let Ok(overridden) = std::env::var("RISC0_DOCKER_CONTAINER_TAG") {
        assert_eq!(
            overridden, RISC0_GUEST_BUILDER_TAG,
            "RISC0_DOCKER_CONTAINER_TAG must not select a different on-chain program",
        );
    }

    println!("cargo:rerun-if-env-changed=LEZ_V02_PREBUILT_GUEST_ELF");
    if let Ok(prebuilt) = std::env::var("LEZ_V02_PREBUILT_GUEST_ELF") {
        embed_prebuilt(Path::new(&prebuilt));
        return;
    }

    let docker = risc0_build::DockerOptionsBuilder::default()
        .root_dir("../..")
        .docker_container_tag(RISC0_GUEST_BUILDER_TAG)
        .build()
        .expect("valid canonical Risc0 Docker options");
    let guest = risc0_build::GuestOptionsBuilder::default()
        .use_docker(docker)
        .build()
        .expect("valid canonical Risc0 guest options");
    let mut guests = HashMap::new();
    guests.insert(GUEST_PACKAGE, guest);
    risc0_build::embed_methods_with_options(guests);
}

/// Embeds a guest ELF built elsewhere by the same digest-pinned builder image.
///
/// That image exists only for amd64: an x86-64 job builds the guest with it
/// natively, and the arm64 job that builds this crate and the deployer takes
/// the file instead of running the image under emulation. The ImageID r0vm
/// derives from the file must be the one pinned in `deployment-manifest.toml`
/// (the caller also checks the file's SHA-256 against the pinned digest), the
/// file is placed where risc0-build would have written it, and the generated
/// constants are the ones risc0-build generates.
fn embed_prebuilt(elf_path: &Path) {
    println!("cargo:rerun-if-changed={}", elf_path.display());
    let elf = std::fs::read(elf_path).expect("readable prebuilt guest ELF");
    assert!(!elf.is_empty(), "prebuilt guest ELF is empty");

    let crate_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let manifest = std::fs::read_to_string(crate_dir.join("guest/deployment-manifest.toml"))
        .expect("readable deployment manifest");
    let expected_id = manifest_string(&manifest, "image_id");
    let expected_words = manifest_words(&manifest, "program_id_words");

    let r0vm = std::env::var("RISC0_SERVER_PATH").unwrap_or_else(|_| "r0vm".to_owned());
    let output = Command::new(&r0vm)
        .env_remove("RUST_LOG")
        .arg("--elf")
        .arg(elf_path)
        .arg("--id")
        .output()
        .expect("r0vm on PATH or RISC0_SERVER_PATH");
    assert!(
        output.status.success(),
        "r0vm --id failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let image_id = String::from_utf8(output.stdout)
        .expect("r0vm prints an ImageID")
        .trim()
        .to_owned();
    assert_eq!(
        image_id, expected_id,
        "the prebuilt guest ELF is not the pinned on-chain program"
    );
    let words = image_id_words(&image_id);
    assert_eq!(
        words, expected_words,
        "deployment-manifest.toml program_id_words drift"
    );

    let path = guest_output_dir()
        .join(GUEST_PACKAGE)
        .join(RISC0_TARGET_TRIPLE)
        .join("docker")
        .join(format!("{GUEST_BINARY}.bin"));
    std::fs::create_dir_all(path.parent().expect("guest output directory"))
        .expect("guest output directory");
    std::fs::write(&path, &elf).expect("writable guest output");
    let path = path.to_str().expect("utf-8 guest output path");
    assert!(!path.contains('#'), "method path cannot include #: {path}");

    let upper = GUEST_BINARY.to_uppercase();
    let methods = format!(
        "pub const {upper}_ELF: &[u8] = include_bytes!({path:?});\n\
         pub const {upper}_PATH: &str = {path:?};\n\
         pub const {upper}_ID: [u32; 8] = {words:?};\n"
    );
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    std::fs::write(out_dir.join("methods.rs"), methods).expect("writable methods.rs");
}

/// `riscv-guest/<this crate>` under the cargo target directory, as
/// risc0-build resolves it.
fn guest_output_dir() -> PathBuf {
    let package = std::env::var("CARGO_PKG_NAME").expect("CARGO_PKG_NAME");
    if let Some(target_dir) = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .filter(|target_dir| target_dir.is_absolute())
    {
        return target_dir.join("riscv-guest").join(package);
    }
    let mut dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    loop {
        let is_target = dir.join(".rustc_info.json").exists()
            || dir.join("CACHEDIR.TAG").exists()
            || (dir.file_name().is_some_and(|name| name == "target")
                && dir
                    .parent()
                    .is_some_and(|parent| parent.join("Cargo.toml").exists()));
        if is_target {
            return dir.join("riscv-guest").join(package);
        }
        assert!(dir.pop(), "cannot find the cargo target directory");
    }
}

/// `key = "value"` from the manifest.
fn manifest_string(manifest: &str, key: &str) -> String {
    let line = manifest
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with(key) && line[key.len()..].trim_start().starts_with('='))
        .unwrap_or_else(|| panic!("deployment-manifest.toml has no {key}"));
    let value = line.split_once('=').expect("key = value").1.trim();
    value.trim_matches('"').to_owned()
}

/// `key = [w0, w1, ...]` from the manifest.
fn manifest_words(manifest: &str, key: &str) -> [u32; 8] {
    let value = manifest_string(manifest, key);
    let words: Vec<u32> = value
        .trim_matches(|c| c == '[' || c == ']')
        .split(',')
        .map(|word| word.trim().parse().expect("u32 word"))
        .collect();
    <[u32; 8]>::try_from(words).expect("eight words")
}

/// The ImageID's hex form as the eight little-endian words risc0 uses.
fn image_id_words(hex: &str) -> [u32; 8] {
    assert_eq!(hex.len(), 64, "an ImageID is 32 bytes of hex");
    let bytes: Vec<u8> = (0..32)
        .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("hex digit"))
        .collect();
    let mut words = [0_u32; 8];
    for (word, chunk) in words.iter_mut().zip(bytes.chunks_exact(4)) {
        *word = u32::from_le_bytes(<[u8; 4]>::try_from(chunk).expect("four bytes"));
    }
    words
}
