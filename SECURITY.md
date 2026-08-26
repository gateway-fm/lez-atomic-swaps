# Security policy

This policy supersedes security-reporting instructions in historical commits.

## Supported versions

`main` is a local-functional proof of concept. The current default branch
receives security fixes, but no version is approved for production custody,
mainnet funds, or an Internet-exposed deployment. Historical milestone tags
and commits are unsupported.

## Report a vulnerability

Email **security@gateway.fm**. Do not open a public issue for a suspected
vulnerability, exposed credential, privacy problem, atomicity failure, unsafe
refund path, or dependency disclosure that has not yet been coordinated.

Include the affected commit, impact, minimal reproduction steps, and a safe
way to contact you. Do not include live secrets or third-party personal data.
Gateway.fm will acknowledge a report within five business days and will then
coordinate validation, remediation, and disclosure. This response target is
not a promise of a bounty or a production support SLA.

GitHub private vulnerability reporting is the preferred web channel once it
is enabled for the public repository.

## Public test fixtures

The repository intentionally contains deterministic local-only material that
can resemble a secret:

- the `lezswap` VNC password and generated Bitcoin RPC credentials protect
  loopback-only disposable demo services, not a public or shared deployment;
- deterministic scalar, key-identifier, transaction, and signature values are
  cryptographic test vectors or disposable local-chain fixtures;
- example URI credentials use loopback or reserved `.test` hosts; and
- generated runtime secrets belong under ignored task-local directories and
  must never be committed.

Reusing any published fixture in a shared, public, testnet, or production
environment is unsupported and should be treated as a vulnerability. A value
that does not clearly match the documented deterministic fixtures must be
reported and rotated rather than assumed safe.

## Scope expectations

Useful reports include cryptographic or protocol failures, cross-role authority
confusion, secret leakage, replay or state-integrity failures, unsafe network
exposure, and vulnerabilities in the repository's source-only distribution.
Upstream dependency vulnerabilities should also be reported here when this
project's use changes their impact.
