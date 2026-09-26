#!/usr/bin/env bash
# Operate a Countersign deployment without copying values around: every address, key and
# token is read from the crates' own .env files (written by fork-setup.sh --write-env or
# mainnet-setup.sh).
#
#   scripts/ops.sh health                    is the countersigner up, and what is enabled
#   scripts/ops.sh stream                    the countersigner's decisions, live
#   scripts/ops.sh wallet                    the wallet as the countersigner sees it
#   scripts/ops.sh close [seller] [reason]   close a tab: revoke the seller, on chain
#                                            (reason: free text or an Intercepta trait, e.g. wallet_drainer)
#   scripts/ops.sh reopen [seller]           ask the wallet's human to reopen it (World ID)
#   scripts/ops.sh approval <id>             where an approval has got to
#   scripts/ops.sh status [seller]           the wallet's on-chain verdict on a seller
#   scripts/ops.sh claim                     make the demo seller cash its vouchers in now
#   scripts/ops.sh tab [url]                 a tab's escrowed and claimed amounts
#   scripts/ops.sh open <url> <amount>       open a tab to any x402 batch-settlement seller
#   scripts/ops.sh withdraw start [url] <amount>   start taking a tab's money back
#   scripts/ops.sh withdraw finish [url]           ...and finish, after the tab's withdrawDelay
#   scripts/ops.sh sweep <amount>            move USDC out of the wallet to the owner
#
# [seller] defaults to the demo seller; [url] to its tab. Amounts are atomic USDC (6 decimals).
set -euo pipefail
source "$(dirname "$0")/lib.sh"

USDC=0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913
ESCROW=0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003
ZERO=0x0000000000000000000000000000000000000000
SALT=0x0000000000000000000000000000000000000000000000000000000000000000
CFG_SIG='(address,address,address,address,address,uint40,bytes32)'

need() { [ -n "$2" ] || die "$1 is not set - run scripts/fork-setup.sh --write-env or scripts/mainnet-setup.sh"; echo "$2"; }

CS_URL=$(env_get agent COUNTERSIGNER_URL); CS_URL=${CS_URL:-http://127.0.0.1:8787}
RPC=$(env_get countersigner BASE_RPC); RPC=${RPC:-$(env_get agent BASE_RPC)}; RPC=${RPC:-https://mainnet.base.org}
WALLET=$(env_get agent COUNTERSIGN_WALLET)
SELLER=$(env_get seller SELLER_RECEIVER)
SELLER_BIND=$(env_get seller SELLER_BIND); SELLER_BIND=${SELLER_BIND:-127.0.0.1:8080}

control() { need CONTROL_TOKEN "$(env_get countersigner CONTROL_TOKEN)"; }
owner_key() { need OWNER_KEY "$(env_get scripts OWNER_KEY)"; }
api() { curl -sS -H "Authorization: Bearer $(control)" -H 'content-type: application/json' "$@"; }

# The channel config for a tab: the demo seller's, or whatever an x402 URL's 402 offers on Base.
cfg_for() {
  local url=${1:-}
  if [ -z "$url" ]; then
    local auth delay
    auth=$(need RECEIVER_AUTHORIZER "$(env_get agent RECEIVER_AUTHORIZER)")
    delay=$(env_get seller SELLER_WITHDRAW_DELAY); delay=${delay:-900}
    echo "($(need COUNTERSIGN_WALLET "$WALLET"),$ZERO,$(need SELLER_RECEIVER "$SELLER"),$auth,$USDC,$delay,$SALT)"
    return
  fi
  local terms
  terms=$(curl -sS -D - -o /dev/null "$url" | tr -d '\r' | awk -F': ' 'tolower($1)=="payment-required"{print $2}' \
    | base64 -d 2>/dev/null \
    | jq -c '[.accepts[] | select(.scheme=="batch-settlement" and .network=="eip155:8453")][0] // empty')
  [ -n "$terms" ] || die "$url does not offer batch-settlement terms on Base"
  echo "($(need COUNTERSIGN_WALLET "$WALLET"),$ZERO,$(echo "$terms" | jq -r .payTo),$(echo "$terms" | jq -r .extra.receiverAuthorizer),$(echo "$terms" | jq -r .asset),$(echo "$terms" | jq -r .extra.withdrawDelay),$SALT)"
}
channel_id() { cast call $ESCROW "getChannelId($CFG_SIG)(bytes32)" "$1" --rpc-url "$RPC"; }
# An explicit nonce, retried: public RPCs load-balance across nodes that can briefly report a
# stale nonce right after the previous transaction.
owner_send() {
  local key out n try
  key=$(owner_key)
  for try in 1 2 3 4; do
    n=$(cast nonce "$(addr "$key")" --rpc-url "$RPC" --block pending)
    if out=$(cast send "$@" --nonce "$n" --rpc-url "$RPC" --private-key "$key" --json 2>&1); then
      echo "$out" | jq -r '"tx " + .transactionHash + "  status " + .status'
      return 0
    fi
    echo "$out" | grep -qiE "nonce too low|already known|underpriced|rate limit" || die "transaction failed: $out"
    log "  (nonce $n was stale or the RPC throttled, retrying)"; sleep 4
  done
  die "transaction failed after retries: $out"
}

# An optional leading seller address, so `close wallet_drainer` and `close 0xabc.. phishing`
# both read naturally. Sets TARGET and leaves the remaining arguments in REST.
target() {
  if [[ "${1:-}" =~ ^0x[0-9a-fA-F]{40}$ ]]; then TARGET=$1; shift; else TARGET=$(need SELLER_RECEIVER "$SELLER"); fi
  REST=("$@")
}

cmd=${1:-help}; shift || true
case "$cmd" in
  health)   curl -sS "$CS_URL/health" | jq ;;
  stream)   curl -sSN -H "Authorization: Bearer $(control)" "$CS_URL/v1/stream" ;;
  wallet)   api "$CS_URL/v1/wallets/$(need COUNTERSIGN_WALLET "$WALLET")" | jq ;;
  close)
    target "$@"
    body=$(jq -nc --arg w "$(need COUNTERSIGN_WALLET "$WALLET")" --arg s "$TARGET" --arg r "${REST[*]:-closed by the operator}" \
      '{wallet:$w, seller:$s, reason:$r}')
    api -X POST "$CS_URL/v1/revoke" -d "$body" | jq ;;
  reopen)
    target "$@"
    body=$(jq -nc --arg w "$(need COUNTERSIGN_WALLET "$WALLET")" --arg s "$TARGET" '{wallet:$w, seller:$s}')
    api -X POST "$CS_URL/v1/restore" -d "$body" \
      | jq '{status, id, userCode, approveAt: (.verificationUriComplete // .verificationUri), reviewPage: .approvalUrl, error}' ;;
  approval) curl -sS "$CS_URL/v1/approvals/${1:?approval id}" | jq ;;
  status)
    target "$@"
    cast call "$(need COUNTERSIGN_WALLET "$WALLET")" 'revocations(address)(bool,uint32,uint64)' "$TARGET" --rpc-url "$RPC" \
      | paste -sd' ' - | awk '{print "revoked " $1 "   reason code " $2 "   at " $3}' ;;
  claim)
    tok=$(need SELLER_ADMIN_TOKEN "$(env_get seller SELLER_ADMIN_TOKEN)")
    curl -sS -X POST -H "Authorization: Bearer $tok" "http://$SELLER_BIND/admin/claim" | jq ;;
  tab)
    cfg=$(cfg_for "${1:-}"); id=$(channel_id "$cfg")
    read -r bal claimed < <(cast call $ESCROW 'channels(bytes32)(uint128,uint128)' "$id" --rpc-url "$RPC" | awk '{print $1}' | paste -sd' ' -)
    echo "tab $id"; echo "escrowed $bal   claimed by the seller $claimed   (atomic USDC)" ;;
  open)
    url=${1:?usage: ops.sh open <url> <amount>}; amount=${2:?amount in atomic USDC}
    cfg=$(cfg_for "$url"); log "opening $cfg"
    owner_send "$WALLET" "openChannel($CFG_SIG,uint128,address)" "$cfg" "$amount" \
      "$(need COUNTERSIGN_COLLECTOR "$(env_get scripts COUNTERSIGN_COLLECTOR)")"
    echo "tab $(channel_id "$cfg")" ;;
  withdraw)
    step=${1:?start or finish}; shift
    url=""; if [[ "${1:-}" == http* ]]; then url=$1; shift; fi
    cfg=$(cfg_for "$url")
    case "$step" in
      start)  owner_send "$WALLET" "initiateWithdraw($CFG_SIG,uint128)" "$cfg" "${1:?amount in atomic USDC}" ;;
      finish) owner_send "$WALLET" "finalizeWithdraw($CFG_SIG)" "$cfg" ;;
      *) die "withdraw start|finish" ;;
    esac ;;
  sweep)
    owner=$(addr "$(owner_key)")
    owner_send "$WALLET" 'sweep(address,address,uint256)' $USDC "$owner" "${1:?amount}" ;;
  *) sed -n '2,20p' "$0" | sed 's/^# \{0,1\}//' ;;
esac
