#!/opt/homebrew/bin/bash
# swap-run.sh — the one-host native run: Basecamp 0.3.2 on the user dir that
# holds both desks from one `package install` each (bc/mac-both), with
# env-both.sh's environment; open LEZ Maker, then LEZ Taker (each starts its
# signer and Node); wait for both Nodes' ready files; run the node-e2e
# scenarios (run-suite.sh); quit Basecamp; count Chat's decode failures.
set -u
W=${SCRATCH:?}
export APP=$HOME/tmp/lez-darwin-variant-work/basecamp-032/app/LogosBasecamp.app
export USER_DIR=$W/bc/mac-both LEZ_NODE_HOME=$W/home DEPLOY=${WORKSPACE:?}/deploy
export CA_BUNDLE=${CHECKOUT:?}-work/tls/bundle.pem
export LEZ_ZEC_NATIVE_TOOLS=$HOME/tmp/lez-031-darwin-target/release
R=$DEPLOY/runtime
# A fresh Node home from the stack's current runtime: the role identities, the
# market manifest and the Bitcoin RPC cookie (0600); no Node state, no wallet,
# no ZEC corridor (node-e2e provisions one per ZEC scenario).
rm -rf "$LEZ_NODE_HOME"; install -d -m 0700 "$LEZ_NODE_HOME" "$LEZ_NODE_HOME/identity"
for role in maker taker; do
  install -d -m 0700 "$LEZ_NODE_HOME/identity/$role"
  install -m 0600 "$R/lez/$role/identity.json" "$R/lez/$role/lez-signer.key" "$R/lez/$role/zcash.key" "$LEZ_NODE_HOME/identity/$role/"
done
install -m 0600 "$R/market-bootstrap.env" "$LEZ_NODE_HOME/market-bootstrap.env"
install -m 0600 "$R/secrets/btc-rpc-cookie" "$LEZ_NODE_HOME/btc-rpc-cookie"
echo "[$(date -u +%TZ)] Node home from $R: $(ls "$LEZ_NODE_HOME" | tr '\n' ' ')"
LOG=$W/logs/swap-basecamp.log
"$W/scripts/start-basecamp.sh" > "$LOG" 2>&1 < /dev/null &
echo "[$(date -u +%TZ)] basecamp started"; sleep 30
osascript -l JavaScript "$W/scripts/ax-click.js" "LEZ Maker"; sleep 40
osascript -l JavaScript "$W/scripts/ax-click.js" "LEZ Taker"
for _ in $(seq 180); do [[ -e $LEZ_NODE_HOME/run/maker/ready && -e $LEZ_NODE_HOME/run/taker/ready ]] && break; sleep 2; done
echo "[$(date -u +%TZ)] ready files: $(ls $LEZ_NODE_HOME/run/maker/ready $LEZ_NODE_HOME/run/taker/ready 2>&1 | tr '\n' ' ')"
printf '%s\n' "btc-happy happy --direction TakerSellsForeign" "btc-happy-lez happy --direction TakerSellsLez" \
  "zec-happy-lez zec-happy --pair Zcash --direction TakerSellsLez" "zec-happy-foreign zec-happy --pair Zcash --direction TakerSellsForeign" |
  "$W/scripts/run-suite.sh" "$W/e2e"
"$W/scripts/quit-basecamp.sh" | tail -1
echo "chat decode failures (messageReceived payload missing or malformed): $(grep -c 'payload missing or malformed' "$LOG")"
echo "delivery message_received events: $(grep -c '"eventType":"message_received"' "$LOG")"
grep -v logos.protocol "$LOG" | grep -E "App launcher clicked|blocked by deps|ui-host: loaded plugin|chat_module::actions|lez_role_node" | cut -c1-220
