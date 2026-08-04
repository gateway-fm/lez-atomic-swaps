#!/usr/bin/env bash
set -euo pipefail

readonly image="ghcr.io/puppeteer/puppeteer@sha256:9665f5b57abc5cc7080a641878964018de219055a4d2c9d8d050ceb1161778ba"
readonly chrome="/home/pptruser/.cache/puppeteer/chrome/linux-150.0.7871.24/chrome-linux64/chrome"
repository_root="$(git rev-parse --show-toplevel)"
readonly repository_root
readonly container_name="lez-m6-ui-$UID-$$"

cleanup() {
  docker rm --force "$container_name" >/dev/null 2>&1 || true
}
trap cleanup EXIT HUP INT TERM

docker run --rm \
  --name "$container_name" \
  --network none \
  --read-only \
  --cap-drop ALL \
  --cap-add SYS_ADMIN \
  --cap-add SYS_CHROOT \
  --pids-limit 512 \
  --memory 2g \
  --cpus 2 \
  --shm-size 1g \
  --tmpfs /tmp:rw,nosuid,nodev,size=1g \
  --tmpfs /home/m6:rw,nosuid,nodev,uid=10042,gid=10042,mode=0700,size=128m \
  --env HOME=/home/m6 \
  --env PUPPETEER_EXECUTABLE_PATH="$chrome" \
  --env M6_UI_TEST_ROOT=/tmp/lez-m6-ui \
  --mount "type=bind,src=$repository_root,dst=/workspace,readonly" \
  --workdir /workspace \
  "$image" \
  node --test --test-concurrency=1 tests/ui/m6-prototype.e2e.test.mjs
