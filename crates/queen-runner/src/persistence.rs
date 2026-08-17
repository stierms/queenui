use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use queen_protocol::{CommandResponse, IDEMPOTENCY_TTL_SECONDS};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, time::Duration};
use subtle::ConstantTimeEq;
use uuid::Uuid;

const DATABASE_FILE: &str = "runner.sqlite3";
const ENROLLMENT_LIFETIME_SECONDS: i64 = 10 * 60;
const MAX_ENROLLMENT_FAILURES: u32 = 5;
const MAX_IDEMPOTENCY_ROWS: i64 = 10_000;
const MAX_IDEMPOTENCY_BYTES: i64 = 64 * 1024 * 1024;
const MAX_STORED_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_NEW_KEYS_PER_MINUTE: i64 = 240;

#[derive(Clone, Debug)]
pub struct RunnerDatabase {
    path: PathBuf,
    runner_id: Uuid,
    certificate_fingerprint: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MintedEnrollment {
    pub code: String,
    pub expires_at: i64,
    pub rotate: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RedeemedBearer {
    pub bearer: String,
    pub generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RedeemError {
    Unavailable,
    Expired,
    Rejected { attempts_remaining: u32 },
    Revoked,
    IdentityMismatch,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdempotencyBinding {
    pub key: Uuid,
    pub credential_generation: u64,
    pub protocol_version: u32,
    pub method: &'static str,
    pub normalized_path: &'static str,
    pub body_digest: [u8; 32],
    pub command_kind: &'static str,
    pub reconciliation: &'static str,
}

#[derive(Clone, Debug)]
pub enum Reservation {
    Execute,
    Replay(CommandResponse),
    Pending,
    Conflict,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionState {
    Done,
    FailedDeterministic,
    FailedTransient,
}

pub fn response_fits(response: &CommandResponse) -> bool {
    serde_json::to_vec(response).is_ok_and(|bytes| bytes.len() <= MAX_STORED_RESPONSE_BYTES)
}

impl RunnerDatabase {
    pub fn open(data_dir: PathBuf, certificate_fingerprint: [u8; 32]) -> Result<Self, String> {
        std::fs::create_dir_all(&data_dir)
            .map_err(|error| format!("Could not create the runner data directory: {error}"))?;
        let path = data_dir.join(DATABASE_FILE);
        let mut connection = open_connection(&path)?;
        initialize_schema(&mut connection)?;
        let existing_runner_id: Option<String> = connection
            .query_row(
                "SELECT value FROM runner_meta WHERE key = 'runner_id'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(database_error)?;
        let runner_id = match existing_runner_id {
            Some(value) => Uuid::parse_str(&value)
                .map_err(|_| "Runner database contains an invalid identity".to_string())?,
            None => {
                let value = Uuid::new_v4();
                connection
                    .execute(
                        "INSERT INTO runner_meta(key, value) VALUES('runner_id', ?1)",
                        [value.to_string()],
                    )
                    .map_err(database_error)?;
                value
            }
        };
        connection
            .execute(
                "INSERT INTO runner_meta(key, value) VALUES('cert_fp', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [hex(&certificate_fingerprint)],
            )
            .map_err(database_error)?;
        Ok(Self {
            path,
            runner_id,
            certificate_fingerprint,
        })
    }

    pub fn runner_id(&self) -> Uuid {
        self.runner_id
    }

    pub fn mint_enrollment(&self, rotate: bool, now: i64) -> Result<MintedEnrollment, String> {
        let code = random_secret()?;
        let code_hash = secret_hash(&code);
        let expires_at = now
            .checked_add(ENROLLMENT_LIFETIME_SECONDS)
            .ok_or_else(|| "Enrollment expiry overflowed".to_string())?;
        let mut connection = open_connection(&self.path)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let has_credential = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM credentials WHERE id = 1)",
                [],
                |row| row.get::<_, bool>(0),
            )
            .map_err(database_error)?;
        if rotate && !has_credential {
            return Err("Cannot rotate before the runner has been paired".into());
        }
        if !rotate && has_credential {
            return Err("Runner is already paired; use pair --rotate for a hard cutover".into());
        }
        // The singleton delete+insert is the atomic supersession point.
        transaction
            .execute("DELETE FROM enrollment", [])
            .and_then(|_| {
                transaction.execute(
                    "INSERT INTO enrollment(
                        id, secret_hash, runner_id, cert_fp, purpose, expires_at,
                        failed_attempts, rotate
                     ) VALUES(1, ?1, ?2, ?3, 'pair', ?4, 0, ?5)",
                    params![
                        code_hash.as_slice(),
                        self.runner_id.to_string(),
                        self.certificate_fingerprint.as_slice(),
                        expires_at,
                        rotate
                    ],
                )
            })
            .map_err(database_error)?;
        transaction.commit().map_err(database_error)?;
        Ok(MintedEnrollment {
            code,
            expires_at,
            rotate,
        })
    }

    pub fn redeem(&self, code: &str, now: i64) -> Result<RedeemedBearer, RedeemError> {
        let candidate_hash = secret_hash(code);
        let new_bearer = random_secret().map_err(|_| RedeemError::Unavailable)?;
        let new_hash = secret_hash(&new_bearer);
        let mut connection = open_connection(&self.path).map_err(|_| RedeemError::Unavailable)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| RedeemError::Unavailable)?;
        let enrollment = transaction
            .query_row(
                "SELECT secret_hash, runner_id, cert_fp, purpose, expires_at,
                        failed_attempts, rotate
                 FROM enrollment WHERE id = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, u32>(5)?,
                        row.get::<_, bool>(6)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| RedeemError::Unavailable)?;
        let Some((stored_hash, runner_id, cert_fp, purpose, expires_at, attempts, rotate)) =
            enrollment
        else {
            return Err(RedeemError::Unavailable);
        };
        if expires_at <= now {
            transaction
                .execute("DELETE FROM enrollment WHERE id = 1", [])
                .and_then(|_| transaction.commit())
                .map_err(|_| RedeemError::Unavailable)?;
            return Err(RedeemError::Expired);
        }
        if runner_id != self.runner_id.to_string()
            || cert_fp.as_slice() != self.certificate_fingerprint
            || purpose != "pair"
        {
            transaction
                .execute("DELETE FROM enrollment WHERE id = 1", [])
                .and_then(|_| transaction.commit())
                .map_err(|_| RedeemError::Unavailable)?;
            return Err(RedeemError::IdentityMismatch);
        }
        if stored_hash.len() != 32
            || !bool::from(stored_hash.as_slice().ct_eq(candidate_hash.as_slice()))
        {
            let failures = attempts.saturating_add(1);
            if failures >= MAX_ENROLLMENT_FAILURES {
                transaction
                    .execute("DELETE FROM enrollment WHERE id = 1", [])
                    .and_then(|_| transaction.commit())
                    .map_err(|_| RedeemError::Unavailable)?;
                return Err(RedeemError::Revoked);
            }
            transaction
                .execute(
                    "UPDATE enrollment SET failed_attempts = ?1 WHERE id = 1",
                    [failures],
                )
                .and_then(|_| transaction.commit())
                .map_err(|_| RedeemError::Unavailable)?;
            return Err(RedeemError::Rejected {
                attempts_remaining: MAX_ENROLLMENT_FAILURES - failures,
            });
        }

        let current_generation: Option<u64> = transaction
            .query_row(
                "SELECT generation FROM credentials WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| RedeemError::Unavailable)?;
        if rotate != current_generation.is_some() {
            transaction
                .execute("DELETE FROM enrollment WHERE id = 1", [])
                .and_then(|_| transaction.commit())
                .map_err(|_| RedeemError::Unavailable)?;
            return Err(RedeemError::Unavailable);
        }
        let generation = current_generation.unwrap_or(0).saturating_add(1);
        // Bearer mint, old-bearer revocation, and code consumption are one
        // transaction. A commit exposes exactly one active generation.
        transaction
            .execute(
                "INSERT INTO credentials(id, generation, bearer_hash, created_at)
                 VALUES(1, ?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET
                    generation = excluded.generation,
                    bearer_hash = excluded.bearer_hash,
                    created_at = excluded.created_at",
                params![generation, new_hash.as_slice(), now],
            )
            .and_then(|_| transaction.execute("DELETE FROM enrollment WHERE id = 1", []))
            .and_then(|_| transaction.commit())
            .map_err(|_| RedeemError::Unavailable)?;
        Ok(RedeemedBearer {
            bearer: new_bearer,
            generation,
        })
    }

    pub fn authenticate(&self, bearer: &str) -> Result<Option<u64>, String> {
        let connection = open_connection(&self.path)?;
        let credential: Option<(u64, Vec<u8>)> = connection
            .query_row(
                "SELECT generation, bearer_hash FROM credentials WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(database_error)?;
        let candidate = secret_hash(bearer);
        Ok(credential.and_then(|(generation, stored)| {
            (stored.len() == 32 && bool::from(stored.as_slice().ct_eq(candidate.as_slice())))
                .then_some(generation)
        }))
    }

    pub fn reserve(&self, binding: &IdempotencyBinding) -> Result<Reservation, String> {
        self.reserve_at(binding, epoch_seconds())
    }

    fn reserve_at(&self, binding: &IdempotencyBinding, now: i64) -> Result<Reservation, String> {
        let mut connection = open_connection(&self.path)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        transaction
            .execute(
                "DELETE FROM idempotency WHERE created_at <= ?1",
                [now.saturating_sub(IDEMPOTENCY_TTL_SECONDS)],
            )
            .map_err(database_error)?;
        let existing = transaction
            .query_row(
                "SELECT protocol_version, method, path, body_digest, state, response
                 FROM idempotency WHERE request_key = ?1 AND credential_generation = ?2",
                params![binding.key.to_string(), binding.credential_generation],
                |row| {
                    Ok((
                        row.get::<_, u32>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Vec<u8>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<Vec<u8>>>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(database_error)?;
        if let Some((version, method, path, digest, state, response)) = existing {
            if version != binding.protocol_version
                || method != binding.method
                || path != binding.normalized_path
                || digest.as_slice() != binding.body_digest
            {
                return Ok(Reservation::Conflict);
            }
            return match state.as_str() {
                "done" | "failed_deterministic" => response
                    .ok_or_else(|| "Idempotency terminal row has no response".to_string())
                    .and_then(|bytes| {
                        serde_json::from_slice(&bytes)
                            .map(Reservation::Replay)
                            .map_err(|error| format!("Could not decode a stored response: {error}"))
                    }),
                "pending" | "ambiguous_crash" => Ok(Reservation::Pending),
                _ => Err("Idempotency row has an invalid state".into()),
            };
        }

        let row_count: i64 = transaction
            .query_row("SELECT COUNT(*) FROM idempotency", [], |row| row.get(0))
            .map_err(database_error)?;
        if row_count >= MAX_IDEMPOTENCY_ROWS {
            return Err("quota: the durable idempotency row quota is full".into());
        }
        let stored_bytes: i64 = transaction
            .query_row(
                "SELECT COALESCE(SUM(
                    CASE WHEN response IS NULL THEN ?1 ELSE length(response) END
                 ), 0) FROM idempotency",
                params![MAX_STORED_RESPONSE_BYTES as i64],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if stored_bytes.saturating_add(MAX_STORED_RESPONSE_BYTES as i64) > MAX_IDEMPOTENCY_BYTES {
            return Err("quota: the durable idempotency byte quota is full".into());
        }
        let recent: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM idempotency
                 WHERE credential_generation = ?1 AND created_at > ?2",
                params![binding.credential_generation, now.saturating_sub(60)],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if recent >= MAX_NEW_KEYS_PER_MINUTE {
            return Err("rate: too many new idempotency keys for this credential".into());
        }
        transaction
            .execute(
                "INSERT INTO idempotency(
                    request_key, credential_generation, protocol_version, method, path,
                    body_digest, state, response, command_kind, reconciliation,
                    created_at, updated_at
                 ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, 'pending', NULL, ?7, ?8, ?9, ?9)",
                params![
                    binding.key.to_string(),
                    binding.credential_generation,
                    binding.protocol_version,
                    binding.method,
                    binding.normalized_path,
                    binding.body_digest.as_slice(),
                    binding.command_kind,
                    binding.reconciliation,
                    now
                ],
            )
            .map_err(database_error)?;
        transaction.commit().map_err(database_error)?;
        Ok(Reservation::Execute)
    }

    pub fn complete(
        &self,
        binding: &IdempotencyBinding,
        state: CompletionState,
        response: &CommandResponse,
    ) -> Result<(), String> {
        let mut connection = open_connection(&self.path)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        if state == CompletionState::FailedTransient {
            transaction
                .execute(
                    "DELETE FROM idempotency
                     WHERE request_key = ?1 AND credential_generation = ?2",
                    params![binding.key.to_string(), binding.credential_generation],
                )
                .map_err(database_error)?;
            return transaction.commit().map_err(database_error);
        }
        let bytes = serde_json::to_vec(response)
            .map_err(|error| format!("Could not encode the durable response: {error}"))?;
        if bytes.len() > MAX_STORED_RESPONSE_BYTES {
            return Err("The command response exceeds the durable replay limit".into());
        }
        let stored_state = match state {
            CompletionState::Done => "done",
            CompletionState::FailedDeterministic => "failed_deterministic",
            CompletionState::FailedTransient => unreachable!(),
        };
        let changed = transaction
            .execute(
                "UPDATE idempotency SET state = ?1, response = ?2, updated_at = ?3
                 WHERE request_key = ?4 AND credential_generation = ?5
                   AND state = 'pending'",
                params![
                    stored_state,
                    bytes,
                    epoch_seconds(),
                    binding.key.to_string(),
                    binding.credential_generation
                ],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err("The idempotency reservation was not pending".into());
        }
        transaction.commit().map_err(database_error)
    }

    /// Startup phase one makes every interrupted execution explicit. This is
    /// called before the router can accept authenticated commands.
    pub fn mark_interrupted_pending(&self) -> Result<usize, String> {
        let connection = open_connection(&self.path)?;
        connection
            .execute(
                "UPDATE idempotency SET state = 'ambiguous_crash', updated_at = ?1
                 WHERE state = 'pending'",
                [epoch_seconds()],
            )
            .map_err(database_error)
    }

    /// Each row records the command-specific reconciliation family. The core's
    /// Tier-A startup reconciliation has already checked Lichess intent state
    /// and the engine process table before this runs. An operation not proven
    /// complete is persisted as a deterministic failed outcome, so it is never
    /// silently executed again under the same key.
    pub fn reconcile_ambiguous(&self) -> Result<usize, String> {
        let mut connection = open_connection(&self.path)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let rows = {
            let mut statement = transaction
                .prepare(
                    "SELECT request_key, credential_generation, command_kind, reconciliation
                     FROM idempotency WHERE state = 'ambiguous_crash'",
                )
                .map_err(database_error)?;
            let collected = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(database_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(database_error)?;
            collected
        };
        for (key, generation, command_kind, reconciliation) in &rows {
            let request_id = Uuid::parse_str(key)
                .map_err(|_| "Idempotency database contains an invalid request key".to_string())?;
            let response = CommandResponse::failure(
                request_id,
                "ambiguous_crash_reconciled",
                format!(
                    "The interrupted {command_kind} operation was reconciled against {reconciliation}; completion could not be proven"
                ),
            );
            let bytes = serde_json::to_vec(&response)
                .map_err(|error| format!("Could not encode reconciliation outcome: {error}"))?;
            transaction
                .execute(
                    "UPDATE idempotency
                     SET state = 'failed_deterministic', response = ?1, updated_at = ?2
                     WHERE request_key = ?3 AND credential_generation = ?4
                       AND state = 'ambiguous_crash'",
                    params![bytes, epoch_seconds(), key, generation],
                )
                .map_err(database_error)?;
        }
        transaction.commit().map_err(database_error)?;
        Ok(rows.len())
    }

    #[cfg(test)]
    fn state_for(&self, key: Uuid, generation: u64) -> Option<String> {
        open_connection(&self.path)
            .ok()?
            .query_row(
                "SELECT state FROM idempotency
                 WHERE request_key = ?1 AND credential_generation = ?2",
                params![key.to_string(), generation],
                |row| row.get(0),
            )
            .optional()
            .ok()?
    }
}

fn open_connection(path: &PathBuf) -> Result<Connection, String> {
    let connection = Connection::open(path).map_err(database_error)?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(database_error)?;
    connection
        .execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             PRAGMA foreign_keys = ON;",
        )
        .map_err(database_error)?;
    Ok(connection)
}

fn initialize_schema(connection: &mut Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS runner_meta(
                key TEXT PRIMARY KEY NOT NULL,
                value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS enrollment(
                id INTEGER PRIMARY KEY CHECK(id = 1),
                secret_hash BLOB NOT NULL CHECK(length(secret_hash) = 32),
                runner_id TEXT NOT NULL,
                cert_fp BLOB NOT NULL CHECK(length(cert_fp) = 32),
                purpose TEXT NOT NULL CHECK(purpose = 'pair'),
                expires_at INTEGER NOT NULL,
                failed_attempts INTEGER NOT NULL,
                rotate INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS credentials(
                id INTEGER PRIMARY KEY CHECK(id = 1),
                generation INTEGER NOT NULL,
                bearer_hash BLOB NOT NULL CHECK(length(bearer_hash) = 32),
                created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS idempotency(
                request_key TEXT NOT NULL,
                credential_generation INTEGER NOT NULL,
                protocol_version INTEGER NOT NULL,
                method TEXT NOT NULL,
                path TEXT NOT NULL,
                body_digest BLOB NOT NULL CHECK(length(body_digest) = 32),
                state TEXT NOT NULL CHECK(state IN (
                    'pending', 'done', 'failed_deterministic', 'ambiguous_crash'
                )),
                response BLOB,
                command_kind TEXT NOT NULL,
                reconciliation TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY(request_key, credential_generation)
             );
             CREATE INDEX IF NOT EXISTS idempotency_cleanup
                ON idempotency(created_at);
             CREATE INDEX IF NOT EXISTS idempotency_rate
                ON idempotency(credential_generation, created_at);",
        )
        .map_err(database_error)
}

fn random_secret() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("The operating system CSPRNG failed: {error}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn secret_hash(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

fn epoch_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn database_error(error: rusqlite::Error) -> String {
    format!("Runner database operation failed: {error}")
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{
        response_fits, CompletionState, IdempotencyBinding, RedeemError, Reservation,
        RunnerDatabase,
    };
    use queen_protocol::{CommandResponse, PROTOCOL_VERSION};
    use serde_json::json;
    use std::{fs, path::PathBuf, sync::Arc, thread};
    use uuid::Uuid;

    fn database() -> (RunnerDatabase, PathBuf) {
        let directory = std::env::temp_dir().join(format!("queen-runner-db-{}", Uuid::new_v4()));
        (
            RunnerDatabase::open(directory.clone(), [7; 32]).unwrap(),
            directory,
        )
    }

    fn binding(key: Uuid, digest: [u8; 32]) -> IdempotencyBinding {
        IdempotencyBinding {
            key,
            credential_generation: 1,
            protocol_version: PROTOCOL_VERSION,
            method: "POST",
            normalized_path: "/v2/commands",
            body_digest: digest,
            command_kind: "startBot",
            reconciliation: "Lichess account state",
        }
    }

    #[test]
    fn concurrent_redeem_has_exactly_one_winner() {
        let (database, directory) = database();
        let enrollment = database.mint_enrollment(false, 100).unwrap();
        let database = Arc::new(database);
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let database = database.clone();
                let barrier = barrier.clone();
                let code = enrollment.code.clone();
                thread::spawn(move || {
                    barrier.wait();
                    database.redeem(&code, 101)
                })
            })
            .collect();
        barrier.wait();
        let results: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn enrollment_expiry_is_absolute_and_enforced_in_consume_transaction() {
        let (database, directory) = database();
        let enrollment = database.mint_enrollment(false, 100).unwrap();
        assert_eq!(
            database.redeem(&enrollment.code, 700),
            Err(RedeemError::Expired)
        );
        assert_eq!(
            database.redeem(&enrollment.code, 699),
            Err(RedeemError::Unavailable)
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn new_enrollment_atomically_supersedes_the_old_code() {
        let (database, directory) = database();
        let old = database.mint_enrollment(false, 100).unwrap();
        let new = database.mint_enrollment(false, 101).unwrap();
        assert_eq!(
            database.redeem(&old.code, 102),
            Err(RedeemError::Rejected {
                attempts_remaining: 4
            })
        );
        assert!(database.redeem(&new.code, 102).is_ok());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn enrollment_and_rotation_survive_restart_with_hard_cutover() {
        let (database, directory) = database();
        let first = database.mint_enrollment(false, 100).unwrap();
        let reopened = RunnerDatabase::open(directory.clone(), [7; 32]).unwrap();
        let original = reopened.redeem(&first.code, 101).unwrap();
        assert_eq!(reopened.authenticate(&original.bearer).unwrap(), Some(1));

        let rotation = reopened.mint_enrollment(true, 200).unwrap();
        let restarted = RunnerDatabase::open(directory.clone(), [7; 32]).unwrap();
        let replacement = restarted.redeem(&rotation.code, 201).unwrap();
        assert_eq!(replacement.generation, 2);
        assert_eq!(restarted.authenticate(&original.bearer).unwrap(), None);
        assert_eq!(
            restarted.authenticate(&replacement.bearer).unwrap(),
            Some(2)
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn fifth_failed_enrollment_attempt_revokes_the_code() {
        let (database, directory) = database();
        let enrollment = database.mint_enrollment(false, 100).unwrap();
        for remaining in (1..=4).rev() {
            assert_eq!(
                database.redeem("wrong", 101),
                Err(RedeemError::Rejected {
                    attempts_remaining: remaining
                })
            );
        }
        assert_eq!(database.redeem("wrong", 101), Err(RedeemError::Revoked));
        assert_eq!(
            database.redeem(&enrollment.code, 101),
            Err(RedeemError::Unavailable)
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn pending_reservation_has_one_executor_and_durable_replay_or_409() {
        let (database, directory) = database();
        let key = Uuid::new_v4();
        let request = binding(key, [1; 32]);
        let database = Arc::new(database);
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let reservations: Vec<_> = (0..2)
            .map(|_| {
                let database = database.clone();
                let barrier = barrier.clone();
                let request = request.clone();
                thread::spawn(move || {
                    barrier.wait();
                    database.reserve(&request).unwrap()
                })
            })
            .collect();
        barrier.wait();
        let reservations: Vec<_> = reservations
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(
            reservations
                .iter()
                .filter(|value| matches!(value, Reservation::Execute))
                .count(),
            1
        );
        assert_eq!(
            reservations
                .iter()
                .filter(|value| matches!(value, Reservation::Pending))
                .count(),
            1
        );
        let response = CommandResponse::success(key, json!({"accepted": true}));
        database
            .complete(&request, CompletionState::Done, &response)
            .unwrap();
        let reopened = RunnerDatabase::open(directory.clone(), [7; 32]).unwrap();
        match reopened.reserve(&request).unwrap() {
            Reservation::Replay(replayed) => assert_eq!(replayed.result, response.result),
            _ => panic!("expected exact replay"),
        }
        assert!(matches!(
            reopened.reserve(&binding(key, [2; 32])).unwrap(),
            Reservation::Conflict
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ambiguous_crash_is_explicit_and_reconciled_before_replay() {
        let (database, directory) = database();
        let key = Uuid::new_v4();
        let request = binding(key, [3; 32]);
        assert!(matches!(
            database.reserve(&request).unwrap(),
            Reservation::Execute
        ));
        assert_eq!(database.mark_interrupted_pending().unwrap(), 1);
        assert_eq!(
            database.state_for(key, 1).as_deref(),
            Some("ambiguous_crash")
        );
        assert_eq!(database.reconcile_ambiguous().unwrap(), 1);
        assert_eq!(
            database.state_for(key, 1).as_deref(),
            Some("failed_deterministic")
        );
        match database.reserve(&request).unwrap() {
            Reservation::Replay(response) => {
                assert_eq!(response.error.unwrap().code, "ambiguous_crash_reconciled")
            }
            _ => panic!("expected reconciled replay"),
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn expired_idempotency_key_is_a_new_request_and_database_uses_wal() {
        let (database, directory) = database();
        let key = Uuid::new_v4();
        let request = binding(key, [4; 32]);
        assert!(matches!(
            database.reserve_at(&request, 100).unwrap(),
            Reservation::Execute
        ));
        let response = CommandResponse::success(key, json!(null));
        database
            .complete(&request, CompletionState::Done, &response)
            .unwrap();
        assert!(matches!(
            database
                .reserve_at(&request, 100 + queen_protocol::IDEMPOTENCY_TTL_SECONDS + 1)
                .unwrap(),
            Reservation::Execute
        ));
        let mode: String = rusqlite::Connection::open(directory.join("runner.sqlite3"))
            .unwrap()
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_ascii_lowercase(), "wal");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rotation_expiry_and_repeated_supersession_keep_old_bearer_valid() {
        let (database, directory) = database();
        let initial = database.mint_enrollment(false, 10).unwrap();
        let original = database.redeem(&initial.code, 11).unwrap();

        let expired = database.mint_enrollment(true, 20).unwrap();
        assert_eq!(
            database.redeem(&expired.code, 620),
            Err(RedeemError::Expired)
        );
        assert_eq!(database.authenticate(&original.bearer).unwrap(), Some(1));

        let superseded = database.mint_enrollment(true, 700).unwrap();
        let replacement = database.mint_enrollment(true, 701).unwrap();
        assert!(matches!(
            database.redeem(&superseded.code, 702),
            Err(RedeemError::Rejected { .. })
        ));
        assert_eq!(database.authenticate(&original.bearer).unwrap(), Some(1));
        let rotated = database.redeem(&replacement.code, 702).unwrap();
        assert_eq!(rotated.generation, 2);
        assert_eq!(database.authenticate(&original.bearer).unwrap(), None);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn failed_rotation_attempt_limit_revokes_only_the_code() {
        let (database, directory) = database();
        let initial = database.mint_enrollment(false, 10).unwrap();
        let original = database.redeem(&initial.code, 11).unwrap();
        let rotation = database.mint_enrollment(true, 20).unwrap();

        for _ in 0..4 {
            assert!(matches!(
                database.redeem("wrong", 21),
                Err(RedeemError::Rejected { .. })
            ));
        }
        assert_eq!(database.redeem("wrong", 21), Err(RedeemError::Revoked));
        assert_eq!(
            database.redeem(&rotation.code, 21),
            Err(RedeemError::Unavailable)
        );
        assert_eq!(database.authenticate(&original.bearer).unwrap(), Some(1));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn lost_rotation_response_is_recovered_by_a_new_admin_enrollment() {
        let (database, directory) = database();
        let initial = database.mint_enrollment(false, 10).unwrap();
        let original = database.redeem(&initial.code, 11).unwrap();

        let lost = database.mint_enrollment(true, 20).unwrap();
        let unseen_replacement = database.redeem(&lost.code, 21).unwrap();
        assert_eq!(unseen_replacement.generation, 2);
        assert_eq!(database.authenticate(&original.bearer).unwrap(), None);

        let recovery = database.mint_enrollment(true, 30).unwrap();
        let recovered = database.redeem(&recovery.code, 31).unwrap();
        assert_eq!(recovered.generation, 3);
        assert_eq!(
            database.authenticate(&unseen_replacement.bearer).unwrap(),
            None
        );
        assert_eq!(database.authenticate(&recovered.bearer).unwrap(), Some(3));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn idempotency_binding_covers_generation_protocol_method_path_and_digest() {
        let (database, directory) = database();
        let key = Uuid::new_v4();
        let request = binding(key, [8; 32]);
        assert!(matches!(
            database.reserve(&request).unwrap(),
            Reservation::Execute
        ));

        let mut changed = request.clone();
        changed.protocol_version += 1;
        assert!(matches!(
            database.reserve(&changed).unwrap(),
            Reservation::Conflict
        ));
        changed = request.clone();
        changed.method = "PUT";
        assert!(matches!(
            database.reserve(&changed).unwrap(),
            Reservation::Conflict
        ));
        changed = request.clone();
        changed.normalized_path = "/v2/other";
        assert!(matches!(
            database.reserve(&changed).unwrap(),
            Reservation::Conflict
        ));
        changed = request.clone();
        changed.body_digest = [9; 32];
        assert!(matches!(
            database.reserve(&changed).unwrap(),
            Reservation::Conflict
        ));

        changed = request;
        changed.credential_generation = 2;
        assert!(matches!(
            database.reserve(&changed).unwrap(),
            Reservation::Execute
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn transient_failure_releases_key_while_deterministic_failure_replays() {
        let (database, directory) = database();
        let transient_key = Uuid::new_v4();
        let transient = binding(transient_key, [10; 32]);
        assert!(matches!(
            database.reserve(&transient).unwrap(),
            Reservation::Execute
        ));
        database
            .complete(
                &transient,
                CompletionState::FailedTransient,
                &CommandResponse::failure(transient_key, "temporary", "retry"),
            )
            .unwrap();
        assert!(matches!(
            database.reserve(&transient).unwrap(),
            Reservation::Execute
        ));

        let deterministic_key = Uuid::new_v4();
        let deterministic = binding(deterministic_key, [11; 32]);
        assert!(matches!(
            database.reserve(&deterministic).unwrap(),
            Reservation::Execute
        ));
        let failure = CommandResponse::failure(deterministic_key, "rejected", "invalid target");
        database
            .complete(
                &deterministic,
                CompletionState::FailedDeterministic,
                &failure,
            )
            .unwrap();
        match database.reserve(&deterministic).unwrap() {
            Reservation::Replay(replayed) => assert_eq!(
                replayed.error.map(|error| error.code),
                Some("rejected".into())
            ),
            _ => panic!("expected deterministic failure replay"),
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn durable_outcomes_enforce_the_per_response_bound() {
        let key = Uuid::new_v4();
        assert!(response_fits(&CommandResponse::success(
            key,
            json!({
                "accepted": true
            })
        )));
        assert!(!response_fits(&CommandResponse::success(
            key,
            json!("x".repeat(256 * 1024))
        )));
    }

    #[test]
    fn enrollment_and_bearer_are_hashed_at_rest() {
        let (database, directory) = database();
        let enrollment = database.mint_enrollment(false, 100).unwrap();
        let stored_enrollment: Vec<u8> =
            rusqlite::Connection::open(directory.join("runner.sqlite3"))
                .unwrap()
                .query_row("SELECT secret_hash FROM enrollment", [], |row| row.get(0))
                .unwrap();
        assert_eq!(stored_enrollment.len(), 32);
        assert_ne!(stored_enrollment, enrollment.code.as_bytes());
        let bearer = database.redeem(&enrollment.code, 101).unwrap();
        let stored_bearer: Vec<u8> = rusqlite::Connection::open(directory.join("runner.sqlite3"))
            .unwrap()
            .query_row("SELECT bearer_hash FROM credentials", [], |row| row.get(0))
            .unwrap();
        assert_eq!(stored_bearer.len(), 32);
        assert_ne!(stored_bearer, bearer.bearer.as_bytes());
        fs::remove_dir_all(directory).unwrap();
    }
}
