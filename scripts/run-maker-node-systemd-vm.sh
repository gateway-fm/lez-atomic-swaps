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
# executed.
#
# The transcript and the console log land in OUTPUT_DIR (default target/maker-systemd-vm).
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly source_bin_dir="${SOURCE_BIN_DIR:-target/debug}"
readonly output_dir="${OUTPUT_DIR:-target/maker-systemd-vm}"
readonly image_base="${UBUNTU_IMAGE_BASE:-https://cloud-images.ubuntu.com/releases/noble/release}"
readonly image_name="ubuntu-24.04-server-cloudimg-amd64.img"

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
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/lez-maker-systemd-vm.XXXXXX")"
readonly work
cleanup() {
  rm -rf -- "$work"
}
trap cleanup EXIT

# The payload is what an operator copies to the machine: the install script, the packaging
# files and the three binaries. It reaches the VM as a raw read-only disk holding a tar.
payload="$work/payload"
install -d "$payload/scripts" "$payload/bin"
readonly binaries=(lez-maker-node lez-maker-cli lez-zec-maker-actor lez-logos-price-worker lez-taker-node lez-taker-cli)
cp scripts/install-maker-node-service.sh scripts/install-taker-node-service.sh "$payload/scripts/"
cp -r packaging "$payload/"
for binary in "${binaries[@]}"; do
  cp "$source_bin_dir/$binary" "$payload/bin/"
done
{
  echo "source commit: $(git rev-parse HEAD)"
  (cd "$payload/bin" && sha256sum "${binaries[@]}")
} >"$payload/build-info"

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
for name in delivery-signing.key maker-claim-recovery.key maker-claim-preimage.key; do
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
chmod 0755 "$payload/guest.sh"
tar -C "$payload" -cf "$work/payload.tar" .
truncate -s %1M "$work/payload.tar"

cat >"$work/user-data" <<'CLOUD'
#cloud-config
runcmd:
  - [sh, -c, "mkdir /payload && tar -xf /dev/disk/by-id/virtio-lezpayload -C /payload && stty -F /dev/ttyS1 raw -echo && /payload/guest.sh >/dev/ttyS1 2>&1; poweroff"]
CLOUD

docker run --rm --device /dev/kvm \
  --env image_base="$image_base" --env image_name="$image_name" --env owner="$(id -u):$(id -g)" \
  --volume "$work:/work" --workdir /work \
  ubuntu:24.04 bash -euo pipefail -c '
    trap "chown -R \"\$owner\" /work" EXIT
    apt-get update -qq
    DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends \
      ca-certificates wget qemu-system-x86 qemu-utils cloud-image-utils >/dev/null
    wget -q "$image_base/$image_name" "$image_base/SHA256SUMS"
    grep " \*$image_name\$" SHA256SUMS | sha256sum -c - | tee image-check
    cloud-localds seed.img user-data
    qemu-img create -q -f qcow2 -b "$image_name" -F qcow2 vm.qcow2 8G
    timeout 1200 qemu-system-x86_64 -enable-kvm -cpu host -m 2048 -smp 2 -display none \
      -drive file=vm.qcow2,if=none,id=root -device virtio-blk-pci,drive=root,bootindex=0 \
      -drive file=seed.img,format=raw,if=virtio \
      -drive file=payload.tar,format=raw,if=none,readonly=on,id=payload \
      -device virtio-blk-pci,drive=payload,serial=lezpayload \
      -nic user,restrict=on \
      -serial file:console.log -serial file:transcript.log
  '

install -d "$output_dir"
cp "$work/transcript.log" "$work/console.log" "$work/image-check" "$work/SHA256SUMS" "$output_dir/"
echo "transcript: $output_dir/transcript.log"
