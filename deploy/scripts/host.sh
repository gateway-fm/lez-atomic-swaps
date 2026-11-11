# shellcheck shell=bash
# host.sh — sourced: what these scripts need from the host shell, and the few
# commands that differ between GNU (Linux) and BSD (macOS) userlands.
#
# The scripts use bash 4.4 features (mapfile, namerefs, associative arrays, empty
# arrays under `set -u`). macOS ships bash 3.2 as /bin/bash, where they fail in
# the middle of a run with "command not found"; this refuses at the start
# instead. On Linux nothing changes: every supported distribution has 4.4.
if [[ -z "${BASH_VERSINFO:-}" ]] || (( BASH_VERSINFO[0] < 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] < 4) )); then
  echo "these scripts need bash 4.4 or newer (regress.sh: 5.1); this shell is bash ${BASH_VERSION:-?}." >&2
  [[ "$(uname -s)" != Darwin ]] ||
    echo "macOS ships bash 3.2: run 'brew install bash coreutils', open a new terminal and start again." >&2
  exit 69
fi
# host_require_bash51: regress.sh waits with `wait -n -p`
host_require_bash51() {
  (( BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 1) )) && return 0
  echo "regress.sh needs bash 5.1 or newer; this shell is bash $BASH_VERSION" >&2
  return 69
}

# macOS before 15 has shasum but no sha256sum.
command -v sha256sum >/dev/null 2>&1 || sha256sum() { shasum -a 256 "$@"; }

# host_mtime <file>: modification time, seconds since the epoch
host_mtime() { stat -c %Y "$1" 2>/dev/null || stat -f %m "$1"; }
# host_mode <file>: permission bits in octal (700)
host_mode() { stat -c %a "$1" 2>/dev/null || stat -f %Lp "$1"; }
# host_sed_inplace <sed script> <file>: GNU sed as before; BSD sed wants `-i ''`
host_sed_inplace() {
  if sed --version >/dev/null 2>&1; then sed -i "$1" "$2"; else sed -i '' "$1" "$2"; fi
}
# host_setsid <command...>: the command as the leader of a new session and
# process group (util-linux setsid; macOS has none, python3 is required anyway)
host_setsid() {
  if command -v setsid >/dev/null 2>&1; then setsid "$@"
  else python3 -c 'import os, sys; os.setsid(); os.execvp(sys.argv[1], sys.argv[1:])' "$@"; fi
}
# host_require_timeout: coreutils `timeout`, which macOS does not ship
host_require_timeout() {
  command -v timeout >/dev/null 2>&1 && return 0
  echo "timeout (GNU coreutils) is required; on macOS: brew install coreutils" >&2
  return 69
}
