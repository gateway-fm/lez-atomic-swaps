//! Who signs the sidecar's planned public transactions.
//!
//! The planner builds every message, chooses every nonce and records the exact
//! bytes it will submit; a signer only turns one message into its witness. Two
//! signers exist: the role's own key (`KeyFileSigner`, what every planner used
//! before) and the LEZ wallet as a Logos module holding that key
//! (`WalletModuleSigner`), reached over an owner-only unix socket. Either way
//! the witness is `(Signature::new(key, message.hash()), public key)`, so the
//! transaction bytes and the signature scheme are the official ones; the
//! planner verifies every witness against the message and its bound account
//! before it is used.

use std::{
    fmt,
    io::{BufRead as _, BufReader, Write as _},
    os::unix::{fs::FileTypeExt as _, net::UnixStream},
    path::{Path, PathBuf},
    time::Duration,
};

use nssa::{
    AccountId, PrivateKey, PublicKey, Signature,
    public_transaction::{Message, WitnessSet},
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

/// How long one signing request may take end to end.
const WALLET_SIGNER_TIMEOUT: Duration = Duration::from_secs(30);
/// The largest response line a signer may send.
const MAX_RESPONSE_BYTES: usize = 1024;

/// Why a signer produced no usable witness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessageSignerError {
    /// The signer could not be reached or did not answer in time.
    Unavailable,
    /// The signer answered with something that is not a witness.
    Malformed,
    /// The signer refused (for example: it holds no key for the account).
    Refused,
}

impl fmt::Display for MessageSignerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unavailable => "the LEZ signer is unavailable",
            Self::Malformed => "the LEZ signer answered with no valid witness",
            Self::Refused => "the LEZ signer refused to sign",
        })
    }
}

impl std::error::Error for MessageSignerError {}

/// Turns one planned public-transaction message into its single witness.
pub trait MessageSigner: Send + Sync + fmt::Debug {
    /// The account whose key signs.
    fn account_id(&self) -> AccountId;

    /// The witness over exactly `message`.
    ///
    /// # Errors
    ///
    /// Returns why no witness is available; the caller never submits without one.
    fn witness(&self, message: &Message) -> Result<WitnessSet, MessageSignerError>;
}

/// The role's own key, held in zeroizing memory.
pub struct KeyFileSigner {
    key_bytes: Zeroizing<[u8; 32]>,
    account_id: AccountId,
}

impl KeyFileSigner {
    /// Takes ownership of the key and keeps only its zeroizing bytes.
    #[must_use]
    #[allow(
        clippy::needless_pass_by_value,
        reason = "the signer takes ownership of the upstream key before retaining only zeroizing bytes"
    )]
    pub fn new(key: PrivateKey) -> Self {
        let account_id = AccountId::from(&PublicKey::new_from_private_key(&key));
        Self {
            key_bytes: Zeroizing::new(*key.value()),
            account_id,
        }
    }
}

impl fmt::Debug for KeyFileSigner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KeyFileSigner")
            .field("key", &"[REDACTED]")
            .field("account_id", &self.account_id)
            .finish_non_exhaustive()
    }
}

impl MessageSigner for KeyFileSigner {
    fn account_id(&self) -> AccountId {
        self.account_id
    }

    fn witness(&self, message: &Message) -> Result<WitnessSet, MessageSignerError> {
        let key =
            PrivateKey::try_new(*self.key_bytes).map_err(|_| MessageSignerError::Malformed)?;
        Ok(WitnessSet::for_message(message, &[&key]))
    }
}

/// The LEZ wallet module (`lez-wallet-signer`) holding the role's key, over
/// its owner-only unix socket. One request per connection: a JSON line
/// `{"account_id", "message_hash"}` (64 hex each), answered by a JSON line
/// `{"signature", "public_key"}` (128 and 64 hex) or `{"error"}`.
#[derive(Debug)]
pub struct WalletModuleSigner {
    socket: PathBuf,
    account_id: AccountId,
}

#[derive(Serialize)]
struct SignRequest<'a> {
    account_id: &'a str,
    message_hash: &'a str,
}

#[derive(Deserialize)]
struct SignResponse {
    signature: Option<String>,
    public_key: Option<String>,
    error: Option<String>,
}

impl WalletModuleSigner {
    /// Binds the socket and the account the wallet signs for.
    ///
    /// # Errors
    ///
    /// Rejects a socket path that is not an existing unix socket.
    pub fn new(
        socket: impl AsRef<Path>,
        account_id: AccountId,
    ) -> Result<Self, MessageSignerError> {
        let socket = socket.as_ref().to_path_buf();
        let metadata =
            std::fs::symlink_metadata(&socket).map_err(|_| MessageSignerError::Unavailable)?;
        if !metadata.file_type().is_socket() {
            return Err(MessageSignerError::Unavailable);
        }
        Ok(Self { socket, account_id })
    }

    fn request(&self, message_hash: &[u8; 32]) -> Result<SignResponse, MessageSignerError> {
        let mut stream =
            UnixStream::connect(&self.socket).map_err(|_| MessageSignerError::Unavailable)?;
        stream
            .set_read_timeout(Some(WALLET_SIGNER_TIMEOUT))
            .and_then(|()| stream.set_write_timeout(Some(WALLET_SIGNER_TIMEOUT)))
            .map_err(|_| MessageSignerError::Unavailable)?;
        let account = hex::encode(self.account_id.value());
        let hash = hex::encode(message_hash);
        let mut line = serde_json::to_vec(&SignRequest {
            account_id: &account,
            message_hash: &hash,
        })
        .map_err(|_| MessageSignerError::Malformed)?;
        line.push(b'\n');
        stream
            .write_all(&line)
            .map_err(|_| MessageSignerError::Unavailable)?;
        let mut response = Vec::new();
        BufReader::new(std::io::Read::take(&stream, MAX_RESPONSE_BYTES as u64))
            .read_until(b'\n', &mut response)
            .map_err(|_| MessageSignerError::Unavailable)?;
        if response.last() != Some(&b'\n') {
            return Err(MessageSignerError::Malformed);
        }
        serde_json::from_slice(&response).map_err(|_| MessageSignerError::Malformed)
    }
}

impl MessageSigner for WalletModuleSigner {
    fn account_id(&self) -> AccountId {
        self.account_id
    }

    fn witness(&self, message: &Message) -> Result<WitnessSet, MessageSignerError> {
        let response = self.request(&message.hash())?;
        if response.error.is_some() {
            return Err(MessageSignerError::Refused);
        }
        let mut signature = [0_u8; 64];
        let mut public_key = [0_u8; 32];
        hex::decode_to_slice(
            response.signature.ok_or(MessageSignerError::Malformed)?,
            &mut signature,
        )
        .map_err(|_| MessageSignerError::Malformed)?;
        hex::decode_to_slice(
            response.public_key.ok_or(MessageSignerError::Malformed)?,
            &mut public_key,
        )
        .map_err(|_| MessageSignerError::Malformed)?;
        let public_key =
            PublicKey::try_new(public_key).map_err(|_| MessageSignerError::Malformed)?;
        Ok(WitnessSet::from_raw_parts(vec![(
            Signature { value: signature },
            public_key,
        )]))
    }
}

/// Whether `witness` is exactly one valid signature over `message` by `account`.
#[must_use]
pub fn witness_binds(witness: &WitnessSet, message: &Message, account: AccountId) -> bool {
    matches!(
        witness.signatures_and_public_keys(),
        [(_, public_key)] if AccountId::from(public_key) == account
    ) && witness.is_valid_for(message)
}

#[cfg(test)]
mod tests {
    use std::{os::unix::net::UnixListener, thread};

    use super::*;

    fn message(signer: AccountId) -> Message {
        Message::try_new([7; 8], vec![signer], vec![3_u128.into()], vec![1_u32, 2, 3])
            .expect("message")
    }

    /// A stand-in for lez-wallet-signer: answers one request per connection with
    /// the given key, or with `answer` verbatim.
    fn serve(
        socket: &Path,
        key: Option<PrivateKey>,
        answer: Option<&'static str>,
    ) -> thread::JoinHandle<String> {
        let listener = UnixListener::bind(socket).expect("bind");
        thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).expect("read");
            let reply = answer.map_or_else(
                || {
                    let request: serde_json::Value = serde_json::from_str(&line).expect("json");
                    let mut hash = [0_u8; 32];
                    hex::decode_to_slice(
                        request["message_hash"].as_str().expect("hash"),
                        &mut hash,
                    )
                    .expect("hex");
                    let key = key.expect("key");
                    serde_json::json!({
                        "signature": hex::encode(Signature::new(&key, &hash).value),
                        "public_key": hex::encode(PublicKey::new_from_private_key(&key).value()),
                    })
                    .to_string()
                },
                str::to_owned,
            );
            (&stream)
                .write_all(format!("{reply}\n").as_bytes())
                .expect("write");
            line
        })
    }

    #[test]
    fn key_file_signer_witness_binds_its_account() {
        let key = PrivateKey::try_new([5; 32]).expect("key");
        let signer = KeyFileSigner::new(key);
        let message = message(signer.account_id());
        let witness = signer.witness(&message).expect("witness");
        assert!(witness_binds(&witness, &message, signer.account_id()));
        assert!(format!("{signer:?}").contains("[REDACTED]"));
    }

    #[test]
    fn wallet_module_signer_returns_the_wallet_witness_over_the_message_hash() {
        let directory = tempfile::tempdir().expect("tempdir");
        let socket = directory.path().join("signer.sock");
        let key = PrivateKey::try_new([9; 32]).expect("key");
        let account = AccountId::from(&PublicKey::new_from_private_key(&key));
        let server = serve(&socket, Some(key), None);
        let signer = WalletModuleSigner::new(&socket, account).expect("signer");
        let message = message(account);
        let witness = signer.witness(&message).expect("witness");
        assert!(witness_binds(&witness, &message, account));
        let request: serde_json::Value =
            serde_json::from_str(&server.join().expect("server")).expect("json");
        assert_eq!(request["account_id"], hex::encode(account.value()));
        assert_eq!(request["message_hash"], hex::encode(message.hash()));
    }

    #[test]
    fn a_witness_by_another_key_does_not_bind() {
        let directory = tempfile::tempdir().expect("tempdir");
        let socket = directory.path().join("signer.sock");
        let expected = AccountId::from(&PublicKey::new_from_private_key(
            &PrivateKey::try_new([9; 32]).expect("key"),
        ));
        let server = serve(
            &socket,
            Some(PrivateKey::try_new([4; 32]).expect("key")),
            None,
        );
        let signer = WalletModuleSigner::new(&socket, expected).expect("signer");
        let message = message(expected);
        let witness = signer.witness(&message).expect("witness");
        server.join().expect("server");
        assert!(!witness_binds(&witness, &message, expected));
    }

    #[test]
    fn a_refusal_or_garbage_is_no_witness() {
        let directory = tempfile::tempdir().expect("tempdir");
        let account = AccountId::new([1; 32]);
        for (name, answer, expected) in [
            (
                "refused.sock",
                r#"{"error":"no key"}"#,
                MessageSignerError::Refused,
            ),
            (
                "garbage.sock",
                r#"{"signature":"zz","public_key":"00"}"#,
                MessageSignerError::Malformed,
            ),
        ] {
            let socket = directory.path().join(name);
            let server = serve(&socket, None, Some(answer));
            let signer = WalletModuleSigner::new(&socket, account).expect("signer");
            assert_eq!(signer.witness(&message(account)).unwrap_err(), expected);
            server.join().expect("server");
        }
    }

    #[test]
    fn a_missing_socket_is_unavailable() {
        let directory = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            WalletModuleSigner::new(
                directory.path().join("absent.sock"),
                AccountId::new([1; 32])
            )
            .unwrap_err(),
            MessageSignerError::Unavailable
        );
    }
}
