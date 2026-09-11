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
    names = ("lez-taker", "lez-maker")
    trusted = lambda name: call(f"-rpcwallet={name}", "getbalances")["mine"]["trusted"]
    for name in names:
        if trusted(name) < 1:
            address = call(f"-rpcwallet={name}", "getnewaddress", "", "bech32m")
            call("generatetoaddress", "105", address)
    # Enough for many local swaps of a few hundred thousand satoshis. A fresh
    # chain mines far more; a chain past its subsidy (halving every 150
    # blocks) mines nothing, and then the other wallet, if it holds coins,
    # hands half of them over.
    floor = 0.05
    for name in names:
        if trusted(name) >= floor:
            continue
        other = next(o for o in names if o != name)
        share = round(trusted(other) / 2, 8)
        if share < floor:
            raise RuntimeError(f"{name} has less than {floor} spendable regtest BTC after seeding and {other} cannot fund it")
        address = call(f"-rpcwallet={name}", "getnewaddress", "", "bech32m")
        call(f"-rpcwallet={other}", "-named", "sendtoaddress", f"address={address}", f"amount={share}", "fee_rate=1")
        call("generatetoaddress", "1", call(f"-rpcwallet={other}", "getnewaddress", "", "bech32m"))
        if trusted(name) < floor:
            raise RuntimeError(f"{name} has less than {floor} spendable regtest BTC after seeding")
    print(f"Regtest wallets ready; Maker {trusted('lez-maker')} BTC, Taker {trusted('lez-taker')} BTC spendable.")


if __name__ == "__main__":
    seed()
