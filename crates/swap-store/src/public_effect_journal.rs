//! Durable one-attempt authority for exact public-chain effects.
//!
//! This journal deliberately stores only public transaction material that can
//! already be disclosed to the relevant node: exact Bitcoin or LEZ transaction
//! bytes, their SHA-256 digest, and public identifiers. It must never be used for
//! Zcash claim transactions or any other payload carrying an unrevealed preimage,
//! scalar, nonce, seed, private key, or equivalent secret-bearing material.

use std::path::Path;

use lez_swap_core::{Participant, SwapId};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest as _, Sha256};

use crate::{StoreError, open_configured_connection, participant_name};

const MAX_EXPECTED_EFFECT_ID_BYTES: usize = 512;
const MAX_EXACT_PUBLIC_BYTES: usize = 4 * 1024 * 1024;

/// Public chain on which one exact effect may be submitted.
///
/// Zcash is intentionally absent: revealing ZEC claim material belongs in the
/// protected claim journal, not this plaintext-public-material boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum PublicEffectChain {
    /// Bitcoin Core transaction submission.
    Bitcoin,
    /// Logos Execution Zone transaction submission.
    Lez,
}

impl PublicEffectChain {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Bitcoin => "bitcoin",
            Self::Lez => "lez",
        }
    }
}

/// Protocol purpose of one public-chain effect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum PublicEffectOperation {
    /// Funding or lock transaction.
    Funding,
    /// Successful claim transaction.
    Claim,
    /// Timeout/refund transaction.
    Refund,
}

impl PublicEffectOperation {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Funding => "funding",
            Self::Claim => "claim",
            Self::Refund => "refund",
        }
    }
}

/// Complete authority key for one effect at one aggregate predecessor.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct PublicEffectKey {
    swap_id: SwapId,
    local_role: Participant,
    chain: PublicEffectChain,
    operation: PublicEffectOperation,
    predecessor_revision: u64,
}

impl PublicEffectKey {
    /// Constructs a key from typed protocol identities.
    pub const fn new(
        swap_id: SwapId,
        local_role: Participant,
        chain: PublicEffectChain,
        operation: PublicEffectOperation,
        predecessor_revision: u64,
    ) -> Self {
        Self {
            swap_id,
            local_role,
            chain,
            operation,
            predecessor_revision,
        }
    }

    /// Stable swap identity.
    #[must_use]
    pub const fn swap_id(&self) -> &SwapId {
        &self.swap_id
    }

    /// Role owning this effect authority.
    #[must_use]
    pub const fn local_role(&self) -> Participant {
        self.local_role
    }

    /// Public chain receiving the exact bytes.
    pub const fn chain(&self) -> PublicEffectChain {
        self.chain
    }

    /// Protocol purpose of the effect.
    pub const fn operation(&self) -> PublicEffectOperation {
        self.operation
    }

    /// Aggregate revision that must precede this effect.
    #[must_use]
    pub const fn predecessor_revision(&self) -> u64 {
        self.predecessor_revision
    }
}

/// Immutable exact public material prepared before any send authorization.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct PreparedPublicEffect {
    key: PublicEffectKey,
    agreement_commitment: [u8; 32],
    expected_effect_id: Box<str>,
    exact_public_bytes: Box<[u8]>,
    public_bytes_sha256: [u8; 32],
}

impl PreparedPublicEffect {
    /// Validates and commits one exact public transaction candidate in memory.
    ///
    /// `exact_public_bytes` must be the complete wire payload. For Bitcoin that
    /// includes witness bytes; a transaction ID alone is not an exact replay
    /// commitment because it does not commit to witness serialization.
    ///
    /// # Errors
    ///
    /// Rejects empty, oversized, or non-public-identifier-shaped input.
    pub fn new(
        key: PublicEffectKey,
        agreement_commitment: [u8; 32],
        expected_effect_id: impl Into<Box<str>>,
        exact_public_bytes: Vec<u8>,
    ) -> Result<Self, StoreError> {
        let expected_effect_id = expected_effect_id.into();
        if agreement_commitment.iter().all(|byte| *byte == 0)
            || expected_effect_id.is_empty()
            || expected_effect_id.len() > MAX_EXPECTED_EFFECT_ID_BYTES
            || !expected_effect_id
                .bytes()
                .all(|byte| byte.is_ascii_graphic())
            || exact_public_bytes.is_empty()
            || exact_public_bytes.len() > MAX_EXACT_PUBLIC_BYTES
            || key.predecessor_revision > i64::MAX as u64
        {
            return Err(StoreError::InvalidPublicEffect);
        }
        let public_bytes_sha256 = <[u8; 32]>::from(Sha256::digest(&exact_public_bytes));
        Ok(Self {
            key,
            agreement_commitment,
            expected_effect_id,
            exact_public_bytes: exact_public_bytes.into_boxed_slice(),
            public_bytes_sha256,
        })
    }

    /// Composite effect identity.
    pub const fn key(&self) -> &PublicEffectKey {
        &self.key
    }

    /// Commitment to the fully signed agreement authorizing this effect.
    #[must_use]
    pub const fn agreement_commitment(&self) -> [u8; 32] {
        self.agreement_commitment
    }

    /// Chain-native expected public effect identifier.
    #[must_use]
    pub const fn expected_effect_id(&self) -> &str {
        &self.expected_effect_id
    }

    /// Exact public transaction wire bytes.
    #[must_use]
    pub const fn exact_public_bytes(&self) -> &[u8] {
        &self.exact_public_bytes
    }

    /// SHA-256 commitment to the complete exact public bytes.
    #[must_use]
    pub const fn public_bytes_sha256(&self) -> [u8; 32] {
        self.public_bytes_sha256
    }
}

/// Durable monotonic state for one exact effect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub enum PublicEffectState {
    /// Exact bytes are durable and no send has been authorized.
    Prepared,
    /// The sole send authorization was durably consumed before the RPC call.
    Started,
    /// Exact chain evidence or the fresh call proved acceptance.
    Accepted,
    /// The fresh call definitively rejected the payload before admission.
    Rejected,
    /// The fresh call outcome was ambiguous; only observation is now allowed.
    Unknown,
}

impl PublicEffectState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Started => "started",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Unknown => "unknown",
        }
    }

    fn parse(value: &str) -> Result<Self, StoreError> {
        match value {
            "prepared" => Ok(Self::Prepared),
            "started" => Ok(Self::Started),
            "accepted" => Ok(Self::Accepted),
            "rejected" => Ok(Self::Rejected),
            "unknown" => Ok(Self::Unknown),
            _ => Err(StoreError::CorruptPublicEffectState),
        }
    }
}

/// Current exact effect and validated durable state.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct PublicEffectSnapshot {
    effect: PreparedPublicEffect,
    state: PublicEffectState,
    attempt_count: u32,
    revision: u64,
}

impl PublicEffectSnapshot {
    /// Immutable exact prepared material.
    pub const fn effect(&self) -> &PreparedPublicEffect {
        &self.effect
    }

    /// Current monotonic journal state.
    pub const fn state(&self) -> PublicEffectState {
        self.state
    }

    /// Durable fresh-send authority consumption count, constrained to zero or one.
    ///
    /// One means either that a send was authorized before transport or that
    /// conflicting chain presence defensively burned that authority without a
    /// transport call.
    #[must_use]
    pub const fn attempt_count(&self) -> u32 {
        self.attempt_count
    }

    /// Journal compare-and-swap revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }
}

/// Read-only chain result obtained before deciding whether to submit.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub enum PublicEffectObservation {
    /// Chain evidence contains these complete exact transaction bytes.
    PresentExact(Vec<u8>),
    /// A definitive bounded observation did not find the exact effect.
    Absent,
    /// Chain observation could not prove either presence or absence.
    Uncertain,
    /// Chain evidence proves presence but contradicts the durable exact bytes.
    ///
    /// This permanently burns any still-fresh send authority without granting
    /// a transport call. A later exact match may still prove acceptance.
    ConflictingPresence,
}

/// Action authorized after reconciling one durable effect with chain truth.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub enum PublicEffectDecision {
    /// The Prepared-to-Started CAS committed; this caller alone may send once.
    SubmitOnce(PublicEffectSnapshot),
    /// No send is authorized; continue exact observation only.
    ObserveOnly(PublicEffectSnapshot),
}

/// Classification recorded immediately after the sole fresh send call.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub enum PublicEffectSubmissionResult {
    /// Node response proved acceptance of this exact expected effect ID.
    Accepted(Box<str>),
    /// Node response proved definitive rejection before admission.
    Rejected,
    /// Transport or response ambiguity could not prove either outcome.
    Unknown,
}

impl PublicEffectSubmissionResult {
    const fn state(&self) -> PublicEffectState {
        match self {
            Self::Accepted(_) => PublicEffectState::Accepted,
            Self::Rejected => PublicEffectState::Rejected,
            Self::Unknown => PublicEffectState::Unknown,
        }
    }
}

/// Result of an idempotent journal write.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct PublicEffectCommit {
    snapshot: PublicEffectSnapshot,
    was_replay: bool,
}

impl PublicEffectCommit {
    /// Validated state after the write or replay.
    pub const fn snapshot(&self) -> &PublicEffectSnapshot {
        &self.snapshot
    }

    /// Whether the exact requested state was already durable.
    #[must_use]
    pub const fn was_replay(&self) -> bool {
        self.was_replay
    }
}

/// SQLite-backed exact public-effect journal.
#[derive(Debug)]
pub struct SqlitePublicEffectJournal {
    connection: Connection,
}

impl SqlitePublicEffectJournal {
    /// Opens or creates the additive public-effect schema using the swap store's
    /// owner-private file checks, WAL, FULL synchronous, and busy timeout.
    ///
    /// # Errors
    ///
    /// Returns a store error for unsafe files, unsupported schemas, or `SQLite`
    /// migration failure.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let mut connection = open_configured_connection(path)?;
        migrate_public_effect_journal(&mut connection)?;
        Ok(Self { connection })
    }

    /// Persists a complete effect exactly once.
    ///
    /// Exact replay is idempotent at every later state. Any agreement, expected
    /// ID, public-byte, or digest drift for the same composite key conflicts.
    ///
    /// # Errors
    ///
    /// Rejects invalid material, conflicting immutable state, corrupt rows, or
    /// storage failures without partially changing the journal.
    pub fn record_prepared(
        &mut self,
        effect: &PreparedPublicEffect,
    ) -> Result<PublicEffectCommit, StoreError> {
        validate_prepared(effect)?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = load_snapshot(&transaction, effect.key())?;
        let (snapshot, was_replay) = if let Some(existing) = existing {
            if existing.effect != *effect {
                return Err(StoreError::PublicEffectConflict);
            }
            (existing, true)
        } else {
            let reused_effect_id: bool = transaction.query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM public_effect_journal
                     WHERE chain = ?1 AND expected_effect_id = ?2
                 )",
                params![
                    effect.key.chain.as_str(),
                    effect.expected_effect_id.as_ref()
                ],
                |row| row.get(0),
            )?;
            if reused_effect_id {
                return Err(StoreError::PublicEffectConflict);
            }
            transaction.execute(
                "INSERT INTO public_effect_journal (
                     swap_id, local_role, chain, operation, predecessor_revision,
                     agreement_commitment, expected_effect_id, exact_public_bytes,
                     public_bytes_sha256, state, attempt_count, revision
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'prepared', 0, 0)",
                params![
                    effect.key.swap_id.as_str(),
                    participant_name(effect.key.local_role),
                    effect.key.chain.as_str(),
                    effect.key.operation.as_str(),
                    revision_to_sql(effect.key.predecessor_revision)?,
                    effect.agreement_commitment.as_slice(),
                    effect.expected_effect_id.as_ref(),
                    effect.exact_public_bytes.as_ref(),
                    effect.public_bytes_sha256.as_slice(),
                ],
            )?;
            let inserted = load_snapshot(&transaction, effect.key())?
                .ok_or(StoreError::CorruptPublicEffectState)?;
            if inserted.effect != *effect
                || inserted.state != PublicEffectState::Prepared
                || inserted.attempt_count != 0
                || inserted.revision != 0
            {
                return Err(StoreError::CorruptPublicEffectState);
            }
            (inserted, false)
        };
        transaction.commit()?;
        Ok(PublicEffectCommit {
            snapshot,
            was_replay,
        })
    }

    /// Reconciles durable authority with one prior exact chain observation.
    ///
    /// `Absent + Prepared` atomically commits `Started` before returning the sole
    /// `SubmitOnce` authorization. `Started` and `Unknown` are never rearmed.
    /// `Uncertain` is retryable and always observe-only. `ConflictingPresence`
    /// atomically consumes still-fresh authority without returning
    /// `SubmitOnce`; later absence can therefore never rearm it. Exact presence
    /// monotonically accepts Prepared, Started, or Unknown state.
    ///
    /// # Errors
    ///
    /// Rejects missing, conflicting, corrupt, stale, or uncommitted state.
    pub fn reconcile(
        &mut self,
        key: &PublicEffectKey,
        observation: PublicEffectObservation,
    ) -> Result<PublicEffectDecision, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut snapshot =
            load_snapshot(&transaction, key)?.ok_or(StoreError::MissingPublicEffect)?;
        let submit_once = match observation {
            PublicEffectObservation::PresentExact(bytes) => {
                if bytes.as_slice() != snapshot.effect.exact_public_bytes() {
                    return Err(StoreError::PublicEffectConflict);
                }
                match snapshot.state {
                    PublicEffectState::Prepared
                    | PublicEffectState::Started
                    | PublicEffectState::Unknown => {
                        advance_to_accepted(&transaction, &mut snapshot)?;
                    }
                    PublicEffectState::Accepted => {}
                    PublicEffectState::Rejected => {
                        return Err(StoreError::PublicEffectConflict);
                    }
                }
                false
            }
            PublicEffectObservation::Absent => {
                if snapshot.state == PublicEffectState::Prepared {
                    begin_once(&transaction, &mut snapshot)?;
                    true
                } else {
                    false
                }
            }
            PublicEffectObservation::Uncertain => false,
            PublicEffectObservation::ConflictingPresence => {
                match snapshot.state {
                    PublicEffectState::Prepared => {
                        burn_authority_without_send(&transaction, &mut snapshot)?;
                    }
                    PublicEffectState::Started | PublicEffectState::Unknown => {}
                    PublicEffectState::Accepted | PublicEffectState::Rejected => {
                        return Err(StoreError::PublicEffectConflict);
                    }
                }
                false
            }
        };
        let postcondition =
            load_snapshot(&transaction, key)?.ok_or(StoreError::CorruptPublicEffectState)?;
        if postcondition != snapshot {
            return Err(StoreError::CorruptPublicEffectState);
        }
        transaction.commit()?;
        if submit_once {
            Ok(PublicEffectDecision::SubmitOnce(snapshot))
        } else {
            Ok(PublicEffectDecision::ObserveOnly(snapshot))
        }
    }

    /// Records the outcome of the one fresh call authorized by `SubmitOnce`.
    ///
    /// Only `Started` may produce a fresh terminal write. Exact terminal replay
    /// is idempotent for crash recovery; a different result conflicts.
    ///
    /// # Errors
    ///
    /// Rejects calls before send authorization, result drift, corrupt state, or
    /// storage failure without partially changing the row.
    pub fn record_submission_result(
        &mut self,
        key: &PublicEffectKey,
        result: &PublicEffectSubmissionResult,
    ) -> Result<PublicEffectCommit, StoreError> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut snapshot =
            load_snapshot(&transaction, key)?.ok_or(StoreError::MissingPublicEffect)?;
        if let PublicEffectSubmissionResult::Accepted(effect_id) = result
            && effect_id.as_ref() != snapshot.effect.expected_effect_id()
        {
            return Err(StoreError::PublicEffectConflict);
        }
        let target = result.state();
        let was_replay = if snapshot.state == PublicEffectState::Started {
            let updated = transaction.execute(
                "UPDATE public_effect_journal
                 SET state = ?6, revision = 2
                 WHERE swap_id = ?1 AND local_role = ?2 AND chain = ?3
                   AND operation = ?4 AND predecessor_revision = ?5
                   AND state = 'started' AND attempt_count = 1 AND revision = 1",
                params![
                    key.swap_id.as_str(),
                    participant_name(key.local_role),
                    key.chain.as_str(),
                    key.operation.as_str(),
                    revision_to_sql(key.predecessor_revision)?,
                    target.as_str(),
                ],
            )?;
            if updated != 1 {
                return Err(StoreError::PublicEffectConflict);
            }
            snapshot.state = target;
            snapshot.revision = 2;
            false
        } else if snapshot.state == target && snapshot.attempt_count == 1 && snapshot.revision == 2
        {
            true
        } else {
            return Err(StoreError::PublicEffectConflict);
        };
        let postcondition =
            load_snapshot(&transaction, key)?.ok_or(StoreError::CorruptPublicEffectState)?;
        if postcondition != snapshot {
            return Err(StoreError::CorruptPublicEffectState);
        }
        transaction.commit()?;
        Ok(PublicEffectCommit {
            snapshot,
            was_replay,
        })
    }

    /// Loads and fully validates one exact durable row.
    ///
    /// # Errors
    ///
    /// Returns a typed corruption or storage error; absence is `None`.
    pub fn current(
        &self,
        key: &PublicEffectKey,
    ) -> Result<Option<PublicEffectSnapshot>, StoreError> {
        load_snapshot(&self.connection, key)
    }
}

fn migrate_public_effect_journal(connection: &mut Connection) -> Result<(), StoreError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS public_effect_journal (
             swap_id               TEXT NOT NULL,
             local_role            TEXT NOT NULL CHECK (local_role IN ('maker', 'taker')),
             chain                 TEXT NOT NULL CHECK (chain IN ('bitcoin', 'lez')),
             operation             TEXT NOT NULL CHECK (operation IN ('funding', 'claim', 'refund')),
             predecessor_revision  INTEGER NOT NULL CHECK (predecessor_revision >= 0),
             agreement_commitment  BLOB NOT NULL CHECK (length(agreement_commitment) = 32),
             expected_effect_id    TEXT NOT NULL CHECK (
                 length(expected_effect_id) BETWEEN 1 AND 512
             ),
             exact_public_bytes    BLOB NOT NULL CHECK (
                 length(exact_public_bytes) BETWEEN 1 AND 4194304
             ),
             public_bytes_sha256   BLOB NOT NULL CHECK (length(public_bytes_sha256) = 32),
             state                 TEXT NOT NULL CHECK (
                 state IN ('prepared', 'started', 'accepted', 'rejected', 'unknown')
             ),
             attempt_count         INTEGER NOT NULL CHECK (attempt_count IN (0, 1)),
             revision              INTEGER NOT NULL CHECK (revision BETWEEN 0 AND 3),
             PRIMARY KEY (
                 swap_id, local_role, chain, operation, predecessor_revision
             ),
             UNIQUE (chain, expected_effect_id),
             CHECK (
                 (state = 'prepared' AND attempt_count = 0 AND revision = 0)
                 OR (state = 'started' AND attempt_count = 1 AND revision = 1)
                 OR (state IN ('rejected', 'unknown') AND attempt_count = 1 AND revision = 2)
                 OR (state = 'accepted' AND (
                     (attempt_count = 0 AND revision = 1)
                     OR (attempt_count = 1 AND revision IN (2, 3))
                 ))
             )
         ) STRICT;",
    )?;
    validate_public_effect_schema(&transaction)?;
    transaction.commit()?;
    Ok(())
}

fn validate_public_effect_schema(connection: &Connection) -> Result<(), StoreError> {
    type Column = (i64, String, String, i64, Option<String>, i64, i64);
    let actual = {
        let mut statement = connection.prepare("PRAGMA table_xinfo(public_effect_journal)")?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            })?
            .collect::<Result<Vec<Column>, _>>()?
    };
    let expected = [
        (0, "swap_id", "TEXT", 1, 1),
        (1, "local_role", "TEXT", 1, 2),
        (2, "chain", "TEXT", 1, 3),
        (3, "operation", "TEXT", 1, 4),
        (4, "predecessor_revision", "INTEGER", 1, 5),
        (5, "agreement_commitment", "BLOB", 1, 0),
        (6, "expected_effect_id", "TEXT", 1, 0),
        (7, "exact_public_bytes", "BLOB", 1, 0),
        (8, "public_bytes_sha256", "BLOB", 1, 0),
        (9, "state", "TEXT", 1, 0),
        (10, "attempt_count", "INTEGER", 1, 0),
        (11, "revision", "INTEGER", 1, 0),
    ];
    let columns_match = actual.len() == expected.len()
        && actual.iter().zip(expected).all(
            |((cid, name, kind, not_null, default, primary_key, hidden), expected)| {
                (*cid, name.as_str(), kind.as_str(), *not_null, *primary_key) == expected
                    && default.is_none()
                    && *hidden == 0
            },
        );
    let strict: Option<i64> = connection
        .query_row(
            "SELECT strict FROM pragma_table_list
             WHERE schema = 'main' AND name = 'public_effect_journal' AND type = 'table'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let triggers: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_schema
         WHERE type = 'trigger' AND tbl_name = 'public_effect_journal'",
        [],
        |row| row.get(0),
    )?;
    let schema_sql: Option<String> = connection
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'table' AND name = 'public_effect_journal'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let schema_sql = schema_sql
        .as_deref()
        .map(|sql| sql.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default();
    let required_checks = [
        "CHECK (local_role IN ('maker', 'taker'))",
        "CHECK (chain IN ('bitcoin', 'lez'))",
        "CHECK (operation IN ('funding', 'claim', 'refund'))",
        "CHECK (length(agreement_commitment) = 32)",
        "CHECK (length(public_bytes_sha256) = 32)",
        "UNIQUE (chain, expected_effect_id)",
        "state IN ('prepared', 'started', 'accepted', 'rejected', 'unknown')",
        "state = 'prepared' AND attempt_count = 0 AND revision = 0",
        "state = 'started' AND attempt_count = 1 AND revision = 1",
        "state IN ('rejected', 'unknown') AND attempt_count = 1 AND revision = 2",
    ];
    if !columns_match
        || strict != Some(1)
        || triggers != 0
        || !required_checks
            .iter()
            .all(|required| schema_sql.contains(required))
    {
        return Err(StoreError::CorruptPublicEffectState);
    }
    Ok(())
}

type StoredPublicEffectRow = (Vec<u8>, String, Vec<u8>, Vec<u8>, String, i64, i64);

fn load_snapshot(
    connection: &Connection,
    key: &PublicEffectKey,
) -> Result<Option<PublicEffectSnapshot>, StoreError> {
    let row = connection
        .query_row(
            "SELECT agreement_commitment, expected_effect_id, exact_public_bytes,
                    public_bytes_sha256, state, attempt_count, revision
             FROM public_effect_journal
             WHERE swap_id = ?1 AND local_role = ?2 AND chain = ?3
               AND operation = ?4 AND predecessor_revision = ?5",
            params![
                key.swap_id.as_str(),
                participant_name(key.local_role),
                key.chain.as_str(),
                key.operation.as_str(),
                revision_to_sql(key.predecessor_revision)?,
            ],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            },
        )
        .optional()?;
    row.map(|row| decode_snapshot(key, row)).transpose()
}

fn decode_snapshot(
    key: &PublicEffectKey,
    row: StoredPublicEffectRow,
) -> Result<PublicEffectSnapshot, StoreError> {
    let (agreement, expected_id, exact_bytes, digest, state, attempts, revision) = row;
    let agreement_commitment = fixed_32(agreement)?;
    let persisted_digest = fixed_32(digest)?;
    let effect =
        PreparedPublicEffect::new(key.clone(), agreement_commitment, expected_id, exact_bytes)
            .map_err(|_| StoreError::CorruptPublicEffectState)?;
    if effect.public_bytes_sha256 != persisted_digest {
        return Err(StoreError::CorruptPublicEffectState);
    }
    let state = PublicEffectState::parse(&state)?;
    let attempt_count =
        u32::try_from(attempts).map_err(|_| StoreError::CorruptPublicEffectState)?;
    let revision = u64::try_from(revision).map_err(|_| StoreError::CorruptPublicEffectState)?;
    validate_transition_shape(state, attempt_count, revision)?;
    Ok(PublicEffectSnapshot {
        effect,
        state,
        attempt_count,
        revision,
    })
}

fn validate_prepared(effect: &PreparedPublicEffect) -> Result<(), StoreError> {
    let reconstructed = PreparedPublicEffect::new(
        effect.key.clone(),
        effect.agreement_commitment,
        effect.expected_effect_id.clone(),
        effect.exact_public_bytes.to_vec(),
    )?;
    if reconstructed == *effect {
        Ok(())
    } else {
        Err(StoreError::InvalidPublicEffect)
    }
}

fn begin_once(
    transaction: &rusqlite::Transaction<'_>,
    snapshot: &mut PublicEffectSnapshot,
) -> Result<(), StoreError> {
    let key = snapshot.effect.key();
    let updated = transaction.execute(
        "UPDATE public_effect_journal
         SET state = 'started', attempt_count = 1, revision = 1
         WHERE swap_id = ?1 AND local_role = ?2 AND chain = ?3
           AND operation = ?4 AND predecessor_revision = ?5
           AND state = 'prepared' AND attempt_count = 0 AND revision = 0",
        params![
            key.swap_id.as_str(),
            participant_name(key.local_role),
            key.chain.as_str(),
            key.operation.as_str(),
            revision_to_sql(key.predecessor_revision)?,
        ],
    )?;
    if updated != 1 {
        return Err(StoreError::PublicEffectConflict);
    }
    snapshot.state = PublicEffectState::Started;
    snapshot.attempt_count = 1;
    snapshot.revision = 1;
    Ok(())
}

fn burn_authority_without_send(
    transaction: &rusqlite::Transaction<'_>,
    snapshot: &mut PublicEffectSnapshot,
) -> Result<(), StoreError> {
    let key = snapshot.effect.key();
    let updated = transaction.execute(
        "UPDATE public_effect_journal
         SET state = 'unknown', attempt_count = 1, revision = 2
         WHERE swap_id = ?1 AND local_role = ?2 AND chain = ?3
           AND operation = ?4 AND predecessor_revision = ?5
           AND state = 'prepared' AND attempt_count = 0 AND revision = 0",
        params![
            key.swap_id.as_str(),
            participant_name(key.local_role),
            key.chain.as_str(),
            key.operation.as_str(),
            revision_to_sql(key.predecessor_revision)?,
        ],
    )?;
    if updated != 1 {
        return Err(StoreError::PublicEffectConflict);
    }
    snapshot.state = PublicEffectState::Unknown;
    snapshot.attempt_count = 1;
    snapshot.revision = 2;
    Ok(())
}

fn advance_to_accepted(
    transaction: &rusqlite::Transaction<'_>,
    snapshot: &mut PublicEffectSnapshot,
) -> Result<(), StoreError> {
    let key = snapshot.effect.key();
    let next_revision = snapshot
        .revision
        .checked_add(1)
        .ok_or(StoreError::CorruptPublicEffectState)?;
    let updated = transaction.execute(
        "UPDATE public_effect_journal
         SET state = 'accepted', revision = ?6
         WHERE swap_id = ?1 AND local_role = ?2 AND chain = ?3
           AND operation = ?4 AND predecessor_revision = ?5
           AND state = ?7 AND attempt_count = ?8 AND revision = ?9",
        params![
            key.swap_id.as_str(),
            participant_name(key.local_role),
            key.chain.as_str(),
            key.operation.as_str(),
            revision_to_sql(key.predecessor_revision)?,
            revision_to_sql(next_revision)?,
            snapshot.state.as_str(),
            i64::from(snapshot.attempt_count),
            revision_to_sql(snapshot.revision)?,
        ],
    )?;
    if updated != 1 {
        return Err(StoreError::PublicEffectConflict);
    }
    snapshot.state = PublicEffectState::Accepted;
    snapshot.revision = next_revision;
    Ok(())
}

fn validate_transition_shape(
    state: PublicEffectState,
    attempt_count: u32,
    revision: u64,
) -> Result<(), StoreError> {
    let valid = match state {
        PublicEffectState::Prepared => attempt_count == 0 && revision == 0,
        PublicEffectState::Started => attempt_count == 1 && revision == 1,
        PublicEffectState::Rejected | PublicEffectState::Unknown => {
            attempt_count == 1 && revision == 2
        }
        PublicEffectState::Accepted => {
            (attempt_count == 0 && revision == 1)
                || (attempt_count == 1 && matches!(revision, 2 | 3))
        }
    };
    if valid {
        Ok(())
    } else {
        Err(StoreError::CorruptPublicEffectState)
    }
}

fn fixed_32(bytes: Vec<u8>) -> Result<[u8; 32], StoreError> {
    bytes
        .try_into()
        .map_err(|_| StoreError::CorruptPublicEffectState)
}

fn revision_to_sql(revision: u64) -> Result<i64, StoreError> {
    i64::try_from(revision).map_err(|_| StoreError::InvalidPublicEffect)
}
