//! Replays every seed of `fuzz/corpus/coordinator` through the fuzz target's own harness and
//! oracles, so the stable workspace test run covers the corpus that only cargo-fuzz (nightly)
//! would otherwise execute.

#[path = "../../../fuzz/fuzz_targets/coordinator_harness.rs"]
mod coordinator_harness;

#[test]
fn coordinator_fuzz_corpus_replays_on_stable() {
    let corpus =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/coordinator");
    let mut replayed = 0;
    for entry in std::fs::read_dir(&corpus).expect("fuzz corpus directory must be readable") {
        let path = entry.expect("fuzz corpus entry must be readable").path();
        let data = std::fs::read(&path).expect("fuzz corpus seed must be readable");
        println!("replaying {}", path.display());
        coordinator_harness::run(&data);
        replayed += 1;
    }
    assert!(replayed > 0, "no seeds found in {}", corpus.display());
}
