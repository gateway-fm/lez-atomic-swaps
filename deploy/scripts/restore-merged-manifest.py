#!/usr/bin/env python3
"""restore-merged-manifest.py — give a merged package back the manifest its
variants were built with.

    scripts/restore-merged-manifest.py <merged.lgx> <variant.lgx>...

`lgx merge` writes the merged package's manifest from the fields its manifest
model knows and drops the others: `display_name` goes missing (the desks
show `lez_atomic_swap_maker` in Basecamp's sidebar instead of "LEZ Maker",
lez_core loses "LEZ Core Module"). This rewrites the merged package's
manifest.json as the variants' common manifest with the merged `main` and
`hashes`. The variants must agree on every field but `main` and `hashes`,
and together carry exactly the merged package's variants with the same
entry points. Only manifest.json changes; the root hash covers the
variants' files, not the manifest. The package is written without a gzip
timestamp, so the same inputs give the same bytes.
"""
import gzip
import io
import json
import os
import sys
import tarfile

PER_VARIANT = ("main", "hashes")


def read(path):
    with tarfile.open(path, "r:gz") as t:
        members = [(m, t.extractfile(m).read() if m.isfile() else None) for m in t.getmembers()]
    data = next((d for m, d in members if m.name == "manifest.json"), None)
    if data is None:
        sys.exit(f"{path}: no manifest.json (not an LGX package)")
    return members, json.loads(data)


def common(manifest):
    return {k: v for k, v in manifest.items() if k not in PER_VARIANT}


def main(argv):
    if len(argv) < 2:
        sys.exit(__doc__.split("\n\n")[1])
    merged_path, variant_paths = argv[0], argv[1:]
    members, merged = read(merged_path)
    variants = [(p, read(p)[1]) for p in variant_paths]
    first_path, first = variants[0]
    for path, manifest in variants[1:]:
        if common(manifest) != common(first):
            keys = sorted(k for k in set(common(manifest)) | set(common(first))
                          if manifest.get(k) != first.get(k))
            sys.exit(f"{path} and {first_path} differ in {', '.join(keys)}")
    entry = {}
    for path, manifest in variants:
        for variant, target in manifest.get("main", {}).items():
            if variant in entry:
                sys.exit(f"{path}: variant {variant} is in two inputs")
            entry[variant] = target
    if entry != merged.get("main"):
        sys.exit(f"{merged_path} carries {merged.get('main')}, its inputs {entry}")
    restored = dict(common(first), main=merged["main"], hashes=merged["hashes"])
    lost = sorted(set(restored) - set(merged))
    tmp = merged_path + ".part"
    with open(tmp, "wb") as raw, gzip.GzipFile(fileobj=raw, mode="wb", mtime=0) as gz, \
            tarfile.open(fileobj=gz, mode="w", format=tarfile.GNU_FORMAT) as t:
        for m, d in members:
            if m.name == "manifest.json":
                d = json.dumps(restored, indent=2).encode()
                m.size = len(d)
            t.addfile(m, io.BytesIO(d) if d is not None else None)
    os.chmod(tmp, 0o644)
    os.replace(tmp, merged_path)
    print(f"{restored['name']} {restored['version']}: manifest restored"
          f" ({', '.join(sorted(restored['main']))}; {'restored ' + ', '.join(lost) if lost else 'nothing was lost'})")


if __name__ == "__main__":
    main(sys.argv[1:])
