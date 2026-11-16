#!/usr/bin/env bash
# pin-package-dependencies.sh — write one release set of Logos packages with
# version requirements on our packages' dependencies.
#
#   scripts/pin-package-dependencies.sh --out <dir> <package.lgx>...
#
# Every package of the set is written to <dir>/<name>.lgx, and <dir>/packages.tsv
# lists name, version, variants, dependencies, sha256 and root hash. Ours
# (the role desks, the LEZ Network desk, lez_swap_node, lez_<role>_node) get
# each dependency once, as `{"name": n, "version": <requirement>}`, so an
# installer cannot substitute the default catalog's other packages of the
# same name (lez_core 0.4.3 and 0.5.1, lez_indexer_module 1.1.2 and 1.2.1,
# chat_module 0.2.2 and 0.3.0, ...). Each dependency of ours must be in the
# set; the requirement is the version of that package in the set:
#   - exact (`0.4.1-sign.4`) for our own packages, which ship as one set at one
#     version (AGENTS.md) and are tested only together, and for every package
#     whose version is a pre-release: the patched upstream modules (lez_core,
#     lez_indexer_module) and the Chat module our catalog publishes
#     (package-chat-module.sh, chat_module 0.2.2-delivery.1). A pre-release
#     sorts below the official release it is cut from, so a range on it takes
#     that release instead (`~0.4.1-sign.4` also takes lez_core 0.4.1 to
#     0.4.3);
#   - `~<version>` for a Logos module at a release version (blockchain_module,
#     delivery_module, and chat_module when the set carries the release, as
#     the images' set does): a patch release of theirs may stand in.
# logosctl 0.3.x resolves each entry of a manifest on its own: a bare name
# next to a pinned entry of the same name pulls in the newest release as
# well, so no name is listed twice and none bare. A package that already
# carries pinned entries (both forms included) is pinned again from its
# names. The other packages (lez_core, lez_indexer_module, blockchain_module,
# chat_module, delivery_module) are written unchanged. For the same reason a
# name that two packages of the set require with a version must be required
# the same way by both, or one plan would hold two versions of it: the desks'
# `delivery_module` requirement must be the one the set's chat_module declares.
#
# Basecamp 0.2.0 reads only string entries from an installed manifest
# (logos-package-manager package_manager_lib.cpp, Basecamp PluginLoader.cpp);
# stage-basecamp-package.sh and the role Node images install the bare names
# for it. metadata.json keeps bare names: the module builder generates one
# client per entry there. Only manifest.json changes; the root hash covers the
# variants' files, not the manifest, so it stays what the builder wrote.
#
# A package built as the module builder's development package (variant
# `<platform>-dev`, libraries resolved from /nix/store) is refused: build its
# `lgx-portable` output.
set -euo pipefail

out=""
pkgs=()
while (($#)); do
  case "$1" in
    --out) out="${2:?}"; shift 2 ;;
    -h|--help) sed -n '2,45p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) pkgs+=("$1"); shift ;;
  esac
done
[[ -n "$out" && ${#pkgs[@]} -gt 0 ]] || { echo "usage: $0 --out <dir> <package.lgx>..." >&2; exit 64; }
command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 66; }
mkdir -p "$out"

python3 -I - "$out" "${pkgs[@]}" <<'PY'
import gzip, hashlib, io, json, os, shutil, sys, tarfile

OURS = {"lez_atomic_swap_maker", "lez_atomic_swap_taker", "lez_network",
        "lez_swap_node", "lez_maker_node", "lez_taker_node"}
out, paths = sys.argv[1], sys.argv[2:]

def dep_names(deps):
    names = []
    for d in deps or []:
        n = d if isinstance(d, str) else d.get("name", "")
        if n and n not in names:
            names.append(n)
    return names

def requirement(name, version):
    # ours and every pre-release (a patched build) exactly; a Logos release
    # with `~`, so a patch release of theirs may stand in
    return version if name in OURS or "-" in version.split("+")[0] else "~" + version

packages = {}
for path in paths:
    with tarfile.open(path, "r:gz") as t:
        members = [(m, t.extractfile(m).read() if m.isfile() else None) for m in t.getmembers()]
    manifest = next((json.loads(d) for m, d in members if m.name == "manifest.json"), None)
    if manifest is None:
        sys.exit(f"{path}: no manifest.json (not an LGX package)")
    name = manifest["name"]
    dev = [v for v in manifest.get("main", {}) if v.endswith("-dev")]
    if dev:
        sys.exit(f"{path}: {name} is the module builder's development package (variant {', '.join(dev)}); build its lgx-portable output")
    if name in packages:
        sys.exit(f"{path}: {name} is in the set twice")
    packages[name] = (path, members, manifest)

for name, (path, members, manifest) in packages.items():
    if name in OURS:
        names = dep_names(manifest.get("dependencies"))
        missing = [n for n in names if n not in packages]
        if missing:
            sys.exit(f"{name}: its dependencies {', '.join(missing)} are not in the release set")
        manifest["dependencies"] = [
            {"name": n, "version": requirement(n, packages[n][2]["version"])} for n in names]

# one requirement per name across the set: the resolver plans each entry on
# its own, so two requirements of one name can plan two versions of it
required = {}
for name, (path, members, manifest) in sorted(packages.items()):
    for d in manifest.get("dependencies") or []:
        if isinstance(d, dict) and d.get("version"):
            required.setdefault(d["name"], {}).setdefault(d["version"], []).append(name)
for dep, by_req in sorted(required.items()):
    if len(by_req) > 1:
        sys.exit(f"{dep} is required in more than one way in the set: "
                 + "; ".join(f"{r} by {', '.join(who)}" for r, who in sorted(by_req.items())))

rows = []
for name, (path, members, manifest) in packages.items():
    dest = os.path.join(out, name + ".lgx")
    tmp = dest + ".part"
    if name in OURS:
        # gzip without a timestamp, so the same inputs give the same bytes
        with open(tmp, "wb") as raw, gzip.GzipFile(fileobj=raw, mode="wb", mtime=0) as gz, \
                tarfile.open(fileobj=gz, mode="w", format=tarfile.GNU_FORMAT) as t:
            for m, d in members:
                if m.name == "manifest.json":
                    d = json.dumps(manifest, indent=2).encode()
                    m.size = len(d)
                t.addfile(m, io.BytesIO(d) if d is not None else None)
    else:
        shutil.copyfile(path, tmp)  # byte for byte: its published sha256 still holds
    os.chmod(tmp, 0o644)
    os.replace(tmp, dest)
    with open(dest, "rb") as f:
        sha = hashlib.sha256(f.read()).hexdigest()
    deps = [f'{d["name"]} {d.get("version", "*")}'
            for d in manifest.get("dependencies") or [] if not isinstance(d, str)] \
        or dep_names(manifest.get("dependencies"))
    rows.append([name, manifest["version"], ",".join(sorted(manifest.get("main", {}))),
                 "; ".join(deps), sha, manifest.get("hashes", {}).get("root", "")])
    print(f"{name} {manifest['version']} -> {dest}")

with open(os.path.join(out, "packages.tsv"), "w") as f:
    f.write("name\tversion\tvariants\tdependencies\tsha256\troot hash\n")
    for r in sorted(rows):
        f.write("\t".join(r) + "\n")
PY
