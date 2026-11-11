#!/usr/bin/env python3
"""Preflight, transaction lookups and the evidence bundle of the public Zcash
Testnet lane (scripts/public-zec-testnet.sh, docs/zcash-testnet-setup.md).

    zec-public-evidence.py preflight --lane-env ENV --route self-hosted|provider --out FILE
    zec-public-evidence.py lookup    --lane-env ENV --route ROUTE --summary FILE --out FILE
    zec-public-evidence.py lez-tip   --lane-env ENV --out FILE
    zec-public-evidence.py corridor  --route R --scenario S --direction D --exit-code N --started T --finished T
                                     [--summary F] [--lookup F] [--cross-check F] [--reason TEXT] --out FILE
    zec-public-evidence.py bundle    --lane-env ENV --run-dir DIR --out DIR [--secret-file F]…
    zec-public-evidence.py validate  DIR [--secret-file F]…

Every call is read-only: this tool never broadcasts, never mines and never
reads a key. A route's credential is read from its file into one request
header and nowhere else; `bundle` and `validate` refuse a bundle that holds
the contents of any --secret-file, or a field whose name says it is a secret.

Exit codes: 0 ok, 1 invalid bundle or failed lookup, 3 blocked (the route or
the swap engine cannot serve this Testnet; the bundle says why).
"""
from __future__ import annotations

import argparse
import base64
import datetime
import hashlib
import json
import pathlib
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

ROUTES = ("self-hosted", "provider")
SCENARIOS = {"zec-happy": "happy", "zec-maker-refund": "refund", "zec-taker-refund": "refund"}
DIRECTIONS = ("TakerSellsLez", "TakerSellsForeign")
# preflight: only the routes were checked (no corridor ran) and none was blocked.
STATUSES = ("passed", "failed", "blocked", "preflight")
# A JSON key naming any of these is a secret, whatever its value.
SECRET_KEY = re.compile(r"(?i)(api[_-]?key|cookie|password|passphrase|secret|private|wif|mnemonic|seed|token)")
# Keys that are allowed although they match: they name a thing, never hold it.
SECRET_KEY_ALLOWED = {"credential", "secret_files_checked"}
HEX64 = re.compile(r"^[0-9a-f]{64}$")
EXIT_BLOCKED = 3


class LaneError(Exception):
    """A request, a lookup or a bundle check failed."""


def utc_now() -> str:
    return datetime.datetime.now(datetime.UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


def read_lane_env(path: pathlib.Path) -> dict[str, str]:
    """KEY=VALUE lines of the lane profile; comments and blanks ignored."""
    values = {}
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, value = line.partition("=")
        values[key.strip()] = value.strip()
    return values


def origin(url: str) -> str:
    """The scheme, host and port of an endpoint: what the bundle may record."""
    parts = urllib.parse.urlsplit(url)
    if parts.username or parts.password or parts.query or parts.fragment:
        raise LaneError("an endpoint must not carry credentials, a query or a fragment")
    return f"{parts.scheme}://{parts.hostname}{':' + str(parts.port) if parts.port else ''}/"


class Route:
    """One read-only JSON-RPC route, paced to its provider's limit."""

    def __init__(self, name: str, url: str, kind: str, credential_file: str | None = None,
                 min_interval: float = 0.0, opener=urllib.request.urlopen, sleep=time.sleep, clock=time.monotonic):
        if name not in ROUTES:
            raise LaneError(f"unknown route {name}")
        self.name, self.url, self.kind = name, url, kind
        self.credential_file = credential_file
        self.min_interval = min_interval
        self.opener, self.sleep, self.clock = opener, sleep, clock
        self.last = None
        self.provider_errors: list[dict] = []

    def headers(self) -> dict[str, str]:
        headers = {"content-type": "application/json"}
        if not self.credential_file:
            return headers
        material = pathlib.Path(self.credential_file).read_text().strip()
        if self.kind == "tatum_testnet_x_api_key":
            headers["x-api-key"] = material
        else:
            # Zebra's .cookie is `__cookie__:<password>`, sent as HTTP basic auth.
            headers["authorization"] = "Basic " + base64.b64encode(material.encode()).decode()
        return headers

    def call(self, method: str, params: list | None = None, attempts: int = 4) -> object:
        if method in {"sendrawtransaction", "generate", "submitblock"}:
            raise LaneError(f"{method} is not a read; this tool never sends it")
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params or []}).encode()
        for attempt in range(attempts):
            if self.last is not None and self.min_interval:
                wait = self.min_interval - (self.clock() - self.last)
                if wait > 0:
                    self.sleep(wait)
            self.last = self.clock()
            request = urllib.request.Request(self.url, data=body, headers=self.headers())
            try:
                with self.opener(request, timeout=60) as response:
                    reply = json.loads(response.read())
            except urllib.error.HTTPError as error:
                retry_after = error.headers.get("retry-after") if error.headers else None
                self.provider_errors.append({"at": utc_now(), "method": method, "http_status": error.code,
                                             "retry_after": retry_after})
                if error.code == 429 and attempt + 1 < attempts:
                    self.sleep(float(retry_after or 60))
                    continue
                raise LaneError(f"{self.name} {method}: HTTP {error.code}") from error
            except OSError as error:
                self.provider_errors.append({"at": utc_now(), "method": method, "error": type(error).__name__})
                raise LaneError(f"{self.name} {method} unreachable: {type(error).__name__}") from error
            if reply.get("error"):
                raise LaneError(f"{self.name} {method}: {json.dumps(reply['error'])[:200]}")
            return reply.get("result")
        raise LaneError(f"{self.name} {method}: still rate limited after {attempts} attempts")


def route_from_env(env: dict[str, str], name: str, **kwargs) -> Route:
    if name == "self-hosted":
        url = env["LEZ_ZEC_SELF_HOSTED_URL"]
        if urllib.parse.urlsplit(url).hostname not in {"127.0.0.1", "::1", "localhost"}:
            raise LaneError("the self-hosted route must be a loopback endpoint")
        return Route(name, url, "self_hosted_cookie", env.get("LEZ_ZEC_SELF_HOSTED_COOKIE_FILE") or None,
                     float(env.get("LEZ_ZEC_SELF_HOSTED_MIN_INTERVAL_SECONDS", "0")), **kwargs)
    url = env["LEZ_ZEC_PROVIDER_URL"]
    if urllib.parse.urlsplit(url).scheme != "https":
        raise LaneError("the provider route must be HTTPS")
    return Route(name, url, "tatum_testnet_x_api_key", env.get("LEZ_ZEC_PROVIDER_API_KEY_FILE") or None,
                 float(env.get("LEZ_ZEC_PROVIDER_MIN_INTERVAL_SECONDS", "13")), **kwargs)


def preflight(env: dict[str, str], route: Route) -> dict:
    """The route's chain identity and tip, and whether the swap engine can sign
    for it: `supported` is false, with `blockers`, on any failed check."""
    info = route.call("getblockchaininfo")
    network = route.call("getnetworkinfo")
    genesis = route.call("getblockhash", [0])
    consensus = info.get("consensus") or {}
    upgrades = {branch: {"name": u.get("name"), "activation_height": u.get("activationheight"), "status": u.get("status")}
                for branch, u in (info.get("upgrades") or {}).items()}
    blocks, estimated = int(info.get("blocks", -1)), int(info.get("estimatedheight", -1))
    wanted_branch = env["LEZ_ZEC_SWAP_PROFILE_BRANCH"]
    checks = [
        {"name": "chain", "ok": info.get("chain") == env["LEZ_ZEC_TESTNET_CHAIN"],
         "detail": f"chain {info.get('chain')!r}, want {env['LEZ_ZEC_TESTNET_CHAIN']!r}"},
        {"name": "genesis", "ok": genesis == env["LEZ_ZEC_TESTNET_GENESIS"], "detail": f"genesis {genesis}"},
        {"name": "synchronized", "ok": blocks >= 0 and estimated - blocks <= int(env["LEZ_ZEC_MAX_TIP_LAG_BLOCKS"]),
         "detail": f"blocks {blocks}, estimated {estimated}, verification {info.get('verificationprogress')}"},
        {"name": "consensus_branch", "ok": consensus.get("nextblock") == wanted_branch,
         "detail": (f"next block on branch {consensus.get('nextblock')} ({(upgrades.get(consensus.get('nextblock')) or {}).get('name')}),"
                    f" the agreement profile {env['LEZ_ZEC_SWAP_PROFILE']} signs for {wanted_branch}"
                    f" ({(upgrades.get(wanted_branch) or {}).get('name')})")},
    ]
    return {
        "route": route.name, "route_kind": route.kind, "endpoint": origin(route.url),
        "credential": "file" if route.credential_file else "none",
        "observed_at": utc_now(),
        "zebra": {"subversion": network.get("subversion"), "version": network.get("version"),
                  "connections": network.get("connections")},
        "chain": info.get("chain"), "genesis_hash": genesis,
        "tip": {"height": blocks, "hash": info.get("bestblockhash"), "estimated_height": estimated,
                "verification_progress": info.get("verificationprogress")},
        "consensus": {"chain_tip": consensus.get("chaintip"), "next_block": consensus.get("nextblock")},
        "upgrades": upgrades,
        "checks": checks,
        "supported": all(check["ok"] for check in checks),
        "blockers": [check["detail"] for check in checks if not check["ok"]],
        "provider_errors": route.provider_errors,
    }


def zcash_effects(summary: dict) -> list[dict]:
    """The Zcash transactions a node-e2e.py ZEC summary names
    (`zcash_funding`, `zcash_claim`, `zcash_refund`, …: {transaction_id, height})."""
    effects = []
    for key, value in sorted(summary.items()):
        if key.startswith("zcash_") and isinstance(value, dict) and HEX64.match(str(value.get("transaction_id", ""))):
            effects.append({"kind": key.removeprefix("zcash_"), "transaction_id": value["transaction_id"],
                            "reported_height": value.get("height")})
    return effects


def lookup(route: Route, effects: list[dict]) -> list[dict]:
    """Each transaction as the route sees it now: height, block, confirmations, version."""
    looked = []
    for effect in effects:
        raw = route.call("getrawtransaction", [effect["transaction_id"], 1])
        looked.append({**effect, "route": route.name, "observed_at": utc_now(),
                       "height": raw.get("height"), "block_hash": raw.get("blockhash"),
                       "confirmations": raw.get("confirmations"), "version": raw.get("version"),
                       "version_group_id": raw.get("versiongroupid")})
    return looked


def corridor_record(route: str, scenario: str, direction: str, exit_code: int, started: str, finished: str,
                    summary: dict | None, looked: list[dict] | None, cross: list[dict] | None, reason: str = "") -> dict:
    """One corridor's record: passed only when the harness passed and every
    Zcash transaction it names was found again on the lane's route."""
    effects = looked or []
    found = bool(effects) and all(isinstance(e.get("height"), int) for e in effects)
    if exit_code == 0 and found:
        status = "passed"
    elif exit_code == EXIT_BLOCKED:
        status = "blocked"
    else:
        status = "failed"
        if exit_code == 0 and not reason:
            reason = "the harness passed, but its Zcash transactions were not all found on the route"
    return {"route": route, "scenario": scenario, "kind": SCENARIOS[scenario], "direction": direction,
            "status": status, "exit_code": exit_code, "reason": reason, "started_at": started, "finished_at": finished,
            "swap_id": (summary or {}).get("swap_id"), "zcash_refund_height": (summary or {}).get("zcash_refund_height"),
            "zcash_effects": effects, "cross_checks": cross or []}


def lez_tip(env: dict[str, str], opener=urllib.request.urlopen) -> dict:
    """The official LEZ testnet sequencer's last block id, and the channel the lane names."""
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "getLastBlockId", "params": {}}).encode()
    request = urllib.request.Request(env["LEZ_LEZ_SEQUENCER_URL"], data=body, headers={"content-type": "application/json"})
    with opener(request, timeout=60) as response:
        reply = json.loads(response.read())
    return {"sequencer": origin(env["LEZ_LEZ_SEQUENCER_URL"]), "channel_id": env["LEZ_V02_CHANNEL_ID"],
            "last_block_id": reply.get("result"), "observed_at": utc_now()}


def read_json(path: pathlib.Path) -> object:
    return json.loads(path.read_text())


def assemble(env: dict[str, str], run_dir: pathlib.Path) -> dict:
    """The bundle's evidence.json from the runner's run directory."""
    provenance = read_json(run_dir / "provenance.json")
    preflights = [read_json(p) for p in sorted(run_dir.glob("preflight-*.json"))]
    corridors = [read_json(p) for p in sorted((run_dir / "corridors").glob("*.json"))] if (run_dir / "corridors").is_dir() else []
    interventions = []
    log = run_dir / "interventions.jsonl"
    if log.is_file():
        interventions = [json.loads(line) for line in log.read_text().splitlines() if line.strip()]
    lez = {name: read_json(run_dir / f"lez-{name}.json") for name in ("start", "end") if (run_dir / f"lez-{name}.json").is_file()}
    coverage = coverage_of(corridors)
    blocked = [p for p in preflights if not p.get("supported")]
    if corridors and all(c.get("status") == "passed" for c in corridors):
        status = "passed"
    elif blocked or any(c.get("status") == "blocked" for c in corridors):
        status = "blocked"
    elif not corridors and preflights:
        status = "preflight"
    else:
        status = "failed"
    # A route proves itself Zcash Testnet by its chain and genesis; a route
    # that does not (a Regtest node on the same port, say) is recorded and
    # refused, and proves nothing.
    public = any(p.get("chain") == env["LEZ_ZEC_TESTNET_CHAIN"]
                 and p.get("genesis_hash") == env["LEZ_ZEC_TESTNET_GENESIS"] for p in preflights)
    has_swap_tx = any(effect.get("confirmations") for c in corridors for effect in c.get("zcash_effects", []))
    return {
        "schema_version": 1,
        "lane_profile": env["LEZ_ZEC_LANE_PROFILE"],
        "swap_profile": {"id": env["LEZ_ZEC_SWAP_PROFILE"], "consensus_branch": env["LEZ_ZEC_SWAP_PROFILE_BRANCH"],
                         "zcash_confirmations": int(env["LEZ_ZEC_SWAP_PROFILE_CONFIRMATIONS"]),
                         "zcash_refund_blocks": int(env["LEZ_ZEC_SWAP_PROFILE_REFUND_BLOCKS"])},
        "status": status,
        # "public-chain-swap" only when a swap transaction was found on a route
        # that proved itself Zcash Testnet; a preflight alone never is swap evidence.
        "evidence_kind": "public-chain-swap" if public and has_swap_tx else ("public-chain-preflight" if public else "none"),
        "provenance": provenance,
        "zcash_routes": preflights,
        "lez": lez,
        "corridors": corridors,
        "coverage": coverage,
        "interventions": interventions,
        "shielding": "not part of atomicity: received transparent TAZ is shielded, if at all, by a separate wallet action",
        "written_at": utc_now(),
    }


def coverage_of(corridors: list[dict]) -> dict:
    passed = [c for c in corridors if c.get("status") == "passed"]
    by_route = {route: {c["kind"] for c in passed if c.get("route") == route} for route in ROUTES}
    coverage = {
        "happy": any(c["kind"] == "happy" for c in passed),
        "refund": any(c["kind"] == "refund" for c in passed),
        "self_hosted_complete_corridor": bool(by_route["self-hosted"]),
        "provider_complete_corridor": bool(by_route["provider"]),
        "directions": sorted({c["direction"] for c in passed}),
    }
    coverage["acceptance_complete"] = all(coverage[k] for k in ("happy", "refund", "self_hosted_complete_corridor",
                                                                 "provider_complete_corridor"))
    return coverage


def secret_strings(secret_files: list[str]) -> list[str]:
    values = []
    for path in secret_files:
        try:
            text = pathlib.Path(path).read_text().strip()
        except OSError:
            continue
        values.append(text)
        # A cookie's password half, a key file's lines.
        values.extend(part for part in re.split(r"[\s:]+", text) if len(part) >= 12)
    return [v for v in values if len(v) >= 12]


def walk(value: object, path: str = ""):
    if isinstance(value, dict):
        for key, inner in value.items():
            yield f"{path}.{key}", key, inner
            yield from walk(inner, f"{path}.{key}")
    elif isinstance(value, list):
        for index, inner in enumerate(value):
            yield from walk(inner, f"{path}[{index}]")


def validate(evidence: dict, files: dict[str, str] | None = None, secret_files: list[str] | None = None) -> list[str]:
    """Every problem with a bundle; empty means it is valid."""
    problems = []
    for field in ("schema_version", "lane_profile", "swap_profile", "status", "evidence_kind", "provenance",
                  "zcash_routes", "lez", "corridors", "coverage", "interventions", "shielding"):
        if field not in evidence:
            problems.append(f"missing field {field}")
    if problems:
        return problems
    if evidence["status"] not in STATUSES:
        problems.append(f"status {evidence['status']!r} is not one of {STATUSES}")
    provenance = evidence["provenance"]
    if not re.fullmatch(r"[0-9a-f]{40}", str(provenance.get("source_sha", ""))):
        problems.append("provenance.source_sha is not a full commit id")
    if not isinstance(provenance.get("artifacts"), list):
        problems.append("provenance.artifacts must list the artifacts and their digests")
    else:
        for artifact in provenance["artifacts"]:
            if not re.fullmatch(r"sha256:[0-9a-f]{64}", str(artifact.get("digest", ""))):
                problems.append(f"artifact {artifact.get('name')!r} has no sha256 digest")
    if not isinstance(evidence["interventions"], list):
        problems.append("interventions must be a list (empty when there were none)")
    for route in evidence["zcash_routes"]:
        for field in ("route", "route_kind", "endpoint", "chain", "genesis_hash", "tip", "consensus", "zebra", "checks"):
            if field not in route:
                problems.append(f"route {route.get('route')!r} lacks {field}")
        if urllib.parse.urlsplit(str(route.get("endpoint", ""))).query:
            problems.append(f"route {route.get('route')!r} records an endpoint with a query")
    confirmations = int(evidence["swap_profile"].get("zcash_confirmations", 1))
    for corridor in evidence["corridors"]:
        name = f"{corridor.get('route')}/{corridor.get('scenario')}/{corridor.get('direction')}"
        for field in ("route", "scenario", "kind", "direction", "status", "started_at", "finished_at", "zcash_effects"):
            if field not in corridor:
                problems.append(f"corridor {name} lacks {field}")
        if corridor.get("route") not in ROUTES:
            problems.append(f"corridor {name}: unknown route")
        if corridor.get("direction") not in DIRECTIONS:
            problems.append(f"corridor {name}: unknown direction")
        if corridor.get("status") == "passed":
            effects = corridor.get("zcash_effects") or []
            kinds = {e.get("kind") for e in effects}
            wanted = {"funding", "claim"} if corridor.get("kind") == "happy" else {"funding", "refund"}
            if not wanted <= kinds:
                problems.append(f"corridor {name} passed without its {sorted(wanted - kinds)} transaction(s)")
            for effect in effects:
                if not HEX64.match(str(effect.get("transaction_id", ""))):
                    problems.append(f"corridor {name}: {effect.get('kind')} has no transaction id")
                if not isinstance(effect.get("height"), int) or (effect.get("confirmations") or 0) < confirmations:
                    problems.append(f"corridor {name}: {effect.get('kind')} {str(effect.get('transaction_id'))[:12]} "
                                    f"has {effect.get('confirmations')} confirmation(s), the profile wants {confirmations}")
    if evidence["evidence_kind"] == "public-chain-swap" and not any(
            c.get("status") == "passed" for c in evidence["corridors"]):
        problems.append("evidence_kind public-chain-swap without a passed corridor")
    if evidence["status"] == "passed" and not evidence["corridors"]:
        problems.append("status passed without a corridor")
    if evidence["status"] == "passed" and not provenance.get("artifacts"):
        problems.append("status passed without the digests of the artifacts that ran")
    for path, key, value in walk(evidence):
        if SECRET_KEY.search(key) and key not in SECRET_KEY_ALLOWED:
            problems.append(f"{path}: a field named like a secret")
    texts = {"evidence.json": json.dumps(evidence), **(files or {})}
    for secret in secret_strings(secret_files or []):
        for name, text in texts.items():
            if secret in text:
                problems.append(f"{name} contains the contents of a secret file")
    return problems


def summary_markdown(evidence: dict) -> str:
    """The human summary: everything in evidence.json a reviewer reads first."""
    lines = [f"# Zcash public Testnet lane — {evidence['status'].upper()}", ""]
    if evidence["evidence_kind"] != "public-chain-swap":
        lines += ["**No swap transaction in this bundle.** It records what the routes reported"
                  " and why no corridor ran; it is not public-chain swap evidence.", ""]
    p = evidence["provenance"]
    lines += [f"- source `{p.get('source_sha')}`{' (dirty tree)' if p.get('dirty') else ''}, workflow run {p.get('workflow_run') or 'none (manual)'}",
              f"- lane profile `{evidence['lane_profile']}`, agreement profile `{evidence['swap_profile']['id']}`"
              f" (branch {evidence['swap_profile']['consensus_branch']}, {evidence['swap_profile']['zcash_confirmations']} confirmations,"
              f" refund {evidence['swap_profile']['zcash_refund_blocks']} blocks)",
              f"- evidence kind `{evidence['evidence_kind']}`; written {evidence.get('written_at')}", ""]
    if p.get("artifacts"):
        lines += ["| artifact | digest |", "|---|---|"]
        lines += [f"| {a.get('name')} | `{a.get('digest')}` |" for a in p["artifacts"]]
        lines.append("")
    lines += ["## Zcash routes", "", "| route | kind | endpoint | Zebra | chain | tip | next-block branch | supported |",
              "|---|---|---|---|---|---|---|---|"]
    for r in evidence["zcash_routes"]:
        lines.append(f"| {r['route']} | {r['route_kind']} | {r['endpoint']} | {r['zebra'].get('subversion')} | {r['chain']} |"
                     f" {r['tip'].get('height')} | {r['consensus'].get('next_block')} | {'yes' if r.get('supported') else 'no'} |")
    lines.append("")
    lines += [f"- **{r['route']} blocked:** {blocker}" for r in evidence["zcash_routes"] for blocker in r.get("blockers", [])]
    lines.append("")
    if evidence["lez"]:
        lines += ["## LEZ testnet", ""]
        for when, tip in evidence["lez"].items():
            lines.append(f"- {when}: sequencer {tip.get('sequencer')} channel `{tip.get('channel_id')}` last block {tip.get('last_block_id')} at {tip.get('observed_at')}")
        lines.append("")
    lines += ["## Corridors", ""]
    if not evidence["corridors"]:
        lines += ["None ran.", ""]
    for c in evidence["corridors"]:
        lines.append(f"### {c['route']} · {c['scenario']} · {c['direction']} — {c['status']}")
        if c.get("reason"):
            lines.append(f"\n{c['reason']}")
        if c.get("swap_id"):
            lines.append(f"\nswap `{c['swap_id']}`, {c['started_at']} → {c['finished_at']}")
        if c.get("zcash_effects"):
            lines += ["", "| Zcash tx | kind | height | confirmations | seen by |", "|---|---|---|---|---|"]
            for e in c["zcash_effects"]:
                lines.append(f"| `{e['transaction_id']}` | {e['kind']} | {e.get('height')} | {e.get('confirmations')} | {e.get('route')} |")
            for e in c.get("cross_checks", []):
                lines.append(f"| `{e['transaction_id']}` | {e['kind']} (cross-check) | {e.get('height')} | {e.get('confirmations')} | {e.get('route')} |")
        if c.get("lez_effects"):
            lines += ["", "| LEZ tx | kind | reported by |", "|---|---|---|"]
            lines += [f"| `{e['transaction_id']}` | {e['kind']} | {e.get('reported_by')} |" for e in c["lez_effects"]]
        lines.append("")
    cov = evidence["coverage"]
    lines += ["## Coverage", "", f"- happy: {cov['happy']}, refund: {cov['refund']}, directions: {', '.join(cov['directions']) or 'none'}",
              f"- complete corridor on the self-hosted route: {cov['self_hosted_complete_corridor']}, on the provider route: {cov['provider_complete_corridor']}",
              f"- acceptance complete: **{cov['acceptance_complete']}**", "", "## Interventions", ""]
    lines += [f"- {i.get('at')}: {i.get('what')}" for i in evidence["interventions"]] or ["None recorded."]
    lines += ["", f"Shielding: {evidence['shielding']}.", ""]
    return "\n".join(lines)


def write_bundle(evidence: dict, out: pathlib.Path, secret_files: list[str]) -> list[str]:
    out.mkdir(parents=True, exist_ok=True)
    files = {"evidence.json": json.dumps(evidence, indent=2, sort_keys=True) + "\n", "SUMMARY.md": summary_markdown(evidence)}
    problems = validate(evidence, files, secret_files)
    if any("secret" in p for p in problems):
        return problems  # nothing is written that could hold a secret
    for name, text in files.items():
        (out / name).write_text(text)
    sums = "".join(f"{hashlib.sha256((out / name).read_bytes()).hexdigest()}  {name}\n" for name in sorted(files))
    (out / "SHA256SUMS").write_text(sums)
    return problems


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("preflight", "lookup", "lez-tip", "bundle"):
        command = sub.add_parser(name)
        command.add_argument("--lane-env", required=True, type=pathlib.Path)
        if name in {"preflight", "lookup"}:
            command.add_argument("--route", required=True, choices=ROUTES)
        if name == "lookup":
            command.add_argument("--summary", required=True, type=pathlib.Path)
        if name == "bundle":
            command.add_argument("--run-dir", required=True, type=pathlib.Path)
            command.add_argument("--secret-file", action="append", default=[])
        command.add_argument("--out", required=True, type=pathlib.Path)
    record = sub.add_parser("corridor")
    record.add_argument("--route", required=True, choices=ROUTES)
    record.add_argument("--scenario", required=True, choices=sorted(SCENARIOS))
    record.add_argument("--direction", required=True, choices=DIRECTIONS)
    record.add_argument("--exit-code", required=True, type=int)
    record.add_argument("--started", required=True)
    record.add_argument("--finished", required=True)
    record.add_argument("--summary", type=pathlib.Path)
    record.add_argument("--lookup", type=pathlib.Path)
    record.add_argument("--cross-check", type=pathlib.Path)
    record.add_argument("--reason", default="")
    record.add_argument("--out", required=True, type=pathlib.Path)
    check = sub.add_parser("validate")
    check.add_argument("bundle", type=pathlib.Path)
    check.add_argument("--secret-file", action="append", default=[])
    args = parser.parse_args(argv)
    try:
        if args.command == "validate":
            evidence = read_json(args.bundle / "evidence.json")
            files = {p.name: p.read_text() for p in args.bundle.iterdir() if p.is_file()}
            problems = validate(evidence, files, args.secret_file)
            for problem in problems:
                print(f"invalid: {problem}", file=sys.stderr)
            print(f"{args.bundle}: {'valid' if not problems else 'INVALID'} ({evidence.get('status')}, {evidence.get('evidence_kind')})")
            return 1 if problems else 0
        if args.command == "corridor":
            def optional(path):
                return read_json(path) if path and path.is_file() else None
            result = corridor_record(args.route, args.scenario, args.direction, args.exit_code, args.started,
                                     args.finished, optional(args.summary), optional(args.lookup),
                                     optional(args.cross_check), args.reason)
            args.out.parent.mkdir(parents=True, exist_ok=True)
            args.out.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
            print(f"  corridor {args.route}/{args.scenario}/{args.direction}: {result['status']}")
            return 0
        env = read_lane_env(args.lane_env)
        if args.command == "preflight":
            result = preflight(env, route_from_env(env, args.route))
            args.out.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
            for check_ in result["checks"]:
                print(f"  {args.route}: {'ok     ' if check_['ok'] else 'BLOCKED'} {check_['name']}: {check_['detail']}")
            return 0 if result["supported"] else EXIT_BLOCKED
        if args.command == "lookup":
            effects = zcash_effects(read_json(args.summary))
            result = lookup(route_from_env(env, args.route), effects)
            args.out.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
            return 0
        if args.command == "lez-tip":
            args.out.write_text(json.dumps(lez_tip(env), indent=2, sort_keys=True) + "\n")
            return 0
        evidence = assemble(env, args.run_dir)
        problems = write_bundle(evidence, args.out, args.secret_file)
        for problem in problems:
            print(f"invalid: {problem}", file=sys.stderr)
        print(f"bundle {args.out}: {evidence['status']}, {evidence['evidence_kind']}, "
              f"acceptance complete {evidence['coverage']['acceptance_complete']}")
        if problems:
            return 1
        return {"passed": 0, "preflight": 0, "blocked": EXIT_BLOCKED}.get(evidence["status"], 1)
    except (LaneError, OSError, KeyError, ValueError) as error:
        print(f"zec-public-evidence {args.command}: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
