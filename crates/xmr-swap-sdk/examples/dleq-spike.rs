//! Reproducible M4 `PoC` spike for the exact scalar and public-point mapping.

use lez_xmr_swap_sdk::{CrossCurveDleqProofV1, CrossCurveScalar};
use rand_chacha::ChaCha20Rng;
use rand_core::SeedableRng as _;
use sha2::{Digest as _, Sha256};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut scalar_bytes = [0_u8; 32];
    scalar_bytes[0] = 1;
    let scalar = CrossCurveScalar::from_monero_little_endian(scalar_bytes)?;
    let mut rng = ChaCha20Rng::from_seed([0x53; 32]);
    let proof = CrossCurveDleqProofV1::prove(&scalar, &mut rng)?;
    proof.verify()?;

    assert_eq!(
        hex::encode(proof.secp256k1_public_key()),
        "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
    );
    assert_eq!(
        hex::encode(proof.ed25519_public_key()),
        "5866666666666666666666666666666666666666666666666666666666666666"
    );

    let proof_sha256: [u8; 32] = Sha256::digest(proof.proof_bytes()).into();
    println!("schema_version=1");
    println!(
        "secp256k1_public_key={}",
        hex::encode(proof.secp256k1_public_key())
    );
    println!(
        "ed25519_public_key={}",
        hex::encode(proof.ed25519_public_key())
    );
    println!("proof_bytes={}", proof.proof_bytes().len());
    println!("proof_sha256={}", hex::encode(proof_sha256));
    println!(
        "transcript_commitment={}",
        hex::encode(proof.transcript_commitment())
    );
    println!("dleq_verified=true");
    println!("musig2_adaptor_point_mapping=true");
    Ok(())
}
