#!/usr/bin/env python3
"""Validate and render the machine-readable threat model.

The checker deliberately uses only the Python standard library so the
repository-local documentation lane remains dependency-free.
"""

from __future__ import annotations

import argparse
import difflib
import json
import os
import re
import subprocess
import sys
import tempfile
from collections import Counter, defaultdict
from pathlib import Path
from typing import Any, Iterable


REPOSITORY_ROOT = Path(__file__).resolve().parent.parent
MODEL_PATH = REPOSITORY_ROOT / "docs/milestone-1/threat-model.json"
DOCUMENT_PATH = REPOSITORY_ROOT / "docs/milestone-1/threat-model.md"

TOP_LEVEL_KEYS = {
    "schema_version",
    "system",
    "legacy_ids",
    "profiles",
    "routes",
    "phases",
    "views",
    "boundaries",
    "elements",
    "flows",
    "assets",
    "invariants",
    "threats",
}
SYSTEM_KEYS = {
    "name",
    "scope",
    "implementation",
    "package",
    "method",
    "exclusions",
}
PROFILE_KEYS = {"id", "name", "meaning"}
ROUTE_KEYS = {"id", "name", "directions"}
PHASE_KEYS = {"id", "name"}
VIEW_KEYS = {
    "id",
    "name",
    "includes",
    "stride_exceptions",
    "privacy_relevant",
}
BOUNDARY_KEYS = {"id", "name"}
ELEMENT_KEYS = {"id", "name", "view", "boundary"}
FLOW_KEYS = {"id", "from", "to", "data", "two_way"}
ASSET_KEYS = {"id", "name"}
INVARIANT_KEYS = {"id", "text", "routes", "proofs"}
THREAT_KEYS = {
    "id",
    "old_ids",
    "view",
    "scenario",
    "scope",
    "classes",
    "harms",
    "milestones",
    "risk",
    "control",
    "proofs",
    "remaining",
    "owner",
    "state",
}
SCOPE_KEYS = {"routes", "directions", "phases", "profiles", "targets"}
RISK_KEYS = {"profile", "inherent", "residual"}
RISK_PAIR_KEYS = {"likelihood", "impact"}

STATES = {"open", "working", "checking"}
MILESTONES = {f"M{number}" for number in range(1, 8)}
STRIDE_CLASSES = {
    "stride.spoofing",
    "stride.tampering",
    "stride.repudiation",
    "stride.information-disclosure",
    "stride.denial-of-service",
    "stride.elevation-of-privilege",
}
LINDDUN_CLASSES = {
    "privacy.linkability",
    "privacy.identifiability",
    "privacy.non-repudiation",
    "privacy.detectability",
    "privacy.disclosure",
    "privacy.unawareness",
    "privacy.non-compliance",
}
PROTOCOL_CLASSES = {
    "protocol.authorization",
    "protocol.binding",
    "protocol.branch-exclusivity",
    "protocol.canonicality",
    "protocol.concurrency",
    "protocol.deadline-inclusion",
    "protocol.economic-griefing",
    "protocol.ordering",
    "protocol.recovery",
    "protocol.replay-durability",
    "protocol.secret-lifecycle",
    "protocol.supply-chain",
}
PROTOCOL_STATE_CLASSES = PROTOCOL_CLASSES - {"protocol.supply-chain"}
KNOWN_CLASSES = STRIDE_CLASSES | LINDDUN_CLASSES | PROTOCOL_CLASSES
ALL_SUPPORTED_DIRECTIONS = "all-supported"
CLASS_PATTERN = re.compile(r"^(?:stride|privacy|protocol)\.[a-z][a-z0-9-]*$")
ID_PATTERN = re.compile(r"^[A-Za-z][A-Za-z0-9._-]*$")
THREAT_ID_PATTERN = re.compile(r"^TM-[A-Z0-9]+(?:-[A-Z0-9]+)*$")
SOURCE_PROOF_PATTERN = re.compile(r"^source@([0-9a-f]{40}):([^\s]+)$")

# Rows are likelihood 1..5; columns are impact 1..5.
RISK_MATRIX = (
    ("Low", "Low", "Low", "Medium", "Medium"),
    ("Low", "Low", "Medium", "Medium", "High"),
    ("Low", "Medium", "Medium", "High", "High"),
    ("Medium", "Medium", "High", "High", "Critical"),
    ("Medium", "High", "High", "Critical", "Critical"),
)
RISK_ORDER = {"Low": 0, "Medium": 1, "High": 2, "Critical": 3}


class DuplicateKeyError(ValueError):
    pass


def unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise DuplicateKeyError(f"duplicate object key {key!r}")
        result[key] = value
    return result


class ModelValidator:
    def __init__(self, model: Any, source_repo: Path | None = None) -> None:
        self.model = model
        self.source_repo = source_repo
        self.errors: list[str] = []
        self.external_proofs: set[str] = set()
        self._source_proof_results: dict[str, bool] = {}

    def error(self, path: str, message: str) -> None:
        self.errors.append(f"{path}: {message}")

    def exact_object(
        self, value: Any, path: str, keys: set[str]
    ) -> dict[str, Any] | None:
        if not isinstance(value, dict):
            self.error(path, "must be an object")
            return None
        actual = set(value)
        missing = sorted(keys - actual)
        unknown = sorted(actual - keys)
        if missing:
            self.error(path, f"missing keys: {', '.join(missing)}")
        if unknown:
            self.error(path, f"unknown keys: {', '.join(unknown)}")
        return value

    def string(self, value: Any, path: str, *, allow_empty: bool = False) -> str | None:
        if not isinstance(value, str):
            self.error(path, "must be a string")
            return None
        if not allow_empty and not value.strip():
            self.error(path, "must not be empty")
            return None
        return value

    def boolean(self, value: Any, path: str) -> bool | None:
        if not isinstance(value, bool):
            self.error(path, "must be a boolean")
            return None
        return value

    def array(
        self, value: Any, path: str, *, nonempty: bool = True
    ) -> list[Any] | None:
        if not isinstance(value, list):
            self.error(path, "must be an array")
            return None
        if nonempty and not value:
            self.error(path, "must not be empty")
        return value

    def string_array(
        self,
        value: Any,
        path: str,
        *,
        nonempty: bool = True,
        unique: bool = True,
    ) -> list[str]:
        values = self.array(value, path, nonempty=nonempty)
        if values is None:
            return []
        result: list[str] = []
        for index, item in enumerate(values):
            if self.string(item, f"{path}[{index}]") is not None:
                result.append(item)
        if unique:
            duplicates = sorted(
                item for item, count in Counter(result).items() if count > 1
            )
            if duplicates:
                self.error(path, f"contains duplicates: {', '.join(duplicates)}")
        return result

    def id_value(self, value: Any, path: str, *, threat: bool = False) -> str | None:
        result = self.string(value, path)
        if result is None:
            return None
        pattern = THREAT_ID_PATTERN if threat else ID_PATTERN
        if pattern.fullmatch(result) is None:
            self.error(path, "has an invalid ID format")
            return None
        return result

    def records(
        self, key: str, keys: set[str]
    ) -> tuple[list[dict[str, Any]], dict[str, dict[str, Any]]]:
        values = self.array(self.model.get(key), key)
        if values is None:
            return [], {}
        records: list[dict[str, Any]] = []
        by_id: dict[str, dict[str, Any]] = {}
        for index, value in enumerate(values):
            path = f"{key}[{index}]"
            record = self.exact_object(value, path, keys)
            if record is None:
                continue
            record_id = self.id_value(
                record.get("id"), f"{path}.id", threat=key == "threats"
            )
            if record_id is not None:
                if record_id in by_id:
                    self.error(f"{path}.id", f"duplicates {record_id}")
                else:
                    by_id[record_id] = record
            records.append(record)
        return records, by_id

    def refs(
        self,
        values: list[str],
        allowed: set[str],
        path: str,
        label: str,
    ) -> None:
        for index, value in enumerate(values):
            if value not in allowed:
                self.error(f"{path}[{index}]", f"unknown {label} {value!r}")

    def proofs(self, values: Any, path: str, *, nonempty: bool = True) -> list[str]:
        proofs = self.string_array(values, path, nonempty=nonempty)
        for index, proof in enumerate(proofs):
            proof_path = f"{path}[{index}]"
            source_match = SOURCE_PROOF_PATTERN.fullmatch(proof)
            if source_match is not None:
                self.external_proofs.add(proof)
                source_path = Path(source_match.group(2))
                if source_path.is_absolute() or ".." in source_path.parts:
                    self.error(
                        proof_path, "source proof path must be repository-relative"
                    )
                    continue
                if self.source_repo is not None:
                    resolved = self._source_proof_results.get(proof)
                    if resolved is None:
                        result = subprocess.run(
                            [
                                "git",
                                "-C",
                                str(self.source_repo),
                                "cat-file",
                                "-e",
                                f"{source_match.group(1)}:{source_match.group(2)}",
                            ],
                            check=False,
                            stdout=subprocess.DEVNULL,
                            stderr=subprocess.DEVNULL,
                        )
                        resolved = result.returncode == 0
                        self._source_proof_results[proof] = resolved
                    if not resolved:
                        self.error(
                            proof_path, "source proof does not resolve in --source-repo"
                        )
                continue
            local_path = Path(proof)
            if local_path.is_absolute() or ".." in local_path.parts:
                self.error(proof_path, "proof path must be repository-relative")
                continue
            if not (REPOSITORY_ROOT / local_path).is_file():
                self.error(proof_path, f"local proof does not exist: {proof!r}")
        return proofs

    def validate(self) -> list[str]:
        root = self.exact_object(self.model, "$", TOP_LEVEL_KEYS)
        if root is None:
            return self.errors

        if type(root.get("schema_version")) is not int:
            self.error("schema_version", "must be integer 1")
        elif root["schema_version"] != 1:
            self.error("schema_version", "unsupported version; expected 1")

        system = self.exact_object(root.get("system"), "system", SYSTEM_KEYS)
        if system is not None:
            for key in sorted(SYSTEM_KEYS - {"exclusions"}):
                self.string(system.get(key), f"system.{key}")
            self.string_array(
                system.get("exclusions"), "system.exclusions", nonempty=False
            )

        legacy_ids = self.string_array(
            root.get("legacy_ids"), "legacy_ids", nonempty=False
        )
        for index, legacy_id in enumerate(legacy_ids):
            self.id_value(legacy_id, f"legacy_ids[{index}]", threat=True)

        profiles, profile_by_id = self.records("profiles", PROFILE_KEYS)
        routes, route_by_id = self.records("routes", ROUTE_KEYS)
        phases, phase_by_id = self.records("phases", PHASE_KEYS)
        views, view_by_id = self.records("views", VIEW_KEYS)
        boundaries, boundary_by_id = self.records("boundaries", BOUNDARY_KEYS)
        elements, element_by_id = self.records("elements", ELEMENT_KEYS)
        flows, flow_by_id = self.records("flows", FLOW_KEYS)
        assets, asset_by_id = self.records("assets", ASSET_KEYS)
        invariants, invariant_by_id = self.records("invariants", INVARIANT_KEYS)
        threats, threat_by_id = self.records("threats", THREAT_KEYS)

        namespaces = {
            "profiles": profile_by_id,
            "routes": route_by_id,
            "phases": phase_by_id,
            "views": view_by_id,
            "boundaries": boundary_by_id,
            "elements": element_by_id,
            "flows": flow_by_id,
            "assets": asset_by_id,
            "invariants": invariant_by_id,
            "threats": threat_by_id,
        }
        global_ids: dict[str, str] = {}
        for namespace, records_by_id in namespaces.items():
            for record_id in records_by_id:
                previous = global_ids.get(record_id)
                if previous is not None:
                    self.error(
                        namespace, f"ID {record_id!r} is already used by {previous}"
                    )
                else:
                    global_ids[record_id] = namespace

        for index, profile in enumerate(profiles):
            path = f"profiles[{index}]"
            self.string(profile.get("name"), f"{path}.name")
            self.string(profile.get("meaning"), f"{path}.meaning")

        all_directions: set[str] = set()
        route_directions: dict[str, set[str]] = {}
        for index, route in enumerate(routes):
            path = f"routes[{index}]"
            self.string(route.get("name"), f"{path}.name")
            directions = self.string_array(
                route.get("directions"), f"{path}.directions"
            )
            route_id = route.get("id")
            if isinstance(route_id, str):
                route_directions[route_id] = set(directions)
            all_directions.update(directions)

        for index, phase in enumerate(phases):
            self.string(phase.get("name"), f"phases[{index}].name")

        for index, view in enumerate(views):
            path = f"views[{index}]"
            self.string(view.get("name"), f"{path}.name")
            self.string_array(view.get("includes"), f"{path}.includes")
            self.boolean(view.get("privacy_relevant"), f"{path}.privacy_relevant")
            exceptions = view.get("stride_exceptions")
            if not isinstance(exceptions, dict):
                self.error(
                    f"{path}.stride_exceptions",
                    "must be an object of STRIDE class to reason",
                )
            else:
                for class_name, reason in exceptions.items():
                    if class_name not in STRIDE_CLASSES:
                        self.error(
                            f"{path}.stride_exceptions.{class_name}",
                            "is not a recognized STRIDE class",
                        )
                    self.string(reason, f"{path}.stride_exceptions.{class_name}")

        for index, boundary in enumerate(boundaries):
            self.string(boundary.get("name"), f"boundaries[{index}].name")

        element_boundaries: set[str] = set()
        for index, element in enumerate(elements):
            path = f"elements[{index}]"
            self.string(element.get("name"), f"{path}.name")
            view = self.string(element.get("view"), f"{path}.view")
            if view is not None and view not in view_by_id:
                self.error(f"{path}.view", f"unknown view {view!r}")
            boundary = self.string(element.get("boundary"), f"{path}.boundary")
            if boundary is not None and boundary not in boundary_by_id:
                self.error(f"{path}.boundary", f"unknown boundary {boundary!r}")
            elif boundary is not None:
                element_boundaries.add(boundary)

        self.require_coverage("boundaries", set(boundary_by_id), element_boundaries)

        for index, flow in enumerate(flows):
            path = f"flows[{index}]"
            source = self.string(flow.get("from"), f"{path}.from")
            target = self.string(flow.get("to"), f"{path}.to")
            self.string(flow.get("data"), f"{path}.data")
            self.boolean(flow.get("two_way"), f"{path}.two_way")
            if source is not None and source not in element_by_id:
                self.error(f"{path}.from", f"unknown element {source!r}")
            if target is not None and target not in element_by_id:
                self.error(f"{path}.to", f"unknown element {target!r}")

        for index, asset in enumerate(assets):
            self.string(asset.get("name"), f"assets[{index}].name")

        for index, invariant in enumerate(invariants):
            path = f"invariants[{index}]"
            self.string(invariant.get("text"), f"{path}.text")
            invariant_routes = self.string_array(
                invariant.get("routes"), f"{path}.routes"
            )
            self.refs(invariant_routes, set(route_by_id), f"{path}.routes", "route")
            self.proofs(invariant.get("proofs"), f"{path}.proofs")

        target_ids = set(element_by_id) | set(flow_by_id)
        harm_ids = set(asset_by_id) | set(invariant_by_id)
        mapped_legacy_ids: Counter[str] = Counter()
        covered_routes: set[str] = set()
        covered_route_directions: set[tuple[str, str]] = set()
        covered_route_phases: set[tuple[str, str]] = set()
        protocol_route_direction_phases: set[tuple[str, str, str]] = set()
        covered_phases: set[str] = set()
        covered_views: set[str] = set()
        covered_targets: set[str] = set()
        covered_assets: set[str] = set()
        covered_invariants: set[str] = set()
        covered_milestones: set[str] = set()
        non_build_milestones: set[str] = set()
        classes_by_view: dict[str, set[str]] = defaultdict(set)
        represented_classes: set[str] = set()

        for index, threat in enumerate(threats):
            path = f"threats[{index}]"
            old_ids = self.string_array(
                threat.get("old_ids"), f"{path}.old_ids", nonempty=False
            )
            for old_index, old_id in enumerate(old_ids):
                self.id_value(old_id, f"{path}.old_ids[{old_index}]", threat=True)
                if old_id not in legacy_ids:
                    self.error(
                        f"{path}.old_ids[{old_index}]",
                        f"undeclared legacy ID {old_id!r}",
                    )
                mapped_legacy_ids[old_id] += 1

            view = self.string(threat.get("view"), f"{path}.view")
            if view is not None:
                if view not in view_by_id:
                    self.error(f"{path}.view", f"unknown view {view!r}")
                else:
                    covered_views.add(view)

            self.string(threat.get("scenario"), f"{path}.scenario")
            scope = self.exact_object(threat.get("scope"), f"{path}.scope", SCOPE_KEYS)
            scope_profiles: list[str] = []
            scope_phases: list[str] = []
            scope_route_directions: set[tuple[str, str]] = set()
            if scope is not None:
                scope_routes = self.string_array(
                    scope.get("routes"), f"{path}.scope.routes"
                )
                scope_directions = self.string_array(
                    scope.get("directions"), f"{path}.scope.directions"
                )
                scope_phases = self.string_array(
                    scope.get("phases"), f"{path}.scope.phases"
                )
                scope_profiles = self.string_array(
                    scope.get("profiles"), f"{path}.scope.profiles"
                )
                scope_targets = self.string_array(
                    scope.get("targets"), f"{path}.scope.targets"
                )
                self.refs(
                    scope_routes, set(route_by_id), f"{path}.scope.routes", "route"
                )
                self.refs(
                    scope_phases, set(phase_by_id), f"{path}.scope.phases", "phase"
                )
                self.refs(
                    scope_profiles,
                    set(profile_by_id),
                    f"{path}.scope.profiles",
                    "profile",
                )
                self.refs(scope_targets, target_ids, f"{path}.scope.targets", "target")
                covered_routes.update(
                    item for item in scope_routes if item in route_by_id
                )
                covered_phases.update(
                    item for item in scope_phases if item in phase_by_id
                )
                covered_targets.update(
                    item for item in scope_targets if item in target_ids
                )
                if scope_directions == [ALL_SUPPORTED_DIRECTIONS]:
                    for route_id in scope_routes:
                        for direction in route_directions.get(route_id, set()):
                            scope_route_directions.add((route_id, direction))
                else:
                    if ALL_SUPPORTED_DIRECTIONS in scope_directions:
                        self.error(
                            f"{path}.scope.directions",
                            f"{ALL_SUPPORTED_DIRECTIONS!r} must be the only direction",
                        )
                    self.refs(
                        scope_directions,
                        all_directions,
                        f"{path}.scope.directions",
                        "direction",
                    )
                    for route_id in scope_routes:
                        supported = route_directions.get(route_id, set())
                        unsupported = sorted(set(scope_directions) - supported)
                        if unsupported:
                            self.error(
                                f"{path}.scope.directions",
                                f"route {route_id!r} does not support: {', '.join(unsupported)}",
                            )
                        for direction in scope_directions:
                            if direction in supported:
                                scope_route_directions.add((route_id, direction))
                covered_route_directions.update(scope_route_directions)
                for route_id in scope_routes:
                    for phase in scope_phases:
                        if phase in phase_by_id:
                            covered_route_phases.add((route_id, phase))

            classes = self.string_array(threat.get("classes"), f"{path}.classes")
            for class_index, class_name in enumerate(classes):
                if CLASS_PATTERN.fullmatch(class_name) is None:
                    self.error(
                        f"{path}.classes[{class_index}]",
                        "must use stride.*, privacy.*, or protocol.* lowercase notation",
                    )
                if class_name not in KNOWN_CLASSES:
                    self.error(
                        f"{path}.classes[{class_index}]",
                        f"unknown threat class {class_name!r}",
                    )
            represented_classes.update(
                item for item in classes if item in KNOWN_CLASSES
            )
            if view in view_by_id:
                classes_by_view[view].update(classes)
            if view != "build-evidence" and any(
                class_name in PROTOCOL_STATE_CLASSES for class_name in classes
            ):
                for route_id, direction in scope_route_directions:
                    for phase in scope_phases:
                        if phase in phase_by_id:
                            protocol_route_direction_phases.add(
                                (route_id, direction, phase)
                            )

            harms = self.string_array(threat.get("harms"), f"{path}.harms")
            self.refs(harms, harm_ids, f"{path}.harms", "asset or invariant")
            covered_assets.update(item for item in harms if item in asset_by_id)
            covered_invariants.update(item for item in harms if item in invariant_by_id)

            milestones = self.string_array(
                threat.get("milestones"), f"{path}.milestones"
            )
            for milestone_index, milestone in enumerate(milestones):
                if milestone not in MILESTONES:
                    self.error(
                        f"{path}.milestones[{milestone_index}]",
                        f"unknown milestone {milestone!r}; expected M1 through M7",
                    )
                else:
                    covered_milestones.add(milestone)
                    if view != "build-evidence":
                        non_build_milestones.add(milestone)

            risks = self.array(threat.get("risk"), f"{path}.risk")
            risk_profiles: list[str] = []
            risk_reduced = False
            if risks is not None:
                for risk_index, risk_value in enumerate(risks):
                    risk_path = f"{path}.risk[{risk_index}]"
                    risk = self.exact_object(risk_value, risk_path, RISK_KEYS)
                    if risk is None:
                        continue
                    profile = self.string(risk.get("profile"), f"{risk_path}.profile")
                    if profile is not None:
                        risk_profiles.append(profile)
                        if profile not in profile_by_id:
                            self.error(
                                f"{risk_path}.profile", f"unknown profile {profile!r}"
                            )
                    inherent = self.validate_risk_pair(
                        risk.get("inherent"), f"{risk_path}.inherent"
                    )
                    residual = self.validate_risk_pair(
                        risk.get("residual"), f"{risk_path}.residual"
                    )
                    if inherent is not None and residual is not None:
                        inherent_rating = risk_rating(*inherent)
                        residual_rating = risk_rating(*residual)
                        if RISK_ORDER[residual_rating] > RISK_ORDER[inherent_rating]:
                            self.error(
                                risk_path,
                                "residual risk rating must not exceed inherent risk rating",
                            )
                        if residual[0] > inherent[0] or residual[1] > inherent[1]:
                            self.error(
                                risk_path,
                                "each residual score must not exceed its inherent score",
                            )
                        if residual[0] < inherent[0] or residual[1] < inherent[1]:
                            risk_reduced = True
            duplicate_risk_profiles = sorted(
                item for item, count in Counter(risk_profiles).items() if count > 1
            )
            if duplicate_risk_profiles:
                self.error(
                    f"{path}.risk",
                    f"duplicates profiles: {', '.join(duplicate_risk_profiles)}",
                )
            if set(risk_profiles) != set(scope_profiles):
                missing = sorted(set(scope_profiles) - set(risk_profiles))
                extra = sorted(set(risk_profiles) - set(scope_profiles))
                details = []
                if missing:
                    details.append(f"missing {', '.join(missing)}")
                if extra:
                    details.append(f"extra {', '.join(extra)}")
                self.error(
                    f"{path}.risk",
                    "risk profiles must exactly match scope profiles"
                    + (f" ({'; '.join(details)})" if details else ""),
                )

            self.string(threat.get("control"), f"{path}.control", allow_empty=True)
            proofs = self.proofs(threat.get("proofs"), f"{path}.proofs", nonempty=False)
            remaining = self.string(
                threat.get("remaining"), f"{path}.remaining", allow_empty=True
            )
            self.string(threat.get("owner"), f"{path}.owner")
            state = self.string(threat.get("state"), f"{path}.state")
            if state is not None and state not in STATES:
                self.error(
                    f"{path}.state",
                    f"must be one of {', '.join(sorted(STATES))}",
                )
            if risk_reduced and not proofs:
                self.error(
                    f"{path}.proofs",
                    "lower residual risk requires proof",
                )
            if state == "checking" and not proofs:
                self.error(f"{path}.proofs", "state 'checking' requires evidence")
            if state in STATES and remaining is not None and not remaining.strip():
                self.error(
                    f"{path}.remaining", f"state {state!r} requires an explanation"
                )

        for legacy_id in legacy_ids:
            if mapped_legacy_ids[legacy_id] == 0:
                self.error(
                    "legacy_ids", f"legacy ID {legacy_id!r} is not mapped by any threat"
                )

        self.require_coverage("routes", set(route_by_id), covered_routes)
        self.require_coverage("phases", set(phase_by_id), covered_phases)
        self.require_coverage("views", set(view_by_id), covered_views)
        self.require_coverage("elements/flows", target_ids, covered_targets)
        self.require_coverage("assets", set(asset_by_id), covered_assets)
        for route_id, directions in route_directions.items():
            for direction in directions:
                if (route_id, direction) not in covered_route_directions:
                    self.error(
                        "threats",
                        f"route/direction {route_id!r}/{direction!r} has no threat coverage",
                    )
            for phase_id in phase_by_id:
                if (route_id, phase_id) not in covered_route_phases:
                    self.error(
                        "threats",
                        f"route/phase {route_id!r}/{phase_id!r} has no threat coverage",
                    )
                for direction in directions:
                    if (
                        route_id,
                        direction,
                        phase_id,
                    ) not in protocol_route_direction_phases:
                        self.error(
                            "threats",
                            "route/direction/phase "
                            f"{route_id!r}/{direction!r}/{phase_id!r} "
                            "has no protocol-classified threat",
                        )

        for view_id, view in view_by_id.items():
            represented = classes_by_view.get(view_id, set())
            exceptions = view.get("stride_exceptions")
            exception_classes = (
                set(exceptions) if isinstance(exceptions, dict) else set()
            )
            for stride_class in sorted(STRIDE_CLASSES):
                if (
                    stride_class not in represented
                    and stride_class not in exception_classes
                ):
                    self.error(
                        f"views[{view_id}]",
                        f"missing {stride_class} threat or justified exception",
                    )
                if stride_class in represented and stride_class in exception_classes:
                    self.error(
                        f"views[{view_id}].stride_exceptions",
                        f"{stride_class} is represented by a threat and must not also be excepted",
                    )
            if view.get("privacy_relevant") is True and not any(
                class_name in LINDDUN_CLASSES for class_name in represented
            ):
                self.error(
                    f"views[{view_id}]",
                    "privacy-relevant view has no privacy-classified threat",
                )

        self.require_coverage(
            "LINDDUN classes",
            LINDDUN_CLASSES,
            represented_classes,
        )

        for invariant_id in invariant_by_id:
            if invariant_id not in covered_invariants:
                self.error("threats", f"invariant {invariant_id!r} is not threatened")

        missing_milestones = sorted(MILESTONES - covered_milestones)
        if missing_milestones:
            self.error(
                "threats",
                f"milestones have no threat provenance: {', '.join(missing_milestones)}",
            )
        missing_non_build_milestones = sorted(MILESTONES - non_build_milestones)
        if missing_non_build_milestones:
            self.error(
                "threats",
                "milestones have only build/evidence provenance: "
                f"{', '.join(missing_non_build_milestones)}",
            )

        return self.errors

    def validate_risk_pair(self, value: Any, path: str) -> tuple[int, int] | None:
        pair = self.exact_object(value, path, RISK_PAIR_KEYS)
        if pair is None:
            return None
        scores: list[int] = []
        for key in ("likelihood", "impact"):
            score = pair.get(key)
            if type(score) is not int or not 1 <= score <= 5:
                self.error(f"{path}.{key}", "must be an integer from 1 through 5")
            else:
                scores.append(score)
        if len(scores) != 2:
            return None
        return scores[0], scores[1]

    def require_coverage(
        self, label: str, declared: set[str], covered: set[str]
    ) -> None:
        missing = sorted(declared - covered)
        if missing:
            self.error("threats", f"uncovered {label}: {', '.join(missing)}")


def risk_rating(likelihood: int, impact: int) -> str:
    return RISK_MATRIX[likelihood - 1][impact - 1]


def markdown_text(value: Any) -> str:
    text = str(value).replace("\r", " ").replace("\n", " ")
    return " ".join(text.split()).replace("|", "\\|")


def mermaid_text(value: Any) -> str:
    text = " ".join(str(value).replace("\r", " ").replace("\n", " ").split())
    return (
        text.replace("&", "&amp;")
        .replace('"', "&quot;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
    )


def joined(values: Iterable[Any]) -> str:
    return ", ".join(markdown_text(value) for value in values)


def render(model: dict[str, Any]) -> str:
    system = model["system"]
    profiles = {item["id"]: item for item in model["profiles"]}
    views = {item["id"]: item for item in model["views"]}

    lines = [
        "<!-- Generated from threat-model.json; run:",
        "python3 scripts/check-threat-model.py --write",
        "Resolve pinned implementation references when the source tree is available:",
        "python3 scripts/check-threat-model.py --check --source-repo PATH -->",
        "",
        f"# {markdown_text(system['name'])} threat model",
        "",
        "## Scope",
        "",
        markdown_text(system["scope"]),
        "",
        f"Implementation: {markdown_text(system['implementation'])}",
        "",
        f"Package: {markdown_text(system['package'])}",
        "",
        f"Method: {markdown_text(system['method'])}",
    ]
    if system["exclusions"]:
        lines.extend(["", "Excluded:"])
        for exclusion in system["exclusions"]:
            lines.append(f"- {markdown_text(exclusion)}")

    lines.extend(["", "## Routes", "", "| Route | Supported directions |", "|---|---|"])
    for route in model["routes"]:
        lines.append(
            f"| {markdown_text(route['name'])} (`{markdown_text(route['id'])}`) "
            f"| {joined(route['directions'])} |"
        )

    lines.extend(
        [
            "",
            "## Independent verification: Taker sells BTC",
            "",
            "```mermaid",
            "flowchart LR",
            '    A["Agreement fixes both locks,<br/>payouts, T, and refunds"] --> B["Taker locks the exact<br/>joint P2TR BTC output"]',
            '    B --> C{"Maker Bitcoin route:<br/>exact and canonical?"}',
            '    C -->|yes| D["Maker funds the exact<br/>LEZ escrow"]',
            '    C -->|no| X["STOP<br/>No LEZ funding"]',
            '    D --> E{"Taker LEZ route:<br/>exact and finalized?"}',
            '    E -->|yes| F["Taker claims LEZ and reveals t<br/>Maker checks tG = T and completes<br/>the fixed BTC spend to Maker"]',
            '    E -->|no| Y["STOP<br/>No claim and no t"]',
            "```",
            "",
            "The Maker checks the signed network, transaction, output key, value, "
            "unspent state, and confirmation policy through its own Bitcoin route. "
            "The Taker checks the exact program, swap, asset, amount, accounts, "
            "custody, parties, and finality through its own LEZ route.",
            "",
            "Reject means stop advancing; it cannot undo a finalized lock. Ordered "
            "refunds recover existing locks. The joint BTC key controls temporary "
            "custody, while the agreement-bound spend fixes the Maker's payout.",
        ]
    )

    lines.extend(["", "## System view", "", "```mermaid", "flowchart LR"])
    boundary_nodes = {
        boundary["id"]: f"B{index}"
        for index, boundary in enumerate(model["boundaries"])
    }
    element_nodes = {
        element["id"]: f"E{index}"
        for index, element in enumerate(
            sorted(model["elements"], key=lambda item: item["id"])
        )
    }
    for boundary in model["boundaries"]:
        lines.append(
            f"    subgraph {boundary_nodes[boundary['id']]}"
            f'["{mermaid_text(boundary["name"])}"]'
        )
        for element in (
            item for item in model["elements"] if item["boundary"] == boundary["id"]
        ):
            lines.append(
                f'        {element_nodes[element["id"]]}["{mermaid_text(element["name"])}"]'
            )
        lines.append("    end")
    for flow in sorted(model["flows"], key=lambda item: item["id"]):
        arrow = "<-->" if flow["two_way"] else "-->"
        lines.append(
            f'    {element_nodes[flow["from"]]} {arrow}|"{mermaid_text(flow["data"])}"| '
            f"{element_nodes[flow['to']]}"
        )
    lines.append("```")
    lines.extend(
        [
            "",
            "Boundary shorthand: B1-B3 show the production trust target, and B3 is "
            "repeated as separate Maker and Taker instances. Private-local evidence and "
            "the m3-plus demo may collapse parts of B1-B3; TM-0001 tracks that gap.",
        ]
    )

    lines.extend(["", "## Invariants", ""])
    for invariant in model["invariants"]:
        lines.append(
            f"- **{markdown_text(invariant['id'])}:** {markdown_text(invariant['text'])}"
        )

    production_counts: Counter[str] = Counter()
    for threat in model["threats"]:
        for assessment in threat["risk"]:
            if assessment["profile"] == "production":
                pair = assessment["residual"]
                production_counts[risk_rating(pair["likelihood"], pair["impact"])] += 1
    lines.extend(
        [
            "",
            "## Risk and release posture",
            "",
            "Likelihood and impact use a 1–5 scale. The checker computes Low, Medium, "
            "High, or Critical. Inherent risk ignores controls; current residual risk is "
            "the owner's estimate after cited controls. References provide traceability, "
            "not independent assurance.",
            "",
            "Likelihood: 1 rare, 2 unlikely, 3 plausible, 4 likely or repeatable, 5 easy "
            "or expected. Impact: 1 negligible, 2 limited, 3 material but recoverable, "
            "4 major loss or exposure, 5 irreversible principal or system-authority loss.",
            "",
            "Current production residual estimate: "
            f"{production_counts['Critical']} Critical, {production_counts['High']} High, "
            f"{production_counts['Medium']} Medium, and {production_counts['Low']} Low. "
            "Every schema-v1 row blocks a value-bearing release; verified closure or removal "
            "requires a future schema revision.",
            "",
            "States: open means a blocking control gap remains; working means control work is "
            "active; checking means implemented but not independently validated. Schema v1 "
            "has no closed or accepted state.",
            "",
            "## Threats",
            "",
            "The table shows production inherent → current residual risk. Exact phases, "
            "profiles, DFD targets, classifications, milestone provenance, scores, legacy "
            "IDs, and evidence references remain in the canonical JSON source.",
            "",
            "| ID | Area | Risk | What can go wrong | Response / owner |",
            "|---|---|---|---|---|",
        ]
    )

    for threat in sorted(model["threats"], key=lambda item: item["id"]):
        scenario = markdown_text(threat["scenario"])
        assessment_by_profile = {item["profile"]: item for item in threat["risk"]}
        selected_profile = next(
            profile_id
            for profile_id in ("production", "public-testnet", "integrated-local")
            if profile_id in assessment_by_profile
        )
        selected = assessment_by_profile[selected_profile]
        inherent = selected["inherent"]
        residual = selected["residual"]
        risk_text = (
            f"{risk_rating(inherent['likelihood'], inherent['impact'])} → "
            f"{risk_rating(residual['likelihood'], residual['impact'])}"
        )
        if selected_profile != "production":
            risk_text = (
                f"{markdown_text(profiles[selected_profile]['name'])}: {risk_text}"
            )
        protection_parts = (
            [markdown_text(threat["control"])] if threat["control"].strip() else []
        )
        if threat["remaining"].strip():
            protection_parts.append(f"Next: {markdown_text(threat['remaining'])}")
        protection_parts.append(
            f"{markdown_text(threat['owner'])} / {markdown_text(threat['state'])}"
        )
        protection = "<br>".join(protection_parts) or "—"
        lines.append(
            f"| {markdown_text(threat['id'])} "
            f"| {markdown_text(views[threat['view']]['name'])} "
            f"| {risk_text} | {scenario} | {protection} |"
        )

    return "\n".join(lines) + "\n"


def load_model() -> dict[str, Any] | None:
    try:
        with MODEL_PATH.open("r", encoding="utf-8") as handle:
            value = json.load(handle, object_pairs_hook=unique_object)
    except FileNotFoundError:
        print(
            f"missing threat model source: {MODEL_PATH.relative_to(REPOSITORY_ROOT)}",
            file=sys.stderr,
        )
        return None
    except json.JSONDecodeError as error:
        print(
            f"{MODEL_PATH.relative_to(REPOSITORY_ROOT)}:{error.lineno}:{error.colno}: "
            f"invalid JSON ({error.msg})",
            file=sys.stderr,
        )
        return None
    except DuplicateKeyError as error:
        print(
            f"{MODEL_PATH.relative_to(REPOSITORY_ROOT)}: invalid JSON ({error})",
            file=sys.stderr,
        )
        return None
    if not isinstance(value, dict):
        print("threat model root must be an object", file=sys.stderr)
        return None
    return value


def write_atomic(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    mode = path.stat().st_mode & 0o777 if path.exists() else 0o644
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", dir=path.parent
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8", newline="\n") as handle:
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
        os.chmod(temporary, mode)
        os.replace(temporary, path)
    finally:
        if temporary.exists():
            temporary.unlink()


def check_document(expected: str) -> bool:
    try:
        actual = DOCUMENT_PATH.read_text(encoding="utf-8")
    except FileNotFoundError:
        print(
            f"missing generated document: {DOCUMENT_PATH.relative_to(REPOSITORY_ROOT)}; "
            "run scripts/check-threat-model.py --write",
            file=sys.stderr,
        )
        return False
    if actual == expected:
        return True
    relative = DOCUMENT_PATH.relative_to(REPOSITORY_ROOT)
    print(f"generated threat model is stale: {relative}", file=sys.stderr)
    diff = difflib.unified_diff(
        actual.splitlines(keepends=True),
        expected.splitlines(keepends=True),
        fromfile=str(relative),
        tofile=f"{relative} (generated)",
    )
    sys.stderr.writelines(diff)
    print("run scripts/check-threat-model.py --write", file=sys.stderr)
    return False


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument(
        "--check", action="store_true", help="validate and check generated Markdown"
    )
    mode.add_argument(
        "--write", action="store_true", help="validate and rewrite generated Markdown"
    )
    parser.add_argument(
        "--source-repo",
        type=Path,
        help="also resolve pinned source@commit:path evidence in this Git repository",
    )
    arguments = parser.parse_args()

    source_repo = arguments.source_repo
    if source_repo is not None:
        source_repo = source_repo.expanduser().resolve()
        result = subprocess.run(
            ["git", "-C", str(source_repo), "rev-parse", "--is-inside-work-tree"],
            check=False,
            capture_output=True,
            text=True,
        )
        if result.returncode != 0 or result.stdout.strip() != "true":
            print(f"not a Git worktree: {source_repo}", file=sys.stderr)
            return 1

    model = load_model()
    if model is None:
        return 1
    validator = ModelValidator(model, source_repo=source_repo)
    errors = validator.validate()
    if errors:
        for error in errors:
            print(f"threat-model: {error}", file=sys.stderr)
        print(f"threat-model: {len(errors)} validation error(s)", file=sys.stderr)
        return 1

    rendered = render(model)
    if arguments.write:
        write_atomic(DOCUMENT_PATH, rendered)
        print(f"wrote {DOCUMENT_PATH.relative_to(REPOSITORY_ROOT)}")
    elif not check_document(rendered):
        return 1
    if source_repo is None and validator.external_proofs:
        print(
            "threat model consistency passed; "
            f"{len(validator.external_proofs)} pinned source proofs were not resolved "
            "(use --source-repo PATH)"
        )
    else:
        print(
            "threat model consistency passed; "
            f"{len(validator.external_proofs)} pinned source proofs resolved"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
