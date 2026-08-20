# ADR 0005: Docker E2E isolation

Status: Accepted — 2026-07-11

```mermaid
flowchart TB
    Run["Unique RUN_ID"] --> Project["COMPOSE_PROJECT_NAME=lez-atomic-swaps-RUN_ID"]
    Project --> Network["Project-scoped network"]
    Project --> Volumes["Labeled project-scoped volumes"]
    Project --> Services["Services with ephemeral host ports"]
    Project --> Data[".e2e/RUN_ID manifest + data"]
    Data --> Cleanup["Cleanup exact recorded project only"]
    Cleanup --> Network
    Cleanup --> Volumes
    Other["Unrelated developer/CI workloads"] -. "never prune/stop" .-> Project
```

## Decision

Every run has a non-empty `lez-atomic-swaps-${RUN_ID}` Compose project. Compose
creates project-scoped networks/volumes; services do not declare fixed container
names; host ports are ephemeral. Cleanup takes the exact recorded project name
and never uses global Docker prune/stop/kill operations.

## Consequences

Suites may run beside unrelated developer and CI workloads. Failed runs leave a
run manifest that permits targeted cleanup without guessing resource ownership.
