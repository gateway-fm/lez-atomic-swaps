def fail($message): error($message);

def valid_base_height:
  ($base_height | type) == "number"
  and ($base_height | floor) == $base_height
  and $base_height >= 0;

def valid_sources:
  .schema_version == 1
  and .network == "regtest"
  and (.base_height | type) == "number"
  and (.base_height | floor) == .base_height
  and .base_height >= 0
  and (.sources | type) == "array"
  and (
    (.allocation == "one_mature_coinbase_outpoint"
      and (.sources | length) == 1
      and ([.sources[].direction] == ["taker_sells_foreign"]
           or [.sources[].direction] == ["taker_sells_lez"]))
    or
    (.allocation == "two_distinct_mature_coinbase_outpoints"
      and (.sources | length) == 2
      and ([.sources[].direction] | sort) ==
        ["taker_sells_foreign", "taker_sells_lez"]
      and ([.sources[].direction] | unique | length) == 2))
  and all(.sources[];
    (.planned_bitcoin_funding_anchor_height == null
      or ((.planned_bitcoin_funding_anchor_height | type) == "number"
        and (.planned_bitcoin_funding_anchor_height | floor) ==
          .planned_bitcoin_funding_anchor_height
        and .planned_bitcoin_funding_anchor_height > 0)));

if (valid_base_height | not) then
  fail("invalid Core base height")
elif (valid_sources | not) then
  fail("invalid Bitcoin funding-source manifest")
elif $base_height < .base_height then
  fail("Core tip rewound below the funding-source allocation height")
elif $mode == "sequential" then
  if $base_height >= 4294967295 then
    fail("sequential Bitcoin funding anchor overflows u32")
  elif ($direction != "taker_sells_foreign" and $direction != "taker_sells_lez") then
    fail("invalid sequential direction")
  elif ([.sources[] | select(.direction == $direction)] | length) != 1 then
    fail("sequential direction is not unique")
  elif ([.sources[] | select(.direction == $direction)
      | select(.planned_bitcoin_funding_anchor_height == null)
      | select(has("anchor_assignment") | not)] | length) != 1 then
    fail("sequential direction already has an anchor reservation")
  else
    .sources |= map(
      if .direction == $direction then
        .planned_bitcoin_funding_anchor_height = ($base_height + 1)
        | .anchor_assignment = {
            mode: "sequential",
            core_tip_before_stage_two: $base_height
          }
      else . end)
  end
elif $mode == "overlap" then
  if .allocation != "two_distinct_mature_coinbase_outpoints" then
    fail("overlap Bitcoin anchors require both direction sources")
  elif $base_height >= 4294967294 then
    fail("overlap Bitcoin funding anchors overflow u32")
  elif $direction != "" then
    fail("overlap assignment must not select one direction")
  elif all(.sources[];
      .planned_bitcoin_funding_anchor_height == null
      and (has("anchor_assignment") | not)) | not then
    fail("overlap anchors must be assigned together from an unreserved manifest")
  else
    .sources |= map(
      .planned_bitcoin_funding_anchor_height =
        ($base_height + (if .direction == "taker_sells_foreign" then 1 else 2 end))
      | .anchor_assignment = {
          mode: "overlap",
          core_tip_before_stage_two: $base_height
        })
  end
else
  fail("invalid Bitcoin anchor-assignment mode")
end
