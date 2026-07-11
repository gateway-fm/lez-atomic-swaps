# Milestone 1 review packet

Status: in progress. The authoritative checklist and dates live in
[the implementation plan](../implementation-plan.md).

This directory holds the reviewable design artefacts:

- [threat model](threat-model.md);
- [SDK trait surface](sdk-trait-surface.md);
- [LEZ escrow and SPEL IDL sketch](lez-escrow-design.md); and
- [LEZ primitive verification](lez-primitive-verification.md).

An artefact marked draft does not satisfy its milestone exit gate. Decisions
that have been accepted are recorded separately in the
[ADR log](../architecture/README.md).

```mermaid
flowchart LR
    Sources["RFP + proposal + upstream code"] --> Protocol["Per-leg protocol + atomicity"]
    Sources --> Primitives["LEZ primitive reproducers"]
    Protocol --> Escrow["Escrow + SPEL IDL"]
    Protocol --> SDK["Common SDK + pair evidence"]
    Protocol --> Threats["Threat model + parameters"]
    Primitives --> Review["Milestone 1 review packet"]
    Escrow --> Review
    SDK --> Review
    Threats --> Review
    Review --> Gates["M2/M3/M4/M5 entry gates"]
```
