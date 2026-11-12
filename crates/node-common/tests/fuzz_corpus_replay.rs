//! Replays every seed of `fuzz/corpus/offer_announcement` and `fuzz/corpus/chat_frame` through
//! the fuzz targets' own harnesses and oracles, so the stable workspace test run covers the
//! corpora that only cargo-fuzz (nightly) would otherwise execute.

#[path = "../../../fuzz/fuzz_targets/chat_frame_harness.rs"]
mod chat_frame_harness;
#[path = "../../../fuzz/fuzz_targets/offer_announcement_harness.rs"]
mod offer_announcement_harness;

fn replay(target: &str, run: fn(&[u8])) {
    let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fuzz/corpus")
        .join(target);
    let mut replayed = 0;
    for entry in std::fs::read_dir(&corpus).expect("fuzz corpus directory must be readable") {
        let path = entry.expect("fuzz corpus entry must be readable").path();
        let data = std::fs::read(&path).expect("fuzz corpus seed must be readable");
        println!("replaying {}", path.display());
        run(&data);
        replayed += 1;
    }
    assert!(replayed > 0, "no seeds found in {}", corpus.display());
}

#[test]
fn offer_announcement_fuzz_corpus_replays_on_stable() {
    replay("offer_announcement", offer_announcement_harness::run);
}

#[test]
fn chat_frame_fuzz_corpus_replays_on_stable() {
    replay("chat_frame", chat_frame_harness::run);
}
