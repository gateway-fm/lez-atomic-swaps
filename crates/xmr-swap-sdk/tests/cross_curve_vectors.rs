//! Regression vectors and negative cases for the cross-curve DLEQ envelope and
//! Monero spend-key-share recovery.
//!
//! No third-party vectors exist for this construction: neither `sigma_fun`
//! 0.9.0 nor COMIT's archived `cross-curve-dleq` crate nor `xmr-btc-swap` ships
//! fixed proofs (their tests are random property tests), and `xmr-btc-swap`
//! instantiates the proof with the standard basepoints rather than the
//! alternate generators used here, so its proofs would not match these bytes.
//! The files under `tests/vectors/` are therefore SELF-GENERATED regression
//! vectors: deterministic inputs run once through the pinned `sigma_fun`,
//! `musig2` and `monero` crates (`regenerate_self_generated_vectors` below).
//! They pin today's bytes; they do not show agreement with another
//! implementation. The independent checks derive the expected public keys and
//! the shared address with libraries the proof path does not use
//! (`secp256k1`/libsecp256k1 and the `monero` crate's `curve25519-dalek`,
//! where `sigma_fun` uses `secp256kfun` and `curve25519-dalek-ng`).

use std::{collections::BTreeMap, fs, path::PathBuf};

use lez_xmr_swap_sdk::{
    CrossCurveDleqError, CrossCurveDleqProofV1, CrossCurveScalar, MoneroAddressNetworkV1,
    MoneroPrivateViewKey, MoneroSharedAddressV1, MoneroSharedSpendError,
    ReconstructedMoneroSpendKey,
};
use monero::{
    Address, Network,
    util::key::{KeyPair, PrivateKey, PublicKey as MoneroPublicKey},
};
use rand_chacha::{ChaCha20Rng, rand_core::SeedableRng as _};
use sha2::{Digest as _, Sha256};
use sigma_fun::{
    ed25519::curve25519_dalek::edwards::CompressedEdwardsY,
    ext::dl_secp256k1_ed25519_eq::CrossCurveDLEQProof,
};
use zeroize::Zeroizing;

const VECTORS_FILE: &str = "cross_curve_dleq_v1.txt";
const DLEQ_VECTORS: [&str; 3] = ["one", "max", "mixed"];
const TRANSCRIPT_COMMITMENT_DOMAIN: &[u8] = b"lez-atomic-swaps/xmr/cross-curve-dleq-transcript/v1";

/// The wire envelope's field order, mirrored so tests can rebuild it.
type Wire = (u8, Vec<u8>, Vec<u8>, Vec<u8>, [u8; 32]);

fn vectors_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors")
}

/// Parses `[section]` headers and `key=hex-or-text` lines; `#` starts a comment.
fn load_vectors() -> BTreeMap<String, BTreeMap<String, String>> {
    let text = fs::read_to_string(vectors_dir().join(VECTORS_FILE)).expect("vector file");
    let mut sections = BTreeMap::<String, BTreeMap<String, String>>::new();
    let mut current = None;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            assert!(
                sections.insert(name.to_owned(), BTreeMap::new()).is_none(),
                "duplicate section {name}"
            );
            current = Some(name.to_owned());
            continue;
        }
        let (key, value) = line.split_once('=').expect("key=value line");
        let section = sections
            .get_mut(current.as_ref().expect("value before any section"))
            .expect("current section");
        assert!(
            section.insert(key.to_owned(), value.to_owned()).is_none(),
            "duplicate key {key}"
        );
    }
    sections
}

fn field<const N: usize>(section: &BTreeMap<String, String>, key: &str) -> [u8; N] {
    hex::decode(&section[key])
        .expect("hex field")
        .try_into()
        .expect("fixed-width field")
}

fn wire_bytes(name: &str) -> Vec<u8> {
    fs::read(vectors_dir().join(format!("cross_curve_dleq_v1_{name}.bin"))).expect("wire file")
}

fn scalar(bytes: [u8; 32]) -> CrossCurveScalar {
    CrossCurveScalar::from_monero_little_endian(bytes).expect("vector scalar")
}

fn prove(scalar_le: [u8; 32], seed: [u8; 32]) -> CrossCurveDleqProofV1 {
    CrossCurveDleqProofV1::prove(&scalar(scalar_le), &mut ChaCha20Rng::from_seed(seed))
        .expect("vector proof")
}

fn commitment(secp: &[u8], ed: &[u8], proof: &[u8]) -> [u8; 32] {
    Sha256::new()
        .chain_update(TRANSCRIPT_COMMITMENT_DOMAIN)
        .chain_update([1])
        .chain_update(secp)
        .chain_update(ed)
        .chain_update(u32::try_from(proof.len()).unwrap().to_le_bytes())
        .chain_update(proof)
        .finalize()
        .into()
}

fn decode_wire(bytes: &[u8]) -> Wire {
    postcard::from_bytes(bytes).expect("vector wire")
}

/// Re-encodes a wire envelope, recomputing the commitment so that only the
/// changed field can be the cause of a rejection.
fn reseal(mut wire: Wire) -> Vec<u8> {
    wire.4 = commitment(&wire.1, &wire.2, &wire.3);
    postcard::to_allocvec(&wire).unwrap()
}

/// Independent secp256k1 public key: libsecp256k1 on the big-endian scalar.
fn libsecp256k1_public_key(scalar_le: [u8; 32]) -> [u8; 33] {
    let mut big_endian = scalar_le;
    big_endian.reverse();
    let secret = secp256k1::SecretKey::from_slice(&big_endian).expect("secp256k1 scalar");
    secp256k1::PublicKey::from_secret_key(&secp256k1::Secp256k1::new(), &secret).serialize()
}

/// Independent Ed25519 public key: the `monero` crate's own key derivation.
fn monero_crate_public_key(scalar_le: [u8; 32]) -> [u8; 32] {
    MoneroPublicKey::from_private_key(&PrivateKey::from_slice(&scalar_le).expect("ed25519 scalar"))
        .to_bytes()
}

#[test]
fn dleq_vectors_reproduce_and_verify_from_fixed_bytes() {
    let vectors = load_vectors();
    for name in DLEQ_VECTORS {
        let vector = &vectors[&format!("dleq {name}")];
        let scalar_le = field::<32>(vector, "scalar_le");
        let secp = field::<33>(vector, "secp256k1_public_key");
        let ed = field::<32>(vector, "ed25519_public_key");
        let fixed_wire = wire_bytes(name);
        assert_eq!(
            hex::encode(Sha256::digest(&fixed_wire)),
            vector["wire_sha256"],
            "{name}: wire file digest"
        );

        // The verifier accepts the committed bytes without running the prover.
        let parsed = CrossCurveDleqProofV1::from_wire_bytes(&fixed_wire).expect("fixed wire");
        assert_eq!(parsed.secp256k1_public_key(), secp, "{name}");
        assert_eq!(parsed.ed25519_public_key(), ed, "{name}");
        assert_eq!(
            parsed.transcript_commitment(),
            field::<32>(vector, "transcript_commitment"),
            "{name}"
        );
        parsed
            .verify_scalar(&scalar(scalar_le))
            .expect("vector scalar opens both points");

        // The pinned prover reproduces the same bytes from the same inputs.
        let reproduced = prove(scalar_le, field::<32>(vector, "rng_seed"));
        assert_eq!(
            reproduced.to_wire_bytes().unwrap(),
            fixed_wire,
            "{name}: prover drift"
        );

        // Independent paths: other libraries, same scalar, same public keys,
        // and the commitment recomputed from its documented layout.
        assert_eq!(
            libsecp256k1_public_key(scalar_le),
            secp,
            "{name}: secp256k1"
        );
        assert_eq!(monero_crate_public_key(scalar_le), ed, "{name}: Ed25519");
        assert_eq!(
            commitment(&secp, &ed, parsed.proof_bytes()),
            parsed.transcript_commitment(),
            "{name}: commitment layout"
        );
    }
}

#[test]
fn recovery_vector_reconstructs_the_same_key_on_both_paths() {
    let vectors = load_vectors();
    let recovery = &vectors["recovery"];
    let maker_le = field::<32>(recovery, "maker_scalar_le");
    let taker_le = field::<32>(recovery, "taker_scalar_le");
    let view_le = field::<32>(recovery, "view_key_le");
    let maker_proof = prove(maker_le, field::<32>(recovery, "maker_rng_seed"));
    let taker_proof = prove(taker_le, field::<32>(recovery, "taker_rng_seed"));
    let view_key = MoneroPrivateViewKey::from_monero_little_endian(view_le).unwrap();
    let address = MoneroSharedAddressV1::derive(
        MoneroAddressNetworkV1::Regtest,
        &maker_proof,
        &taker_proof,
        &view_key,
    )
    .unwrap();
    let expected_spend = field::<32>(recovery, "shared_public_spend_key");
    let expected_private = field::<32>(recovery, "reconstructed_spend_key_le");
    assert_eq!(address.public_spend_key(), expected_spend);
    assert_eq!(address.address_string(), recovery["regtest_address"]);

    // Claim path: the Taker learns the Maker's share from the adaptor.
    let claim = ReconstructedMoneroSpendKey::reconstruct(
        &address,
        &maker_proof,
        scalar(taker_le),
        scalar(maker_le).adaptor_scalar_big_endian(),
    )
    .unwrap();
    // Refund path: the Maker learns the Taker's share.
    let refund = ReconstructedMoneroSpendKey::reconstruct(
        &address,
        &taker_proof,
        scalar(maker_le),
        scalar(taker_le).adaptor_scalar_big_endian(),
    )
    .unwrap();
    assert_eq!(claim.public_key(), expected_spend);
    assert_eq!(refund.public_key(), expected_spend);
    assert_eq!(*claim.into_monero_little_endian(), expected_private);
    assert_eq!(*refund.into_monero_little_endian(), expected_private);

    // Independent path: the `monero` crate adds the private shares and derives
    // the address from private keys, never from the summed public points.
    let spend =
        PrivateKey::from_slice(&maker_le).unwrap() + PrivateKey::from_slice(&taker_le).unwrap();
    assert_eq!(spend.to_bytes(), expected_private);
    assert_eq!(
        MoneroPublicKey::from_private_key(&spend).to_bytes(),
        expected_spend
    );
    let keys = KeyPair {
        view: PrivateKey::from_slice(&view_le).unwrap(),
        spend,
    };
    assert_eq!(
        Address::from_keypair(Network::Mainnet, &keys).to_string(),
        recovery["regtest_address"]
    );
}

#[test]
fn tampered_proof_and_commitment_are_rejected() {
    let fixed = wire_bytes("mixed");

    let mut swapped = decode_wire(&fixed);
    let mut proof: CrossCurveDLEQProof = postcard::from_bytes(&swapped.3).unwrap();
    proof.commitments.swap(0, 1);
    swapped.3 = postcard::to_allocvec(&proof).unwrap();
    assert_eq!(
        CrossCurveDleqProofV1::from_wire_bytes(&reseal(swapped)).unwrap_err(),
        CrossCurveDleqError::InvalidProof
    );

    let mut blinding = decode_wire(&fixed);
    let mut proof: CrossCurveDLEQProof = postcard::from_bytes(&blinding.3).unwrap();
    proof.sum_blindings.1 += sigma_fun::ed25519::curve25519_dalek::scalar::Scalar::one();
    blinding.3 = postcard::to_allocvec(&proof).unwrap();
    assert_eq!(
        CrossCurveDleqProofV1::from_wire_bytes(&reseal(blinding)).unwrap_err(),
        CrossCurveDleqError::InvalidProof
    );

    let mut stale = decode_wire(&fixed);
    stale.4[0] ^= 1;
    assert_eq!(
        CrossCurveDleqProofV1::from_wire_bytes(&postcard::to_allocvec(&stale).unwrap())
            .unwrap_err(),
        CrossCurveDleqError::TranscriptCommitmentMismatch
    );

    // A flipped proof byte under the old commitment never verifies.
    let mut flipped = decode_wire(&fixed);
    let middle = flipped.3.len() / 2;
    flipped.3[middle] ^= 0x40;
    assert!(
        CrossCurveDleqProofV1::from_wire_bytes(&postcard::to_allocvec(&flipped).unwrap()).is_err()
    );
}

#[test]
fn wrong_point_on_either_curve_is_rejected() {
    let fixed = wire_bytes("mixed");
    let other = decode_wire(&wire_bytes("max"));

    let mut secp = decode_wire(&fixed);
    secp.1.clone_from(&other.1);
    assert_eq!(
        CrossCurveDleqProofV1::from_wire_bytes(&reseal(secp)).unwrap_err(),
        CrossCurveDleqError::InvalidProof
    );
    let mut secp_malformed = decode_wire(&fixed);
    secp_malformed.1[0] = 0x05;
    assert_eq!(
        CrossCurveDleqProofV1::from_wire_bytes(&reseal(secp_malformed)).unwrap_err(),
        CrossCurveDleqError::InvalidSecp256k1Point
    );

    let mut ed = decode_wire(&fixed);
    ed.2.clone_from(&other.2);
    assert_eq!(
        CrossCurveDleqProofV1::from_wire_bytes(&reseal(ed)).unwrap_err(),
        CrossCurveDleqError::InvalidProof
    );
    let mut identity = [0_u8; 32];
    identity[0] = 1;
    // y = 0 decodes to a point of order 4.
    let order_four = [0_u8; 32];
    let not_on_curve = (2_u8..=u8::MAX)
        .map(|y| {
            let mut bytes = [0_u8; 32];
            bytes[0] = y;
            bytes
        })
        .find(|bytes| CompressedEdwardsY(*bytes).decompress().is_none())
        .unwrap();
    for bad in [identity, order_four, not_on_curve] {
        let mut ed_bad = decode_wire(&fixed);
        ed_bad.2 = bad.to_vec();
        assert_eq!(
            CrossCurveDleqProofV1::from_wire_bytes(&reseal(ed_bad)).unwrap_err(),
            CrossCurveDleqError::InvalidEd25519Point,
            "{}",
            hex::encode(bad)
        );
    }
}

#[test]
fn share_that_does_not_match_is_rejected() {
    let vectors = load_vectors();
    let recovery = &vectors["recovery"];
    let maker_le = field::<32>(recovery, "maker_scalar_le");
    let taker_le = field::<32>(recovery, "taker_scalar_le");
    let maker_proof = prove(maker_le, field::<32>(recovery, "maker_rng_seed"));
    let taker_proof = prove(taker_le, field::<32>(recovery, "taker_rng_seed"));
    let view_key =
        MoneroPrivateViewKey::from_monero_little_endian(field::<32>(recovery, "view_key_le"))
            .unwrap();
    let address = MoneroSharedAddressV1::derive(
        MoneroAddressNetworkV1::Regtest,
        &maker_proof,
        &taker_proof,
        &view_key,
    )
    .unwrap();

    assert_eq!(
        maker_proof.verify_scalar(&scalar(taker_le)).unwrap_err(),
        CrossCurveDleqError::AdaptorPointMismatch
    );
    // The Taker's own share offered as the Maker's revealed one.
    assert_eq!(
        ReconstructedMoneroSpendKey::reconstruct(
            &address,
            &maker_proof,
            scalar(taker_le),
            scalar(taker_le).adaptor_scalar_big_endian(),
        )
        .unwrap_err(),
        MoneroSharedSpendError::CrossCurve(CrossCurveDleqError::AdaptorPointMismatch)
    );
    // The right revealed share with a wrong retained share.
    assert_eq!(
        ReconstructedMoneroSpendKey::reconstruct(
            &address,
            &maker_proof,
            scalar(maker_le),
            scalar(maker_le).adaptor_scalar_big_endian(),
        )
        .unwrap_err(),
        MoneroSharedSpendError::ReconstructedSpendKeyMismatch
    );
    // An extracted scalar of 2^252 is outside the common representation.
    let mut wide = [0_u8; 32];
    wide[0] = 0x10;
    assert_eq!(
        ReconstructedMoneroSpendKey::reconstruct(
            &address,
            &maker_proof,
            scalar(taker_le),
            Zeroizing::new(wide),
        )
        .unwrap_err(),
        MoneroSharedSpendError::CrossCurve(CrossCurveDleqError::InvalidScalar)
    );
    let mut wide_le = [0_u8; 32];
    wide_le[31] = 0x10;
    for bad in [[0_u8; 32], wide_le] {
        assert_eq!(
            CrossCurveScalar::from_monero_little_endian(bad).unwrap_err(),
            CrossCurveDleqError::InvalidScalar
        );
    }
}

#[test]
fn truncated_and_over_long_encodings_are_rejected() {
    let fixed = wire_bytes("one");

    assert_eq!(
        CrossCurveDleqProofV1::from_wire_bytes(&[]).unwrap_err(),
        CrossCurveDleqError::ProofTooLarge
    );
    for length in [1, fixed.len() / 2, fixed.len() - 1] {
        assert_eq!(
            CrossCurveDleqProofV1::from_wire_bytes(&fixed[..length]).unwrap_err(),
            CrossCurveDleqError::ProofWireEncoding,
            "truncated to {length}"
        );
    }
    let mut trailing = fixed.clone();
    trailing.push(0);
    assert_eq!(
        CrossCurveDleqProofV1::from_wire_bytes(&trailing).unwrap_err(),
        CrossCurveDleqError::NonCanonicalProofWire
    );
    let mut oversized = fixed.clone();
    oversized.resize(128 * 1024 + 257, 0);
    assert_eq!(
        CrossCurveDleqProofV1::from_wire_bytes(&oversized).unwrap_err(),
        CrossCurveDleqError::ProofTooLarge
    );

    let mut long_secp = decode_wire(&fixed);
    long_secp.1.push(0);
    let mut short_ed = decode_wire(&fixed);
    short_ed.2.pop();
    for wire in [long_secp, short_ed] {
        assert_eq!(
            CrossCurveDleqProofV1::from_wire_bytes(&reseal(wire)).unwrap_err(),
            CrossCurveDleqError::ProofWireEncoding
        );
    }
    let mut schema = decode_wire(&fixed);
    schema.0 = 2;
    assert_eq!(
        CrossCurveDleqProofV1::from_wire_bytes(&reseal(schema)).unwrap_err(),
        CrossCurveDleqError::UnsupportedSchema
    );
}

#[test]
fn trailing_proof_bytes_are_rejected() {
    let mut proof_trailing = decode_wire(&wire_bytes("one"));
    proof_trailing.3.push(0);
    assert_eq!(
        CrossCurveDleqProofV1::from_wire_bytes(&reseal(proof_trailing)).unwrap_err(),
        CrossCurveDleqError::ProofSerialization
    );
}

/// Writes the self-generated vectors. Run only to replace them on purpose:
/// `LEZ_XMR_WRITE_VECTORS=1 cargo test -p lez-xmr-swap-sdk --test cross_curve_vectors -- --ignored`
#[test]
#[ignore = "rewrites tests/vectors; run only to regenerate the self-generated vectors"]
fn regenerate_self_generated_vectors() {
    use std::fmt::Write as _;

    assert_eq!(
        std::env::var("LEZ_XMR_WRITE_VECTORS").as_deref(),
        Ok("1"),
        "set LEZ_XMR_WRITE_VECTORS=1 to overwrite the committed vectors"
    );
    let mut one = [0_u8; 32];
    one[0] = 1;
    let mut max = [0xff_u8; 32];
    max[31] = 0x0f;
    let mixed = fixed_scalar(b"mixed");
    let inputs = [
        ("one", one, [0x53; 32]),
        ("max", max, [0x5a; 32]),
        ("mixed", mixed, [0x5b; 32]),
    ];

    let mut text = String::from(
        "# SELF-GENERATED regression vectors, NOT third-party vectors.\n\
         # No published fixed vectors exist for this construction (sigma_fun 0.9.0,\n\
         # COMIT cross-curve-dleq and xmr-btc-swap test only with random inputs).\n\
         # Produced once from the deterministic inputs below by the pinned sigma_fun\n\
         # 0.9.0 / musig2 0.4.1 / monero 0.22.0 through this crate; see\n\
         # tests/cross_curve_vectors.rs for how to regenerate and what is checked\n\
         # independently. Scalars are Monero little-endian; wire bytes are in\n\
         # cross_curve_dleq_v1_<name>.bin.\n",
    );
    for (name, scalar_le, seed) in inputs {
        let proof = prove(scalar_le, seed);
        let wire = proof.to_wire_bytes().unwrap();
        fs::write(
            vectors_dir().join(format!("cross_curve_dleq_v1_{name}.bin")),
            &wire,
        )
        .unwrap();
        write!(
            text,
            "\n[dleq {name}]\nscalar_le={}\nrng_seed={}\nsecp256k1_public_key={}\n\
             ed25519_public_key={}\ntranscript_commitment={}\nwire_sha256={}\n",
            hex::encode(scalar_le),
            hex::encode(seed),
            hex::encode(proof.secp256k1_public_key()),
            hex::encode(proof.ed25519_public_key()),
            hex::encode(proof.transcript_commitment()),
            hex::encode(Sha256::digest(&wire)),
        )
        .unwrap();
    }

    let maker_le = mixed;
    let taker_le = fixed_scalar(b"taker");
    let view_le = fixed_scalar(b"view");
    let (maker_seed, taker_seed) = ([0x5b; 32], [0x5c; 32]);
    let maker_proof = prove(maker_le, maker_seed);
    let taker_proof = prove(taker_le, taker_seed);
    let address = MoneroSharedAddressV1::derive(
        MoneroAddressNetworkV1::Regtest,
        &maker_proof,
        &taker_proof,
        &MoneroPrivateViewKey::from_monero_little_endian(view_le).unwrap(),
    )
    .unwrap();
    let reconstructed = ReconstructedMoneroSpendKey::reconstruct(
        &address,
        &maker_proof,
        scalar(taker_le),
        scalar(maker_le).adaptor_scalar_big_endian(),
    )
    .unwrap();
    write!(
        text,
        "\n[recovery]\nmaker_scalar_le={}\nmaker_rng_seed={}\ntaker_scalar_le={}\n\
         taker_rng_seed={}\nview_key_le={}\nshared_public_spend_key={}\n\
         reconstructed_spend_key_le={}\nregtest_address={}\n",
        hex::encode(maker_le),
        hex::encode(maker_seed),
        hex::encode(taker_le),
        hex::encode(taker_seed),
        hex::encode(view_le),
        hex::encode(address.public_spend_key()),
        hex::encode(*reconstructed.into_monero_little_endian()),
        address.address_string(),
    )
    .unwrap();
    fs::write(vectors_dir().join(VECTORS_FILE), text).unwrap();
}

/// SHA-256 of a label with the top four bits cleared: a fixed scalar below 2^252.
fn fixed_scalar(label: &[u8]) -> [u8; 32] {
    let mut bytes: [u8; 32] = Sha256::new()
        .chain_update(b"lez-atomic-swaps/xmr/cross-curve-dleq-vector/v1/")
        .chain_update(label)
        .finalize()
        .into();
    bytes[31] &= 0x0f;
    bytes
}
