# ADR 0148: Enter M7 through independent-review readiness

Status: Accepted on 2026-08-04; repository-controlled preparation active

## Context

RFP-003 and accepted issue #112 make M7 a formal S12 review of all on-chain
locking logic, formal S13 review of the cross-chain implementation, remediation,
hard-requirement CI closure, a mainnet-readiness write-up and five Logos doc
packets. An implementation team can prepare, self-review, test and remediate
what it discovers, but it cannot independently attest to its own work.

## Decision

M7 uses two explicit gates. The repository-controlled gate closes every
implementation, QA, chaos, security, production-readiness, documentation and
review-handoff item that Gateway can prove. Only then is the immutable candidate
sent to the mutually agreed reputable reviewer. The external gate closes only
after the report is received, Critical/High findings are independently verified
as remediated, and Medium/Low dispositions are recorded.

The five documentation packets pin the exact official template. Review scope,
assumptions, commands, evidence and finding state are versioned in the same
repository; external reports may be linked or stored according to their
publication terms, but their identity and SHA-256 must be recorded.

```mermaid
flowchart TB
    RFP["Live RFP plus accepted issue"] --> Matrix["Hard-requirement closure matrix"]
    Matrix --> SelfReview["Implementation, QA, chaos and security self-review"]
    SelfReview --> Candidate["Clean immutable review candidate"]
    Candidate --> S12["Independent S12 on-chain review"]
    Candidate --> S13["Independent S13 protocol and application review"]
    S12 --> Findings["Common finding register"]
    S13 --> Findings
    Findings --> Fixes["Critical and High fixes; Medium and Low decisions"]
    Fixes --> Verification["Regression and full-gate verification"]
    Verification --> ExternalClosure["Reviewer closure"]
    ExternalClosure --> M7["M7 completion decision"]
```

## Review handoff flow

```mermaid
sequenceDiagram
    participant Gateway
    participant Repository
    participant Reviewer
    participant Logos
    Gateway->>Repository: Push clean candidate, evidence manifest and packets
    Repository-->>Gateway: Immutable commit and artifact hashes
    Gateway->>Reviewer: Provide read-only reproducible bundle
    Reviewer->>Reviewer: Perform S12 and S13 independent review
    Reviewer-->>Gateway: Versioned report and findings
    Gateway->>Repository: Add regressions, fixes and dispositions
    Gateway->>Reviewer: Provide exact remediation commit and evidence
    Reviewer-->>Logos: Confirm closures and residual findings
    Logos->>Repository: Record acceptance or required follow-up
```

## Consequences

- A `review-ready` result may be proven without mislabeling it M7 complete.
- No Logos-owned exception can waive repository-controlled safety findings.
- Public deployment evidence remains optional under the owner's stealth/local
  policy, but public/mainnet activation remains blocked until configuration,
  program deployment, calibration, upstream and review gates are actually met.
- The first unavoidable external blocker is raised only after the candidate is
  independently reproducible and every self-owned gap has been resolved.

