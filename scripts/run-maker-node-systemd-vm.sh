#!/usr/bin/env bash
# Installs the packaged Maker Node into a throwaway Ubuntu VM and runs it under systemd.
#
# The VM is QEMU/KVM inside a container that gets /dev/kvm and nothing else: no
# --privileged, no unit on this host. The stock cloud image is fetched and checked inside the
# container. Inside the VM the guide in docs/maker-systemd-install.md is followed as root, then
# the unit's readiness, hardening, owner socket, crash restart, CLI stop/start and durable
# state are recorded; the Delivery key reaches the Node only as an encrypted systemd
# credential. The Taker Node is then installed and started under its own unit beside it. No
# chain node, sidecar or Chat runs: both Nodes use run-local Delivery only, and no swap is
# executed. Without chains each unit runs its Node directly from node.json (a drop-in replaces
# the unit's launcher ExecStart).
#
# With LEZ_STACK_DEPLOY_DIR naming the devnet's deploy directory (its runtime/ holds lane 1's
# identities, cookie and market constants), the VM instead runs both units through the role
# launcher against the devnet's chains and executes one Bitcoin swap, Taker sells BTC, with a
# kill -9 of the Maker between the Taker's lock and its claim. The VM reaches exactly three
# endpoints, through QEMU guestfwd from a container on the stack's network: Bitcoin Core
# :18443, the LEZ sequencer :3040 and the indexer :8779. Lane 1's two Node containers use the
# same identities, so they are stopped for the run and started again afterwards. Run it under
# the stack lock (~/m2-work/stack-lock.sh), with the stack up and lane 1 idle.
#
# SIDECAR_PROGRAM names the LEZ sidecar from the builder image (provision/data/sidecar).
# The transcript and the console log land in OUTPUT_DIR (default target/maker-systemd-vm).
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly source_bin_dir="${SOURCE_BIN_DIR:-target/debug}"
readonly output_dir="${OUTPUT_DIR:-target/maker-systemd-vm}"
readonly image_base="${UBUNTU_IMAGE_BASE:-https://cloud-images.ubuntu.com/releases/noble/release}"
readonly image_name="ubuntu-24.04-server-cloudimg-amd64.img"
readonly sidecar_program="${SIDECAR_PROGRAM:?SIDECAR_PROGRAM must name the LEZ sidecar (lez-v02-bridge-poc)}"
readonly stack="${LEZ_STACK_DEPLOY_DIR:-}"
readonly stack_network="${LEZ_STACK_NETWORK:-lez-swap-chains}"
readonly lane_nodes=(lez-maker-node lez-taker-node)
btc_cli=(docker exec lez-bitcoin-core bitcoin-cli -conf=/run-config/bitcoin.conf -datadir=/var/lib/bitcoin)

[[ -c /dev/kvm ]] || {
  echo "/dev/kvm is required" >&2
  exit 1
}
command -v docker >/dev/null || {
  echo "Docker is required to host the VM" >&2
  exit 1
}

if [[ -z "${SOURCE_BIN_DIR:-}" ]]; then
  cargo build --locked -p lez-maker-node --features pair-zec --bins
  cargo build --locked -p lez-taker-node --features pair-zec --bins
  cargo build --locked -p zec-reference-actor --bin lez-zec-maker-actor
  cargo build --locked -p lez-logos-price-c-api --bin lez-logos-price-worker
  cargo build --locked -p btc-reference-actor --bins
  cargo build --locked -p lez-runtime-healthcheck
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/lez-maker-systemd-vm.XXXXXX")"
readonly work
cleanup() {
  if [[ -n "$stack" ]]; then
    docker start "${lane_nodes[@]}" >/dev/null || true
  fi
  rm -rf -- "$work"
}
trap cleanup EXIT

# The payload is what an operator copies to the machine: the install script, the packaging
# files and the three binaries. It reaches the VM as a raw read-only disk holding a tar.
payload="$work/payload"
install -d "$payload/scripts" "$payload/bin" "$payload/modules/lez-role-node"
readonly binaries=(lez-maker-node lez-maker-cli lez-zec-maker-actor lez-logos-price-worker lez-btc-maker-actor lez-runtime-healthcheck
  lez-taker-node lez-taker-cli lez-btc-taker-actor lez-taker-registry-init)
cp scripts/install-maker-node-service.sh scripts/install-taker-node-service.sh "$payload/scripts/"
cp -r packaging "$payload/"
cp modules/lez-role-node/node-entrypoint.sh "$payload/modules/lez-role-node/"
for binary in "${binaries[@]}"; do
  cp "$source_bin_dir/$binary" "$payload/bin/"
done
cp "$sidecar_program" "$payload/bin/lez-v02-bridge-poc"
{
  echo "source commit: $(git rev-parse HEAD)"
  (cd "$payload/bin" && sha256sum "${binaries[@]}" lez-v02-bridge-poc)
  sha256sum modules/lez-role-node/node-entrypoint.sh
} >"$payload/build-info"

# Each party's holdings read from the chains, as node-e2e.py's party_balances does: its
# Core wallet's trusted + pending + immature balance and its LEZ account at the indexer.
balances() {
  local role wallet account sat lez
  for role in maker taker; do
    wallet="$(cat "$payload/stack/$role/wallet")"
    account="$(jq -r .account_id "$payload/stack/$role/identity.json")"
    sat="$("${btc_cli[@]}" -rpcwallet="$wallet" getbalances \
      | jq '.mine | [.trusted, .untrusted_pending, .immature] | map(. * 100000000 | round) | add')"
    lez="$(curl -fsS http://127.0.0.1:8779/ -H 'content-type: application/json' \
      --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getAccount\",\"params\":[\"$account\"]}" \
      | jq .result.balance)"
    echo "$role wallet=$wallet btc_sat=$sat lez_account=$account lez_units=$lez"
  done
}

guest="guest.sh"
if [[ -n "$stack" ]]; then
  guest="guest-swap.sh"
  # Lane 1's identities, cookie, market constants and timing profile; the payload is deleted
  # with the work directory and never leaves this host.
  for role in maker taker; do
    install -d -m 0700 "$payload/stack/$role"
    cp "$stack/runtime/lez/$role/identity.json" "$stack/runtime/lez/$role/lez-signer.key" "$payload/stack/$role/"
    docker inspect -f '{{range .Config.Env}}{{println .}}{{end}}' "lez-$role-node" \
      | sed -n 's/^LEZ_BTC_WALLET=//p' >"$payload/stack/$role/wallet"
  done
  cp "$stack/runtime/market-bootstrap.env" "$stack/runtime/secrets/btc-rpc-cookie" "$payload/stack/"
  grep -E '^(LEZ_V02_CHANNEL_ID|LEZ_BTC_|LEZ_LEZ_DISCOVERY_MAX_BLOCKS|LEZ_MAKER_ACTOR_|LEZ_TAKER_OBSERVER_|LEZ_NODE_FINALITY_)' \
    "$stack/runtime/runtime.env" >"$payload/stack/profile.env"
  {
    echo "stack: $(git -C "$stack" rev-parse HEAD)"
    for container in lez-bitcoin-core lez-sequencer lez-network "${lane_nodes[@]}"; do
      echo "$container image $(docker inspect -f '{{.Image}}' "$container")"
    done
  } >>"$payload/build-info"
  echo "balances before:" >"$work/balances"
  balances >>"$work/balances"
  docker stop "${lane_nodes[@]}" >/dev/null
fi

cat >"$payload/guest.sh" <<'GUEST'
#!/usr/bin/env bash
set -uo pipefail
export SYSTEMD_COLORS=0 SYSTEMD_URLIFY=0 SYSTEMD_PAGER='' TERM=dumb
readonly cli=/usr/bin/lez-maker-cli
readonly unit=lez-maker-node
step() { printf '\n## %s\n' "$*"; }
run() { printf '$ %s\n' "$*"; "$@"; printf '[exit %s]\n' "$?"; }

step "VM"
cat /payload/build-info
run grep PRETTY_NAME /etc/os-release
run uname -r
run systemctl --version

step "Install (docs/maker-systemd-install.md)"
run useradd --system --no-create-home --shell /usr/sbin/nologin lez-swap
run env SOURCE_BIN_DIR=/payload/bin /payload/scripts/install-maker-node-service.sh
for name in delivery-signing.key maker-claim-recovery.key maker-claim-preimage.key lez-signer.key; do
  printf '$ head -c 32 /dev/urandom | systemd-creds encrypt --name=%s - /etc/lez/maker/credentials/%s.cred\n' "$name" "$name"
  head -c 32 /dev/urandom | systemd-creds encrypt --name="$name" - "/etc/lez/maker/credentials/$name.cred"
done
run ls -l /etc/lez/maker /etc/lez/maker/credentials
cat >/etc/lez/maker/node.json <<'JSON'
{
  "schema_version": 1,
  "arguments": [
    "--socket", "/run/lez/maker/node.sock",
    "--database", "/var/lib/lez/maker/maker.sqlite3",
    "--ready-file", "/run/lez/maker/ready",
    "--delivery-directory", "/var/lib/lez/maker/delivery",
    "--delivery-signing-key-file", "/run/credentials/lez-maker-node.service/delivery-signing.key"
  ]
}
JSON
run chown lez-swap:lez-swap /etc/lez/maker/node.json
run chmod 0600 /etc/lez/maker/node.json
run cat /etc/lez/maker/node.json
node_only() {
  install -d "/etc/systemd/system/lez-$1-node.service.d"
  printf '[Service]\nEnvironmentFile=\nExecStart=\nExecStart=/usr/bin/lez-%s-node --config /etc/lez/%s/node.json\n' "$1" "$1" \
    >"/etc/systemd/system/lez-$1-node.service.d/node-only.conf"
  run cat "/etc/systemd/system/lez-$1-node.service.d/node-only.conf"
}
# No chains here: the Node runs alone from node.json instead of through the role launcher.
node_only maker
run systemctl daemon-reload
run systemctl enable --now "$unit"

step "Unit active and READY via sd_notify"
run systemctl show -p Type,ActiveState,SubState,MainPID,NRestarts,StatusText "$unit"
run systemctl status --no-pager --lines=0 "$unit"
run journalctl -u "$unit" --no-pager -o cat
run find /etc/lez -type f -name '*.key'

step "The key the Node read: a systemd credential, root-owned"
run systemd-run --quiet --wait --pipe -p User=lez-swap \
  -p LoadCredentialEncrypted=delivery-signing.key:/etc/lez/maker/credentials/delivery-signing.key.cred \
  sh -c 'id; stat -c "%n %U:%G %a" "$CREDENTIALS_DIRECTORY" "$CREDENTIALS_DIRECTORY"/*'

step "Hardening"
run systemd-analyze security --no-pager "$unit"

step "Owner socket"
run ls -ld /run/lez /run/lez/maker /var/lib/lez/maker
run stat -c '%n %F %U:%G %a' /run/lez/maker/node.sock
run runuser -u lez-swap -- "$cli" health
run useradd --create-home other-user
run runuser -u other-user -- "$cli" health

step "Advertise and price (run-local Delivery, no chain)"
as_owner() { run runuser -u lez-swap -- "$cli" "$@"; }
as_owner configure-pair --request-id vm-pair-1 --pair zcash --direction taker-sells-lez \
  --enabled false --minimum-foreign-units 10 --maximum-foreign-units 10000 --offer-ttl-seconds 3600
as_owner set-local-price --request-id vm-price-1 --pair zcash --direction taker-sells-lez \
  --lez-units-per-lot 5 --foreign-units-per-lot 2
as_owner configure-pair --request-id vm-pair-2 --expected-revision 1 --pair zcash \
  --direction taker-sells-lez --enabled true --minimum-foreign-units 10 \
  --maximum-foreign-units 10000 --offer-ttl-seconds 3600
as_owner quote --pair zcash --direction taker-sells-lez
as_owner publish-offer --request-id vm-offer-1 --offer-id vm-offer-1 --pair zcash \
  --direction taker-sells-lez
run find /var/lib/lez/maker/delivery -type f

step "kill -9 of the main process; systemd restarts it"
pid="$(systemctl show -p MainPID --value "$unit")"
[[ "$pid" -gt 0 ]] && run kill -9 "$pid"
for _ in $(seq 60); do
  [[ "$(systemctl show -p NRestarts --value "$unit")" == 1 \
    && "$(systemctl show -p ActiveState --value "$unit")" == active ]] && break
  sleep 1
done
run systemctl show -p ActiveState,SubState,MainPID,NRestarts,StatusText "$unit"
as_owner health
as_owner pairs
as_owner prices
as_owner offers

step "lez-maker-cli stop / start (systemctl through service_control)"
run "$cli" stop
run systemctl show -p ActiveState,NRestarts "$unit"
as_owner health
run "$cli" start
run systemctl show -p ActiveState,SubState,NRestarts,StatusText "$unit"
as_owner health
as_owner offers

step "Taker Node under its own unit"
run env SOURCE_BIN_DIR=/payload/bin /payload/scripts/install-taker-node-service.sh
printf '$ head -c 32 /dev/urandom | systemd-creds encrypt --name=lez-signer.key - /etc/lez/taker/credentials/lez-signer.key.cred\n'
head -c 32 /dev/urandom | systemd-creds encrypt --name=lez-signer.key - /etc/lez/taker/credentials/lez-signer.key.cred 2>/dev/null
# The Taker trusts the Maker's Delivery identity, derived here from the same credential.
printf '$ maker_key=$(systemd-run ... lez-maker-cli delivery-identity --signing-key-file "$CREDENTIALS_DIRECTORY/delivery-signing.key")\n'
identity="$(systemd-run --quiet --wait --pipe -p User=lez-swap \
  -p LoadCredentialEncrypted=delivery-signing.key:/etc/lez/maker/credentials/delivery-signing.key.cred \
  sh -c '/usr/bin/lez-maker-cli delivery-identity --signing-key-file "$CREDENTIALS_DIRECTORY/delivery-signing.key"')"
echo "$identity"
maker_key="$(sed -n 's/.*"public_key":"\([0-9a-f]*\)".*/\1/p' <<<"$identity")"
cat >/etc/lez/taker/role.json <<JSON
{
  "schema_version": 1,
  "delivery_sources": [
    {"directory": "/var/lib/lez/maker/delivery", "maker_public_key": "$maker_key", "source_id": "vm-maker"}
  ],
  "maximum_offers": 16
}
JSON
run cp /etc/lez/taker/node.json.example /etc/lez/taker/node.json
run chown lez-swap:lez-swap /etc/lez/taker/node.json /etc/lez/taker/role.json
run chmod 0600 /etc/lez/taker/node.json /etc/lez/taker/role.json
run cat /etc/lez/taker/node.json /etc/lez/taker/role.json
node_only taker
run systemctl daemon-reload
run systemctl start lez-taker-node
run systemctl show -p Type,ActiveState,SubState,MainPID,StatusText lez-taker-node
run systemd-analyze security --no-pager lez-taker-node
run stat -c '%n %F %U:%G %a' /run/lez/taker /run/lez/taker/node.sock
run runuser -u lez-swap -- /usr/bin/lez-taker-cli health
run runuser -u other-user -- /usr/bin/lez-taker-cli health
run runuser -u lez-swap -- /usr/bin/lez-taker-cli swap offers
run /usr/bin/lez-taker-cli stop
run journalctl -u lez-taker-node --no-pager -o cat

step "What the journal shows when the Node refuses its key"
printf '$ (umask 022; head -c 32 /dev/urandom >/etc/lez/maker/world-readable.key)\n'
(umask 022; head -c 32 /dev/urandom >/etc/lez/maker/world-readable.key)
run chown lez-swap:lez-swap /etc/lez/maker/world-readable.key
run cp /etc/lez/maker/node.json /root/node.json.good
run sed -i 's#/run/credentials/lez-maker-node.service/delivery-signing.key#/etc/lez/maker/world-readable.key#' \
  /etc/lez/maker/node.json
run systemctl reset-failed "$unit"
run systemctl restart "$unit"
run journalctl -u "$unit" --no-pager -o cat -n 6
run systemctl stop "$unit"
run rm /etc/lez/maker/world-readable.key
run install -o lez-swap -g lez-swap -m 0600 /root/node.json.good /etc/lez/maker/node.json
run systemctl reset-failed "$unit"
run systemctl start "$unit"
run systemctl show -p ActiveState,StatusText "$unit"

step "Journal"
run journalctl -u "$unit" --no-pager -o short-monotonic
GUEST

cat >"$payload/guest-swap.sh" <<'GUEST'
#!/usr/bin/env bash
set -uo pipefail
export SYSTEMD_COLORS=0 SYSTEMD_URLIFY=0 SYSTEMD_PAGER='' TERM=dumb
step() { printf '\n## %s\n' "$*"; }
run() { printf '$ %s\n' "$*"; "$@"; printf '[exit %s]\n' "$?"; }
maker() { runuser -u lez-swap -- /usr/bin/lez-maker-cli "$@"; }
taker() { runuser -u lez-swap -- /usr/bin/lez-taker-cli "$@"; }
# A CLI call shown with its output, retried while it fails (a take or a lock answers
# dependency-unavailable while the other side is still busy; both are idempotent).
retry() {
  local attempt output
  for attempt in 1 2 3 4 5 6; do
    printf '$ %s\n' "$*"
    output="$("$@" 2>&1)" && { printf '%s\n[exit 0]\n' "$output"; return 0; }
    printf '%s\n[exit 1, attempt %s]\n' "$output" "$attempt"
    sleep 20
  done
  return 1
}

step "VM"
cat /payload/build-info
run grep PRETTY_NAME /etc/os-release
run systemctl --version

step "Prerequisites: jq and socat (the launcher's), from Ubuntu's archive"
run sh -c 'dpkg -i /payload/debs/*.deb >/dev/null && dpkg-query -W jq socat'

step "Install both roles (docs/maker-systemd-install.md)"
run useradd --system --no-create-home --shell /usr/sbin/nologin lez-swap
run env SOURCE_BIN_DIR=/payload/bin /payload/scripts/install-maker-node-service.sh
run env SOURCE_BIN_DIR=/payload/bin /payload/scripts/install-taker-node-service.sh

step "The chains on loopback: QEMU forwards 10.0.2.100 to the devnet, socat to 127.0.0.1"
for port in 18443 3040 8779; do
  run systemd-run --quiet --unit="lez-chain-$port" -p Restart=always \
    socat "TCP-LISTEN:$port,bind=127.0.0.1,fork,reuseaddr" "TCP:10.0.2.100:$port"
done
sleep 2
run sh -c 'curl -sS --user "$(cat /payload/stack/btc-rpc-cookie)" -H "content-type: application/json" --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getblockcount\",\"params\":[]}" http://127.0.0.1:18443/'
run curl -sS http://127.0.0.1:8779/ -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"getLastFinalizedBlockId","params":[]}'

step "Inputs: credentials, identities, cookie copies, market constants, env files"
for name in delivery-signing.key maker-claim-recovery.key maker-claim-preimage.key; do
  printf '$ head -c 32 /dev/urandom | systemd-creds encrypt --name=%s - /etc/lez/maker/credentials/%s.cred\n' "$name" "$name"
  head -c 32 /dev/urandom | systemd-creds encrypt --name="$name" - "/etc/lez/maker/credentials/$name.cred" 2>/dev/null
done
run install -m 0644 /payload/stack/market-bootstrap.env /etc/lez/market-bootstrap.env
for role in maker taker; do
  run install -d -m 0755 "/etc/lez/$role/identity"
  run install -o lez-swap -g lez-swap -m 0400 "/payload/stack/$role/identity.json" "/etc/lez/$role/identity/"
  # The role's LEZ signer key exists on this machine only as its encrypted credential.
  run systemd-creds encrypt --name=lez-signer.key "/payload/stack/$role/lez-signer.key" \
    "/etc/lez/$role/credentials/lez-signer.key.cred"
  run install -o lez-swap -g lez-swap -m 0400 /payload/stack/btc-rpc-cookie "/etc/lez/$role/btc-rpc-cookie"
  sed -e "s#^LEZ_BTC_WALLET=.*#LEZ_BTC_WALLET=$(cat "/payload/stack/$role/wallet")#" \
    -e '/^LEZ_V02_CHANNEL_ID=/d' "/etc/lez/$role/env.example" >"/etc/lez/$role/env"
  # The devnet's channel and timing profile (its runtime.env), as the lane's containers get them.
  cat /payload/stack/profile.env >>"/etc/lez/$role/env"
  run cat "/etc/lez/$role/env"
done
run find /etc/lez -ls
run find /etc/lez /var/lib/lez -name 'lez-signer.key*'

step "Start both units through the launcher"
run systemctl daemon-reload
run systemctl start lez-maker-node
run systemctl start lez-taker-node
for unit in lez-maker-node lez-taker-node; do
  run systemctl show -p Type,ActiveState,SubState,MainPID,NRestarts,StatusText "$unit"
  run journalctl -u "$unit" --no-pager -o cat
done
run systemd-analyze security --no-pager lez-maker-node
run systemd-analyze security --no-pager lez-taker-node
run runuser -u lez-swap -- /usr/bin/lez-maker-cli health
run runuser -u lez-swap -- /usr/bin/lez-taker-cli health

step "The Maker prices and advertises Bitcoin, Taker sells BTC (1 000 000 sat for 1 000 LEZ)"
route=(--pair bitcoin --direction taker-sells-foreign)
bounds=(--minimum-foreign-units 1000000 --maximum-foreign-units 1000000 --offer-ttl-seconds 3600)
retry maker configure-pair --request-id systemd-pair-1 "${route[@]}" --enabled false "${bounds[@]}"
retry maker set-local-price --request-id systemd-price-1 "${route[@]}" --lez-units-per-lot 1 --foreign-units-per-lot 1000
retry maker configure-pair --request-id systemd-pair-2 --expected-revision 1 "${route[@]}" --enabled true "${bounds[@]}"
retry maker quote "${route[@]}"
retry maker publish-offer --request-id systemd-offer-1 --offer-id systemd-offer-1 "${route[@]}"

step "The Taker discovers, takes and locks"
retry taker swap offers "${route[@]}"
retry taker swap take --request-id systemd-take-1 --offer-id systemd-offer-1 --foreign-units 1000000
swap_id="$(taker swap list | jq -r '[.. | objects | select(has("swap_id")) | .swap_id] | first')"
echo "swap: $swap_id"
retry taker swap lock --swap-id "$swap_id"

step "kill -9 of the Maker between the Taker's lock and its claim; systemd restarts it"
pid="$(systemctl show -p MainPID --value lez-maker-node)"
[[ "$pid" -gt 0 ]] && run kill -9 "$pid"
for _ in $(seq 60); do
  [[ "$(systemctl show -p NRestarts --value lez-maker-node)" == 1 \
    && "$(systemctl show -p ActiveState --value lez-maker-node)" == active ]] && break
  sleep 1
done
run systemctl show -p ActiveState,SubState,MainPID,NRestarts,StatusText lez-maker-node

step "The Taker waits for claim_available, claims, and both sides complete"
taker_state() { taker swap monitor --swap-id "$swap_id" 2>/dev/null | jq -r '(.swap // .) | "\(.state) \(.progress_generation)"'; }
last=""
for _ in $(seq 400); do
  read -r state generation <<<"$(taker_state)"
  [[ "$state" != "$last" ]] && echo "taker: $state (generation $generation) at $(date -u +%H:%M:%S)" && last="$state"
  [[ "$state" == claim_available || "$state" == completed || "$state" == refunded || "$state" == attention_required ]] && break
  sleep 3
done
retry taker swap claim --swap-id "$swap_id" --request-id systemd-claim-1 --expected-generation "$generation"
for _ in $(seq 400); do
  read -r state generation <<<"$(taker_state)"
  [[ "$state" != "$last" ]] && echo "taker: $state (generation $generation) at $(date -u +%H:%M:%S)" && last="$state"
  maker_view="$(maker monitor --id "$swap_id" 2>/dev/null)"
  [[ "$state" == completed && "$(jq -r .schedule_state <<<"$maker_view")" == terminal ]] && break
  sleep 3
done
run sh -c "runuser -u lez-swap -- /usr/bin/lez-taker-cli swap monitor --swap-id $swap_id"
run sh -c "runuser -u lez-swap -- /usr/bin/lez-maker-cli monitor --id $swap_id"
run sh -c "runuser -u lez-swap -- /usr/bin/lez-maker-cli history"

step "No copy of either LEZ signer key on disk: only the encrypted credentials"
run find / -xdev -name 'lez-signer.key*' -not -path '/payload/*'

step "Journals"
for unit in lez-maker-node lez-taker-node; do
  run journalctl -u "$unit" --no-pager -o short-monotonic
done
GUEST
chmod 0755 "$payload/guest.sh" "$payload/guest-swap.sh"
tar -C "$payload" -cf "$work/payload.tar" .

cat >"$work/user-data" <<CLOUD
#cloud-config
runcmd:
  - [sh, -c, "mkdir /payload && tar -xf /dev/disk/by-id/virtio-lezpayload -C /payload && stty -F /dev/ttyS1 raw -echo && /payload/$guest >/dev/ttyS1 2>&1; poweroff"]
CLOUD

# The VM reaches the devnet only through these forwards, each a socat per connection to one
# service by its name on the stack's network.
nic=user,restrict=on
docker_network=()
if [[ -n "$stack" ]]; then
  for forward in bitcoin-core:18443 sequencer:3040 indexer:8779; do
    nic+=",guestfwd=tcp:10.0.2.100:${forward#*:}-cmd:socat - TCP:$forward"
  done
  docker_network=(--network "$stack_network")
fi

docker run --rm --device /dev/kvm "${docker_network[@]}" \
  --env image_base="$image_base" --env image_name="$image_name" --env owner="$(id -u):$(id -g)" \
  --env nic="$nic" \
  --volume "$work:/work" --workdir /work \
  ubuntu:24.04 bash -euo pipefail -c '
    trap "chown -R \"\$owner\" /work" EXIT
    apt-get update -qq
    # jq and socat for the VM, which has no network: downloaded here, before this container
    # installs socat itself, so their dependencies come along; dpkg -i in the VM.
    mkdir -p debs
    DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends --download-only \
      -o Dir::Cache::archives=/work/debs jq socat >/dev/null
    tar -C /work -rf payload.tar debs
    truncate -s %1M payload.tar
    DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
      ca-certificates wget qemu-system-x86 qemu-utils cloud-image-utils socat >/dev/null
    wget -q "$image_base/$image_name" "$image_base/SHA256SUMS"
    grep " \*$image_name\$" SHA256SUMS | sha256sum -c - | tee image-check
    cloud-localds seed.img user-data
    qemu-img create -q -f qcow2 -b "$image_name" -F qcow2 vm.qcow2 8G
    timeout 2400 qemu-system-x86_64 -enable-kvm -cpu host -m 4096 -smp 2 -display none \
      -drive file=vm.qcow2,if=none,id=root -device virtio-blk-pci,drive=root,bootindex=0 \
      -drive file=seed.img,format=raw,if=virtio \
      -drive file=payload.tar,format=raw,if=none,readonly=on,id=payload \
      -device virtio-blk-pci,drive=payload,serial=lezpayload \
      -nic "$nic" \
      -serial file:console.log -serial file:transcript.log
  '

install -d "$output_dir"
if [[ -n "$stack" ]]; then
  docker start "${lane_nodes[@]}" >/dev/null
  echo "balances after:" >>"$work/balances"
  balances >>"$work/balances"
  cp "$work/balances" "$output_dir/"
fi
cp "$work/transcript.log" "$work/console.log" "$work/image-check" "$work/SHA256SUMS" "$output_dir/"
echo "transcript: $output_dir/transcript.log"
