# ADR 0034: Gate actor activation on complete signing material

Status: Accepted and GREEN through claim revisions three and four in both
actual-node happy directions and through deterministic actor refund recovery.
Fresh actual-node timeout/refund execution, process-kill, and production key
custody hardening remain active.

## Context

The first public actor config schema could activate an agreement and observe
both locks without naming the two completed adaptor sessions or the exact
prepared LEZ claim. Adding optional fields to that strict schema would allow the
same bypass, while adding required fields without a version change would
silently redefine a documented `deny_unknown_fields` format.

The role-runner session JSON and packet files are ceremony transport. Treating
them as actor authority would duplicate agreement parsing and permit the signed
keys, role order, messages, adaptor point, or Bitcoin Taproot tweak to drift.

## Decision

Private `ActorConfig` schema 3 retains every schema-2 signing requirement and
adds role-shaped Bitcoin refund authority. It requires:

- distinct nonzero Bitcoin and LEZ session IDs;
- distinct normalized role-local journal paths; and
- `prepared_witnessed_claim_result_file`, the complete persisted
  `PrepareWitnessedClaimResult` path;
- for the taker only, `adaptor_secret_file`, a mode-0600, single-link regular
  file containing the exact lowercase 32-byte hex scalar. Maker configs must
  omit it; and
- only for the agreement-selected Bitcoin funder,
  `refund.bitcoin_refund_key_file`, another mode-0600, single-link regular file
  containing the exact lowercase 32-byte hex scalar whose derived x-only key
  equals that participant's countersigned refund key. The other role must omit
  the field and must not receive the private file.

Output schema remains version 1. Older private config schemas fail explicitly.
`activate` loads and validates the countersigned agreement, then performs the
following gate before it may create or accept revision zero:

1. stable-read and strict-decode the full prepared-result envelope;
2. bind its run ID, LEZ claimant role, and echoed preparation request ID;
3. validate its nonempty exact official message bytes and domain-separated hash;
4. require that hash to equal the claim-message hash in the signed agreement;
5. derive the Bitcoin and LEZ adaptor contexts from the agreement and configured
   session IDs;
6. open both signer databases existing-only, require the exact local-role
   identity and `PresignatureVerified` phase;
7. independently verify each retained aggregate presignature under its derived
   context; and
8. for the taker only, stable-read and point-check the adaptor scalar against
   the agreement without creating a final signature; and
9. select the Bitcoin funder from the agreement, reject refund authority on the
   other role, and stable-read plus point-check the selected funder's key.

```mermaid
flowchart TD
    Config["Private schema 3 actor config"]
    Agreement["Validated countersigned agreement"]
    Prepared["Full prepared LEZ claim result"]
    BtcJournal[("Existing Bitcoin signer journal")]
    LezJournal[("Existing LEZ signer journal")]
    Bind["Run claimant request and message hash gate"]
    Derive["Agreement derived Bitcoin and LEZ contexts"]
    Verify["Exact identity phase and presignature verification"]
    Secret["Taker only private scalar point check"]
    RefundRole["Agreement selected Bitcoin funder"]
    RefundSecret["Funder only refund-key point check"]
    State[("Create or accept actor revision zero")]
    Refuse["Fail closed without state creation"]

    Config --> Agreement
    Config --> Prepared
    Config --> BtcJournal
    Config --> LezJournal
    Agreement --> Bind
    Prepared --> Bind
    Agreement --> Derive
    BtcJournal --> Verify
    LezJournal --> Verify
    Config --> Secret
    Agreement --> Secret
    Agreement --> RefundRole
    Config --> RefundSecret
    RefundRole --> RefundSecret
    Derive --> Verify
    Bind --> Verify
    Verify -->|"all exact"| Secret
    Secret -->|"taker exact or maker absent"| RefundSecret
    RefundSecret -->|"funder exact and nonfunder absent"| State
    Bind -->|"drift"| Refuse
    Verify -->|"missing incomplete or invalid"| Refuse
    Secret -->|"missing unsafe forbidden or mismatched"| Refuse
    RefundSecret -->|"missing unsafe cross-role or mismatched"| Refuse
```

`status` still parses only the config and existing recovery state. It neither
opens signer journals nor constructs an RPC client. The same material gate must
run again immediately before later claim use, because activation cannot prevent
an owner from replacing files afterward.

## Consequences

- The current actor gate is 49/49 library tests plus eight CLI integrations.
- Valid fixtures create real completed MuSig2 journals; the activation gate does
  not trust arbitrary phase labels or fabricated presignature bytes.
- Missing material, cross-domain journals, changed run/claimant/request
  identity, an invalid internal message hash, and an internally valid prepared
  message not signed by the agreement all fail before actor state creation.
  Missing, world-readable, symlinked, hard-linked, or point-mismatched taker and
  refund secrets fail before state creation. Maker configs cannot carry or
  explicitly null taker adaptor authority; the Bitcoin non-funder cannot carry
  refund authority. Fresh-process assertions find neither raw nor hex-encoded
  scalar bytes in stdout, SQLite, or surviving WAL/SHM artifacts.
- The operator must prepare the exact LEZ claim and finish both signing
  ceremonies before activation and before the first chain effect.
- This gate now protects both actual-node claim execution and deterministic
  timeout recovery. It does not claim that a fresh actual-node refund has run,
  that local SQLite and either chain form one distributed transaction, or that
  production key custody is complete.
