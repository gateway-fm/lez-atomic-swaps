# Regression on merged main — what ran against what

This is the release-candidate run: the whole v0.2.5 stack squash-merged, regressed as one
commit rather than as the pre-merge branch head. The earlier 43/43 run proved the stack; this
one proves what is actually being tagged.

## Commit under test

`4b8631b` — `test(e2e): both late-lock scenarios run in both directions now (#101)`, the tip of
`main` after all eight PRs merged:

| PR | Squashed as | Answers |
|---|---|---|
| #90 | `59de6a1` | docs |
| #94 | `f5958ff` | reported issue #92 |
| #95 | `43cbedf` | reported issue #91 |
| #96 | `e655691` | CODE-3 + PKG-1 |
| #97 | `8f24413` | CODE-4 |
| #98 | `8831f01` | COV-1 + COV-4 |
| #99 | `ce9448a` | COV-3 |
| #101 | `4b8631b` | Roman's I7 |

## 45 checks, not 43

Two more than the pre-merge run, and the difference is the point of #101: `early-lock` and
`refund-then-maker-lock` each used to refuse one direction, so each now contributes a second
scenario.

| Phase | Count |
|---|---|
| `node-e2e.py`, TakerSellsForeign | 14 |
| `node-e2e.py`, TakerSellsLez | 14 |
| desk suites (maker, taker) | 2 |
| desk scenarios (8 forward, 7 reversed) | 15 |

## Binaries

Rebuilt from `4b8631b` rather than reused from the pre-merge build, so the evidence is against
the commit being tagged and not merely against equivalent content.

**How the fixes are verified, and how they are not.** Earlier runs in this release claimed to
confirm the binaries by grepping them for a symbol — `taker_funding_confirmed` for CODE-4,
`has no usable probe executable` for CODE-3. That is not sound and is no longer relied on: a
private serde field's name is not guaranteed to survive as a discoverable literal, and the
probe gave contradictory answers for the same field across two builds of the same source. The
absence proved nothing either way.

What is relied on instead, against this commit's source in the pinned Linux container:

    cargo test -p lez-swap-core
      a_leg_that_is_still_off_chain_refuses_the_claim_after_the_other_leg_returns ... ok
      a_taker_leg_returning_does_not_confirm_a_maker_leg_that_never_met_policy ... ok
      a_taker_depth_regression_while_the_maker_leg_confirms_is_remembered ... ok
      arbitrary_event_sequences_preserve_atomicity_and_absorbing_terminal_states ... ok

Those four are the CODE-4 tests, each checked to **fail** when `both_legs_funded()` is removed
(the property test fails in 26 cases). CODE-3 is covered the same way by
`crates/maker-node/tests/route_health.rs`, whose
`a_missing_probe_executable_degrades_its_route_and_the_daemon_still_starts` fails with "Maker
daemon exited early" — the reported symptom — when the startup tolerance is reverted.

CODE-4 is in any case not reachable in a released Bitcoin swap: the BTC path never calls the
coordinator's removal entry points, so the e2e suite below cannot exercise it and was never
the evidence for it.

Persisted swaps were reset with `reset-swaps.sh` immediately after the rebuild. This is not
optional: a persisted swap pins the actor program's hash, so every swap written by the previous
binaries stops loading, and the symptom is a `taker_swap_list_v1` that fails with
`taker_monitor_unavailable` while individual swaps read fine. That cost an hour and two
abandoned runs earlier in this release.

## Chain recreations during the run

<!-- filled in as the run proceeds -->
