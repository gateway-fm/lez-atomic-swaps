#!/usr/bin/env bash
# prepare-ui-swap.sh — one-time owner preparation that unlocks UI-initiated swaps.
#
# Models the repository's M6 lane (run-m2-taker-sells-lez-poc.sh with
# M6_TAKER_SERVICE_MODE=1) and the zec_chat_process.rs service test:
# provisions the deterministic local ZEC corridor fixture, restarts the maker
# daemon with its real Chat authority, publishes a ZEC offer, prepares the
# taker's unsigned draft + actor source, and rewrites the taker service
# configuration with a prepared entry (execute_prepared_zec: true).
#
# After this runs once, the Basecamp Taker UI journey is real end-to-end:
#   Browse authenticated offers -> review -> "Confirm and initiate"
# performs the REAL Maker Chat propose/complete and durable actor
# provisioning (state NotActivated; chain effects remain actor work).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

TOKEN="ui$(date -u +%m%d%H%M%S)-$$"
SWAP_ID="${TOKEN}-swap"
OFFER_ID="offer-${TOKEN}-zec"
RESERVATION_ID="reserve-${TOKEN}"
DIRECTION_CLI="${UI_SWAP_DIRECTION:-taker-sells-lez}"
DIRECTION_JSON="TakerSellsLez"
DIRECTION_SPEC="taker_sells_lez"
FOREIGN_UNITS=100000000
LEZ_UNITS=50000
TTL=7200
MAKER_ACCOUNT="B1UN3hPgxacgHKBRoThcAmsPajGcUf6YXUhgB36x4DAd"
TAKER_ACCOUNT="34Kqgek6R7N1zU5FSJz8ziXwSPEPCuWGcn1T7GCVrfib"

log() { printf '[prep %s] %s\n' "$TOKEN" "$*"; }
compose() { docker compose "$@"; }

# ---------------------------------------------------------------- 1. fixture
log "provisioning deterministic ZEC corridor fixture"
compose run --rm --no-deps --entrypoint bash maker-node -c "
set -e
mkdir -p /prep/${TOKEN}
rm -rf /prep/${TOKEN}/actors
/usr/local/bin/zec-ui-fixture --output-root /prep/${TOKEN}/actors --swap-id ${SWAP_ID} > /prep/${TOKEN}/fixture-summary.json
"
AGREEMENT="/prep/${TOKEN}/actors/shared/agreement-v2.borsh"
log "fixture provisioned (agreement at ${AGREEMENT})"

# ------------------------------------------------- 2. maker daemon: chat on
log "enabling the maker daemon ZEC Chat authority"
PROG_SHA="$(compose run --rm --no-deps --entrypoint bash maker-node -c '
if [ ! -f /prep/actor-program ]; then cp /usr/bin/true /prep/actor-program && chmod 0555 /prep/actor-program; fi
sha256sum /prep/actor-program' | tail -1 | cut -d' ' -f1)"
compose run --rm --no-deps --entrypoint bash maker-node -c "
set -e
mkdir -p /prep/${TOKEN}/maker-actors
chmod 700 /prep/${TOKEN}/maker-actors
# concrete args file consumed by the daemon command in compose.yaml
cat > /prep/daemon-args.sh <<EOF
--chat-socket /run/lez/chat.sock
--maker-claim-key-id ui-zec-claim-${TOKEN}
--maker-claim-key-file /prep/${TOKEN}/actors/maker/claim-recovery.key
--maker-claim-preimage-file /prep/${TOKEN}/actors/maker/claim-preimage.key
--zec-source-maker-config /prep/${TOKEN}/actors/maker/actor-config.json
--zec-maker-actor-root /prep/${TOKEN}/maker-actors
--zec-actor-program /prep/actor-program
--zec-actor-program-sha256 ${PROG_SHA}
EOF
" >/dev/null
compose stop maker-node >/dev/null
compose up -d maker-node >/dev/null
for i in $(seq 1 40); do
  compose exec -T maker-node lez-maker --socket /run/lez/maker.sock health >/dev/null 2>&1 && break
  sleep 1
done
log "maker daemon is back with chat authority"

# --------------------------------------------------------- 3. publish offer
log "publishing the ZEC offer"
CLI=(compose exec -T maker-node lez-maker --socket /run/lez/maker.sock)
rev=""
if ! cfg_out="$("${CLI[@]}" configure-pair --request-id "cfg-${TOKEN}-off" --pair zcash --direction "$DIRECTION_CLI" \
      --enabled false --price-source local --minimum-foreign-units "$FOREIGN_UNITS" \
      --maximum-foreign-units "$FOREIGN_UNITS" --offer-ttl-seconds "$TTL" 2>&1)"; then
  rev="$(grep -oE 'actual Some\([0-9]+\)' <<<"$cfg_out" | grep -oE '[0-9]+' || true)"
fi
extra_cfg=()
[[ -n "$rev" ]] && extra_cfg=(--expected-revision "$rev")
"${CLI[@]}" configure-pair --request-id "cfg-${TOKEN}-off2" "${extra_cfg[@]}" --pair zcash \
  --direction "$DIRECTION_CLI" --enabled false --price-source local \
  --minimum-foreign-units "$FOREIGN_UNITS" --maximum-foreign-units "$FOREIGN_UNITS" \
  --offer-ttl-seconds "$TTL" >/dev/null
price_rev=""
if ! price_out="$("${CLI[@]}" set-local-price --request-id "price-${TOKEN}" --pair zcash --direction "$DIRECTION_CLI" \
      --lez-units-per-lot 1 --foreign-units-per-lot 2000 2>&1)"; then
  price_rev="$(grep -oE 'actual Some\([0-9]+\)' <<<"$price_out" | grep -oE '[0-9]+' || true)"
fi
price_extra=()
[[ -n "$price_rev" ]] && price_extra=(--expected-revision "$price_rev")
"${CLI[@]}" set-local-price --request-id "price-${TOKEN}2" "${price_extra[@]}" --pair zcash \
  --direction "$DIRECTION_CLI" --lez-units-per-lot 1 --foreign-units-per-lot 2000 >/dev/null
on_rev=$(( ${rev:-0} + 1 ))
"${CLI[@]}" configure-pair --request-id "cfg-${TOKEN}-on" --expected-revision "$on_rev" --pair zcash \
  --direction "$DIRECTION_CLI" --enabled true --price-source local \
  --minimum-foreign-units "$FOREIGN_UNITS" --maximum-foreign-units "$FOREIGN_UNITS" \
  --offer-ttl-seconds "$TTL" >/dev/null
"${CLI[@]}" publish-offer --request-id "pub-${TOKEN}" --offer-id "$OFFER_ID" \
  --pair zcash --direction "$DIRECTION_CLI" >/dev/null
log "offer ${OFFER_ID} published"

# ---------------------------------------------------- 4. discovery + draft
log "discovering the signed offer and preparing the taker draft"
MAKER_KEY="$(compose run --rm --no-deps --entrypoint bash maker-node -c \
  '/usr/local/bin/lez-maker delivery-identity --signing-key-file /var/lib/lez/delivery-signing.key | tr -d " \n" | grep -o "02[0-9a-f]\{64\}\|03[0-9a-f]\{64\}"')"
DISCOVERY="$(compose run --rm --no-deps --entrypoint bash maker-node -c "
  /usr/local/bin/lez-taker --delivery-directory /delivery --maker-public-key ${MAKER_KEY} \
    --now-unix-seconds \$(date -u +%s) --pair zcash --direction ${DIRECTION_CLI}")"
COMMITMENT="$(jq -er --arg offer "$OFFER_ID" '.offers[] | select(.offer.id == $offer) | .signed_envelope_sha256' <<<"$DISCOVERY")"
EXPIRES="$(jq -er --arg offer "$OFFER_ID" '.offers[] | select(.offer.id == $offer) | .offer.expires_at_unix_seconds' <<<"$DISCOVERY")"
SIGNED_ENVELOPE="/delivery/${OFFER_ID}.offer.json"
NOW="$(date -u +%s)"
compose run --rm --no-deps --entrypoint bash maker-node -c "
set -e
/usr/local/bin/zec-local-poc-chat-draft \
  --source-agreement-file ${AGREEMENT} \
  --now-unix-seconds ${NOW} --reservation-id ${RESERVATION_ID} \
  --offer-commitment ${COMMITMENT} \
  --offer-expires-at-unix-seconds ${EXPIRES} \
  --output-file /prep/${TOKEN}/unsigned-draft.borsh >/dev/null
" >/dev/null
log "draft bound to offer commitment ${COMMITMENT:0:16}…"

# ------------------------------------------- 5. registry + service config
log "writing the taker service prepared configuration"
compose run --rm --no-deps --entrypoint bash \
  -v lez-prep-state:/prep -v lez-taker-state:/t -v lez-delivery-dir:/delivery maker-node -c "
set -e
mkdir -p /t/registry && chmod 700 /t/registry
REG=/t/registry/taker-service.sqlite3
rm -f \"\$REG\"
/usr/local/bin/lez-taker-registry-init --database \"\$REG\" >/dev/null
jq -n \
  --arg maker '${MAKER_KEY}' --arg chat /run/lez-maker-chat/chat.sock \
  --arg registry /var/lib/lez-taker/registry/taker-service.sqlite3 --arg swap '${SWAP_ID}' --arg offer '${OFFER_ID}' \
  --arg reservation '${RESERVATION_ID}' \
  --arg envelope '${SIGNED_ENVELOPE}' --arg envelope_sha '${COMMITMENT}' \
  --arg draft /prep/${TOKEN}/unsigned-draft.borsh \
  --arg draft_sha \"\$(sha256sum /prep/${TOKEN}/unsigned-draft.borsh | cut -d' ' -f1)\" \
  --arg key /prep/${TOKEN}/actors/taker/zcash.key \
  --arg src /prep/${TOKEN}/actors/taker/actor-config.json \
  --arg src_sha \"\$(sha256sum /prep/${TOKEN}/actors/taker/actor-config.json | cut -d' ' -f1)\" \
  --arg agreement /prep/${TOKEN}/final-agreement.borsh \
  --arg actor_root /prep/${TOKEN}/taker-actors \
  --arg receipt /prep/${TOKEN}/acceptance-receipt.json \
  '{schema_version:1,
    delivery_sources:[{source_id:\"local-maker\", directory:\"/delivery\", maker_public_key:\$maker}],
    chat_socket:\$chat, maximum_offers:16,
    initiation:{execute_prepared_zec:true, registry_database:\$registry,
      prepared_zec:[{source_id:\"local-maker\", swap_id:\$swap, offer_id:\$offer,
        reservation_id:\$reservation, foreign_units:${FOREIGN_UNITS}, lez_units:${LEZ_UNITS},
        signed_envelope:{path:\$envelope, sha256:\$envelope_sha},
        unsigned_draft:{path:\$draft, sha256:\$draft_sha},
        signing_key:{path:\$key},
        source_config:{path:\$src, sha256:\$src_sha},
        agreement_output:\$agreement, actor_root:\$actor_root, receipt_output:\$receipt}]}}' \
  > /t/taker-service.json
chmod 0600 /t/taker-service.json
" >/dev/null

compose stop taker-service >/dev/null
compose up -d taker-service >/dev/null
for i in $(seq 1 40); do
  compose exec -T taker-service curl -sf --max-time 2 --unix-socket /run/lez/taker.sock \
    --header 'content-type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"taker_health","params":[{"schema_version":1}]}' \
    http://localhost/ >/dev/null 2>&1 && break
  sleep 1
done

cat <<BANNER

──────────────────────────────────────────────────────────────
 UI-initiated swaps are armed (token ${TOKEN})

 In Basecamp (VNC :5901), Taker Route:
   1. pair: Zcash, direction: TakerSellsLez
   2. Browse authenticated offers -> offer ${OFFER_ID}
   3. Copy its facts into the review form and press
      "Confirm and initiate"  — REAL Chat acceptance runs
   4. "List my swaps" / Monitor -> state NotActivated

 replay-safe: repeat the identical click to see was_replay
──────────────────────────────────────────────────────────────
BANNER
