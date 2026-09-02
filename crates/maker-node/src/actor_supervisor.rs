//! Pair-specific validation for pair-neutral maker actor scheduling.

mod runtime;

pub use runtime::{
    MakerActorSupervisorCancellation, MakerActorSupervisorConfig, MakerActorSupervisorError,
    MakerActorSupervisorOutcome, MakerActorSupervisorResolution,
    supervise_one_abandoned_maker_actor, supervise_one_abandoned_maker_actor_until,
    supervise_one_due_maker_actor, supervise_one_due_maker_actor_until,
};

use btc_reference_actor::validate_maker_manifest_config_bytes as validate_btc_config;
use lez_swap_store::{
    MakerActorArtifacts, MakerActorKindV1, MakerActorProcessError, MakerActorProcessRecordV1,
};
#[cfg(feature = "pair-xmr")]
use xmr_reference_actor::validate_maker_manifest_config_bytes as validate_xmr_config;
#[cfg(feature = "pair-zec")]
use zec_reference_actor::validate_maker_manifest_config_bytes as validate_zec_config;

/// Prepares one exact deployment after pair-specific manifest validation.
///
/// The semantic validator receives the same hash-verified config bytes that
/// [`MakerActorArtifacts`] seals into child FD 196. Deployment-path replacement
/// therefore cannot make validation and execution observe different configs.
///
/// # Errors
///
/// Rejects unsafe/mismatched deployment artifacts, any non-Maker role, a
/// different application swap or role-state database, or a BTC config that
/// lacks the supervised schema-6 agreement binding.
pub fn prepare_maker_actor(
    record: &MakerActorProcessRecordV1,
) -> Result<MakerActorArtifacts, MakerActorProcessError> {
    let manifest = record.manifest();
    MakerActorArtifacts::open_validated(record, |config| match manifest.kind() {
        MakerActorKindV1::Bitcoin => {
            validate_btc_config(config, manifest.swap_id(), manifest.state_database_path())
                .map_err(|_| ())
        }
        #[cfg(feature = "pair-xmr")]
        MakerActorKindV1::Monero => {
            let mut expected_swap_id = [0_u8; 32];
            hex::decode_to_slice(manifest.swap_id().as_str(), &mut expected_swap_id)
                .map_err(|_| ())?;
            validate_xmr_config(config, expected_swap_id, manifest.state_database_path())
                .map_err(|_| ())
        }
        #[cfg(feature = "pair-zec")]
        MakerActorKindV1::Zcash => {
            validate_zec_config(config, manifest.swap_id(), manifest.state_database_path())
                .map_err(|_| ())
        }
        // Pairs that are not compiled into this build never reach a child process.
        #[cfg(not(all(feature = "pair-xmr", feature = "pair-zec")))]
        _ => Err(()),
    })
}
