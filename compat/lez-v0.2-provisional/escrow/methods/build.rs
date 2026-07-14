use std::collections::HashMap;

const RISC0_GUEST_BUILDER_TAG: &str =
    "r0.1.94.1@sha256:c2f63fdd720337c0727e05c5e1733083baba04c00a864a89b0e3f4f8d92617be";

fn main() {
    if let Ok(overridden) = std::env::var("RISC0_DOCKER_CONTAINER_TAG") {
        assert_eq!(
            overridden, RISC0_GUEST_BUILDER_TAG,
            "RISC0_DOCKER_CONTAINER_TAG must not select a different on-chain program",
        );
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
    guests.insert("lez-zec-escrow-v02-guest", guest);
    risc0_build::embed_methods_with_options(guests);
}
