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
        MakerActorKindV1::Monero => Err(()),
        MakerActorKindV1::Zcash => {
            validate_zec_config(config, manifest.swap_id(), manifest.state_database_path())
                .map_err(|_| ())
        }
    })
}
