#!/usr/bin/env python3
"""Idempotently prepare the local regtest wallets of both roles (never other chains)."""
import json
import subprocess

CLI = ["docker", "exec", "lez-bitcoin-core", "bitcoin-cli",
       "-conf=/run-config/bitcoin.conf", "-datadir=/var/lib/bitcoin"]


def rpc(*args):
    result = subprocess.run(CLI + list(args), check=True, capture_output=True, text=True)
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError:
        return result.stdout.strip()


def seed(call=rpc):
    if call("getblockchaininfo")["chain"] != "regtest":
        raise RuntimeError("wallet seeding requires Bitcoin regtest")
    loaded = set(call("listwallets"))
    existing = {wallet["name"] for wallet in call("listwalletdir")["wallets"]}
    for name in ("lez-maker", "lez-taker"):
        if name not in loaded:
            if name in existing:
                call("-named", "loadwallet", f"filename={name}", "load_on_startup=true")
            else:
                call("-named", "createwallet", f"wallet_name={name}", "load_on_startup=true")
    # Whoever sells Bitcoin locks it from its own Core wallet: the Taker when
    # the Taker sells Bitcoin, the Maker when the Maker does. Both get a
    # spendable regtest balance (each mine matures after 100 more blocks).
    for name in ("lez-taker", "lez-maker"):
        wallet = f"-rpcwallet={name}"
        balance = call(wallet, "getbalances")["mine"]["trusted"]
        if balance < 1:
            address = call(wallet, "getnewaddress", "", "bech32m")
            call("generatetoaddress", "105", address)
        balance = call(wallet, "getbalances")["mine"]["trusted"]
        if balance < 1:
            raise RuntimeError(f"{name} has less than 1 spendable regtest BTC after seeding")
    print("Regtest wallets ready; Maker and Taker each have at least 1 spendable BTC.")


if __name__ == "__main__":
    seed()
