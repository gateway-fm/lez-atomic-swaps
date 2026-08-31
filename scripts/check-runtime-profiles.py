#!/usr/bin/env python3
"""Fail-closed validator for executable catalog and runtime profile contracts."""

from __future__ import annotations

import json
import pathlib
import re
import subprocess
import sys


ROOT = pathlib.Path(__file__).resolve().parent.parent
CLASSES = {"product", "demo", "reference", "compatibility", "guest", "fuzz"}
STABILITIES = {"local_demo", "experimental", "reference"}


def fail(message: str) -> None:
    raise ValueError(message)


def load_json(path: pathlib.Path) -> object:
    try:
        return json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        fail(f"{path.relative_to(ROOT)}: invalid JSON: {error}")


def cargo_targets(manifest: str) -> set[tuple[str, str, str]]:
    result = subprocess.run(
        ["cargo", "metadata", "--manifest-path", manifest, "--no-deps", "--format-version", "1"],
        cwd=ROOT, check=True, capture_output=True, text=True,
    )
    metadata = json.loads(result.stdout)
    return {
        (manifest, package["name"], target["name"])
        for package in metadata["packages"]
        for target in package["targets"]
        if "bin" in target["kind"]
    }


def validate_catalog(profile_ids: set[str]) -> tuple[list[dict], set[str]]:
    value = load_json(ROOT / "deploy/executables.json")
    if not isinstance(value, dict) or value.get("schema_version") != 1:
        fail("deploy/executables.json: unsupported schema")
    entries = value.get("executables")
    if not isinstance(entries, list) or not entries:
        fail("deploy/executables.json: executable list is empty")
    required = {
        "manifest", "package", "target", "class", "stability", "canonical_name",
        "legacy_aliases", "role", "pair", "profiles",
    }
    observed = set()
    manifests = set()
    canonical_names = set()
    for index, entry in enumerate(entries):
        if not isinstance(entry, dict) or set(entry) != required:
            fail(f"executable {index}: fields do not match catalog schema v1")
        identity = (entry["manifest"], entry["package"], entry["target"])
        if identity in observed:
            fail(f"duplicate executable identity: {identity}")
        observed.add(identity)
        manifests.add(entry["manifest"])
        if entry["class"] not in CLASSES:
            fail(f"{entry['target']}: unknown executable class")
        if not all(isinstance(entry[field], str) and entry[field] for field in
                   ("manifest", "package", "target", "stability", "canonical_name", "role", "pair")):
            fail(f"{entry['target']}: empty catalog field")
        if not isinstance(entry["legacy_aliases"], list) or not isinstance(entry["profiles"], list):
            fail(f"{entry['target']}: aliases/profiles must be arrays")
        unknown_profiles = set(entry["profiles"]) - profile_ids
        if unknown_profiles:
            fail(f"{entry['target']}: unknown profiles {sorted(unknown_profiles)}")
        if entry["class"] == "product" and entry["target"] == entry["canonical_name"]:
            if entry["canonical_name"] in canonical_names:
                fail(f"duplicate canonical product name: {entry['canonical_name']}")
            canonical_names.add(entry["canonical_name"])
    actual = set()
    for manifest in sorted(manifests):
        actual.update(cargo_targets(manifest))
    missing = actual - observed
    stale = observed - actual
    if missing or stale:
        fail(f"executable catalog drift; missing={sorted(missing)} stale={sorted(stale)}")
    return entries, canonical_names


def validate_profiles() -> tuple[set[str], list[dict]]:
    paths = sorted((ROOT / "deploy/profiles").glob("*.json"))
    if not paths:
        fail("no runtime profiles found")
    profiles = []
    ids = set()
    required = {
        "schema_version", "profile_id", "stability", "description", "artifact_identity_policy",
        "components", "verification_commands", "nonclaims",
    }
    component_required = {
        "id", "lifecycle", "executable", "role", "pair", "sockets", "state",
        "credentials", "public_identities", "health", "depends_on",
    }
    for path in paths:
        profile = load_json(path)
        if not isinstance(profile, dict) or set(profile) != required or profile["schema_version"] != 1:
            fail(f"{path.relative_to(ROOT)}: fields do not match profile schema v1")
        if profile["profile_id"] in ids or profile["stability"] not in STABILITIES:
            fail(f"{path.relative_to(ROOT)}: duplicate ID or invalid stability")
        if not isinstance(profile["artifact_identity_policy"], str) \
                or "SHA-256" not in profile["artifact_identity_policy"]:
            fail(f"{path.relative_to(ROOT)}: artifact SHA-256 policy is missing")
        ids.add(profile["profile_id"])
        components = profile["components"]
        if not isinstance(components, list) or not components:
            fail(f"{profile['profile_id']}: no components")
        component_ids = set()
        socket_owners = {}
        state_owners = {}
        for component in components:
            if not isinstance(component, dict) or set(component) != component_required:
                fail(f"{profile['profile_id']}: component fields do not match schema v1")
            if component["id"] in component_ids:
                fail(f"{profile['profile_id']}: duplicate component {component['id']}")
            component_ids.add(component["id"])
            for socket in component["sockets"]:
                if socket in socket_owners:
                    fail(f"{profile['profile_id']}: socket {socket} has two owners")
                socket_owners[socket] = component["id"]
            for state in component["state"]:
                if not isinstance(state, dict) or set(state) != {"path", "backup"}:
                    fail(f"{profile['profile_id']}: invalid state ownership entry")
                if state["path"] in state_owners:
                    fail(f"{profile['profile_id']}: state {state['path']} has two owners")
                state_owners[state["path"]] = component["id"]
        for component in components:
            unknown = set(component["depends_on"]) - component_ids
            if unknown:
                fail(f"{profile['profile_id']}/{component['id']}: unknown dependencies {unknown}")
        if not profile["verification_commands"] or not profile["nonclaims"]:
            fail(f"{profile['profile_id']}: verification and nonclaims are mandatory")
        profiles.append(profile)
    return ids, profiles


def validate_repository_contract(
        entries: list[dict], canonical_names: set[str], profiles: list[dict]) -> None:
    compose = (ROOT / "deploy/compose.yaml").read_text()
    compose_services = compose.split("services:\n", 1)[1].split("\nnetworks:\n", 1)[0]
    compose_service_ids = {
        line.strip()[:-1]
        for line in compose_services.splitlines()
        if line.startswith("  ") and not line.startswith("    ") and line.strip().endswith(":")
    }
    local_profile = next(
        profile for profile in profiles if profile["profile_id"] == "local-btc-demo-v1"
    )
    local_component_ids = {component["id"] for component in local_profile["components"]}
    if compose_service_ids != local_component_ids:
        fail(
            "local profile/Compose component drift; "
            f"missing={sorted(compose_service_ids - local_component_ids)} "
            f"stale={sorted(local_component_ids - compose_service_ids)}"
        )
    controller = compose.split("  btc-demo-controller:\n", 1)[1].split("  basecamp-ui:\n", 1)[0]
    launcher = compose.split("  btc-demo-launcher:\n", 1)[1].split("  btc-demo-controller:\n", 1)[0]
    taker_init = compose.split("  taker-init:\n", 1)[1].split("  taker-node:\n", 1)[0]
    if "/var/run/docker.sock" in controller or "/var/run/docker.sock" not in launcher:
        fail("Docker socket authority must belong only to btc-demo-launcher")
    if "maker_state:/maker-state" in taker_init or "maker-delivery-identity.pub" not in taker_init:
        fail("taker-init must consume only the Maker public identity")
    if "daemon-args.sh" in compose or "lez-maker-node --config" not in compose:
        fail("Compose must use strict Rust-loaded Maker configuration")
    for name in [
        "lez-maker-node", "lez-taker-node", "lez-maker-cli", "lez-taker-cli",
        "lez-maker-chat-gateway", "lez-taker-chat-gateway",
        "lez-btc-maker-actor", "lez-btc-taker-actor",
        "lez-zec-maker-actor", "lez-zec-taker-actor",
    ]:
        if name not in canonical_names:
            fail(f"canonical executable is absent from catalog: {name}")
    if any(entry["class"] in {"demo", "reference", "fuzz", "guest"}
           and "local-btc-demo-v1" in entry["profiles"]
           for entry in entries):
        fail("reference/demo executable declared in local product image profile")
    for role in ("maker", "taker"):
        config = load_json(ROOT / f"deploy/assets/{role}-node.json")
        if not isinstance(config, dict) or set(config) != {"schema_version", "arguments"} \
                or config["schema_version"] != 1 or "--config" in config["arguments"]:
            fail(f"default {role.title()} Node configuration is not strict schema v1")

    catalog_targets = {entry["target"] for entry in entries}
    obsolete_targets = {
        "lez-maker-daemon", "lez-taker-service", "lez-maker", "lez-taker",
        "lez-logos-chat-gateway", "xmr-maker-actor",
        "btc-reference-actor", "zec-reference-actor",
    }
    if catalog_targets & obsolete_targets:
        fail(f"obsolete public targets remain: {sorted(catalog_targets & obsolete_targets)}")
    if any(entry["legacy_aliases"] for entry in entries if entry["class"] == "product"):
        fail("product executable catalog must not carry migration aliases")

    validate_role_symmetry(compose)


def validate_role_symmetry(compose: str) -> None:
    contract = load_json(ROOT / "deploy/role-symmetry.json")
    required = {
        "schema_version", "shared_node_flags", "swap_directions", "roles",
        "intentional_asymmetries",
    }
    if not isinstance(contract, dict) or set(contract) != required \
            or contract["schema_version"] != 1:
        fail("deploy/role-symmetry.json: fields do not match schema v1")
    if contract["shared_node_flags"] != ["--config", "--socket", "--ready-file"]:
        fail("role symmetry contract must fix the common Node startup flags")
    if contract["swap_directions"] != {
        "btc_to_lez": "TakerSellsForeign", "lez_to_btc": "TakerSellsLez"
    }:
        fail("role symmetry contract must fix both BTC/LEZ economic directions")
    if set(contract["roles"]) != {"maker", "taker"}:
        fail("role symmetry contract must contain exactly Maker and Taker")
    role_fields = {
        "node", "image", "cli", "ui", "chat_gateway", "systemd_unit", "node_config",
        "runtime_directory", "state_directory", "socket", "ready_file",
        "private_authority", "peer_material",
    }
    flake = (ROOT / "apps/basecamp/flake.nix").read_text()
    ui_image = (ROOT / "deploy/images/basecamp-ui/Dockerfile").read_text()
    unit_baseline = None
    for role in ("maker", "taker"):
        details = contract["roles"][role]
        if not isinstance(details, dict) or set(details) != role_fields:
            fail(f"role symmetry {role}: fields do not match schema v1")
        expected = {
            "node": f"lez-{role}-node",
            "image": f"lez-{role}-node:local",
            "cli": f"lez-{role}-cli",
            "ui": f"lez-{role}-ui",
            "chat_gateway": f"lez-{role}-chat-gateway",
            "systemd_unit": f"lez-{role}-node.service",
            "node_config": f"deploy/assets/{role}-node.json",
            "runtime_directory": f"/run/lez/{role}",
            "state_directory": f"/var/lib/lez/{role}",
            "socket": f"/run/lez/{role}/node.sock",
            "ready_file": f"/run/lez/{role}/ready",
        }
        for field, value in expected.items():
            if details[field] != value:
                fail(f"role symmetry {role}: {field} must equal {value}")
        opposite = "taker" if role == "maker" else "maker"
        source_root = ROOT / f"crates/{role}-node/src"
        for source in source_root.rglob("*.rs"):
            if source.name.startswith((f"lez-{opposite}-", f"{opposite}_")):
                fail(
                    f"role symmetry {role}: {source.relative_to(ROOT)} "
                    f"is owned by {opposite}"
                )
        catalog = load_json(ROOT / "deploy/executables.json")
        catalog_packages = {
            entry["target"]: entry["package"] for entry in catalog["executables"]
        }
        for executable in (details["node"], details["cli"], details["chat_gateway"]):
            if catalog_packages.get(executable) != f"lez-{role}-node":
                fail(f"role symmetry {role}: {executable} is not owned by its role package")
        dockerfile = (ROOT / f"deploy/images/{role}-node/Dockerfile").read_text()
        for executable in (details["node"], details["cli"], details["chat_gateway"]):
            copy = f"COPY --chmod=0555 {executable} /usr/local/bin/{executable}"
            if copy not in dockerfile:
                fail(f"role symmetry {role}: image omits {executable}")
        if f"lez-{opposite}-" in dockerfile:
            fail(f"role symmetry {role}: image contains {opposite} executable")
        if details["runtime_directory"] not in dockerfile \
                or details["state_directory"] not in dockerfile:
            fail(f"role symmetry {role}: image omits role-local runtime directories")
        if f"/run/lez/{opposite}" in dockerfile \
                or f"/var/lib/lez/{opposite}" in dockerfile:
            fail(f"role symmetry {role}: image creates {opposite} directories")
        service_marker = f"  {role}-node:\n"
        try:
            service = compose.split(service_marker, 1)[1]
        except IndexError:
            fail(f"role symmetry {role}: Compose service is absent")
        service = re.split(r"\n  (?=\S)", service, maxsplit=1)[0]
        if f"image: {details['image']}" not in service:
            fail(f"role symmetry {role}: Compose image must equal {details['image']}")
        config = load_json(ROOT / details["node_config"])
        arguments = config["arguments"]
        for flag, value in (("--socket", details["socket"]),
                            ("--ready-file", details["ready_file"])):
            try:
                index = arguments.index(flag)
            except ValueError:
                fail(f"role symmetry {role}: Node config omits {flag}")
            if index + 1 >= len(arguments) or arguments[index + 1] != value:
                fail(f"role symmetry {role}: {flag} path drift")
        unit = (ROOT / "packaging/systemd" / details["systemd_unit"]).read_text()
        for directive in (
            "Type=notify", "NotifyAccess=main", "RuntimeDirectory=lez/" + role,
            "StateDirectory=lez/" + role, "UMask=0077", "NoNewPrivileges=yes",
            "ProtectSystem=strict", "KillMode=control-group",
            f"ExecStart=/usr/bin/lez-{role}-node --config /etc/lez/{role}/node.json",
        ):
            if directive not in unit:
                fail(f"role symmetry {role}: systemd unit omits {directive}")
        baseline = {
            line for line in unit.splitlines()
            if line.startswith(("Protect", "Private", "Restrict", "Capability", "Memory",
                                "LockPersonality", "NoNewPrivileges", "KillMode", "UMask"))
        }
        if unit_baseline is None:
            unit_baseline = baseline
        elif baseline != unit_baseline:
            fail("Maker and Taker systemd hardening baselines drift")
        for attr in (details["ui"], details["ui"] + "-lgx",
                     details["ui"] + "-install", details["ui"] + "-integration-test"):
            if f"{attr} =" not in flake:
                fail(f"role symmetry {role}: Basecamp output {attr} is absent")
        if f"/usr/local/bin/{details['ui']}" not in ui_image:
            fail(f"role symmetry {role}: UI launcher is absent")
        for public_name in (details["node"], details["cli"], details["chat_gateway"]):
            if public_name not in compose and public_name not in (ROOT / "deploy/executables.json").read_text():
                fail(f"role symmetry {role}: public component {public_name} is absent")
    if len(contract["intentional_asymmetries"]) < 4:
        fail("role symmetry contract must explain intentional key and protocol asymmetries")
    taker_sections = compose.split("  taker-init:\n", 1)[1].split("  btc-demo-init:\n", 1)[0]
    if "delivery-signing.key" in taker_sections or "maker_state:" in taker_sections:
        fail("Taker setup must not receive Maker private key or state authority")


def main() -> None:
    profile_ids, profiles = validate_profiles()
    entries, canonical_names = validate_catalog(profile_ids)
    validate_repository_contract(entries, canonical_names, profiles)
    print(f"runtime profiles valid: {len(profile_ids)} profiles, {len(entries)} Cargo targets")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, subprocess.CalledProcessError) as error:
        print(f"runtime profile validation failed: {error}", file=sys.stderr)
        raise SystemExit(1)
