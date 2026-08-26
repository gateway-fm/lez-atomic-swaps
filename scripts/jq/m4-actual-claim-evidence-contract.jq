def lowercase_hex($width):
  type == "string"
  and length == $width
  and test("^[0-9a-f]+$");

def one_effect($chain; $effect; $role):
  .chain == $chain
  and .effect == $effect
  and .role == $role
  and (.transaction_id | lowercase_hex(64))
  and .automatic_submission_retry == false;

def exact_source_binding:
  (.source_binding.commit | lowercase_hex(40))
  and .source_binding.expected_commit == .source_binding.commit
  and .source_binding.clean_before == true
  and .source_binding.clean_after == true
  and .source_binding.exact_committed_tree_replay_completed == true
  and (.source_binding.binary_sha256 | type == "object")
  and (.source_binding.binary_sha256 | length >= 8)
  and all(.source_binding.binary_sha256[]; lowercase_hex(64));

def exact_ordered_effects:
  (.ordered_effects | length == 6)
  and ([.ordered_effects[].order] == [1, 2, 3, 4, 5, 6])
  and (.ordered_effects[0] | one_effect("lez"; "initialize_native_xmr"; "taker"))
  and (.ordered_effects[1] | one_effect("lez"; "fund_native"; "taker"))
  and (.ordered_effects[2] | one_effect("monero"; "fund_stage_a_shared_address"; "maker_funding_boundary"))
  and (.ordered_effects[3] | one_effect("lez"; "authorize_native_xmr_claim"; "taker_release_worker"))
  and (.ordered_effects[4] | one_effect("lez"; "claim_native_xmr"; "maker"))
  and (.ordered_effects[5] | one_effect("monero"; "reconstructed_spend_key_sweep"; "taker"))
  and (.ordered_effects[0].finalized_height < .ordered_effects[1].finalized_height)
  and (.ordered_effects[1].finalized_height < .ordered_effects[3].finalized_height)
  and (.ordered_effects[3].finalized_height < .ordered_effects[4].finalized_height)
  and (.ordered_effects[2].confirmations >= .ordered_effects[2].required_confirmations)
  and (.ordered_effects[2].required_confirmations >= 10)
  and (.ordered_effects[5].confirmations >= .ordered_effects[5].required_confirmations)
  and (.ordered_effects[5].required_confirmations >= 10)
  and (.ordered_effects[5].funded_amount_piconero
       == (.ordered_effects[5].received_amount_piconero + .ordered_effects[5].fee_piconero))
  and ([.ordered_effects[].transaction_id] as $ids
       | ($ids | length) == ($ids | unique | length));

def exact_cleanup:
  .cleanup.result == "passed"
  and .cleanup.exact_run_resources_absent == true
  and .cleanup.sidecar_processes_absent == true
  and .cleanup.sidecar_ports_closed == true
  and .cleanup.foreign_sentinel_survived_exact_cleanup == true
  and .cleanup.foreign_resources_targeted == false
  and .cleanup.broad_cleanup_used == false;

.schema == "lez-atomic-swaps-m4-actual-local-claim-poc"
and .version == 2
and .result == "passed_exact_committed_tree_replay"
and .milestone == "M4"
and .certification_status == "exact_committed_tree_replay_passed"
and .m4_complete_tag_authorized == false
and exact_source_binding
and exact_ordered_effects
and .agreement.taker_claim_partial_committed_before_effects == true
and .agreement.taker_claim_partial_withheld_until_confirmed_monero_funding == true
and .role_and_atomicity_evidence.maker_consumed_canonical_finalized_tag14 == true
and .role_and_atomicity_evidence.tag15_finalized_signature_matched_maker_packet == true
and .role_and_atomicity_evidence.maker_adaptor_share_extracted_only_after_finalized_tag15 == true
and .role_and_atomicity_evidence.successful_claim_branch_conditionally_atomic == true
and .role_and_atomicity_evidence.distributed_cross_chain_transaction_claimed == false
and .resource_and_secret_boundary.runtime_external_resources == []
and .resource_and_secret_boundary.public_rpc_used == false
and .resource_and_secret_boundary.peer_used == false
and .resource_and_secret_boundary.faucet_used == false
and .resource_and_secret_boundary.public_funds_used == false
and .resource_and_secret_boundary.external_finality_service_used == false
and .resource_and_secret_boundary.credentials_or_private_keys_in_packet == false
and .resource_and_secret_boundary.private_paths_in_packet == false
and .resource_and_secret_boundary.extracted_scalar_or_hash_in_packet == false
and exact_cleanup
