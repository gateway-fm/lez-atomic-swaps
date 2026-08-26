#!/usr/bin/env bash
set -euo pipefail

role="$(basename "$LEE_WALLET_HOME_DIR")"
case "$role" in maker | taker) ;; *) exit 64 ;; esac

args="$*"
if [[ "$args" == account\ import\ public\ --private-key\ * ]]; then
  printf '%s\t%s\n' "$role" 'account import public --private-key <redacted>' \
    >>"$FAKE_WALLET_CALLS"
  printf '{}\n' >"${LEE_WALLET_HOME_DIR}/storage.json"
  chmod 0664 "${LEE_WALLET_HOME_DIR}/storage.json"
  if [[ "$role" == maker ]]; then
    printf '%s\n' 'Recovery phrase:' '  fixture secret words' \
      'Imported public account Public/@MAKER@'
  else
    printf '%s\n' 'Recovery phrase:' '  fixture secret words' \
      'Imported public account Public/@TAKER@'
  fi
  exit 0
fi

printf '%s\t%s\n' "$role" "$args" >>"$FAKE_WALLET_CALLS"
case "$role:$args" in
  'maker:account new public --label f7-a-definition')
    echo 'Generated new account with account_id Public/@DEF_A@ at path m/1'
    ;;
  'maker:account new public --label f7-a-supply')
    echo 'Generated new account with account_id Public/@SUPPLY_A@ at path m/2'
    ;;
  'taker:account new public --label f7-b-definition')
    echo 'Generated new account with account_id Public/@DEF_B@ at path m/1'
    ;;
  'taker:account new public --label f7-b-supply')
    echo 'Generated new account with account_id Public/@SUPPLY_B@ at path m/2'
    ;;
  'maker:token new --definition-account-id f7-a-definition --supply-account-id f7-a-supply --name M3F7A --total-supply 1000')
    echo 'Transaction hash is aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1'
    ;;
  'taker:token new --definition-account-id f7-b-definition --supply-account-id f7-b-supply --name M3F7B --total-supply 1000')
    echo 'Transaction hash is bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb2'
    ;;
  "maker:ata address --owner @MAKER@ --token-definition @DEF_A@")
    echo '@MAKER_A@'
    ;;
  "maker:ata address --owner @TAKER@ --token-definition @DEF_A@")
    echo '@TAKER_A@'
    ;;
  "maker:ata address --owner @MAKER@ --token-definition @DEF_B@")
    echo '@MAKER_B@'
    ;;
  "maker:ata address --owner @TAKER@ --token-definition @DEF_B@")
    echo '@TAKER_B@'
    ;;
  "maker:ata create --owner Public/@MAKER@ --token-definition @DEF_A@")
    echo 'Transaction hash is ccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc3'
    ;;
  "taker:ata create --owner Public/@TAKER@ --token-definition @DEF_A@")
    echo 'Transaction hash is ddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd4'
    ;;
  "maker:ata create --owner Public/@MAKER@ --token-definition @DEF_B@")
    echo 'Transaction hash is eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee5'
    ;;
  "taker:ata create --owner Public/@TAKER@ --token-definition @DEF_B@")
    echo 'Transaction hash is fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff6'
    ;;
  "maker:token send --from f7-a-supply --to Public/@MAKER_A@ --amount 250")
    echo 'Transaction hash is 1111111111111111111111111111111111111111111111111111111111111117'
    ;;
  "taker:token send --from f7-b-supply --to Public/@TAKER_B@ --amount 250")
    echo 'Transaction hash is 2222222222222222222222222222222222222222222222222222222222222228'
    ;;
  "maker:ata list --owner @MAKER@ --token-definition @DEF_A@ @DEF_B@")
    echo 'ATA @MAKER_A@ (definition @DEF_A@): balance 250'
    echo 'ATA @MAKER_B@ (definition @DEF_B@): balance 0'
    ;;
  "maker:ata list --owner @TAKER@ --token-definition @DEF_A@ @DEF_B@")
    echo 'ATA @TAKER_A@ (definition @DEF_A@): balance 0'
    echo 'ATA @TAKER_B@ (definition @DEF_B@): balance 250'
    ;;
  *) exit 64 ;;
esac
