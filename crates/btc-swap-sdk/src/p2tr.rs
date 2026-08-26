use core::fmt;

use bitcoin::hashes::Hash;
use bitcoin::key::{TweakedPublicKey, UntweakedPublicKey};
use bitcoin::opcodes::all::{OP_CHECKSIG, OP_CSV, OP_DROP};
use bitcoin::script::Builder;
use bitcoin::secp256k1::{Parity, Secp256k1, XOnlyPublicKey};
use bitcoin::taproot::{ControlBlock, LeafVersion, TapLeafHash, TapTweakHash, TaprootBuilder};
use bitcoin::{ScriptBuf, Sequence};

/// The role of an x-only key rejected at the public byte boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XOnlyKeyPurpose {
    /// Two-party aggregate internal key for the cooperative key path.
    CooperativeAggregate,
    /// Funder-owned key for the delayed refund tapleaf.
    Refund,
}

/// A supplied byte string is not a canonical secp256k1 x-only public key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidXOnlyKey {
    purpose: XOnlyKeyPurpose,
}

impl InvalidXOnlyKey {
    /// Identifies the rejected protocol key.
    #[must_use]
    pub const fn purpose(self) -> XOnlyKeyPurpose {
        self.purpose
    }
}

impl fmt::Display for InvalidXOnlyKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid {:?} BIP-340 x-only public key",
            self.purpose
        )
    }
}

impl std::error::Error for InvalidXOnlyKey {}

/// Validated two-party aggregate internal key.
///
/// This wrapper proves only canonical BIP-340 encoding. The later `MuSig2`
/// transcript must independently prove that both participant keys were
/// aggregated with the accepted coefficients and parity convention.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TwoPartyAggregateKey(XOnlyPublicKey);

impl TwoPartyAggregateKey {
    /// Parses a canonical 32-byte BIP-340 key.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidXOnlyKey`] when the bytes are not a canonical curve
    /// point encoding.
    pub fn from_bytes(bytes: [u8; 32]) -> Result<Self, InvalidXOnlyKey> {
        XOnlyPublicKey::from_slice(&bytes)
            .map(Self)
            .map_err(|_| InvalidXOnlyKey {
                purpose: XOnlyKeyPurpose::CooperativeAggregate,
            })
    }

    /// Returns the canonical BIP-340 key bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0.serialize()
    }
}

/// Validated funder refund key committed by the CSV tapleaf.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RefundXOnlyKey(XOnlyPublicKey);

impl RefundXOnlyKey {
    /// Parses a canonical 32-byte BIP-340 key.
    ///
    /// # Errors
    ///
    /// Returns [`InvalidXOnlyKey`] when the bytes are not a canonical curve
    /// point encoding.
    pub fn from_bytes(bytes: [u8; 32]) -> Result<Self, InvalidXOnlyKey> {
        XOnlyPublicKey::from_slice(&bytes)
            .map(Self)
            .map_err(|_| InvalidXOnlyKey {
                purpose: XOnlyKeyPurpose::Refund,
            })
    }

    /// Returns the canonical BIP-340 key bytes.
    #[must_use]
    pub fn to_bytes(self) -> [u8; 32] {
        self.0.serialize()
    }
}

/// Invalid block-based BIP-68/BIP-112 delay.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CsvBlockDelayError {
    /// Zero would make the refund immediately eligible.
    Zero,
    /// Block-based relative lock time has a 16-bit consensus payload.
    TooLarge {
        /// Rejected block count.
        blocks: u32,
    },
}

impl fmt::Display for CsvBlockDelayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Zero => formatter.write_str("CSV block delay must be nonzero"),
            Self::TooLarge { blocks } => {
                write!(formatter, "CSV block delay {blocks} exceeds 65535 blocks")
            }
        }
    }
}

impl std::error::Error for CsvBlockDelayError {}

/// Nonzero block-based relative lock time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CsvBlockDelay(u16);

impl CsvBlockDelay {
    /// Constructs a nonzero block delay representable by BIP-68.
    ///
    /// # Errors
    ///
    /// Returns [`CsvBlockDelayError`] for zero or a value above 65,535.
    pub fn new(blocks: u32) -> Result<Self, CsvBlockDelayError> {
        if blocks == 0 {
            return Err(CsvBlockDelayError::Zero);
        }
        let blocks = u16::try_from(blocks).map_err(|_| CsvBlockDelayError::TooLarge { blocks })?;
        Ok(Self(blocks))
    }

    /// Returns the negotiated block count.
    #[must_use]
    pub const fn blocks(self) -> u16 {
        self.0
    }

    /// Returns the exact block-based input sequence required by BIP-68.
    #[must_use]
    pub fn sequence(self) -> u32 {
        Sequence::from_height(self.0).to_consensus_u32()
    }

    pub(crate) fn bitcoin_sequence(self) -> Sequence {
        Sequence::from_height(self.0)
    }
}

/// Parity of the BIP-341 tweaked output key `Q`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputKeyParity {
    /// Even Y coordinate.
    Even,
    /// Odd Y coordinate.
    Odd,
}

impl From<Parity> for OutputKeyParity {
    fn from(parity: Parity) -> Self {
        match parity {
            Parity::Even => Self::Even,
            Parity::Odd => Self::Odd,
        }
    }
}

/// Failure to build or internally verify the one-leaf Taproot commitment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum P2trSwapOutputError {
    /// The standard library rejected the single-leaf tree construction.
    TreeConstruction,
    /// A finalized one-leaf tree unexpectedly had no Merkle root.
    MissingMerkleRoot,
    /// The finalized tree unexpectedly had no proof for the refund leaf.
    MissingControlBlock,
    /// The generated proof did not commit the leaf to the generated output key.
    InvalidControlBlock,
}

impl fmt::Display for P2trSwapOutputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::TreeConstruction => "failed to construct the refund Taproot tree",
            Self::MissingMerkleRoot => "refund Taproot tree has no Merkle root",
            Self::MissingControlBlock => "refund Taproot tree has no control block",
            Self::InvalidControlBlock => "refund control block does not commit to the output key",
        })
    }
}

impl std::error::Error for P2trSwapOutputError {}

/// Exact one-leaf P2TR output selected by ADR 0009.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct P2trSwapOutput {
    aggregate_key: TwoPartyAggregateKey,
    refund_key: RefundXOnlyKey,
    refund_delay: CsvBlockDelay,
    refund_script: ScriptBuf,
    tapleaf_hash: TapLeafHash,
    merkle_root: bitcoin::taproot::TapNodeHash,
    tap_tweak_hash: TapTweakHash,
    output_key: TweakedPublicKey,
    output_key_parity: OutputKeyParity,
    control_block: ControlBlock,
    script_pubkey: ScriptBuf,
}

impl P2trSwapOutput {
    /// Builds and verifies the exact aggregate-key plus CSV-refund commitment.
    ///
    /// # Errors
    ///
    /// Returns [`P2trSwapOutputError`] if the canonical Taproot library cannot
    /// construct or internally verify the one-leaf commitment.
    pub fn new(
        aggregate_key: TwoPartyAggregateKey,
        refund_key: RefundXOnlyKey,
        refund_delay: CsvBlockDelay,
    ) -> Result<Self, P2trSwapOutputError> {
        let refund_script = Builder::new()
            .push_sequence(refund_delay.bitcoin_sequence())
            .push_opcode(OP_CSV)
            .push_opcode(OP_DROP)
            .push_x_only_key(&refund_key.0)
            .push_opcode(OP_CHECKSIG)
            .into_script();
        let leaf_version = LeafVersion::TapScript;
        let secp = Secp256k1::verification_only();
        let spend_info = TaprootBuilder::new()
            .add_leaf(0, refund_script.clone())
            .map_err(|_| P2trSwapOutputError::TreeConstruction)?
            .finalize(&secp, UntweakedPublicKey::from(aggregate_key.0))
            .map_err(|_| P2trSwapOutputError::TreeConstruction)?;
        let merkle_root = spend_info
            .merkle_root()
            .ok_or(P2trSwapOutputError::MissingMerkleRoot)?;
        let tap_tweak_hash = spend_info.tap_tweak();
        let output_key = spend_info.output_key();
        let output_key_parity = spend_info.output_key_parity().into();
        let control_block = spend_info
            .control_block(&(refund_script.clone(), leaf_version))
            .ok_or(P2trSwapOutputError::MissingControlBlock)?;
        if !control_block.verify_taproot_commitment(
            &secp,
            output_key.to_x_only_public_key(),
            &refund_script,
        ) {
            return Err(P2trSwapOutputError::InvalidControlBlock);
        }
        let script_pubkey = ScriptBuf::new_p2tr_tweaked(output_key);
        Ok(Self {
            aggregate_key,
            refund_key,
            refund_delay,
            tapleaf_hash: TapLeafHash::from_script(&refund_script, leaf_version),
            refund_script,
            merkle_root,
            tap_tweak_hash,
            output_key,
            output_key_parity,
            control_block,
            script_pubkey,
        })
    }

    /// Returns the aggregate internal key `P` as canonical BIP-340 bytes.
    #[must_use]
    pub fn aggregate_internal_key_bytes(&self) -> [u8; 32] {
        self.aggregate_key.to_bytes()
    }

    /// Returns the funder refund key as canonical BIP-340 bytes.
    #[must_use]
    pub fn refund_key_bytes(&self) -> [u8; 32] {
        self.refund_key.to_bytes()
    }

    /// Returns the negotiated block-based delay.
    #[must_use]
    pub const fn refund_delay(&self) -> CsvBlockDelay {
        self.refund_delay
    }

    /// Returns the exact BIP-342 refund leaf bytes.
    #[must_use]
    pub fn refund_script_bytes(&self) -> &[u8] {
        self.refund_script.as_bytes()
    }

    /// Returns the consensus leaf-version byte.
    #[must_use]
    pub fn refund_leaf_version(&self) -> u8 {
        LeafVersion::TapScript.to_consensus()
    }

    /// Returns the tagged `TapLeaf` hash.
    #[must_use]
    pub fn tapleaf_hash_bytes(&self) -> [u8; 32] {
        self.tapleaf_hash.to_byte_array()
    }

    /// Returns the one-leaf tree root committed by the tweak.
    #[must_use]
    pub fn merkle_root_bytes(&self) -> [u8; 32] {
        self.merkle_root.to_byte_array()
    }

    /// Returns `H_TapTweak(P || merkle_root)` committed by the agreement.
    #[must_use]
    pub fn tap_tweak_hash_bytes(&self) -> [u8; 32] {
        self.tap_tweak_hash.to_byte_array()
    }

    /// Returns the tweaked output key `Q` as canonical BIP-340 bytes.
    #[must_use]
    pub fn output_key_bytes(&self) -> [u8; 32] {
        self.output_key.to_x_only_public_key().serialize()
    }

    /// Returns the output-key parity committed in the control block.
    #[must_use]
    pub const fn output_key_parity(&self) -> OutputKeyParity {
        self.output_key_parity
    }

    /// Returns the exact script-path proof for the refund leaf.
    #[must_use]
    pub fn refund_control_block_bytes(&self) -> Vec<u8> {
        self.control_block.serialize()
    }

    /// Returns the P2TR scriptPubKey bytes.
    #[must_use]
    pub fn script_pubkey_bytes(&self) -> &[u8] {
        self.script_pubkey.as_bytes()
    }

    /// Returns the exact BIP-68 sequence required by a refund input.
    #[must_use]
    pub fn refund_sequence(&self) -> u32 {
        self.refund_delay.sequence()
    }

    pub(crate) const fn output_key(&self) -> TweakedPublicKey {
        self.output_key
    }

    pub(crate) fn script_pubkey(&self) -> &ScriptBuf {
        &self.script_pubkey
    }

    pub(crate) const fn refund_key(&self) -> XOnlyPublicKey {
        self.refund_key.0
    }

    pub(crate) const fn refund_script(&self) -> &ScriptBuf {
        &self.refund_script
    }

    pub(crate) const fn refund_control_block(&self) -> &ControlBlock {
        &self.control_block
    }

    pub(crate) const fn tapleaf_hash(&self) -> TapLeafHash {
        self.tapleaf_hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GENERATOR_X: [u8; 32] = [
        0x79, 0xbe, 0x66, 0x7e, 0xf9, 0xdc, 0xbb, 0xac, 0x55, 0xa0, 0x62, 0x95, 0xce, 0x87, 0x0b,
        0x07, 0x02, 0x9b, 0xfc, 0xdb, 0x2d, 0xce, 0x28, 0xd9, 0x59, 0xf2, 0x81, 0x5b, 0x16, 0xf8,
        0x17, 0x98,
    ];
    const TWO_G_X: [u8; 32] = [
        0xc6, 0x04, 0x7f, 0x94, 0x41, 0xed, 0x7d, 0x6d, 0x30, 0x45, 0x40, 0x6e, 0x95, 0xc0, 0x7c,
        0xd8, 0x5c, 0x77, 0x8e, 0x4b, 0x8c, 0xef, 0x3c, 0xa7, 0xab, 0xac, 0x09, 0xb9, 0x5c, 0x70,
        0x9e, 0xe5,
    ];

    #[test]
    fn one_leaf_contract_is_self_consistent() {
        let contract = P2trSwapOutput::new(
            TwoPartyAggregateKey::from_bytes(GENERATOR_X).unwrap(),
            RefundXOnlyKey::from_bytes(TWO_G_X).unwrap(),
            CsvBlockDelay::new(144).unwrap(),
        )
        .unwrap();

        let expected_script =
            [&[0x02, 0x90, 0x00, 0xb2, 0x75, 0x20][..], &TWO_G_X, &[0xac]].concat();
        assert_eq!(contract.refund_script_bytes(), expected_script);
        assert_eq!(contract.refund_leaf_version(), 0xc0);
        let expected_tapleaf = [
            0x49, 0xb1, 0x70, 0xc9, 0x8e, 0x17, 0x5f, 0x70, 0xe0, 0x7e, 0x8d, 0x33, 0x73, 0x4f,
            0x8a, 0x2d, 0x8f, 0x37, 0x63, 0x52, 0x9c, 0xe2, 0xa6, 0x7a, 0xec, 0xa9, 0x52, 0x03,
            0x89, 0x25, 0x29, 0x9c,
        ];
        let expected_output_key = [
            0x5e, 0x72, 0xa3, 0x45, 0x9c, 0xd7, 0x87, 0x30, 0xef, 0x1c, 0x4c, 0xbd, 0xa8, 0x46,
            0xfb, 0x18, 0x0c, 0x80, 0x1d, 0xf6, 0x38, 0xf0, 0xb6, 0x35, 0xb4, 0x37, 0x22, 0x74,
            0x89, 0x19, 0xbd, 0x1b,
        ];
        let expected_tap_tweak = [
            0x39, 0x0c, 0xd1, 0x4e, 0x59, 0xb1, 0x9b, 0xcd, 0xd5, 0xbb, 0xf5, 0xb4, 0x06, 0x3a,
            0x41, 0xcb, 0x66, 0x63, 0x0b, 0x33, 0x6c, 0x2e, 0x27, 0x29, 0xa2, 0xb7, 0x11, 0x99,
            0x92, 0xe8, 0x3b, 0xda,
        ];
        let expected_control_block = [&[0xc1][..], &GENERATOR_X].concat();
        assert_eq!(contract.tapleaf_hash_bytes(), expected_tapleaf);
        assert_eq!(contract.merkle_root_bytes(), expected_tapleaf);
        assert_eq!(contract.tap_tweak_hash_bytes(), expected_tap_tweak);
        assert_eq!(contract.output_key_bytes(), expected_output_key);
        assert_eq!(contract.output_key_parity(), OutputKeyParity::Odd);
        assert_eq!(
            contract.refund_control_block_bytes(),
            expected_control_block
        );
        assert_eq!(contract.script_pubkey_bytes()[..2], [0x51, 0x20]);
        assert_eq!(
            contract.script_pubkey_bytes()[2..],
            contract.output_key_bytes()
        );
        assert_eq!(contract.refund_sequence(), 144);
    }

    #[test]
    fn key_and_delay_boundaries_fail_closed() {
        assert_eq!(
            TwoPartyAggregateKey::from_bytes([0xff; 32])
                .unwrap_err()
                .purpose(),
            XOnlyKeyPurpose::CooperativeAggregate
        );
        assert_eq!(
            RefundXOnlyKey::from_bytes([0xff; 32])
                .unwrap_err()
                .purpose(),
            XOnlyKeyPurpose::Refund
        );
        assert_eq!(CsvBlockDelay::new(0), Err(CsvBlockDelayError::Zero));
        assert_eq!(
            CsvBlockDelay::new(65_536),
            Err(CsvBlockDelayError::TooLarge { blocks: 65_536 })
        );
        assert_eq!(CsvBlockDelay::new(65_535).unwrap().sequence(), 65_535);
    }
}
