# lez-tools — the one-shot tools a prebuilt stack still needs outside the
# services: the escrow deployer and the vault-claim tool for the one-time
# market bootstrap, and the identity tool that mints the wallet identities.
# Runs only as `docker compose --profile tools run --rm tools …` (start.sh);
# it is never a long-lived service and takes part in no swap.
#
# The staged payloads are debug builds; strip them so the image ships only
# what runs.
FROM debian:bookworm-slim AS strip
# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends binutils \
    && rm -rf /var/lib/apt/lists/*
COPY lez-zec-escrow-v02-deployer /payload/lez-zec-escrow-v02-deployer
COPY lez-v02-vault-claim-poc /payload/lez-v02-vault-claim-poc
COPY lez-v02-local-actor-identity /payload/lez-v02-local-actor-identity
RUN strip /payload/*

# python:3.12 because the sidecar-family tools link libpython3.12, exactly
# like the role Node images.
FROM python:3.12-slim-bookworm

LABEL org.logos-co.atomic-swaps.purpose="local-market-bootstrap-tools"

# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl jq socat \
    && rm -rf /var/lib/apt/lists/*

COPY --from=strip --chmod=0555 /payload/lez-zec-escrow-v02-deployer /usr/local/bin/lez-zec-escrow-v02-deployer
COPY --from=strip --chmod=0555 /payload/lez-v02-vault-claim-poc /usr/local/bin/lez-v02-vault-claim-poc
COPY --from=strip --chmod=0555 /payload/lez-v02-local-actor-identity /usr/local/bin/lez-v02-local-actor-identity

ENV HOME=/tmp
WORKDIR /tmp
ENTRYPOINT ["/bin/bash", "-c"]
