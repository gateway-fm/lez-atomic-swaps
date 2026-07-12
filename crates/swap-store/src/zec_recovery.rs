//! Role-fixed `SQLite` implementation of the concrete ZEC SDK recovery port.

use std::{
    path::Path,
    sync::{Arc, Mutex, MutexGuard},
};

use async_trait::async_trait;
use lez_swap_core::{Participant, SwapId, UnixSeconds};
use lez_zec_swap_sdk::{
    AcceptedZecAgreementEnvelopeV1, AcceptedZecAgreementV1, CreateAgreementOutcome,
    CreateFirstLockOutcome, FIRST_LOCK_RECORD_SCHEMA_V1, FirstLockIntentRecordV1,
    FirstLockIntentV1, FirstLockProjectionCommit, FirstLockTransitionRecordV1,
    FirstLockTransitionV1, RecoveryStore,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::{StoreError, open_configured_connection, participant_name, revision_from_sql};

const AGREEMENT_PAYLOAD_VERSION: i64 = 1;

/// Cloneable, role-fixed SDK recovery repository.
///
/// Clones share one configured `SQLite` connection. The local participant is
/// fixed when the adapter opens and is included in every composite key.
#[derive(Clone, Debug)]
pub struct SqliteZecRecoveryStore {
    local_participant: Participant,
    connection: Arc<Mutex<Connection>>,
}

impl SqliteZecRecoveryStore {
    /// Opens or creates a schema-v5 recovery store for one local role.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError`] when `SQLite` cannot open, configure, or migrate
    /// the database.
    pub fn open(
        path: impl AsRef<Path>,
        local_participant: Participant,
    ) -> Result<Self, StoreError> {
        Ok(Self {
            local_participant,
            connection: Arc::new(Mutex::new(open_configured_connection(path)?)),
        })
    }

    /// Participant fixed for every operation on this adapter.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.local_participant
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>, StoreError> {
        self.connection
            .lock()
            .map_err(|_| StoreError::ZecRecoveryLockPoisoned)
    }

    fn role_name(&self) -> &'static str {
        participant_name(self.local_participant)
    }

    fn require_role(&self, actual: Participant) -> Result<(), StoreError> {
        if actual == self.local_participant {
            Ok(())
        } else {
            Err(StoreError::ZecRecoveryRoleMismatch)
        }
    }
}

#[async_trait]
impl RecoveryStore for SqliteZecRecoveryStore {
    type Error = StoreError;

    async fn create_agreement(
        &self,
        envelope: &AcceptedZecAgreementEnvelopeV1,
    ) -> Result<CreateAgreementOutcome, Self::Error> {
        let accepted = AcceptedZecAgreementV1::resume(envelope)?;
        self.require_role(accepted.local_participant())?;
        let swap_id = accepted.agreement().coordinator().id();
        let accepted_at = sql_u64(envelope.accepted_at().value())?;
        let accepted_revision = sql_u64(envelope.revision())?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = load_agreement_row(&transaction, self.role_name(), swap_id)?;
        let outcome = match existing {
            None => {
                transaction.execute(
                    "
                    INSERT INTO zec_sdk_agreements (
                        local_role, swap_id, payload_version, agreement_wire,
                        accepted_at, accepted_revision, active_revision
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
                    ",
                    params![
                        self.role_name(),
                        swap_id.as_str(),
                        AGREEMENT_PAYLOAD_VERSION,
                        envelope.agreement_wire(),
                        accepted_at,
                        accepted_revision
                    ],
                )?;
                CreateAgreementOutcome::Created
            }
            Some(row) => {
                require_payload(
                    "SDK agreement",
                    row.payload_version,
                    AGREEMENT_PAYLOAD_VERSION,
                )?;
                let _ = validated_agreement(&row, self.local_participant, swap_id)?;
                if row.agreement_wire == envelope.agreement_wire()
                    && row.accepted_at == accepted_at
                    && row.accepted_revision == accepted_revision
                {
                    CreateAgreementOutcome::ExistingSame
                } else {
                    CreateAgreementOutcome::Conflict
                }
            }
        };
        transaction.commit()?;
        Ok(outcome)
    }

    async fn load_agreement(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<AcceptedZecAgreementEnvelopeV1>, Self::Error> {
        let connection = self.connection()?;
        load_agreement_row(&connection, self.role_name(), swap_id)?
            .map(|row| validated_agreement(&row, self.local_participant, swap_id))
            .transpose()?
            .map(|accepted| accepted.durable_envelope().map_err(StoreError::from))
            .transpose()
    }

    async fn create_first_lock_intent(
        &self,
        intent: &FirstLockIntentV1,
    ) -> Result<CreateFirstLockOutcome, Self::Error> {
        self.require_role(intent.local_participant())?;
        let record = FirstLockIntentRecordV1::from(intent);
        let payload_version = i64::from(record.schema_version());
        let payload_json = serde_json::to_string(&record)?;
        let predecessor = sql_u64(intent.predecessor_revision())?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let agreement_row = load_agreement_row(&transaction, self.role_name(), intent.swap_id())?
            .ok_or(StoreError::MissingZecRecoveryAgreement)?;
        let active_revision = revision_from_sql(agreement_row.active_revision)?;
        let accepted =
            validated_agreement(&agreement_row, self.local_participant, intent.swap_id())?;
        let trusted = record.revalidate(&accepted, active_revision)?;
        if &trusted != intent {
            return Err(StoreError::InvalidZecRecoveryState);
        }

        let existing = load_intent_row(&transaction, self.role_name(), intent.swap_id())?;
        let outcome = match existing {
            None => {
                transaction.execute(
                    "
                    INSERT INTO zec_sdk_first_lock_intents (
                        local_role, swap_id, predecessor_revision,
                        payload_version, payload_json, closed_revision
                    ) VALUES (?1, ?2, ?3, ?4, ?5, NULL)
                    ",
                    params![
                        self.role_name(),
                        intent.swap_id().as_str(),
                        predecessor,
                        payload_version,
                        payload_json
                    ],
                )?;
                CreateFirstLockOutcome::Created
            }
            Some(row)
                if row.closed_revision.is_none()
                    && row.predecessor_revision == predecessor
                    && row.payload_version == payload_version
                    && row.payload_json == payload_json =>
            {
                CreateFirstLockOutcome::ExistingSame
            }
            Some(_) => CreateFirstLockOutcome::Conflict,
        };
        transaction.commit()?;
        Ok(outcome)
    }

    async fn load_first_lock_intent(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<FirstLockIntentV1>, Self::Error> {
        let connection = self.connection()?;
        let Some(intent_row) = load_intent_row(&connection, self.role_name(), swap_id)? else {
            return Ok(None);
        };
        if intent_row.closed_revision.is_some() {
            return Ok(None);
        }
        let agreement_row = load_agreement_row(&connection, self.role_name(), swap_id)?
            .ok_or(StoreError::MissingZecRecoveryAgreement)?;
        let active_revision = revision_from_sql(agreement_row.active_revision)?;
        let accepted = validated_agreement(&agreement_row, self.local_participant, swap_id)?;
        let record = decode_intent_record(intent_row.payload_version, &intent_row.payload_json)?;
        Ok(Some(record.revalidate(&accepted, active_revision)?))
    }

    async fn commit_first_lock_transition(
        &self,
        transition: &FirstLockTransitionV1,
    ) -> Result<FirstLockProjectionCommit, Self::Error> {
        let record = FirstLockTransitionRecordV1::from(transition);
        let payload_version = i64::from(record.schema_version());
        let payload_json = serde_json::to_string(&record)?;
        let predecessor = transition.predecessor_revision();
        let committed = predecessor
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        let predecessor_sql = sql_u64(predecessor)?;
        let committed_sql = sql_u64(committed)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let agreement_row =
            load_agreement_row(&transaction, self.role_name(), transition.swap_id())?
                .ok_or(StoreError::MissingZecRecoveryAgreement)?;
        let active_revision = revision_from_sql(agreement_row.active_revision)?;
        let accepted =
            validated_agreement(&agreement_row, self.local_participant, transition.swap_id())?;
        let intent_row = load_intent_row(&transaction, self.role_name(), transition.swap_id())?
            .ok_or(StoreError::MissingZecFirstLockIntent)?;
        let intent_record =
            decode_intent_record(intent_row.payload_version, &intent_row.payload_json)?;
        let trusted = record.revalidate(&accepted, &intent_record, predecessor)?;
        if &trusted != transition {
            return Err(StoreError::InvalidZecRecoveryState);
        }

        if let Some(existing) = load_transition_row(
            &transaction,
            self.role_name(),
            transition.swap_id(),
            predecessor_sql,
        )? {
            let replay = validate_transition_replay(
                &existing,
                &intent_row,
                &accepted,
                transition,
                active_revision,
                committed,
            )?;
            transaction.commit()?;
            return Ok(replay);
        }
        if active_revision != predecessor
            || intent_row.predecessor_revision != predecessor_sql
            || intent_row.closed_revision.is_some()
        {
            return Err(StoreError::InvalidZecRecoveryState);
        }

        transaction.execute(
            "
            INSERT INTO zec_sdk_first_lock_transitions (
                local_role, swap_id, predecessor_revision, committed_revision,
                payload_version, payload_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ",
            params![
                self.role_name(),
                transition.swap_id().as_str(),
                predecessor_sql,
                committed_sql,
                payload_version,
                payload_json
            ],
        )?;
        let agreement_updates = transaction.execute(
            "
            UPDATE zec_sdk_agreements
            SET active_revision = ?1
            WHERE local_role = ?2 AND swap_id = ?3 AND active_revision = ?4
            ",
            params![
                committed_sql,
                self.role_name(),
                transition.swap_id().as_str(),
                predecessor_sql
            ],
        )?;
        let intent_updates = transaction.execute(
            "
            UPDATE zec_sdk_first_lock_intents
            SET closed_revision = ?1
            WHERE local_role = ?2 AND swap_id = ?3
              AND predecessor_revision = ?4 AND closed_revision IS NULL
            ",
            params![
                committed_sql,
                self.role_name(),
                transition.swap_id().as_str(),
                predecessor_sql
            ],
        )?;
        if agreement_updates != 1 || intent_updates != 1 {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        transaction.commit()?;
        Ok(FirstLockProjectionCommit::new(committed, false))
    }

    async fn load_first_lock_transition(
        &self,
        swap_id: &SwapId,
        predecessor_revision: u64,
    ) -> Result<Option<FirstLockTransitionV1>, Self::Error> {
        let connection = self.connection()?;
        let predecessor_sql = sql_u64(predecessor_revision)?;
        let agreement_row = load_agreement_row(&connection, self.role_name(), swap_id)?
            .ok_or(StoreError::MissingZecRecoveryAgreement)?;
        let active_revision = revision_from_sql(agreement_row.active_revision)?;
        let accepted = validated_agreement(&agreement_row, self.local_participant, swap_id)?;
        let Some(transition_row) =
            load_transition_row(&connection, self.role_name(), swap_id, predecessor_sql)?
        else {
            if active_revision != predecessor_revision {
                return Err(StoreError::InvalidZecRecoveryState);
            }
            if let Some(intent_row) = load_intent_row(&connection, self.role_name(), swap_id)? {
                if intent_row.predecessor_revision != predecessor_sql
                    || intent_row.closed_revision.is_some()
                {
                    return Err(StoreError::InvalidZecRecoveryState);
                }
                let intent_record =
                    decode_intent_record(intent_row.payload_version, &intent_row.payload_json)?;
                let _ = intent_record.revalidate(&accepted, predecessor_revision)?;
            }
            return Ok(None);
        };
        let intent_row = load_intent_row(&connection, self.role_name(), swap_id)?
            .ok_or(StoreError::MissingZecFirstLockIntent)?;
        let expected_committed = predecessor_sql
            .checked_add(1)
            .ok_or(StoreError::RevisionOverflow)?;
        if transition_row.committed_revision != expected_committed
            || intent_row.predecessor_revision != predecessor_sql
            || intent_row.closed_revision != Some(expected_committed)
            || agreement_row.active_revision != expected_committed
        {
            return Err(StoreError::InvalidZecRecoveryState);
        }
        let intent_record =
            decode_intent_record(intent_row.payload_version, &intent_row.payload_json)?;
        let transition_record =
            decode_transition_record(transition_row.payload_version, &transition_row.payload_json)?;
        Ok(Some(transition_record.revalidate(
            &accepted,
            &intent_record,
            predecessor_revision,
        )?))
    }
}

#[derive(Debug)]
struct AgreementRow {
    payload_version: i64,
    agreement_wire: Vec<u8>,
    accepted_at: i64,
    accepted_revision: i64,
    active_revision: i64,
}

#[derive(Debug)]
struct IntentRow {
    predecessor_revision: i64,
    payload_version: i64,
    payload_json: String,
    closed_revision: Option<i64>,
}

#[derive(Debug)]
struct TransitionRow {
    committed_revision: i64,
    payload_version: i64,
    payload_json: String,
}

fn load_agreement_row(
    connection: &Connection,
    role: &str,
    swap_id: &SwapId,
) -> Result<Option<AgreementRow>, StoreError> {
    connection
        .query_row(
            "
            SELECT payload_version, agreement_wire, accepted_at,
                   accepted_revision, active_revision
            FROM zec_sdk_agreements
            WHERE local_role = ?1 AND swap_id = ?2
            ",
            params![role, swap_id.as_str()],
            |row| {
                Ok(AgreementRow {
                    payload_version: row.get(0)?,
                    agreement_wire: row.get(1)?,
                    accepted_at: row.get(2)?,
                    accepted_revision: row.get(3)?,
                    active_revision: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::from)
}

fn load_intent_row(
    connection: &Connection,
    role: &str,
    swap_id: &SwapId,
) -> Result<Option<IntentRow>, StoreError> {
    connection
        .query_row(
            "
            SELECT predecessor_revision, payload_version, payload_json, closed_revision
            FROM zec_sdk_first_lock_intents
            WHERE local_role = ?1 AND swap_id = ?2
            ",
            params![role, swap_id.as_str()],
            |row| {
                Ok(IntentRow {
                    predecessor_revision: row.get(0)?,
                    payload_version: row.get(1)?,
                    payload_json: row.get(2)?,
                    closed_revision: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::from)
}

fn load_transition_row(
    connection: &Connection,
    role: &str,
    swap_id: &SwapId,
    predecessor_revision: i64,
) -> Result<Option<TransitionRow>, StoreError> {
    connection
        .query_row(
            "
            SELECT committed_revision, payload_version, payload_json
            FROM zec_sdk_first_lock_transitions
            WHERE local_role = ?1 AND swap_id = ?2 AND predecessor_revision = ?3
            ",
            params![role, swap_id.as_str(), predecessor_revision],
            |row| {
                Ok(TransitionRow {
                    committed_revision: row.get(0)?,
                    payload_version: row.get(1)?,
                    payload_json: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::from)
}

fn validated_agreement(
    row: &AgreementRow,
    local_participant: Participant,
    requested: &SwapId,
) -> Result<AcceptedZecAgreementV1, StoreError> {
    require_payload(
        "SDK agreement",
        row.payload_version,
        AGREEMENT_PAYLOAD_VERSION,
    )?;
    let maximum_active = row
        .accepted_revision
        .checked_add(1)
        .ok_or(StoreError::RevisionOverflow)?;
    if row.active_revision < row.accepted_revision || row.active_revision > maximum_active {
        return Err(StoreError::InvalidZecRecoveryState);
    }
    let accepted_at = UnixSeconds::new(revision_from_sql(row.accepted_at)?);
    let accepted_revision = revision_from_sql(row.accepted_revision)?;
    let accepted = AcceptedZecAgreementV1::resume_from_durable_parts(
        &row.agreement_wire,
        accepted_at,
        local_participant,
        accepted_revision,
    )?;
    if accepted.agreement().coordinator().id() != requested {
        return Err(StoreError::InvalidZecRecoveryState);
    }
    Ok(accepted)
}

fn decode_intent_record(
    payload_version: i64,
    payload_json: &str,
) -> Result<FirstLockIntentRecordV1, StoreError> {
    require_payload(
        "SDK first-lock intent",
        payload_version,
        i64::from(FIRST_LOCK_RECORD_SCHEMA_V1),
    )?;
    serde_json::from_str(payload_json).map_err(StoreError::from)
}

fn decode_transition_record(
    payload_version: i64,
    payload_json: &str,
) -> Result<FirstLockTransitionRecordV1, StoreError> {
    require_payload(
        "SDK first-lock transition",
        payload_version,
        i64::from(FIRST_LOCK_RECORD_SCHEMA_V1),
    )?;
    serde_json::from_str(payload_json).map_err(StoreError::from)
}

fn validate_transition_replay(
    row: &TransitionRow,
    intent_row: &IntentRow,
    accepted: &AcceptedZecAgreementV1,
    expected: &FirstLockTransitionV1,
    active_revision: u64,
    committed_revision: u64,
) -> Result<FirstLockProjectionCommit, StoreError> {
    let committed_sql = sql_u64(committed_revision)?;
    if row.committed_revision != committed_sql
        || intent_row.closed_revision != Some(committed_sql)
        || active_revision != committed_revision
    {
        return Err(StoreError::InvalidZecRecoveryState);
    }
    let intent_record = decode_intent_record(intent_row.payload_version, &intent_row.payload_json)?;
    let transition_record = decode_transition_record(row.payload_version, &row.payload_json)?;
    let trusted =
        transition_record.revalidate(accepted, &intent_record, expected.predecessor_revision())?;
    if &trusted != expected {
        return Err(StoreError::ConflictingZecFirstLockTransition);
    }
    Ok(FirstLockProjectionCommit::new(committed_revision, true))
}

fn require_payload(kind: &'static str, version: i64, expected: i64) -> Result<(), StoreError> {
    if version == expected {
        Ok(())
    } else {
        Err(StoreError::UnsupportedPayloadVersion { kind, version })
    }
}

fn sql_u64(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::RevisionOverflow)
}
