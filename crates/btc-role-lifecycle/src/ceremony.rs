//! The two-leg adaptor ceremony as each role drives it over Chat.
//!
//! Both legs (Bitcoin claim, LEZ claim) advance together per round so one
//! request/response carries both packets. Every step replays through the
//! journals (`CeremonySeat`), so a repeated request yields the same bytes.

use anyhow::{Context as _, Result, ensure};
use lez_adaptor_role_runner::{CeremonySeat, Role, ValidatedSession};
use lez_btc_swap_sdk::{BtcAdaptorSessionDomain, BtcAgreementV1};
use lez_swap_core::Participant;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::{layout::SwapLayout, wire::LegPacketsV1};

/// The two fresh session ids one ceremony runs under.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LegSessions {
    pub bitcoin: [u8; 32],
    pub lez: [u8; 32],
}

impl LegSessions {
    /// Two distinct random session ids.
    ///
    /// # Errors
    ///
    /// Fails when OS randomness is unavailable.
    pub fn fresh() -> Result<Self> {
        let mut bitcoin = [0_u8; 32];
        let mut lez = [0_u8; 32];
        getrandom::fill(&mut bitcoin).map_err(|_| anyhow::anyhow!("OS randomness unavailable"))?;
        getrandom::fill(&mut lez).map_err(|_| anyhow::anyhow!("OS randomness unavailable"))?;
        ensure!(
            bitcoin != lez && bitcoin != [0; 32] && lez != [0; 32],
            "degenerate session ids"
        );
        Ok(Self { bitcoin, lez })
    }

    /// Both validated sessions derived from the agreement (ADR 0032).
    ///
    /// # Errors
    ///
    /// Fails when the agreement cannot produce a signing context.
    pub fn validated(
        &self,
        agreement: &BtcAgreementV1,
    ) -> Result<(ValidatedSession, ValidatedSession)> {
        ensure!(self.bitcoin != self.lez, "leg session ids must differ");
        let bitcoin = agreement
            .adaptor_session_context(BtcAdaptorSessionDomain::Bitcoin, self.bitcoin)
            .map_err(|error| anyhow::anyhow!("Bitcoin session context: {error}"))?;
        let lez = agreement
            .adaptor_session_context(BtcAdaptorSessionDomain::Lez, self.lez)
            .map_err(|error| anyhow::anyhow!("LEZ session context: {error}"))?;
        Ok((
            ValidatedSession::from_context(bitcoin).context("Bitcoin session")?,
            ValidatedSession::from_context(lez).context("LEZ session")?,
        ))
    }
}

/// Packets for both legs in one round.
pub type CeremonyLegPackets = LegPacketsV1;

const fn runner_role(role: Participant) -> Role {
    match role {
        Participant::Maker => Role::Maker,
        Participant::Taker => Role::Taker,
    }
}

fn open_seats(
    layout: &SwapLayout,
    agreement: &BtcAgreementV1,
    sessions: &LegSessions,
    role: Participant,
) -> Result<(CeremonySeat, CeremonySeat)> {
    let (bitcoin, lez) = sessions.validated(agreement)?;
    Ok((
        CeremonySeat::open(&layout.bitcoin_journal(), bitcoin, runner_role(role))
            .context("Bitcoin journal")?,
        CeremonySeat::open(&layout.lez_journal(), lez, runner_role(role)).context("LEZ journal")?,
    ))
}

/// The presignatures both roles must hold identically at the end.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TakerCeremonyOutcome {
    pub bitcoin_presignature: [u8; 65],
    pub lez_presignature: [u8; 65],
}

/// The Taker's seats: it opens every round.
#[derive(Debug)]
pub struct TakerCeremony {
    bitcoin: CeremonySeat,
    lez: CeremonySeat,
}

impl TakerCeremony {
    /// Opens (or reopens) the Taker's journals for `sessions`.
    ///
    /// # Errors
    ///
    /// Fails when a journal belongs to another session or the agreement is
    /// inconsistent.
    pub fn open(
        layout: &SwapLayout,
        agreement: &BtcAgreementV1,
        sessions: &LegSessions,
    ) -> Result<Self> {
        let (bitcoin, lez) = open_seats(layout, agreement, sessions, Participant::Taker)?;
        Ok(Self { bitcoin, lez })
    }

    /// Round 1 payload: the Taker's nonce commitments.
    ///
    /// # Errors
    ///
    /// Fails when the key does not match the Taker's agreement key.
    pub fn commitments(
        &mut self,
        agreement_key: &Zeroizing<[u8; 32]>,
    ) -> Result<CeremonyLegPackets> {
        Ok(LegPacketsV1 {
            bitcoin: self
                .bitcoin
                .reserve(agreement_key)
                .context("Bitcoin commitment")?,
            lez: self.lez.reserve(agreement_key).context("LEZ commitment")?,
        })
    }

    /// Records the Maker's commitments (round 1 answer).
    ///
    /// # Errors
    ///
    /// Fails on a packet for another session or a conflicting replay.
    pub fn accept_maker_commitments(&mut self, maker: &CeremonyLegPackets) -> Result<()> {
        self.bitcoin
            .accept_commitment(&maker.bitcoin)
            .context("Maker Bitcoin commitment")?;
        self.lez
            .accept_commitment(&maker.lez)
            .context("Maker LEZ commitment")?;
        Ok(())
    }

    /// Round 2 payload: the Taker's public nonces.
    ///
    /// # Errors
    ///
    /// Fails before the Maker's commitments are durable.
    pub fn nonces(&mut self) -> Result<CeremonyLegPackets> {
        Ok(LegPacketsV1 {
            bitcoin: self.bitcoin.reveal_nonce().context("Bitcoin nonce")?,
            lez: self.lez.reveal_nonce().context("LEZ nonce")?,
        })
    }

    /// Round 3 payload: verifies the Maker's nonces and signs the Taker's
    /// partials (round 2 answer in, round 3 request out).
    ///
    /// # Errors
    ///
    /// Fails when a Maker nonce does not open its commitment.
    pub fn sign(
        &mut self,
        maker_nonces: &CeremonyLegPackets,
        agreement_key: &Zeroizing<[u8; 32]>,
    ) -> Result<CeremonyLegPackets> {
        Ok(LegPacketsV1 {
            bitcoin: self
                .bitcoin
                .accept_nonce_sign(&maker_nonces.bitcoin, agreement_key)
                .context("Bitcoin partial")?,
            lez: self
                .lez
                .accept_nonce_sign(&maker_nonces.lez, agreement_key)
                .context("LEZ partial")?,
        })
    }

    /// Verifies the Maker's partials, aggregates, and requires the Maker's
    /// presignatures (round 3 answer) to match byte for byte.
    ///
    /// # Errors
    ///
    /// Fails when a Maker partial does not verify or the presignatures differ.
    pub fn finish(
        &mut self,
        maker_partials: &CeremonyLegPackets,
        maker_presignatures: &CeremonyLegPackets,
    ) -> Result<TakerCeremonyOutcome> {
        let bitcoin = self
            .bitcoin
            .accept_peer_partial(&maker_partials.bitcoin)
            .context("Maker Bitcoin partial")?;
        let lez = self
            .lez
            .accept_peer_partial(&maker_partials.lez)
            .context("Maker LEZ partial")?;
        ensure!(
            bitcoin == maker_presignatures.bitcoin && lez == maker_presignatures.lez,
            "presignatures did not converge"
        );
        Ok(TakerCeremonyOutcome {
            bitcoin_presignature: self.bitcoin.presignature()?,
            lez_presignature: self.lez.presignature()?,
        })
    }
}

/// The Maker's seats: it answers every round.
#[derive(Debug)]
pub struct MakerCeremony {
    bitcoin: CeremonySeat,
    lez: CeremonySeat,
}

impl MakerCeremony {
    /// Opens (or reopens) the Maker's journals for `sessions`.
    ///
    /// # Errors
    ///
    /// Fails when a journal belongs to another session.
    pub fn open(
        layout: &SwapLayout,
        agreement: &BtcAgreementV1,
        sessions: &LegSessions,
    ) -> Result<Self> {
        let (bitcoin, lez) = open_seats(layout, agreement, sessions, Participant::Maker)?;
        Ok(Self { bitcoin, lez })
    }

    /// Round 1: reserve own nonces, record the Taker's commitments, answer
    /// with the Maker's commitments.
    ///
    /// # Errors
    ///
    /// Fails on a foreign packet or a conflicting replay.
    pub fn reserve_round(
        &mut self,
        taker_commitments: &CeremonyLegPackets,
        agreement_key: &Zeroizing<[u8; 32]>,
    ) -> Result<CeremonyLegPackets> {
        let bitcoin = self
            .bitcoin
            .reserve(agreement_key)
            .context("Bitcoin commitment")?;
        let lez = self.lez.reserve(agreement_key).context("LEZ commitment")?;
        self.bitcoin
            .accept_commitment(&taker_commitments.bitcoin)
            .context("Taker Bitcoin commitment")?;
        self.lez
            .accept_commitment(&taker_commitments.lez)
            .context("Taker LEZ commitment")?;
        Ok(LegPacketsV1 { bitcoin, lez })
    }

    /// Round 2: verify the Taker's nonces, reveal own nonces, sign partials.
    ///
    /// # Errors
    ///
    /// Fails when a Taker nonce does not open its commitment.
    pub fn nonce_round(
        &mut self,
        taker_nonces: &CeremonyLegPackets,
        agreement_key: &Zeroizing<[u8; 32]>,
    ) -> Result<(CeremonyLegPackets, CeremonyLegPackets)> {
        let nonces = LegPacketsV1 {
            bitcoin: self.bitcoin.reveal_nonce().context("Bitcoin nonce")?,
            lez: self.lez.reveal_nonce().context("LEZ nonce")?,
        };
        let partials = LegPacketsV1 {
            bitcoin: self
                .bitcoin
                .accept_nonce_sign(&taker_nonces.bitcoin, agreement_key)
                .context("Bitcoin partial")?,
            lez: self
                .lez
                .accept_nonce_sign(&taker_nonces.lez, agreement_key)
                .context("LEZ partial")?,
        };
        Ok((nonces, partials))
    }

    /// Round 3: verify the Taker's partials and aggregate.
    ///
    /// # Errors
    ///
    /// Fails when a Taker partial does not verify.
    pub fn partial_round(
        &mut self,
        taker_partials: &CeremonyLegPackets,
    ) -> Result<CeremonyLegPackets> {
        Ok(LegPacketsV1 {
            bitcoin: self
                .bitcoin
                .accept_peer_partial(&taker_partials.bitcoin)
                .context("Taker Bitcoin partial")?,
            lez: self
                .lez
                .accept_peer_partial(&taker_partials.lez)
                .context("Taker LEZ partial")?,
        })
    }

    /// Whether both legs hold a verified presignature.
    #[must_use]
    pub fn complete(&self) -> bool {
        self.bitcoin.presignature().is_ok() && self.lez.presignature().is_ok()
    }
}
