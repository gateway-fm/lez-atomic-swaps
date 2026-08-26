use lez_zec_swap_sdk::Bip199Contract;
use zcash_script::{
    interpreter::{Flags, SignatureChecker},
    script::{Code, Raw},
    signature::Decoded,
};

const VALID_DER_SIGNATURE_WITH_SIGHASH_ALL: [u8; 9] =
    [0x30, 0x06, 0x02, 0x01, 0x01, 0x02, 0x01, 0x01, 0x01];
const CLAIMANT_PUBLIC_KEY: [u8; 33] = [0x02; 33];
const REFUND_PUBLIC_KEY: [u8; 33] = [0x03; 33];
const PREIMAGE: [u8; 32] = [0x44; 32];

struct BoundaryChecker {
    transaction_lock_time: i64,
    final_sequence: bool,
}

impl SignatureChecker for BoundaryChecker {
    fn check_sig(&self, _signature: &Decoded, _public_key: &[u8], _script_code: &Code) -> bool {
        true
    }

    fn check_lock_time(&self, required_lock_time: i64) -> bool {
        required_lock_time <= self.transaction_lock_time && !self.final_sequence
    }
}

fn contract() -> Bip199Contract {
    Bip199Contract::new(
        500_000,
        [
            0xf3, 0x28, 0xc7, 0x40, 0xe5, 0x72, 0xb4, 0xa6, 0xf7, 0xc1, 0xab, 0x87, 0x6e, 0x7a,
            0xff, 0xfd, 0x75, 0xbd, 0xfc, 0xb3,
        ],
        [
            0xbb, 0x39, 0x14, 0x15, 0xc0, 0x5e, 0x39, 0xd7, 0x7c, 0xa1, 0x73, 0x81, 0xd3, 0xbe,
            0x3f, 0x7d, 0x0c, 0xd5, 0xe5, 0x33, 0x2e, 0x5a, 0x57, 0x93, 0x11, 0xad, 0xaa, 0x0a,
            0xa6, 0x21, 0x06, 0xe9,
        ],
        [
            0x51, 0x81, 0x4f, 0x10, 0x86, 0x70, 0xac, 0xed, 0x2d, 0x77, 0xc1, 0x80, 0x5d, 0xdd,
            0x66, 0x34, 0xbc, 0x9d, 0x47, 0x31,
        ],
    )
}

fn flags() -> Flags {
    Flags::P2SH
        | Flags::SigPushOnly
        | Flags::MinimalData
        | Flags::CleanStack
        | Flags::CHECKLOCKTIMEVERIFY
        | Flags::StrictEnc
}

#[test]
fn correct_preimage_claims_and_wrong_preimage_is_rejected() {
    let contract = contract();
    let checker = BoundaryChecker {
        transaction_lock_time: 0,
        final_sequence: false,
    };
    let valid = Raw::from_raw_parts(
        contract
            .claim_script_sig(
                &VALID_DER_SIGNATURE_WITH_SIGHASH_ALL,
                &CLAIMANT_PUBLIC_KEY,
                &PREIMAGE,
            )
            .unwrap(),
        contract.p2sh_script_pubkey().to_vec(),
    );
    assert_eq!(valid.eval(flags(), &checker), Ok(true));

    let wrong = Raw::from_raw_parts(
        contract
            .claim_script_sig(
                &VALID_DER_SIGNATURE_WITH_SIGHASH_ALL,
                &CLAIMANT_PUBLIC_KEY,
                &[0x45; 32],
            )
            .unwrap(),
        contract.p2sh_script_pubkey().to_vec(),
    );
    assert!(wrong.eval(flags(), &checker).is_err());
}

#[test]
fn refund_requires_threshold_and_non_final_sequence() {
    let contract = contract();
    let refund = || {
        Raw::from_raw_parts(
            contract
                .refund_script_sig(&VALID_DER_SIGNATURE_WITH_SIGHASH_ALL, &REFUND_PUBLIC_KEY)
                .unwrap(),
            contract.p2sh_script_pubkey().to_vec(),
        )
    };

    assert!(
        refund()
            .eval(
                flags(),
                &BoundaryChecker {
                    transaction_lock_time: 499_999,
                    final_sequence: false,
                },
            )
            .is_err()
    );
    assert_eq!(
        refund().eval(
            flags(),
            &BoundaryChecker {
                transaction_lock_time: 500_000,
                final_sequence: false,
            },
        ),
        Ok(true),
    );
    assert!(
        refund()
            .eval(
                flags(),
                &BoundaryChecker {
                    transaction_lock_time: 500_000,
                    final_sequence: true,
                },
            )
            .is_err()
    );
}
