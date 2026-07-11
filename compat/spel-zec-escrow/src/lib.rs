//! Minimal executable SPEL/LEZ compatibility contract for the ZEC escrow.

#![allow(dead_code)]

use spel_framework::prelude::*;

#[derive(BorshSerialize, BorshDeserialize)]
pub enum EscrowStatus {
    Empty,
    Funded,
    Claimed,
    Refunded,
}

#[account_type]
#[derive(BorshSerialize, BorshDeserialize)]
pub struct EscrowMetadata {
    pub version: u8,
    pub swap_id: [u8; 32],
    pub terms_hash: [u8; 32],
    pub secret_digest: [u8; 32],
    pub amount: u128,
    pub refund_at: u64,
    pub status: EscrowStatus,
}

#[lez_program]
mod zec_escrow {
    #[allow(unused_imports)]
    use super::*;

    #[instruction]
    // The SPEL ABI keeps accounts and each signed swap term explicit in the IDL.
    #[allow(clippy::too_many_arguments)]
    pub fn initialize(
        #[account(init, pda = arg("swap_id"))] metadata: AccountWithMetadata,
        #[account(mut, signer)] depositor: AccountWithMetadata,
        #[account(mut)] custody: AccountWithMetadata,
        swap_id: [u8; 32],
        terms_hash: [u8; 32],
        secret_digest: [u8; 32],
        amount: u128,
        refund_at: u64,
    ) -> SpelResult {
        let _ = (terms_hash, secret_digest, amount, refund_at);
        Ok(SpelOutput::execute(
            vec![metadata, depositor, custody],
            vec![],
        ))
    }

    #[instruction]
    pub fn claim_hashlock(
        #[account(mut, pda = arg("swap_id"))] metadata: AccountWithMetadata,
        #[account(mut)] custody: AccountWithMetadata,
        #[account(mut)] claimant: AccountWithMetadata,
        swap_id: [u8; 32],
        preimage: [u8; 32],
        refund_at: u64,
    ) -> SpelResult {
        let _ = preimage;
        Ok(
            SpelOutput::execute(vec![metadata, custody, claimant], vec![])
                .with_timestamp_validity_window(..refund_at),
        )
    }

    #[instruction]
    pub fn refund(
        #[account(mut, pda = arg("swap_id"))] metadata: AccountWithMetadata,
        #[account(mut)] custody: AccountWithMetadata,
        #[account(mut)] depositor: AccountWithMetadata,
        swap_id: [u8; 32],
        refund_at: u64,
    ) -> SpelResult {
        Ok(
            SpelOutput::execute(vec![metadata, custody, depositor], vec![])
                .with_timestamp_validity_window(refund_at..),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_idl_has_the_contractual_zec_escrow_surface() {
        let idl = __program_idl();
        let instruction_names = idl
            .instructions
            .iter()
            .map(|instruction| instruction.name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(idl.name, "zec_escrow");
        assert_eq!(
            instruction_names,
            ["initialize", "claim_hashlock", "refund"]
        );
        assert!(idl
            .accounts
            .iter()
            .any(|account| account.name == "EscrowMetadata"));
    }

    #[test]
    fn idl_json_is_generated_by_spel_not_maintained_by_hand() {
        assert!(PROGRAM_IDL_JSON.contains("claim_hashlock"));
        assert!(PROGRAM_IDL_JSON.contains("EscrowMetadata"));
        assert!(PROGRAM_IDL_JSON.contains("refund_at"));
    }
}
