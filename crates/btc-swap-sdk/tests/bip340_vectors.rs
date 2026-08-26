mod support;

use bitcoin::secp256k1::{Message, Secp256k1, XOnlyPublicKey, schnorr};
use k256::schnorr::{Signature as K256Signature, SigningKey, VerifyingKey};
use musig2::secp::{Point, Scalar};
use musig2::{BinaryEncoding as _, CompactSignature};

use support::{hex_array, vector_root};

#[derive(Debug)]
struct Vector<'a> {
    index: usize,
    secret_key: Option<&'a str>,
    public_key: &'a str,
    aux_rand: Option<&'a str>,
    message: &'a str,
    signature: &'a str,
    valid: bool,
    comment: &'a str,
}

fn vectors(contents: &str) -> Vec<Vector<'_>> {
    contents
        .lines()
        .skip(1)
        .map(|line| {
            let fields = line.splitn(8, ',').collect::<Vec<_>>();
            assert_eq!(fields.len(), 8, "BIP-340 row must have eight fields");
            Vector {
                index: fields[0].parse().expect("numeric BIP-340 index"),
                secret_key: (!fields[1].is_empty()).then_some(fields[1]),
                public_key: fields[2],
                aux_rand: (!fields[3].is_empty()).then_some(fields[3]),
                message: fields[4],
                signature: fields[5],
                valid: match fields[6] {
                    "TRUE" => true,
                    "FALSE" => false,
                    other => panic!("invalid BIP-340 result: {other}"),
                },
                comment: fields[7],
            }
        })
        .collect()
}

#[test]
fn official_bip340_vectors_cross_check_three_implementations() {
    let path = vector_root().join("bip-0340/test-vectors.csv");
    let contents = std::fs::read_to_string(&path).expect("read pinned BIP-340 CSV");
    let vectors = vectors(&contents);
    assert_eq!(vectors.len(), 19);

    for vector in &vectors {
        let public_key = hex_array::<32>(vector.public_key);
        let message = support::hex_vec(vector.message);
        let signature = hex_array::<64>(vector.signature);

        let musig_result = Point::lift_x(public_key)
            .and_then(|point| {
                CompactSignature::from_bytes(&signature)
                    .map_err(|_| musig2::secp::errors::InvalidPointBytes)
                    .and_then(|signature| {
                        musig2::verify_single(point, signature, &message)
                            .map_err(|_| musig2::secp::errors::InvalidPointBytes)
                    })
            })
            .is_ok();
        assert_eq!(
            musig_result, vector.valid,
            "musig2 disagreed on BIP-340 vector {}: {}",
            vector.index, vector.comment
        );

        let k256_result = VerifyingKey::from_bytes(&public_key)
            .and_then(|key| {
                K256Signature::try_from(signature.as_slice())
                    .and_then(|signature| key.verify_raw(&message, &signature))
            })
            .is_ok();
        assert_eq!(
            k256_result, vector.valid,
            "k256 disagreed on BIP-340 vector {}: {}",
            vector.index, vector.comment
        );

        if message.len() == 32 {
            let bitcoin_result = XOnlyPublicKey::from_slice(&public_key)
                .and_then(|key| {
                    schnorr::Signature::from_slice(&signature).and_then(|signature| {
                        Secp256k1::verification_only().verify_schnorr(
                            &signature,
                            &Message::from_digest(message.clone().try_into().expect("32 bytes")),
                            &key,
                        )
                    })
                })
                .is_ok();
            assert_eq!(
                bitcoin_result, vector.valid,
                "rust-bitcoin disagreed on fixed-message BIP-340 vector {}: {}",
                vector.index, vector.comment
            );
        }
    }
}

#[test]
fn official_bip340_signing_rows_match_with_musig2_and_k256() {
    let contents = std::fs::read_to_string(vector_root().join("bip-0340/test-vectors.csv"))
        .expect("read pinned BIP-340 CSV");

    for vector in vectors(&contents)
        .into_iter()
        .filter(|vector| vector.secret_key.is_some())
    {
        let secret_key = hex_array::<32>(vector.secret_key.expect("filtered"));
        let aux_rand = hex_array::<32>(vector.aux_rand.expect("signing vector has aux_rand"));
        let message = support::hex_vec(vector.message);
        let expected = hex_array::<64>(vector.signature);

        let musig_signature: CompactSignature = musig2::sign_solo(
            Scalar::from_slice(&secret_key).expect("valid official scalar"),
            &message,
            aux_rand,
        );
        assert_eq!(
            musig_signature.to_bytes(),
            expected,
            "vector {}",
            vector.index
        );

        let k256_signature = SigningKey::from_bytes(&secret_key)
            .expect("valid official scalar")
            .sign_raw(&message, &aux_rand)
            .expect("official signing vector");
        assert_eq!(
            k256_signature.to_bytes().as_slice(),
            expected,
            "vector {}",
            vector.index
        );
    }
}
