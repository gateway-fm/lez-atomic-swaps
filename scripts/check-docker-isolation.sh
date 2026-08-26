#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

compose_file="tests/e2e/zebra/compose.yml"
config_file="tests/e2e/zebra/zebrad.toml"
dockerfile="tests/e2e/zebra/Dockerfile"

for required_file in "$compose_file" "$config_file" "$dockerfile"; do
  if [[ ! -f "$required_file" ]]; then
    echo "missing isolated Zebra fixture: ${required_file}" >&2
    exit 1
  fi
done

RUN_ID=policy-check ZEBRA_IMAGE=lez-atomic-swaps-zebra:policy-check docker compose \
  --project-name lez-atomic-swaps-policy-check \
  --file "$compose_file" config --quiet

required_compose_terms=(
  '${ZEBRA_IMAGE:?ZEBRA_IMAGE is required}'
  '127.0.0.1::18232'
  'org.logos-co.atomic-swaps.run'
  'mem_limit: 2g'
  'cpus: 2.0'
  'user: "65532:65532"'
  'read_only: true'
)

for term in "${required_compose_terms[@]}"; do
  if ! rg -Fq "$term" "$compose_file"; then
    echo "Zebra Compose fixture is missing isolation control: ${term}" >&2
    exit 1
  fi
done

if rg -q '^\s*container_name:' "$compose_file"; then
  echo "fixed Docker container names are forbidden" >&2
  exit 1
fi

if rg -q '127\.0\.0\.1:[0-9]+:18232' "$compose_file"; then
  echo "fixed Zebra host ports are forbidden" >&2
  exit 1
fi

if rg -q '^\s*cap_add:' "$compose_file"; then
  echo "Zebra must not regain Linux capabilities" >&2
  exit 1
fi

required_dockerfile_terms=(
  'docker.io/zfnd/zebra:5.2.0@sha256:477e65add4dacf52074ba04da8d763c89c26cc57f911dba2127401f8e1da597d'
  'gcr.io/distroless/cc-debian13:nonroot@sha256:a77defd6fedbb3392b175ba8ea3d1c22be963c1597c248c3ba987ddd80bfb512'
  'COPY --from=zebra /usr/local/bin/zebrad /usr/local/bin/zebrad'
  'USER 65532:65532'
  'ENTRYPOINT ["/usr/local/bin/zebrad"]'
)

for term in "${required_dockerfile_terms[@]}"; do
  if ! rg -Fq "$term" "$dockerfile"; then
    echo "Zebra Dockerfile is missing provenance/runtime control: ${term}" >&2
    exit 1
  fi
done

required_config_terms=(
  'network = "Regtest"'
  '"NU6.2" = 1'
  'ephemeral = true'
  'initial_testnet_peers = []'
)

for term in "${required_config_terms[@]}"; do
  if ! rg -Fq "$term" "$config_file"; then
    echo "Zebra config is missing deterministic isolation control: ${term}" >&2
    exit 1
  fi
done
