{
  schema_version: 1,
  direction: $direction,
  gate: "open",
  actor_revision: {maker: 2, taker: 2},
  bitcoin: {
    transaction_id: $bitcoin,
    confirmation_policy_satisfied: true
  },
  lez: ({
    initialization_transaction_id: $initialization,
    funding_transaction_id: $funding,
    finality: "Finalized",
    discovery_window: {
      start_height: $window_start,
      max_blocks: $window_blocks
    }
  } + (if $asset_mode == "custom_token" then {
    custody_creation_transaction_id: $custody,
    asset_commitment: $asset_commitment,
    exact_effect_order: ["initialize_witnessed", "create_custody_ata", "fund"]
  } else {} end)),
  adaptor_authority_eligible_only_after_this_evidence: true,
  opened_at: $opened_at
}
