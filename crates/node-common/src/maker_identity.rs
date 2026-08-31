use secp256k1::PublicKey;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};

/// Canonical compressed secp256k1 Maker identity used at role-neutral boundaries.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TakerMakerIdentityV1([u8; 33]);

impl TakerMakerIdentityV1 {
    /// Validates one exact compressed secp256k1 public identity.
    ///
    /// # Errors
    ///
    /// Returns an error when `bytes` do not encode a valid compressed secp256k1 public key.
    pub fn new(bytes: [u8; 33]) -> Result<Self, secp256k1::Error> {
        PublicKey::from_slice(&bytes)?;
        Ok(Self(bytes))
    }

    /// Returns the exact compressed identity bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 33] {
        &self.0
    }
}

impl Serialize for TakerMakerIdentityV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&hex::encode(self.0))
    }
}

impl<'de> Deserialize<'de> for TakerMakerIdentityV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value.len() != 66
            || value
                .bytes()
                .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
        {
            return Err(D::Error::custom(
                "Maker identity is not canonical lowercase hex",
            ));
        }
        let mut bytes = [0_u8; 33];
        hex::decode_to_slice(&value, &mut bytes)
            .map_err(|_| D::Error::custom("Maker identity is not canonical lowercase hex"))?;
        Self::new(bytes)
            .map_err(|_| D::Error::custom("Maker identity is not a compressed secp256k1 point"))
    }
}
