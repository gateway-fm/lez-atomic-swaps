#!/opt/homebrew/bin/bash
# swap-run.sh — the one-host native run: Basecamp 0.3.2 on the user dir that
# holds both desks from one `package install` each (bc/mac-both), with
# env-both.sh's environment; open LEZ Maker, then LEZ Taker (each starts its
# signer and Node); wait for both Nodes' ready files; run the node-e2e
# scenarios (run-suite.sh); quit Basecamp; count Chat's decode failures.
set -u
W=<scratch>
export APP=<home>/tmp/lez-darwin-variant-work/basecamp-032/app/LogosBasecamp.app
export USER_DIR=$W/bc/mac-both LEZ_NODE_HOME=$W/home DEPLOY=<workspace>/deploy
export CA_BUNDLE=<scratch>/previous/tls/bundle.pem
LOG=$W/logs/swap-basecamp.log
"$W/scripts/start-basecamp.sh" > "$LOG" 2>&1 < /dev/null &
echo "[$(date -u +%TZ)] basecamp started"; sleep 30
osascript -l JavaScript "$W/scripts/ax-click.js" "LEZ Maker"; sleep 40
osascript -l JavaScript "$W/scripts/ax-click.js" "LEZ Taker"
for i in $(seq 180); do [[ -e $LEZ_NODE_HOME/run/maker/ready && -e $LEZ_NODE_HOME/run/taker/ready ]] && break; sleep 2; done
echo "[$(date -u +%TZ)] ready files: $(ls $LEZ_NODE_HOME/run/maker/ready $LEZ_NODE_HOME/run/taker/ready 2>&1 | tr '\n' ' ')"
printf '%s\n' "btc-happy happy --direction TakerSellsForeign" "btc-happy-lez happy --direction TakerSellsLez" \
  "zec-happy-lez zec-happy --pair Zcash --direction TakerSellsLez" "zec-happy-foreign zec-happy --pair Zcash --direction TakerSellsForeign" |
  "$W/scripts/run-suite.sh" "$W/e2e"
"$W/scripts/quit-basecamp.sh" | tail -1
echo "chat decode failures (messageReceived payload missing or malformed): $(grep -c 'payload missing or malformed' "$LOG")"
echo "delivery message_received events: $(grep -c '"eventType":"message_received"' "$LOG")"
grep -v logos.protocol "$LOG" | grep -E "App launcher clicked|blocked by deps|ui-host: loaded plugin|chat_module::actions|lez_role_node" | cut -c1-220
