use lez_node_common::{
    XmrCeremonyNonceRequestV1, XmrCeremonyNonceResponseV1, XmrCeremonyPartialRequestV1,
    XmrCeremonyPartialResponseV1, XmrCeremonyReserveRequestV1, XmrCeremonyReserveResponseV1,
    XmrReserveRequestV1, XmrReserveResponseV1,
};
use lez_xmr_swap_sdk::{
    MoneroPrivateViewKey, ValidatedXmrActivationBodyV1, ValidatedXmrAgreementBodyV1,
    XmrActivatedAgreementV1, XmrAgreementV1,
};
use std::sync::OnceLock;

/// The countersigned Stage A of the checked-in seeds, which every Stage-B decoder is bound to.
const SEED_AGREEMENT: &[u8] = include_bytes!("../corpus/xmr_reserve_wire/stage-a");
/// The seeds' per-swap private view key (test material, Monero little-endian scalar).
const SEED_VIEW_KEY: [u8; 32] = [
    0xa7, 0x0c, 0xe8, 0x59, 0x39, 0xc4, 0xc0, 0x44, 0x1d, 0x9c, 0xe9, 0xda, 0xfa, 0x9f, 0xbb, 0x0e,
    0x41, 0xa8, 0xb7, 0xa6, 0x6a, 0x3f, 0xc2, 0xf3, 0xf1, 0xf2, 0xd9, 0x52, 0x4e, 0x2e, 0xf5, 0x03,
];

fn seed() -> &'static (XmrAgreementV1, MoneroPrivateViewKey) {
    static SEED: OnceLock<(XmrAgreementV1, MoneroPrivateViewKey)> = OnceLock::new();
    SEED.get_or_init(|| {
        (
            XmrAgreementV1::from_wire(SEED_AGREEMENT).expect("seed agreement must decode"),
            MoneroPrivateViewKey::from_monero_little_endian(SEED_VIEW_KEY)
                .expect("seed view key must decode"),
        )
    })
}

macro_rules! json_round_trip {
    ($data:expr, $($wire:ty),+) => {
        $(
            if let Ok(value) = serde_json::from_slice::<$wire>($data) {
                let encoded = serde_json::to_vec(&value).expect("decoded wire must re-encode");
                let again: $wire =
                    serde_json::from_slice(&encoded).expect("re-encoded wire must decode");
                assert!(again == value, "wire changed across re-encoding");
            }
        )+
    };
}

/// Replays one fuzz input as Monero take-time bytes from the Chat peer: the agreement and
/// activation wires the `xmr_*` methods carry (unsigned and countersigned Stage A, and Stage B
/// against the seed agreement and view key), and the JSON of each `xmr_reserve_v1` and
/// `xmr_ceremony_*_v1` request and response. Whatever decodes re-encodes to the same value.
pub fn run(data: &[u8]) {
    if let Ok(body) = ValidatedXmrAgreementBodyV1::from_unsigned_wire(data) {
        let encoded = body
            .encode_unsigned_wire()
            .expect("decoded Stage A body must re-encode");
        assert!(encoded == data, "Stage A body changed across re-encoding");
    }
    if let Ok(agreement) = XmrAgreementV1::from_wire(data) {
        let encoded = agreement
            .encode_wire()
            .expect("decoded agreement must re-encode");
        assert!(encoded == data, "agreement changed across re-encoding");
    }
    let (agreement, view_key) = seed();
    if let Ok(body) = ValidatedXmrActivationBodyV1::from_unsigned_wire(agreement, data, view_key) {
        let encoded = body
            .encode_unsigned_wire()
            .expect("decoded Stage B body must re-encode");
        assert!(encoded == data, "Stage B body changed across re-encoding");
    }
    if let Ok(activated) = XmrActivatedAgreementV1::from_wire(agreement, data, view_key) {
        let encoded = activated
            .encode_wire()
            .expect("decoded activation must re-encode");
        assert!(encoded == data, "activation changed across re-encoding");
    }
    json_round_trip!(
        data,
        XmrReserveRequestV1,
        XmrReserveResponseV1,
        XmrCeremonyReserveRequestV1,
        XmrCeremonyReserveResponseV1,
        XmrCeremonyNonceRequestV1,
        XmrCeremonyNonceResponseV1,
        XmrCeremonyPartialRequestV1,
        XmrCeremonyPartialResponseV1
    );
}
