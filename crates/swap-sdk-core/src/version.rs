//! Small nonzero protocol and schema version values.

use std::num::NonZeroU16;

use serde::{Deserialize, Serialize};

/// Protocol-semantics version bound into signed swap terms.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
#[must_use]
pub struct ProtocolVersion(NonZeroU16);

impl ProtocolVersion {
    /// Initial protocol version.
    pub const V1: Self = Self(NonZeroU16::MIN);

    /// Constructs a nonzero protocol version.
    ///
    /// # Errors
    ///
    /// Returns [`VersionError::Zero`] for the reserved zero value.
    pub const fn new(value: u16) -> Result<Self, VersionError> {
        match NonZeroU16::new(value) {
            Some(value) => Ok(Self(value)),
            None => Err(VersionError::Zero),
        }
    }

    /// Numeric protocol version.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0.get()
    }
}

/// Encoding/schema version carried by a public wire or persistence DTO.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
#[must_use]
pub struct SchemaVersion(NonZeroU16);

impl SchemaVersion {
    /// Initial schema version.
    pub const V1: Self = Self(NonZeroU16::MIN);

    /// Constructs a nonzero schema version.
    ///
    /// # Errors
    ///
    /// Returns [`VersionError::Zero`] for the reserved zero value.
    pub const fn new(value: u16) -> Result<Self, VersionError> {
        match NonZeroU16::new(value) {
            Some(value) => Ok(Self(value)),
            None => Err(VersionError::Zero),
        }
    }

    /// Numeric schema version.
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0.get()
    }
}

/// Invalid shared version value.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum VersionError {
    /// Version zero is reserved and cannot identify a protocol or schema.
    #[error("version zero is reserved")]
    Zero,
}

#[cfg(test)]
mod tests {
    use super::{ProtocolVersion, SchemaVersion, VersionError};

    #[test]
    fn versions_reject_zero() {
        assert_eq!(ProtocolVersion::new(0), Err(VersionError::Zero));
        assert_eq!(SchemaVersion::new(0), Err(VersionError::Zero));
        assert_eq!(ProtocolVersion::new(1).expect("v1").get(), 1);
        assert_eq!(SchemaVersion::new(2).expect("v2").get(), 2);
    }
}
