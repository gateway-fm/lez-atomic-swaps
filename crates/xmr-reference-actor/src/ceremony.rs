//! The claim and refund adaptor ceremony of one Monero swap as each role
//! drives it over Chat (R3–R6 of `xmr_reserve_v1`).
//!
//! The Bitcoin model (`btc-role-lifecycle/src/ceremony.rs`): the Taker drives
//! every round and the Maker answers; both sessions advance together, each in
//! the role's one adaptor journal (`adaptor.sqlite3` in the role root), so a
//! repeated request yields the same bytes and never takes a second nonce. The
//! session ids derive from the agreement commitment, so none travel. The
//! claim seat signs with `claim.key`, the refund seat with `refund.key`.
//!
//! The Taker withholds its claim partial: only its refund partial crosses
//! Chat, and Stage B carries a commitment to the claim partial, which the
//! Taker publishes on LEZ at tag 14. The Maker's claim seat therefore stays at
//! its own partial until then.

use std::{fs::File, path::Path};

use anyhow::{Context as _, Result, ensure};
use lez_adaptor_role_runner::{CeremonySeat, Role, ValidatedSession};
use lez_xmr_swap_sdk::{
    MAX_XMR_ACTIVATION_WIRE_BYTES, MAX_XMR_AGREEMENT_WIRE_BYTES, MoneroPrivateViewKey,
    ValidatedXmrActivationBodyV1, XmrActivatedAgreementV1, XmrAgreementV1, XmrRoleV1,
};
use secp256k1::Secp256k1;
use zeroize::Zeroizing;

use crate::{
    AGREEMENT_KEY_FILE, ActorRole, CLAIM_KEY_FILE, LoadedSecpKey, REFUND_KEY_FILE, VIEW_KEY_FILE,
    compose_activation_body, open_private_directory, read_private_manifest_at, read_secret_hex_at,
    reserve::{
        XMR_ADAPTOR_JOURNAL_FILE, XMR_STAGE_A_FILE, XMR_STAGE_B_FILE, read_role_file,
        sign_commitment, write_role_file,
    },
};

/// One packet per session in one round.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XmrCeremonyPackets {
    pub claim: Vec<u8>,
    pub refund: Vec<u8>,
}

/// The Taker's unsigned Stage B and its signature over it (R5).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct XmrTakerStageB {
    pub unsigned_stage_b_wire: Vec<u8>,
    pub taker_stage_b_signature: [u8; 64],
}

/// One role's countersigned Stage A and the private keys its ceremony uses,
/// each checked against the identity Stage A commits for the role.
struct RoleSwap {
    agreement: XmrAgreementV1,
    agreement_key: Zeroizing<[u8; 32]>,
    claim_key: Zeroizing<[u8; 32]>,
    refund_key: Zeroizing<[u8; 32]>,
    view: MoneroPrivateViewKey,
    claim: CeremonySeat,
    refund: CeremonySeat,
}

impl RoleSwap {
    fn open(role_root: &Path, role: ActorRole) -> Result<Self> {
        let wire = read_role_file(
            role_root,
            XMR_STAGE_A_FILE,
            MAX_XMR_AGREEMENT_WIRE_BYTES as u64,
            "signed Stage A",
        )?;
        let agreement = XmrAgreementV1::from_wire(&wire).context("signed Stage A is invalid")?;
        let identity = agreement.body().participants().for_role(match role {
            ActorRole::Maker => XmrRoleV1::Maker,
            ActorRole::Taker => XmrRoleV1::Taker,
        });
        let root = open_private_directory(role_root, "private role root")?;
        let manifest = read_private_manifest_at(&root)?;
        ensure!(
            manifest.role() == role && manifest.lez_owner_account() == identity.lez_owner_account(),
            "the role root is not this role's in Stage A"
        );
        let agreement_key = read_key(
            &root,
            AGREEMENT_KEY_FILE,
            "agreement",
            identity.agreement_public_key(),
        )?;
        let claim_key = read_key(
            &root,
            CLAIM_KEY_FILE,
            "claim",
            identity.claim_session_public_key(),
        )?;
        let refund_key = read_key(
            &root,
            REFUND_KEY_FILE,
            "refund",
            identity.refund_session_public_key(),
        )?;
        let view = MoneroPrivateViewKey::from_monero_little_endian(*read_secret_hex_at(
            &root,
            VIEW_KEY_FILE,
            "Monero view key",
        )?)
        .context("private Monero view key is invalid")?;
        ensure!(
            view.public_key() == agreement.body().monero().public_view_key(),
            "private Monero view key is not Stage A's"
        );
        let runner = match role {
            ActorRole::Maker => Role::Maker,
            ActorRole::Taker => Role::Taker,
        };
        let journal = role_root.join(XMR_ADAPTOR_JOURNAL_FILE);
        let session = |descriptor: lez_xmr_swap_sdk::XmrAdaptorSessionDescriptorV1, label| {
            ValidatedSession::from_untweaked_context(
                descriptor
                    .context()
                    .with_context(|| format!("{label} session descriptor is invalid"))?,
            )
            .with_context(|| format!("{label} session is invalid"))
        };
        let claim = CeremonySeat::open(
            &journal,
            session(agreement.claim_session_descriptor(), "claim")?,
            runner,
        )
        .context("claim journal")?;
        let refund = CeremonySeat::open(
            &journal,
            session(agreement.refund_session_descriptor(), "refund")?,
            runner,
        )
        .context("refund journal")?;
        Ok(Self {
            agreement,
            agreement_key,
            claim_key,
            refund_key,
            view,
            claim,
            refund,
        })
    }

    fn commitments(&mut self) -> Result<XmrCeremonyPackets> {
        Ok(XmrCeremonyPackets {
            claim: self
                .claim
                .reserve(&self.claim_key)
                .context("claim commitment")?,
            refund: self
                .refund
                .reserve(&self.refund_key)
                .context("refund commitment")?,
        })
    }

    fn accept_commitments(&mut self, peer: &XmrCeremonyPackets) -> Result<()> {
        self.claim
            .accept_commitment(&peer.claim)
            .context("peer claim commitment")?;
        self.refund
            .accept_commitment(&peer.refund)
            .context("peer refund commitment")?;
        Ok(())
    }

    fn nonces(&mut self) -> Result<XmrCeremonyPackets> {
        Ok(XmrCeremonyPackets {
            claim: self.claim.reveal_nonce().context("claim nonce")?,
            refund: self.refund.reveal_nonce().context("refund nonce")?,
        })
    }

    fn sign(&mut self, peer_nonces: &XmrCeremonyPackets) -> Result<XmrCeremonyPackets> {
        Ok(XmrCeremonyPackets {
            claim: self
                .claim
                .accept_nonce_sign(&peer_nonces.claim, &self.claim_key)
                .context("claim partial")?,
            refund: self
                .refund
                .accept_nonce_sign(&peer_nonces.refund, &self.refund_key)
                .context("refund partial")?,
        })
    }
}

impl std::fmt::Debug for RoleSwap {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RoleSwap")
            .field(
                "agreement_commitment",
                &hex::encode(self.agreement.agreement_commitment()),
            )
            .finish_non_exhaustive()
    }
}

fn read_key(
    root: &File,
    name: &str,
    label: &'static str,
    expected: [u8; 33],
) -> Result<Zeroizing<[u8; 32]>> {
    let bytes = read_secret_hex_at(root, name, label)?;
    let loaded = LoadedSecpKey::from_bytes(Zeroizing::new(*bytes), label)?;
    ensure!(
        loaded.public_key(&Secp256k1::new()) == expected,
        "private {label} key is not Stage A's"
    );
    Ok(bytes)
}

/// The Taker's seats: it opens every round.
#[derive(Debug)]
pub struct XmrTakerCeremony {
    swap: RoleSwap,
    role_root: Box<Path>,
}

impl XmrTakerCeremony {
    /// Opens (or reopens) the Taker's journal for the countersigned Stage A in
    /// `role_root`.
    ///
    /// # Errors
    ///
    /// Fails when the role root is not the Taker's in Stage A or the journal
    /// belongs to another session.
    pub fn open(role_root: &Path) -> Result<Self> {
        Ok(Self {
            swap: RoleSwap::open(role_root, ActorRole::Taker)?,
            role_root: role_root.into(),
        })
    }

    /// R3 request: the Taker's nonce commitments.
    ///
    /// # Errors
    ///
    /// Fails when the journal refuses the reservation.
    pub fn commitments(&mut self) -> Result<XmrCeremonyPackets> {
        self.swap.commitments()
    }

    /// Records the Maker's commitments (R3 answer).
    ///
    /// # Errors
    ///
    /// Fails on a packet for another session or a conflicting replay.
    pub fn accept_maker_commitments(&mut self, maker: &XmrCeremonyPackets) -> Result<()> {
        self.swap.accept_commitments(maker)
    }

    /// R4 request: the Taker's public nonces.
    ///
    /// # Errors
    ///
    /// Fails before the Maker's commitments are durable.
    pub fn nonces(&mut self) -> Result<XmrCeremonyPackets> {
        self.swap.nonces()
    }

    /// Verifies the Maker's nonces (R4 answer) and signs both of the Taker's
    /// partials; returns only the refund partial packet, the one R5 carries.
    /// The claim partial stays in the journal until tag 14 publishes it.
    ///
    /// # Errors
    ///
    /// Fails when a Maker nonce does not open its commitment.
    pub fn sign(&mut self, maker_nonces: &XmrCeremonyPackets) -> Result<Vec<u8>> {
        Ok(self.swap.sign(maker_nonces)?.refund)
    }

    /// Verifies the Maker's partials (R4 answer), aggregates both
    /// presignatures, and composes and signs Stage B (R5 request).
    ///
    /// # Errors
    ///
    /// Fails when a Maker partial does not verify or Stage B does not validate.
    pub fn finish(&mut self, maker_partials: &XmrCeremonyPackets) -> Result<XmrTakerStageB> {
        self.swap
            .claim
            .accept_peer_partial(&maker_partials.claim)
            .context("Maker claim partial")?;
        self.swap
            .refund
            .accept_peer_partial(&maker_partials.refund)
            .context("Maker refund partial")?;
        let validated = self.stage_b()?;
        Ok(XmrTakerStageB {
            unsigned_stage_b_wire: validated
                .encode_unsigned_wire()
                .context("encode unsigned Stage B")?,
            taker_stage_b_signature: sign_commitment(
                &self.swap.agreement_key,
                validated.commitment(),
            )?,
        })
    }

    /// Requires the Maker's refund presignature (R5 answer) to be the Taker's
    /// own, attaches both Stage-B signatures and writes the activated Stage B
    /// into the role root; returns its wire (R6 request).
    ///
    /// # Errors
    ///
    /// Fails when the presignatures differ or a signature is invalid.
    pub fn activate(
        &self,
        maker_refund_presignature: [u8; 65],
        maker_stage_b_signature: [u8; 64],
    ) -> Result<Vec<u8>> {
        let validated = self.stage_b()?;
        ensure!(
            validated.body().refund_presignature() == maker_refund_presignature,
            "refund presignatures did not converge"
        );
        let taker_signature = sign_commitment(&self.swap.agreement_key, validated.commitment())?;
        let wire = validated
            .attach_signatures(maker_stage_b_signature, taker_signature)
            .context("the Maker's Stage-B signature is invalid")?
            .encode_wire()
            .context("encode Stage B")?;
        XmrActivatedAgreementV1::from_wire(&self.swap.agreement, &wire, &self.swap.view)
            .context("the activated Stage B is invalid")?;
        write_role_file(&self.role_root, XMR_STAGE_B_FILE, &wire, "signed Stage B")?;
        Ok(wire)
    }

    fn stage_b(&self) -> Result<ValidatedXmrActivationBodyV1> {
        compose_activation_body(
            &self.swap.agreement,
            &self.role_root.join(XMR_ADAPTOR_JOURNAL_FILE),
            &self.swap.view,
        )
    }
}

/// The Maker's seats: it answers every round.
#[derive(Debug)]
pub struct XmrMakerCeremony {
    swap: RoleSwap,
    role_root: Box<Path>,
}

impl XmrMakerCeremony {
    /// Opens (or reopens) the Maker's journal for the countersigned Stage A in
    /// `role_root`.
    ///
    /// # Errors
    ///
    /// Fails when the role root is not the Maker's in Stage A or the journal
    /// belongs to another session.
    pub fn open(role_root: &Path) -> Result<Self> {
        Ok(Self {
            swap: RoleSwap::open(role_root, ActorRole::Maker)?,
            role_root: role_root.into(),
        })
    }

    /// R3: reserves own nonces, records the Taker's commitments and answers
    /// with the Maker's.
    ///
    /// # Errors
    ///
    /// Fails on a foreign packet or a conflicting replay.
    pub fn reserve_round(
        &mut self,
        taker_commitments: &XmrCeremonyPackets,
    ) -> Result<XmrCeremonyPackets> {
        let commitments = self.swap.commitments()?;
        self.swap.accept_commitments(taker_commitments)?;
        Ok(commitments)
    }

    /// R4: verifies the Taker's nonces, reveals own nonces and signs both
    /// Maker partials; answers `(nonces, partials)`.
    ///
    /// # Errors
    ///
    /// Fails when a Taker nonce does not open its commitment.
    pub fn nonce_round(
        &mut self,
        taker_nonces: &XmrCeremonyPackets,
    ) -> Result<(XmrCeremonyPackets, XmrCeremonyPackets)> {
        let nonces = self.swap.nonces()?;
        let partials = self.swap.sign(taker_nonces)?;
        Ok((nonces, partials))
    }

    /// R5: verifies the Taker's refund partial and aggregates the refund
    /// presignature, validates the Taker's Stage B (with this swap's view key)
    /// and its signature, and answers the refund presignature and the
    /// Maker's Stage-B signature. The claim seat stays at the Maker's own
    /// partial.
    ///
    /// # Errors
    ///
    /// Fails when the partial does not verify, Stage B is not this swap's
    /// sessions, or the Taker's signature is invalid.
    pub fn partial_round(
        &mut self,
        taker_refund_partial: &[u8],
        unsigned_stage_b_wire: &[u8],
        taker_stage_b_signature: [u8; 64],
    ) -> Result<([u8; 65], [u8; 64])> {
        self.swap
            .refund
            .accept_peer_partial(taker_refund_partial)
            .context("Taker refund partial")?;
        let presignature = self.swap.refund.presignature()?;
        let validate = || {
            ValidatedXmrActivationBodyV1::from_unsigned_wire(
                &self.swap.agreement,
                unsigned_stage_b_wire,
                &self.swap.view,
            )
            .context("the Taker's Stage B is invalid")
        };
        let validated = validate()?;
        ensure!(
            validated.body().refund_presignature() == presignature,
            "the Taker's Stage B is not this Maker's refund session"
        );
        let maker_signature = sign_commitment(&self.swap.agreement_key, validated.commitment())?;
        validate()?
            .attach_signatures(maker_signature, taker_stage_b_signature)
            .context("the Taker's Stage-B signature is invalid")?;
        Ok((presignature, maker_signature))
    }

    /// R6: accepts the activated Stage B and writes it into the role root.
    ///
    /// # Errors
    ///
    /// Fails for an invalid wire or one over another refund session.
    pub fn accept_activation(&self, stage_b_wire: &[u8]) -> Result<XmrActivatedAgreementV1> {
        let activated =
            XmrActivatedAgreementV1::from_wire(&self.swap.agreement, stage_b_wire, &self.swap.view)
                .context("the activated Stage B is invalid")?;
        ensure!(
            activated.body().refund_presignature() == self.swap.refund.presignature()?,
            "the activated Stage B is not this Maker's refund session"
        );
        ensure!(
            stage_b_wire.len() <= MAX_XMR_ACTIVATION_WIRE_BYTES,
            "the activated Stage B is oversized"
        );
        write_role_file(
            &self.role_root,
            XMR_STAGE_B_FILE,
            stage_b_wire,
            "signed Stage B",
        )?;
        Ok(activated)
    }
}

#[cfg(test)]
mod tests {
    use lez_swap_store::AdaptorSessionPhase;

    use super::*;
    use crate::reserve::{accept_xmr_maker_stage_a, tests::reserve};

    /// R1–R6 in process: both roles end with the same activated Stage B, the
    /// Taker's claim partial never leaves it, and the Maker's rounds replay
    /// byte for byte.
    #[test]
    fn a_full_in_process_ceremony_activates_an_agreement_both_roles_accept() {
        let round = reserve();
        let stage_a = round
            .check(&round.maker, &crate::reserve::tests::taker_check())
            .unwrap();
        accept_xmr_maker_stage_a(&round.maker_root, &stage_a).unwrap();
        let mut taker = XmrTakerCeremony::open(&round.taker_root).unwrap();
        let mut maker = XmrMakerCeremony::open(&round.maker_root).unwrap();

        // R3
        let taker_commitments = taker.commitments().unwrap();
        let maker_commitments = maker.reserve_round(&taker_commitments).unwrap();
        assert_eq!(
            maker.reserve_round(&taker_commitments).unwrap(),
            maker_commitments
        );
        taker.accept_maker_commitments(&maker_commitments).unwrap();
        // R4
        let taker_nonces = taker.nonces().unwrap();
        let (maker_nonces, maker_partials) = maker.nonce_round(&taker_nonces).unwrap();
        assert_eq!(
            maker.nonce_round(&taker_nonces).unwrap(),
            (maker_nonces.clone(), maker_partials.clone())
        );
        let taker_refund_partial = taker.sign(&maker_nonces).unwrap();
        let stage_b = taker.finish(&maker_partials).unwrap();
        // R5
        let (presignature, maker_signature) = maker
            .partial_round(
                &taker_refund_partial,
                &stage_b.unsigned_stage_b_wire,
                stage_b.taker_stage_b_signature,
            )
            .unwrap();
        assert_eq!(
            maker
                .partial_round(
                    &taker_refund_partial,
                    &stage_b.unsigned_stage_b_wire,
                    stage_b.taker_stage_b_signature,
                )
                .unwrap(),
            (presignature, maker_signature)
        );
        // R6
        let activated_wire = taker.activate(presignature, maker_signature).unwrap();
        let activated = maker.accept_activation(&activated_wire).unwrap();
        assert_eq!(activated.encode_wire().unwrap(), activated_wire);
        assert_eq!(
            std::fs::read(round.maker_root.join(XMR_STAGE_B_FILE)).unwrap(),
            std::fs::read(round.taker_root.join(XMR_STAGE_B_FILE)).unwrap()
        );
        // Reopened, the Taker's view key validates what the Maker accepted.
        let reopened = XmrTakerCeremony::open(&round.taker_root).unwrap();
        XmrActivatedAgreementV1::from_wire(
            &reopened.swap.agreement,
            &activated_wire,
            &reopened.swap.view,
        )
        .unwrap();

        // The Taker's claim partial is in neither the R5 request nor Stage B,
        // and the Maker's claim seat stops at its own partial.
        let claim_partial = taker.swap.claim.replay_partial().unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&claim_partial).unwrap();
        let partial = hex::decode(payload["payload"].as_str().unwrap()).unwrap();
        for sent in [
            &taker_refund_partial,
            &stage_b.unsigned_stage_b_wire,
            &activated_wire,
        ] {
            assert!(!sent.windows(partial.len()).any(|bytes| bytes == partial));
        }
        assert_eq!(
            maker.swap.claim.phase().unwrap(),
            Some(AdaptorSessionPhase::PartialPersisted)
        );
        assert_eq!(
            taker.swap.claim.phase().unwrap(),
            Some(AdaptorSessionPhase::PresignatureVerified)
        );
    }

    #[test]
    fn the_maker_refuses_a_stage_b_it_did_not_take_part_in() {
        let round = reserve();
        let stage_a = round
            .check(&round.maker, &crate::reserve::tests::taker_check())
            .unwrap();
        accept_xmr_maker_stage_a(&round.maker_root, &stage_a).unwrap();
        let mut taker = XmrTakerCeremony::open(&round.taker_root).unwrap();
        let mut maker = XmrMakerCeremony::open(&round.maker_root).unwrap();
        let maker_commitments = maker.reserve_round(&taker.commitments().unwrap()).unwrap();
        taker.accept_maker_commitments(&maker_commitments).unwrap();
        let (maker_nonces, maker_partials) = maker.nonce_round(&taker.nonces().unwrap()).unwrap();
        let taker_refund_partial = taker.sign(&maker_nonces).unwrap();
        let stage_b = taker.finish(&maker_partials).unwrap();
        // A changed Stage-B byte, or a signature that is not the Taker's.
        let mut edited = stage_b.unsigned_stage_b_wire.clone();
        let last = edited.len() - 1;
        edited[last] ^= 1;
        assert!(
            maker
                .partial_round(
                    &taker_refund_partial,
                    &edited,
                    stage_b.taker_stage_b_signature
                )
                .is_err()
        );
        let mut signature = stage_b.taker_stage_b_signature;
        signature[0] ^= 1;
        assert!(
            maker
                .partial_round(
                    &taker_refund_partial,
                    &stage_b.unsigned_stage_b_wire,
                    signature
                )
                .is_err()
        );
        // A Maker signature that is not the Maker's is refused by the Taker.
        let (presignature, maker_signature) = maker
            .partial_round(
                &taker_refund_partial,
                &stage_b.unsigned_stage_b_wire,
                stage_b.taker_stage_b_signature,
            )
            .unwrap();
        let mut forged = maker_signature;
        forged[0] ^= 1;
        assert!(taker.activate(presignature, forged).is_err());
        let mut other = presignature;
        other[1] ^= 1;
        assert!(taker.activate(other, maker_signature).is_err());
        assert!(!round.taker_root.join(XMR_STAGE_B_FILE).exists());
        taker.activate(presignature, maker_signature).unwrap();
    }
}
