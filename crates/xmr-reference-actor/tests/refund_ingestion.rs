#![cfg(feature = "sessions")]

use clap::Parser as _;
use xmr_reference_actor::{Action, Cli};

#[test]
fn finalized_refund_ingestion_is_a_role_fixed_maker_command() {
    let cli = Cli::try_parse_from([
        "xmr-reference-actor",
        "ingest-finalized-refund-signature",
        "--private-root",
        "/private/maker",
        "--own-public-packet",
        "/exchange/maker.json",
        "--peer-public-packet",
        "/exchange/taker.json",
        "--agreement-stage-a",
        "/exchange/agreement.bin",
        "--activation-stage-b",
        "/exchange/activation.bin",
        "--journal",
        "/private/maker.sqlite",
        "--run-id",
        "m5-refund",
        "--finalized-refund",
        "/exchange/finalized-refund.json",
        "--output-final-signature",
        "/private/observed-refund-signature.json",
    ])
    .expect("parse role-fixed Maker refund ingestion");

    let Action::IngestFinalizedRefundSignature {
        private_root,
        own_public_packet,
        peer_public_packet,
        agreement_stage_a,
        activation_stage_b,
        journal,
        run_id,
        finalized_refund,
        output_final_signature,
    } = cli.action
    else {
        panic!("wrong action");
    };

    assert_eq!(private_root.to_str(), Some("/private/maker"));
    assert_eq!(own_public_packet.to_str(), Some("/exchange/maker.json"));
    assert_eq!(peer_public_packet.to_str(), Some("/exchange/taker.json"));
    assert_eq!(agreement_stage_a.to_str(), Some("/exchange/agreement.bin"));
    assert_eq!(
        activation_stage_b.to_str(),
        Some("/exchange/activation.bin")
    );
    assert_eq!(journal.to_str(), Some("/private/maker.sqlite"));
    assert_eq!(run_id, "m5-refund");
    assert_eq!(
        finalized_refund.to_str(),
        Some("/exchange/finalized-refund.json")
    );
    assert_eq!(
        output_final_signature.to_str(),
        Some("/private/observed-refund-signature.json")
    );
}
