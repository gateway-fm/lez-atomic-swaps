use lez_swap_core::UnixSeconds;
use lez_zec_swap_sdk::{ZecAgreementDraftV1, ZecAgreementV1, ZecMakerAgreementProposalV1};

/// Validation time of the checked-in agreement seeds.
const NOW: UnixSeconds = UnixSeconds::new(10);

/// Replays one fuzz input as Zcash agreement bytes from the Chat peer, through each decoder that
/// takes them: the unsigned draft (Maker, `zec_chat_propose_v1`), the Maker proposal (Taker), and
/// the countersigned agreement (Maker, `zec_chat_complete_v1`). Whatever decodes re-encodes and
/// decodes again to the same value.
pub fn run(data: &[u8]) {
    if let Ok(draft) = ZecAgreementDraftV1::from_wire_at(data, NOW) {
        let encoded = ZecAgreementDraftV1::new(draft.body().clone())
            .encode_wire()
            .expect("decoded draft must re-encode");
        let again =
            ZecAgreementDraftV1::from_wire_at(&encoded, NOW).expect("re-encoded draft must decode");
        assert!(
            again.body() == draft.body(),
            "draft changed across re-encoding"
        );
    }
    if let Ok(proposal) = ZecMakerAgreementProposalV1::from_wire_at(data, NOW) {
        let encoded = proposal
            .encode_wire()
            .expect("decoded proposal must re-encode");
        let again = ZecMakerAgreementProposalV1::from_wire_at(&encoded, NOW)
            .expect("re-encoded proposal must decode");
        assert!(again == proposal, "proposal changed across re-encoding");
    }
    if let Ok(agreement) = ZecAgreementV1::from_wire_at(data, NOW) {
        let encoded = agreement
            .encode_wire()
            .expect("decoded agreement must re-encode");
        let again =
            ZecAgreementV1::from_wire_at(&encoded, NOW).expect("re-encoded agreement must decode");
        assert!(
            again.record() == agreement.record(),
            "agreement changed across re-encoding"
        );
    }
}
