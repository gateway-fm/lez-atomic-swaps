#!/usr/bin/env bash
# Release gate (issue #171): a tag is released only from a commit whose ci.yml
# and public-readiness.yml runs have finished green. The first job of
# release-images.yml and release-packages.yml runs this script.
#
# It asks the Actions API for the runs of each workflow on the commit and
# judges the newest one. It never waits: a run that is still queued or in
# progress fails the gate with a message that names it; re-run the release
# when that run has finished green.
#
# Gated: a release event and any run on a tag ref. A workflow_dispatch on a
# branch is a test build and passes through, saying so; what it may publish
# under is scripts/release-image-tag.sh's rule (never a release's name, #191).
#
# Environment: GH_TOKEN (the workflow's GITHUB_TOKEN, permission actions:
# read), GITHUB_REPOSITORY, GITHUB_SHA, GITHUB_EVENT_NAME, GITHUB_REF_TYPE,
# GITHUB_REF. Needs gh and jq.
set -euo pipefail

readonly gated_workflows=(ci.yml public-readiness.yml)

event="${GITHUB_EVENT_NAME:?GITHUB_EVENT_NAME is required}"
ref_type="${GITHUB_REF_TYPE:?GITHUB_REF_TYPE is required}"
ref="${GITHUB_REF:?GITHUB_REF is required}"
sha="${RELEASE_COMMIT:-${GITHUB_SHA:?GITHUB_SHA is required}}"
repository="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"

if [[ "$event" != release && "$ref_type" != tag ]]; then
  echo "release CI gate: not gated: ${event} on ${ref_type} ${ref} is a test build;" \
    "only a release event or a tag ref must have green ci.yml and public-readiness.yml runs"
  exit 0
fi

if [[ ! "$sha" =~ ^[0-9a-f]{40}$ ]]; then
  echo "release CI gate: not a full commit SHA: ${sha}" >&2
  exit 1
fi

for command in gh jq; do
  command -v "$command" >/dev/null || {
    echo "release CI gate: ${command} is required" >&2
    exit 1
  }
done

echo "release CI gate: ${event} on ${ref} at ${sha}"
failed=0
for workflow in "${gated_workflows[@]}"; do
  runs="$(gh api --method GET \
    "repos/${repository}/actions/workflows/${workflow}/runs" \
    -f head_sha="$sha" -F per_page=100 \
    --jq '[.workflow_runs[]
           | {id, status, conclusion, event, html_url, created_at, run_attempt}]
          | sort_by(.created_at) | reverse')"
  if [[ "$(jq 'length' <<<"$runs")" == 0 ]]; then
    echo "release CI gate: ${workflow} has no run on ${sha};" \
      "tag a commit of main whose CI has finished green" >&2
    failed=1
    continue
  fi
  newest="$(jq -c '.[0]' <<<"$runs")"
  status="$(jq -r '.status' <<<"$newest")"
  conclusion="$(jq -r '.conclusion // "none"' <<<"$newest")"
  url="$(jq -r '.html_url' <<<"$newest")"
  run_event="$(jq -r '.event' <<<"$newest")"
  attempt="$(jq -r '.run_attempt' <<<"$newest")"
  if [[ "$status" != completed ]]; then
    echo "release CI gate: ${workflow} run on ${sha} has not finished (${status}," \
      "${run_event}, attempt ${attempt}): ${url}; re-run this release when it is green" >&2
    failed=1
  elif [[ "$conclusion" != success ]]; then
    echo "release CI gate: ${workflow} run on ${sha} concluded ${conclusion}" \
      "(${run_event}, attempt ${attempt}): ${url}" >&2
    failed=1
  else
    echo "release CI gate: ${workflow} passed on ${sha} (${run_event}, attempt ${attempt}): ${url}"
  fi
done

if [[ "$failed" != 0 ]]; then
  echo "release CI gate: refusing to release ${ref}" >&2
  exit 1
fi
echo "release CI gate: passed"
