use core::fmt;

use bitcoin::consensus::serialize;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::{Message, Secp256k1};
use bitcoin::sighash::{Prevouts, SighashCache, TapSighashType};
use bitcoin::taproot;
use bitcoin::{
    Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness, absolute, transaction,
};

use crate::P2trSwapOutput;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SpendValueError {
    NullFundingOutpoint,
    EmptyOutputs,
    FundingValueOutOfRange,
    OutputValueOverflow,
    Overspend,
    ZeroFee,
}

fn validate_spend_values(
    funding_outpoint: OutPoint,
    funding_value: Amount,
    outputs: &[TxOut],
) -> Result<Amount, SpendValueError> {
    if funding_outpoint.is_null() {
        return Err(SpendValueError::NullFundingOutpoint);
    }
    if outputs.is_empty() {
        return Err(SpendValueError::EmptyOutputs);
    }
    if funding_value > Amount::MAX_MONEY {
        return Err(SpendValueError::FundingValueOutOfRange);
    }
    let output_value = outputs.iter().try_fold(0_u64, |sum, output| {
        sum.checked_add(output.value.to_sat())
            .ok_or(SpendValueError::OutputValueOverflow)
    })?;
    let funding_value_sat = funding_value.to_sat();
    if output_value > funding_value_sat {
        return Err(SpendValueError::Overspend);
    }
    if output_value == funding_value_sat {
        return Err(SpendValueError::ZeroFee);
    }
    Ok(Amount::from_sat(funding_value_sat - output_value))
}

/// Invalid cooperative key-path transaction or completed signature.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CooperativeKeyPathSpendError {
    /// The null outpoint cannot identify a real funding output.
    NullFundingOutpoint,
    /// A spend with no outputs would be an accidental full-value fee.
    EmptyOutputs,
    /// Funding value exceeds Bitcoin Core's consensus money range.
    FundingValueOutOfRange,
    /// Output value addition exceeded the Bitcoin money representation.
    OutputValueOverflow,
    /// Output value exceeds the funding value.
    Overspend,
    /// Input and outputs are equal, leaving no miner fee.
    ZeroFee,
    /// The canonical library could not compute the BIP-341 digest.
    Sighash,
    /// The supplied 64 bytes are not a canonical Schnorr signature.
    InvalidSignatureEncoding,
    /// The signature does not verify for the exact digest and tweaked key `Q`.
    SignatureVerification,
}

impl fmt::Display for CooperativeKeyPathSpendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NullFundingOutpoint => "cooperative spend funding outpoint is null",
            Self::EmptyOutputs => "cooperative spend must have at least one output",
            Self::FundingValueOutOfRange => {
                "cooperative spend funding value exceeds Bitcoin MAX_MONEY"
            }
            Self::OutputValueOverflow => "cooperative spend output value overflow",
            Self::Overspend => "cooperative spend outputs exceed the funding value",
            Self::ZeroFee => "cooperative spend fee must be nonzero",
            Self::Sighash => "failed to compute the cooperative BIP-341 sighash",
            Self::InvalidSignatureEncoding => "invalid canonical Schnorr signature encoding",
            Self::SignatureVerification => {
                "cooperative signature does not verify under the tweaked output key"
            }
        })
    }
}

impl std::error::Error for CooperativeKeyPathSpendError {}

impl From<SpendValueError> for CooperativeKeyPathSpendError {
    fn from(value: SpendValueError) -> Self {
        match value {
            SpendValueError::NullFundingOutpoint => Self::NullFundingOutpoint,
            SpendValueError::EmptyOutputs => Self::EmptyOutputs,
            SpendValueError::FundingValueOutOfRange => Self::FundingValueOutOfRange,
            SpendValueError::OutputValueOverflow => Self::OutputValueOverflow,
            SpendValueError::Overspend => Self::Overspend,
            SpendValueError::ZeroFee => Self::ZeroFee,
        }
    }
}

/// Unsigned one-input BIP-341 key-path spend bound to its exact prevout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CooperativeKeyPathSpend {
    unsigned_transaction: Transaction,
    funding_outpoint: OutPoint,
    funding_prevout: TxOut,
    sighash: [u8; 32],
    fee: Amount,
    output_key: bitcoin::key::TweakedPublicKey,
}

impl CooperativeKeyPathSpend {
    /// Constructs the exact v2, locktime-zero, RBF-signalling transaction and
    /// its `SIGHASH_DEFAULT` key-path digest. Annexes are not supported.
    ///
    /// # Errors
    ///
    /// Returns [`CooperativeKeyPathSpendError`] for a null outpoint, empty or
    /// invalid output totals, a zero fee, or a canonical sighash failure.
    pub fn new(
        contract: &P2trSwapOutput,
        funding_outpoint: OutPoint,
        funding_value: Amount,
        outputs: Vec<TxOut>,
    ) -> Result<Self, CooperativeKeyPathSpendError> {
        let fee = validate_spend_values(funding_outpoint, funding_value, &outputs)?;
        let unsigned_transaction = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: funding_outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::new(),
            }],
            output: outputs,
        };
        let funding_prevout = TxOut {
            value: funding_value,
            script_pubkey: contract.script_pubkey().clone(),
        };
        let prevouts = [funding_prevout.clone()];
        let sighash = SighashCache::new(&unsigned_transaction)
            .taproot_key_spend_signature_hash(0, &Prevouts::All(&prevouts), TapSighashType::Default)
            .map_err(|_| CooperativeKeyPathSpendError::Sighash)?
            .to_byte_array();

        Ok(Self {
            unsigned_transaction,
            funding_outpoint,
            funding_prevout,
            sighash,
            fee,
            output_key: contract.output_key(),
        })
    }

    /// Returns the unsigned canonical transaction.
    #[must_use]
    pub const fn unsigned_transaction(&self) -> &Transaction {
        &self.unsigned_transaction
    }

    /// Returns its consensus encoding.
    #[must_use]
    pub fn unsigned_transaction_bytes(&self) -> Vec<u8> {
        serialize(&self.unsigned_transaction)
    }

    /// Returns the exact funding outpoint committed by input zero.
    #[must_use]
    pub const fn funding_outpoint(&self) -> OutPoint {
        self.funding_outpoint
    }

    /// Returns the exact P2TR prevout used by the BIP-341 digest.
    #[must_use]
    pub const fn funding_prevout(&self) -> &TxOut {
        &self.funding_prevout
    }

    /// Returns the exact 32-byte BIP-341 digest for the adaptor transcript.
    #[must_use]
    pub const fn sighash_bytes(&self) -> [u8; 32] {
        self.sighash
    }

    /// Returns the transaction fee.
    #[must_use]
    pub const fn fee(&self) -> Amount {
        self.fee
    }

    /// Verifies a completed `SIGHASH_DEFAULT` aggregate signature under the
    /// tweaked key `Q` before creating the one-item key-path witness.
    ///
    /// # Errors
    ///
    /// Returns [`CooperativeKeyPathSpendError`] if the signature encoding is
    /// invalid or it does not verify for this exact transaction and key `Q`.
    pub fn finalize(
        mut self,
        signature_bytes: [u8; 64],
    ) -> Result<Transaction, CooperativeKeyPathSpendError> {
        let signature = taproot::Signature::from_slice(&signature_bytes)
            .map_err(|_| CooperativeKeyPathSpendError::InvalidSignatureEncoding)?;
        let message = Message::from_digest(self.sighash);
        Secp256k1::verification_only()
            .verify_schnorr(
                &signature.signature,
                &message,
                &self.output_key.to_x_only_public_key(),
            )
            .map_err(|_| CooperativeKeyPathSpendError::SignatureVerification)?;
        self.unsigned_transaction.input[0].witness = Witness::p2tr_key_spend(&signature);
        Ok(self.unsigned_transaction)
    }
}

/// Invalid unilateral BIP-342 CSV refund transaction or completed signature.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefundScriptPathSpendError {
    /// The null outpoint cannot identify a real funding output.
    NullFundingOutpoint,
    /// A spend with no outputs would be an accidental full-value fee.
    EmptyOutputs,
    /// Funding value exceeds Bitcoin Core's consensus money range.
    FundingValueOutOfRange,
    /// Output value addition exceeded the Bitcoin money representation.
    OutputValueOverflow,
    /// Output value exceeds the funding value.
    Overspend,
    /// The input and outputs are equal, leaving no miner fee.
    ZeroFee,
    /// The canonical library could not compute the BIP-342 digest.
    Sighash,
    /// The supplied 64 bytes are not a canonical Schnorr signature.
    InvalidSignatureEncoding,
    /// The signature does not verify for the exact digest and refund key.
    SignatureVerification,
}

impl fmt::Display for RefundScriptPathSpendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NullFundingOutpoint => "refund funding outpoint is null",
            Self::EmptyOutputs => "refund spend must have at least one output",
            Self::FundingValueOutOfRange => "refund funding value exceeds Bitcoin MAX_MONEY",
            Self::OutputValueOverflow => "refund output value overflow",
            Self::Overspend => "refund outputs exceed the funding value",
            Self::ZeroFee => "refund fee must be nonzero",
            Self::Sighash => "failed to compute the refund BIP-342 sighash",
            Self::InvalidSignatureEncoding => "invalid canonical Schnorr signature encoding",
            Self::SignatureVerification => {
                "refund signature does not verify under the signed refund key"
            }
        })
    }
}

impl std::error::Error for RefundScriptPathSpendError {}

impl From<SpendValueError> for RefundScriptPathSpendError {
    fn from(value: SpendValueError) -> Self {
        match value {
            SpendValueError::NullFundingOutpoint => Self::NullFundingOutpoint,
            SpendValueError::EmptyOutputs => Self::EmptyOutputs,
            SpendValueError::FundingValueOutOfRange => Self::FundingValueOutOfRange,
            SpendValueError::OutputValueOverflow => Self::OutputValueOverflow,
            SpendValueError::Overspend => Self::Overspend,
            SpendValueError::ZeroFee => Self::ZeroFee,
        }
    }
}

/// Unsigned one-input BIP-342 script-path refund bound to its exact prevout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefundScriptPathSpend {
    unsigned_transaction: Transaction,
    funding_outpoint: OutPoint,
    funding_prevout: TxOut,
    sighash: [u8; 32],
    fee: Amount,
    refund_key: bitcoin::secp256k1::XOnlyPublicKey,
    refund_script: ScriptBuf,
    refund_control_block: Vec<u8>,
}

impl RefundScriptPathSpend {
    /// Constructs the exact v2, locktime-zero, single-input refund transaction.
    ///
    /// Input zero uses the agreement's block-based BIP-68 sequence. Its
    /// `SIGHASH_DEFAULT` BIP-342 digest commits the exact funding output and
    /// refund tapleaf. Annexes and additional tapscript leaves are not supported.
    ///
    /// # Errors
    ///
    /// Returns [`RefundScriptPathSpendError`] for a null outpoint, empty or
    /// invalid output totals, a zero fee, or a canonical sighash failure.
    pub fn new(
        contract: &P2trSwapOutput,
        funding_outpoint: OutPoint,
        funding_value: Amount,
        outputs: Vec<TxOut>,
    ) -> Result<Self, RefundScriptPathSpendError> {
        let fee = validate_spend_values(funding_outpoint, funding_value, &outputs)?;
        let unsigned_transaction = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: funding_outpoint,
                script_sig: ScriptBuf::new(),
                sequence: contract.refund_delay().bitcoin_sequence(),
                witness: Witness::new(),
            }],
            output: outputs,
        };
        let funding_prevout = TxOut {
            value: funding_value,
            script_pubkey: contract.script_pubkey().clone(),
        };
        let prevouts = [funding_prevout.clone()];
        let sighash = SighashCache::new(&unsigned_transaction)
            .taproot_script_spend_signature_hash(
                0,
                &Prevouts::All(&prevouts),
                contract.tapleaf_hash(),
                TapSighashType::Default,
            )
            .map_err(|_| RefundScriptPathSpendError::Sighash)?
            .to_byte_array();

        Ok(Self {
            unsigned_transaction,
            funding_outpoint,
            funding_prevout,
            sighash,
            fee,
            refund_key: contract.refund_key(),
            refund_script: contract.refund_script().clone(),
            refund_control_block: contract.refund_control_block().serialize(),
        })
    }

    /// Returns the unsigned canonical transaction.
    #[must_use]
    pub const fn unsigned_transaction(&self) -> &Transaction {
        &self.unsigned_transaction
    }

    /// Returns its consensus encoding.
    #[must_use]
    pub fn unsigned_transaction_bytes(&self) -> Vec<u8> {
        serialize(&self.unsigned_transaction)
    }

    /// Returns the exact funding outpoint committed by input zero.
    #[must_use]
    pub const fn funding_outpoint(&self) -> OutPoint {
        self.funding_outpoint
    }

    /// Returns the exact P2TR prevout used by the BIP-342 digest.
    #[must_use]
    pub const fn funding_prevout(&self) -> &TxOut {
        &self.funding_prevout
    }

    /// Returns the exact 32-byte BIP-342 digest signed by the refund key.
    #[must_use]
    pub const fn sighash_bytes(&self) -> [u8; 32] {
        self.sighash
    }

    /// Returns the exact transaction fee.
    #[must_use]
    pub const fn fee(&self) -> Amount {
        self.fee
    }

    /// Verifies the funder's `SIGHASH_DEFAULT` signature before assembling the
    /// signature/script/control-block witness required by BIP-342.
    ///
    /// # Errors
    ///
    /// Returns [`RefundScriptPathSpendError`] if the signature encoding is
    /// invalid or it does not verify for this exact transaction and refund key.
    pub fn finalize(
        mut self,
        signature_bytes: [u8; 64],
    ) -> Result<Transaction, RefundScriptPathSpendError> {
        let signature = taproot::Signature::from_slice(&signature_bytes)
            .map_err(|_| RefundScriptPathSpendError::InvalidSignatureEncoding)?;
        let message = Message::from_digest(self.sighash);
        Secp256k1::verification_only()
            .verify_schnorr(&signature.signature, &message, &self.refund_key)
            .map_err(|_| RefundScriptPathSpendError::SignatureVerification)?;
        self.unsigned_transaction.input[0].witness = Witness::from_slice(&[
            signature_bytes.as_slice(),
            self.refund_script.as_bytes(),
            self.refund_control_block.as_slice(),
        ]);
        Ok(self.unsigned_transaction)
    }
}

#[cfg(test)]
mod tests {
    use bitcoin::hashes::Hash;
    use bitcoin::key::{Keypair, TapTweak};
    use bitcoin::secp256k1::{SecretKey, XOnlyPublicKey};
    use bitcoin::taproot::TapNodeHash;
    use bitcoin::{Txid, secp256k1};

    use super::*;
    use crate::{CsvBlockDelay, RefundXOnlyKey, TwoPartyAggregateKey};

    fn contract_and_internal_keypair() -> (P2trSwapOutput, Keypair) {
        let secp = Secp256k1::new();
        let internal_secret = SecretKey::from_slice(&[1; 32]).unwrap();
        let refund_secret = SecretKey::from_slice(&[2; 32]).unwrap();
        let internal_keypair = Keypair::from_secret_key(&secp, &internal_secret);
        let refund_keypair = Keypair::from_secret_key(&secp, &refund_secret);
        let (internal_key, _) = internal_keypair.x_only_public_key();
        let (refund_key, _) = refund_keypair.x_only_public_key();
        let contract = P2trSwapOutput::new(
            TwoPartyAggregateKey::from_bytes(internal_key.serialize()).unwrap(),
            RefundXOnlyKey::from_bytes(refund_key.serialize()).unwrap(),
            CsvBlockDelay::new(72).unwrap(),
        )
        .unwrap();
        (contract, internal_keypair)
    }

    fn spend(contract: &P2trSwapOutput) -> CooperativeKeyPathSpend {
        let destination_key = XOnlyPublicKey::from_slice(&[
            0x79, 0xbe, 0x66, 0x7e, 0xf9, 0xdc, 0xbb, 0xac, 0x55, 0xa0, 0x62, 0x95, 0xce, 0x87,
            0x0b, 0x07, 0x02, 0x9b, 0xfc, 0xdb, 0x2d, 0xce, 0x28, 0xd9, 0x59, 0xf2, 0x81, 0x5b,
            0x16, 0xf8, 0x17, 0x98,
        ])
        .unwrap();
        let destination =
            ScriptBuf::new_p2tr(&Secp256k1::verification_only(), destination_key, None);
        CooperativeKeyPathSpend::new(
            contract,
            OutPoint {
                txid: Txid::from_byte_array([3; 32]),
                vout: 1,
            },
            Amount::from_sat(100_000),
            vec![TxOut {
                value: Amount::from_sat(99_000),
                script_pubkey: destination,
            }],
        )
        .unwrap()
    }

    fn refund_spend(contract: &P2trSwapOutput) -> RefundScriptPathSpend {
        RefundScriptPathSpend::new(
            contract,
            OutPoint {
                txid: Txid::from_byte_array([3; 32]),
                vout: 1,
            },
            Amount::from_sat(100_000),
            vec![TxOut {
                value: Amount::from_sat(99_000),
                script_pubkey: ScriptBuf::new_p2tr(
                    &Secp256k1::verification_only(),
                    XOnlyPublicKey::from_slice(&[
                        0x79, 0xbe, 0x66, 0x7e, 0xf9, 0xdc, 0xbb, 0xac, 0x55, 0xa0, 0x62, 0x95,
                        0xce, 0x87, 0x0b, 0x07, 0x02, 0x9b, 0xfc, 0xdb, 0x2d, 0xce, 0x28, 0xd9,
                        0x59, 0xf2, 0x81, 0x5b, 0x16, 0xf8, 0x17, 0x98,
                    ])
                    .unwrap(),
                    None,
                ),
            }],
        )
        .unwrap()
    }

    #[test]
    fn completed_signature_creates_one_item_key_path_witness() {
        let (contract, internal_keypair) = contract_and_internal_keypair();
        let unsigned = spend(&contract);
        assert_eq!(
            bitcoin::consensus::encode::serialize_hex(unsigned.unsigned_transaction()),
            "020000000103030303030303030303030303030303030303030303030303030303030303030100000000fdffffff01b882010000000000225120da4710964f7852695de2da025290e24af6d8c281de5a0b902b7135fd9fd74d2100000000"
        );
        assert_eq!(
            unsigned.sighash_bytes(),
            [
                0xd0, 0xe3, 0x16, 0xbf, 0xc4, 0x29, 0xd4, 0x6e, 0xd9, 0x66, 0xcc, 0xeb, 0x38, 0xbc,
                0x9f, 0xe9, 0xec, 0x49, 0x8d, 0xd2, 0x47, 0x24, 0x0a, 0x6e, 0x6e, 0x85, 0x72, 0x70,
                0xf8, 0x4a, 0xee, 0x20,
            ]
        );
        let secp = Secp256k1::new();
        let merkle_root = TapNodeHash::from_byte_array(contract.merkle_root_bytes());
        let tweaked = internal_keypair.tap_tweak(&secp, Some(merkle_root));
        let message = secp256k1::Message::from_digest(unsigned.sighash_bytes());
        let signature = secp.sign_schnorr_no_aux_rand(&message, tweaked.as_keypair());
        let signature_bytes = signature.serialize();

        let mut changed_outputs = unsigned.unsigned_transaction().output.clone();
        changed_outputs[0].value = Amount::from_sat(98_999);
        let changed = CooperativeKeyPathSpend::new(
            &contract,
            unsigned.funding_outpoint(),
            unsigned.funding_prevout().value,
            changed_outputs,
        )
        .unwrap();
        assert_eq!(
            changed.finalize(signature_bytes),
            Err(CooperativeKeyPathSpendError::SignatureVerification)
        );

        let signed = unsigned.finalize(signature_bytes).unwrap();
        assert_eq!(
            bitcoin::consensus::encode::serialize_hex(&signed),
            "0200000000010103030303030303030303030303030303030303030303030303030303030303030100000000fdffffff01b882010000000000225120da4710964f7852695de2da025290e24af6d8c281de5a0b902b7135fd9fd74d2101407345af8e20686ce0fe32d375e31e84d4ac791b4d04130024fbafe358d0b054109f608aa12edc077aa8a8273306888a2b34bfcb354996b77ea5b79edf667e5d1e00000000"
        );
        assert_eq!(
            signed.compute_txid().to_string(),
            "353ac67e19252a0805301135871aaeb401d44b26aec6f4f0e6516adf74ff9252"
        );
        assert_eq!(
            signed.compute_wtxid().to_string(),
            "7b3a2ccbee21869a4b52794a9f9b593af2c4812ebdfec38c3181e3a44ee5002b"
        );
        assert_eq!(signed.input.len(), 1);
        assert!(signed.input[0].script_sig.is_empty());
        assert_eq!(signed.input[0].witness.len(), 1);
        assert_eq!(signed.input[0].witness.iter().next().unwrap().len(), 64);
    }

    #[test]
    fn transaction_and_signature_boundaries_fail_closed() {
        let (contract, _) = contract_and_internal_keypair();
        let destination = TxOut {
            value: Amount::from_sat(1),
            script_pubkey: ScriptBuf::new(),
        };
        assert_eq!(
            CooperativeKeyPathSpend::new(
                &contract,
                OutPoint::null(),
                Amount::from_sat(2),
                vec![destination.clone()],
            ),
            Err(CooperativeKeyPathSpendError::NullFundingOutpoint)
        );
        assert_eq!(
            CooperativeKeyPathSpend::new(
                &contract,
                OutPoint {
                    txid: Txid::from_byte_array([3; 32]),
                    vout: 0
                },
                Amount::from_sat(2),
                Vec::new(),
            ),
            Err(CooperativeKeyPathSpendError::EmptyOutputs)
        );
        assert_eq!(
            CooperativeKeyPathSpend::new(
                &contract,
                OutPoint {
                    txid: Txid::from_byte_array([3; 32]),
                    vout: 0
                },
                Amount::from_sat(Amount::MAX_MONEY.to_sat() + 1),
                vec![destination.clone()],
            ),
            Err(CooperativeKeyPathSpendError::FundingValueOutOfRange)
        );
        assert_eq!(
            CooperativeKeyPathSpend::new(
                &contract,
                OutPoint {
                    txid: Txid::from_byte_array([3; 32]),
                    vout: 0
                },
                Amount::from_sat(1),
                vec![TxOut {
                    value: Amount::from_sat(2),
                    script_pubkey: ScriptBuf::new()
                }],
            ),
            Err(CooperativeKeyPathSpendError::Overspend)
        );
        assert_eq!(
            CooperativeKeyPathSpend::new(
                &contract,
                OutPoint {
                    txid: Txid::from_byte_array([3; 32]),
                    vout: 0
                },
                Amount::from_sat(1),
                vec![destination],
            ),
            Err(CooperativeKeyPathSpendError::ZeroFee)
        );
        assert_eq!(
            spend(&contract).finalize([0; 64]),
            Err(CooperativeKeyPathSpendError::SignatureVerification)
        );
    }

    #[test]
    fn csv_refund_uses_the_exact_script_path_and_funder_key() {
        let (contract, _) = contract_and_internal_keypair();
        let unsigned = refund_spend(&contract);
        assert_eq!(
            unsigned.unsigned_transaction().input[0]
                .sequence
                .to_consensus_u32(),
            contract.refund_sequence()
        );
        assert_eq!(unsigned.fee(), Amount::from_sat(1_000));

        let secp = Secp256k1::new();
        let refund_keypair = Keypair::from_secret_key(
            &secp,
            &SecretKey::from_slice(&[2; 32]).expect("refund secret"),
        );
        let message = secp256k1::Message::from_digest(unsigned.sighash_bytes());
        let signature = secp.sign_schnorr_no_aux_rand(&message, &refund_keypair);
        let signed = unsigned.finalize(signature.serialize()).unwrap();

        let witness = &signed.input[0].witness;
        assert_eq!(witness.len(), 3);
        let mut items = witness.iter();
        assert_eq!(items.next().unwrap(), signature.as_ref());
        assert_eq!(items.next().unwrap(), contract.refund_script_bytes());
        assert_eq!(
            items.next().unwrap(),
            contract.refund_control_block_bytes().as_slice()
        );
        assert!(items.next().is_none());
    }

    #[test]
    fn csv_refund_rejects_wrong_key_and_changed_outputs() {
        let (contract, _) = contract_and_internal_keypair();
        let unsigned = refund_spend(&contract);
        let secp = Secp256k1::new();
        let wrong_keypair = Keypair::from_secret_key(
            &secp,
            &SecretKey::from_slice(&[3; 32]).expect("wrong secret"),
        );
        let signature = secp.sign_schnorr_no_aux_rand(
            &secp256k1::Message::from_digest(unsigned.sighash_bytes()),
            &wrong_keypair,
        );
        let original_sighash = unsigned.sighash_bytes();
        assert_eq!(
            unsigned.finalize(signature.serialize()),
            Err(RefundScriptPathSpendError::SignatureVerification)
        );

        let changed = RefundScriptPathSpend::new(
            &contract,
            OutPoint {
                txid: Txid::from_byte_array([3; 32]),
                vout: 1,
            },
            Amount::from_sat(100_000),
            vec![TxOut {
                value: Amount::from_sat(98_999),
                script_pubkey: ScriptBuf::new(),
            }],
        )
        .unwrap();
        let refund_keypair = Keypair::from_secret_key(
            &secp,
            &SecretKey::from_slice(&[2; 32]).expect("refund secret"),
        );
        let changed_signature = secp.sign_schnorr_no_aux_rand(
            &secp256k1::Message::from_digest(changed.sighash_bytes()),
            &refund_keypair,
        );
        assert_ne!(original_sighash, changed.sighash_bytes());
        assert_eq!(
            changed.finalize(signature.serialize()),
            Err(RefundScriptPathSpendError::SignatureVerification)
        );
        assert!(changed_signature.serialize() != signature.serialize());
    }
}
