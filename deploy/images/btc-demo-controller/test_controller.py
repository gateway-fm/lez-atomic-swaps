"""Direction-symmetry tests for the local BTC/LEZ application controller."""

from __future__ import annotations

import importlib.util
import os
import pathlib
import tempfile
import unittest


TEST_ROOT = tempfile.TemporaryDirectory(prefix="lez-controller-test-")
os.environ["LEZ_BTC_DEMO_SOCKET"] = str(pathlib.Path(TEST_ROOT.name) / "controller.sock")
os.environ["LEZ_BTC_LAUNCHER_SOCKET"] = str(pathlib.Path(TEST_ROOT.name) / "launcher.sock")
os.environ["LEZ_M3_EVIDENCE_ROOT"] = TEST_ROOT.name
os.environ["LEZ_M3_BTC_EVIDENCE_FILE"] = str(pathlib.Path(TEST_ROOT.name) / "evidence.json")

MODULE_PATH = pathlib.Path(__file__).with_name("controller.py")
SPEC = importlib.util.spec_from_file_location("lez_btc_demo_controller", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
controller = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(controller)


class DirectionSymmetryTests(unittest.TestCase):
    def test_both_happy_flows_have_four_role_owned_gates(self) -> None:
        expected = {
            "taker_sells_foreign": {
                "display": "BTC → LEZ",
                "ui_direction": "TakerSellsForeign",
                "ordered": ("lock_btc", "fund_lez", "claim_lez", "claim_btc"),
                "roles": ("taker", "maker", "taker", "maker"),
            },
            "taker_sells_lez": {
                "display": "LEZ → BTC",
                "ui_direction": "TakerSellsLez",
                "ordered": ("lock_lez", "lock_btc", "claim_btc", "claim_lez"),
                "roles": ("taker", "maker", "taker", "maker"),
            },
        }
        self.assertEqual(set(controller.DIRECTIONS), set(expected))
        for name, wanted in expected.items():
            direction = controller.DIRECTIONS[name]
            self.assertEqual(direction["display"], wanted["display"])
            self.assertEqual(direction["ui_direction"], wanted["ui_direction"])
            self.assertEqual(direction["ordered"], wanted["ordered"])
            self.assertEqual(
                tuple(direction["actions"][action]["role"] for action in direction["ordered"]),
                wanted["roles"],
            )

    def test_both_directions_round_trip_through_the_wallet_market(self) -> None:
        maker = {
            "schema_version": 2,
            "role": "maker",
            "wallet_id": "maker-munich-01",
            "count": 1,
            "bitcoin_sats": controller.FIXED_BITCOIN_SATS,
            "lez_units": controller.FIXED_LEZ_UNITS,
        }
        taker = {"schema_version": 2, "role": "taker", "wallet_id": "taker-zurich-01"}
        for tag, name in zip(("forward", "reverse"), controller.DIRECTIONS, strict=True):
            create = dict(
                maker,
                direction=name,
                request_id=f"ui-maker-direction-{tag}-1700000000000",
            )
            inventory = controller.MARKET.create_offers(create)["inventory"]
            matching = [
                offer for offer in inventory
                if offer["state"] == "pending" and offer["direction"] == name
            ]
            self.assertTrue(matching)
            offer = max(matching, key=lambda value: value["created_at"])
            order = next(
                row for row in controller.MARKET.snapshot(taker)["order_book"]
                if row["offer_id"] == offer["offer_id"]
            )
            if name == "taker_sells_foreign":
                self.assertEqual(
                    (order["taker_pays_display"], order["taker_receives_display"]),
                    ("0.01000000 BTC", "1,000 LEZ"),
                )
            else:
                self.assertEqual(
                    (order["taker_pays_display"], order["taker_receives_display"]),
                    ("1,000 LEZ", "0.01000000 BTC"),
                )


if __name__ == "__main__":
    unittest.main()
