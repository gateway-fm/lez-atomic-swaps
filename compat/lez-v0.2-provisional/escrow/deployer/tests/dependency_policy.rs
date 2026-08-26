#[test]
fn locked_deployer_graph_excludes_the_generated_clients_full_wallet() {
    let lock: toml::Value = include_str!("../Cargo.lock")
        .parse()
        .expect("deployer Cargo.lock must remain valid TOML");
    let packages = lock["package"]
        .as_array()
        .expect("Cargo.lock package array");
    let package_names = packages
        .iter()
        .map(|package| package["name"].as_str().expect("locked package name"))
        .collect::<Vec<_>>();
    assert!(
        !package_names.contains(&"wallet"),
        "the generated client's full upstream WalletCore graph is test-only and must never enter the deployment tool"
    );
    assert!(
        !include_str!("../Cargo.toml").contains("wallet ="),
        "the deployer must use only official transaction/RPC types, never WalletCore"
    );
}
