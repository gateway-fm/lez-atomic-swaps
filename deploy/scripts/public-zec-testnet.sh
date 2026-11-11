#!/usr/bin/env bash
# public-zec-testnet.sh — the public Zcash Testnet acceptance lane: the ZEC
# corridors of node-e2e.py, run against Zcash Testnet and the official LEZ v0.2
# testnet, with an evidence bundle of public data only
# (docs/zcash-testnet-setup.md, "Public Testnet acceptance lane").
#
#   scripts/public-zec-testnet.sh [--routes self-hosted,provider] [--scenarios zec-happy,zec-maker-refund]
#                                 [--directions TakerSellsLez] [--preflight-only] [--out DIR]
#                                 [--intervention TEXT]...
#
# For every route (a self-hosted Testnet Zebra on loopback, or the Tatum
# Zebrad gateway) the preflight records Zebra's version, the chain, genesis,
# tip, sync and the consensus branch of the next block, and refuses the route
# when the agreement profile cannot sign for that branch: nothing is tried on
# a route that cannot succeed, and the bundle says why (exit 3, blocked). On a
# supported route each scenario runs through node-e2e.py with
# LEZ_ZEC_PUBLIC_TESTNET=1 — it never mines, it waits for Testnet's blocks — and
# every Zcash transaction the scenario names is looked up again on the route
# (and on the other route, when both are supported, as an independent check).
#
# Nothing here mines. The Nodes must already run on the public networks
# (docs/testnet-run.md: LEZ side services; this lane: the Zcash route), the
# Maker under the owner-funded Testnet key: each corridor is provisioned by
# zec-provision.sh --zcash-network test (profile public-testnet-v2, NU7),
# which spends an ordinary output of that key and reads the role keys from
# LEZ_ZEC_TESTNET_KEY_DIR (outside the repository) and the funder search start
# from LEZ_ZEC_TESTNET_FUNDING_FROM_HEIGHT on its first run. Credentials are
# files, named by LEZ_ZEC_PROVIDER_API_KEY_FILE and
# LEZ_ZEC_SELF_HOSTED_COOKIE_FILE; their contents never reach a log or the
# bundle (zec-public-evidence.py checks).
#
# Environment: LEZ_ZEC_LANE_ENV (default deploy/zec-public-testnet.env),
# LEZ_CONTAINER_PREFIX (default lez-testnet: the Nodes of compose.testnet.yaml),
# LEZ_ZEC_ARTIFACT_IMAGES (images whose digests the bundle records; default the
# two Node images), LEZ_ZEC_ARTIFACT_FILES (release files to digest),
# LEZ_ZEC_INTERVENTIONS_FILE (one manual intervention per line, appended).
#
# Exit: 0 every corridor passed (or, with --preflight-only, every route is usable), 3 blocked (no route or no provisioner can
# serve this Testnet), 1 a corridor failed or the bundle is invalid.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

ROUTES=self-hosted,provider
SCENARIOS=zec-happy,zec-maker-refund
DIRECTIONS=TakerSellsLez
PREFLIGHT_ONLY=0
OUT=""
INTERVENTIONS=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --routes) ROUTES="$2"; shift 2 ;;
    --scenarios) SCENARIOS="$2"; shift 2 ;;
    --directions) DIRECTIONS="$2"; shift 2 ;;
    --preflight-only) PREFLIGHT_ONLY=1; shift ;;
    --out) OUT="$2"; shift 2 ;;
    --intervention) INTERVENTIONS+=("$2"); shift 2 ;;
    -h|--help) sed -n '2,34p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 64 ;;
  esac
done

LANE_ENV="${LEZ_ZEC_LANE_ENV:-zec-public-testnet.env}"
[[ -f "$LANE_ENV" ]] || { echo "lane profile $LANE_ENV not found" >&2; exit 64; }
set -a
# shellcheck source=../zec-public-testnet.env
source "$LANE_ENV"
set +a
export LEZ_CONTAINER_PREFIX="${LEZ_CONTAINER_PREFIX:-lez-testnet}"
evidence=(python3 scripts/zec-public-evidence.py)

stamp="$(date -u +%Y%m%dT%H%M%SZ)"
RUN="runtime/zec-public/$stamp"
OUT="${OUT:-$RUN/bundle}"
mkdir -p "$RUN/corridors" "$RUN/logs" "$RUN/raw"
now() { date -u +%Y-%m-%dT%H:%M:%SZ; }
intervene() { jq -cn --arg at "$(now)" --arg what "$1" '{at: $at, what: $what}' >> "$RUN/interventions.jsonl"; }
touch "$RUN/interventions.jsonl"
for text in "${INTERVENTIONS[@]}"; do intervene "$text"; done
if [[ -n "${LEZ_ZEC_INTERVENTIONS_FILE:-}" && -f "$LEZ_ZEC_INTERVENTIONS_FILE" ]]; then
  while IFS= read -r text; do [[ -z "$text" ]] || intervene "$text"; done < "$LEZ_ZEC_INTERVENTIONS_FILE"
fi
secret_args=()
for file in "${LEZ_ZEC_PROVIDER_API_KEY_FILE:-}" "${LEZ_ZEC_SELF_HOSTED_COOKIE_FILE:-}"; do
  [[ -z "$file" ]] || secret_args+=(--secret-file "$file")
done

# Provenance: the exact source and the artifacts that ran. An image's digest
# is its content id; a release file's is its SHA-256.
artifacts='[]'
read -r -a images <<< "${LEZ_ZEC_ARTIFACT_IMAGES:-lez-maker-node:${LEZ_IMAGE_TAG:-testnet} lez-taker-node:${LEZ_IMAGE_TAG:-testnet}}"
for image in "${images[@]}"; do
  if id="$(docker image inspect --format '{{.Id}}' "$image" 2>/dev/null)"; then
    artifacts="$(jq -c --arg n "$image" --arg d "$id" '. + [{name: $n, kind: "image", digest: $d}]' <<< "$artifacts")"
  fi
done
read -r -a files <<< "${LEZ_ZEC_ARTIFACT_FILES:-}"
for file in "${files[@]}"; do
  digest="sha256:$(sha256sum "$file" | cut -d' ' -f1)"
  artifacts="$(jq -c --arg n "$(basename "$file")" --arg d "$digest" '. + [{name: $n, kind: "file", digest: $d}]' <<< "$artifacts")"
done
run_url=""
[[ -z "${GITHUB_RUN_ID:-}" ]] || run_url="${GITHUB_SERVER_URL:-https://github.com}/${GITHUB_REPOSITORY:-}/actions/runs/$GITHUB_RUN_ID"
jq -n --arg sha "$(git rev-parse HEAD)" --argjson dirty "$([[ -z "$(git status --porcelain)" ]] && echo false || echo true)" \
  --arg ref "$(git describe --tags --always 2>/dev/null || true)" --arg run "$run_url" --argjson artifacts "$artifacts" \
  --arg prefix "$LEZ_CONTAINER_PREFIX" \
  '{source_sha: $sha, dirty: $dirty, describe: $ref, workflow_run: $run, artifacts: $artifacts, node_containers: $prefix}' \
  > "$RUN/provenance.json"

"${evidence[@]}" lez-tip --lane-env "$LANE_ENV" --out "$RUN/lez-start.json" \
  || intervene "LEZ testnet sequencer unreachable at the start ($LEZ_LEZ_SEQUENCER_URL)"

route_env() {
  # The harness's zebra URL and credential for one route.
  case "$1" in
    self-hosted) echo "LEZ_ZEBRA_URL=$LEZ_ZEC_SELF_HOSTED_URL" "LEZ_ZEBRA_COOKIE_FILE=${LEZ_ZEC_SELF_HOSTED_COOKIE_FILE:-}" ;;
    provider) echo "LEZ_ZEBRA_URL=$LEZ_ZEC_PROVIDER_URL" "LEZ_ZEBRA_API_KEY_FILE=${LEZ_ZEC_PROVIDER_API_KEY_FILE:-}" ;;
  esac
}

IFS=, read -r -a routes <<< "$ROUTES"
supported=()
for route in "${routes[@]}"; do
  echo "== preflight: $route"
  set +e
  "${evidence[@]}" preflight --lane-env "$LANE_ENV" --route "$route" --out "$RUN/preflight-$route.json"
  code=$?
  set -e
  case "$code" in
    0) supported+=("$route") ;;
    3) echo "   $route is blocked; no corridor runs on it" ;;
    *) intervene "preflight of the $route route failed (exit $code); the route was skipped"
       rm -f "$RUN/preflight-$route.json" ;;
  esac
done

if (( PREFLIGHT_ONLY == 0 )); then
  IFS=, read -r -a scenarios <<< "$SCENARIOS"
  IFS=, read -r -a directions <<< "$DIRECTIONS"
  for route in "${supported[@]}"; do
    for scenario in "${scenarios[@]}"; do
      for direction in "${directions[@]}"; do
        name="$route-$scenario-$direction"
        suffix=""; [[ "$direction" == TakerSellsLez ]] || suffix="-foreign"
        summary="runtime/e2e/$scenario$suffix.json"
        rm -f "$summary"
        started="$(now)"
        echo "== corridor: $route $scenario $direction (log $RUN/logs/$name.log)"
        set +e
        # shellcheck disable=SC2046 # route_env yields whole KEY=VALUE words
        env LEZ_ZEC_PUBLIC_TESTNET=1 LEZ_E2E_INDEXER_CHECK_ONLY=1 $(route_env "$route") \
          python3 scripts/node-e2e.py --pair Zcash --direction "$direction" "$scenario" > "$RUN/logs/$name.log" 2>&1
        code=$?
        set -e
        finished="$(now)"
        record=(--route "$route" --scenario "$scenario" --direction "$direction" --exit-code "$code"
                --started "$started" --finished "$finished")
        if [[ -f "$summary" ]]; then
          cp "$summary" "$RUN/raw/$name.summary.json"
          record+=(--summary "$RUN/raw/$name.summary.json")
          if "${evidence[@]}" lookup --lane-env "$LANE_ENV" --route "$route" \
               --summary "$summary" --out "$RUN/raw/$name.lookup.json"; then
            record+=(--lookup "$RUN/raw/$name.lookup.json")
          fi
          for other in "${supported[@]}"; do
            [[ "$other" != "$route" ]] || continue
            if "${evidence[@]}" lookup --lane-env "$LANE_ENV" --route "$other" \
                 --summary "$summary" --out "$RUN/raw/$name.cross-$other.json"; then
              record+=(--cross-check "$RUN/raw/$name.cross-$other.json")
            fi
          done
        fi
        reason=""
        [[ "$code" != 3 ]] || reason="$(grep -m1 'blocked:' "$RUN/logs/$name.log" | sed 's/.*blocked: //' || true)"
        [[ "$code" == 0 || "$code" == 3 ]] || reason="$(tail -n 1 "$RUN/logs/$name.log")"
        "${evidence[@]}" corridor "${record[@]}" --reason "$reason" --out "$RUN/corridors/$name.json"
      done
    done
  done
fi

"${evidence[@]}" lez-tip --lane-env "$LANE_ENV" --out "$RUN/lez-end.json" \
  || intervene "LEZ testnet sequencer unreachable at the end ($LEZ_LEZ_SEQUENCER_URL)"

set +e
"${evidence[@]}" bundle --lane-env "$LANE_ENV" --run-dir "$RUN" "${secret_args[@]}" --out "$OUT"
code=$?
set -e
echo "evidence bundle: $OUT (run files: $RUN)"
exit "$code"
