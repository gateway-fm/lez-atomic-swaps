# Historical runtime diagrams

These three diagrams (`.mmd` sources and their rendered `.svg`) came into the
repository with ADR 0213 stage 1 (commit `5194037c`) and were not changed
since. They show the runtime before that stage, with components that no
longer run: `lez-maker-daemon`, `lez-taker-service`, `btc-demo-controller.py`,
the `lez-runner-arm` build host with the host Docker socket, and the LEZ
devnet as separate Bedrock, sequencer and indexer services. Nothing links to
them; they are kept as history.

The current stack is drawn in
[`../architecture/deployment-components-and-rpcs.md`](../architecture/deployment-components-and-rpcs.md)
(processes, modules, sockets and keys) and [`../diagrams.md`](../diagrams.md)
(the swap flow). Which parts run in Basecamp and which stay in Compose is in
[`../submission-115.md`](../submission-115.md).
