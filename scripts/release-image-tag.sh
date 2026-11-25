#!/usr/bin/env bash
# The image tag a release-images.yml run publishes under (issue #191), printed
# on stdout; a refused tag fails with a message that says why.
#
# Only a run on a tag ref (a release event, or a dispatch on the tag) publishes
# under a release's name, and only under its own tag: release.env, the bundle's
# name and every image of that release carry it. A dispatch on a branch is a
# test build: it publishes under sha-<12 hex of the commit> by default, or under
# the image_tag input when that is namespaced (sha-<...> or branch-<...>). It
# never publishes under a release-shaped name (v<semver>) or the name of a tag
# that exists, so a branch build cannot overwrite a release's images.
#
# The ci-gate job runs this before any build (so a refused tag stops the run at
# once), and the plan job runs it for the tag every push uses.
#
# Environment: GITHUB_EVENT_NAME, GITHUB_REF_TYPE, GITHUB_REF_NAME, GITHUB_SHA,
# GITHUB_REPOSITORY, IMAGE_TAG_INPUT (the dispatch input; empty otherwise) and,
# on a branch, GH_TOKEN (contents: read) for the lookup of existing tags. Needs
# gh on a branch.
set -euo pipefail

event="${GITHUB_EVENT_NAME:?GITHUB_EVENT_NAME is required}"
ref_type="${GITHUB_REF_TYPE:?GITHUB_REF_TYPE is required}"
ref_name="${GITHUB_REF_NAME:?GITHUB_REF_NAME is required}"
sha="${GITHUB_SHA:?GITHUB_SHA is required}"
input="${IMAGE_TAG_INPUT:-}"

refuse() {
  echo "release image tag: $*" >&2
  exit 1
}

readonly release_shaped='^v[0-9]+\.[0-9]+\.[0-9]+([-+.][0-9A-Za-z.+-]*)?$'
readonly namespaced='^(sha|branch)-[A-Za-z0-9][A-Za-z0-9_.-]*$'

# tag_exists <name>: the repository has a git tag of that name. Any answer but
# the tag or a 404 refuses: the lookup fails closed.
tag_exists() {
  local answer
  if answer="$(gh api "repos/${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}/git/ref/tags/$1" --jq .ref 2>&1)"; then
    [[ "$answer" == "refs/tags/$1" ]] || refuse "unexpected answer looking up the tag $1: $answer"
    return 0
  fi
  [[ "$answer" == *"HTTP 404"* ]] || refuse "cannot tell whether the tag $1 exists: $answer"
  return 1
}

[[ "$sha" =~ ^[0-9a-f]{40}$ ]] || refuse "not a full commit SHA: $sha"
if [[ "$event" == release && "$ref_type" != tag ]]; then
  refuse "a release event on ${ref_type} ${ref_name}, not on a tag"
fi

if [[ "$ref_type" == tag ]]; then
  # A release, or a dispatch on the tag: the tag's own name, nothing else.
  [[ -z "$input" || "$input" == "$ref_name" ]] ||
    refuse "a run on the tag ${ref_name} publishes under ${ref_name} only, not under ${input}"
  tag="$ref_name"
else
  if [[ -z "$input" ]]; then
    tag="sha-${sha:0:12}"
  else
    [[ ! "$input" =~ $release_shaped ]] ||
      refuse "${input} is a release's name: only a run on the tag ${input} publishes under it; a dispatch on ${ref_type} ${ref_name} takes sha-<commit> or branch-<name>"
    [[ "$input" =~ $namespaced ]] ||
      refuse "a dispatch on ${ref_type} ${ref_name} publishes under a namespaced tag (sha-<commit> or branch-<name>), not under ${input}"
    tag="$input"
  fi
  command -v gh >/dev/null || refuse "gh is required to look up the repository's tags"
  ! tag_exists "$tag" ||
    refuse "${tag} is the name of a tag of this repository: only a run on that tag publishes under it"
fi

[[ "$tag" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]*$ ]] || refuse "invalid image tag: $tag"
echo "$tag"
