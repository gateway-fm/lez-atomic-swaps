#!/usr/bin/env python3
"""Characterization tests for the frozen controller/launcher v1 boundary."""

import base64
import importlib.util
import os
import pathlib
import unittest
from unittest import mock


os.environ.setdefault("LEZ_M3_RUNNER_REPO_IN_CONTAINER", "/runner/repo")
MODULE_PATH = pathlib.Path(__file__).with_name("launcher.py")
SPEC = importlib.util.spec_from_file_location("btc_demo_launcher", MODULE_PATH)
launcher = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(launcher)


class LauncherContractTests(unittest.TestCase):
    def job(self) -> dict:
        run_id = "m5arm-0830123456"
        payload = base64.b64encode(b"#!/bin/bash\n").decode()
        return {
            "schema_version": 1,
            "operation": "run_swap",
            "run_id": run_id,
            "direction": "taker_sells_foreign",
            "offer_id": "offer-example-btc",
            "reservation_id": "ui-reserve-example",
            "maker_wallet_id": "maker-munich-01",
            "taker_wallet_id": "taker-zurich-01",
            "files": {
                f"lez-run-full-btc-ui-{run_id}.sh": payload,
                f"lez-interactive-m3-outer-{run_id}.sh": payload,
                f"lez-interactive-m3-direction-{run_id}.sh": payload,
                "lez-export-btc-ui-evidence.sh": payload,
            },
        }

    @mock.patch.object(launcher, "create_exec", return_value="a" * 64)
    @mock.patch.object(launcher, "upload_bundle")
    def test_run_swap_job_has_one_exact_command_and_allowlisted_environment(
            self, upload: mock.Mock, create: mock.Mock) -> None:
        result = launcher.dispatch(self.job())
        self.assertEqual(result, {
            "kind": "RunSwapResultV1",
            "run_id": "m5arm-0830123456",
            "exec_id": "a" * 64,
        })
        upload.assert_called_once()
        command, environment = create.call_args.args
        self.assertEqual(command, ["bash", "/tmp/lez-run-full-btc-ui-m5arm-0830123456.sh"])
        self.assertEqual(len(environment), 10)
        self.assertFalse(any("DOCKER" in value for value in environment))

    def test_unknown_fields_operations_and_directions_fail_closed(self) -> None:
        invalid = self.job()
        invalid["command"] = ["sh", "-c", "arbitrary"]
        with self.assertRaises(ValueError):
            launcher.dispatch(invalid)
        with self.assertRaises(ValueError):
            launcher.dispatch({"schema_version": 1, "operation": "docker_exec"})
        invalid = self.job()
        invalid["direction"] = "maker_supplied_shell"
        with self.assertRaises(ValueError):
            launcher.dispatch(invalid)

    @mock.patch.object(launcher, "upload_bundle")
    def test_action_approval_path_is_derived_not_supplied(self, upload: mock.Mock) -> None:
        result = launcher.dispatch({
            "schema_version": 1,
            "operation": "approve_action",
            "run_id": "m5arm-0830123456",
            "direction": "taker_sells_foreign",
            "role": "taker",
            "action": "lock_btc",
            "expected_revision": 0,
            "approved_at": "2026-08-30T12:00:00Z",
        })
        self.assertTrue(result["approved"])
        self.assertEqual(
            upload.call_args.args[1],
            "/runner/repo/.e2e/m5arm-0830123456/m3-actor-poc/private/"
            "directions/taker_sells_foreign/interactive-gates",
        )


if __name__ == "__main__":
    unittest.main()
