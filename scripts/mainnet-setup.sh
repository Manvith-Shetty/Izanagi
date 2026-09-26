#!/usr/bin/env bash
# Deploy a Countersign wallet on Base MAINNET with real USDC, open a tab to your own demo
# seller, and write every value the services need into their .env files.
#
#   1. put your funded key in scripts/.env:   OWNER_KEY=0x...
#   2. scripts/mainnet-setup.sh               (shows the plan, asks before spending anything)
#
# What it spends (defaults, all overridable in scripts/.env):
#   FUND=2000000              2 USDC moved from you into the wallet (recoverable)
#   CHANNEL_DEPOSIT=500000    0.50 USDC of that escrowed in a tab to your own seller (recoverable)
#   GAS_TOPUP=0.0003ether     sent to each of the oracle and the seller's claim key, for gas
#   + about 2 cents of gas for the deployment itself
#
# It generates FRESH agent, oracle and seller keys. It never reuses the fork's keys, which
# anyone can derive from fork-setup.sh.
set -euo pipefail
source "$(dirname "$0")/lib.sh"

ASSUME_YES=false
[ "${1:-}" = "--yes" ] && ASSUME_YES=true

OWNER_KEY=$(env_get scripts OWNER_KEY)
[ -n "$OWNER_KEY" ] || die "put your funded key in scripts/.env as OWNER_KEY=0x... (see scripts/.env.example)"
RPC=$(env_get scripts RPC); RPC=${RPC:-https://mainnet.base.org}
FUND=$(env_get scripts FUND); FUND=${FUND:-2000000}
CHANNEL_DEPOSIT=$(env_get scripts CHANNEL_DEPOSIT); CHANNEL_DEPOSIT=${CHANNEL_DEPOSIT:-500000}
GAS_TOPUP=$(env_get scripts GAS_TOPUP); GAS_TOPUP=${GAS_TOPUP:-0.0003ether}

ESCROW=0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003
USDC=0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913
ZERO=0x0000000000000000000000000000000000000000
SALT=0x0000000000000000000000000000000000000000000000000000000000000000
WITHDRAW_DELAY=900
SCAN=https://basescan.org

OWNER=$(addr "$OWNER_KEY")
SELLER_RECEIVER=$(env_get scripts SELLER_RECEIVER); SELLER_RECEIVER=${SELLER_RECEIVER:-$OWNER}

# ---- preflight: nothing is sent until every check passes ----
[ "$(cast chain-id --rpc-url "$RPC")" = 8453 ] || die "$RPC is not Base mainnet (chain 8453)"
[ "$(cast code $ESCROW --rpc-url "$RPC")" != 0x ] || die "Coinbase's escrow has no code at $ESCROW on $RPC"
[ "$FUND" -ge "$CHANNEL_DEPOSIT" ] || die "FUND ($FUND) must cover CHANNEL_DEPOSIT ($CHANNEL_DEPOSIT)"

ETH_HAVE=$(cast balance "$OWNER" --rpc-url "$RPC")
ETH_NEED=$(( $(cast to-wei "${GAS_TOPUP%ether}") * 2 + 200000000000000 ))   # top-ups + ~0.0002 ETH of gas
USDC_HAVE=$(cast call $USDC 'balanceOf(address)(uint256)' "$OWNER" --rpc-url "$RPC" | cut -d' ' -f1)
[ "$ETH_HAVE" -ge "$ETH_NEED" ] || die "owner $OWNER has $(cast from-wei "$ETH_HAVE") ETH, needs about $(cast from-wei "$ETH_NEED")"
[ "$USDC_HAVE" -ge "$FUND" ] || die "owner $OWNER has $USDC_HAVE atomic USDC, needs $FUND"

AGENT_KEY=$(new_key); ORACLE_KEY=$(new_key); SELLER_AUTHORIZER_KEY=$(new_key)
AGENT=$(addr "$AGENT_KEY"); ORACLE=$(addr "$ORACLE_KEY"); SELLER_AUTHORIZER=$(addr "$SELLER_AUTHORIZER_KEY")

usdc() { awk -v a="$1" 'BEGIN { printf "%.6f", a / 1e6 }'; }
cat >&2 <<PLAN

  Base mainnet, via $RPC
  owner (you)        $OWNER   $(cast from-wei "$ETH_HAVE") ETH, $(usdc "$USDC_HAVE") USDC
  new agent key      $AGENT
  new oracle key     $ORACLE   gets $GAS_TOPUP for revoke/restore gas
  new seller claims  $SELLER_AUTHORIZER   gets $GAS_TOPUP for claim gas
  seller receives at $SELLER_RECEIVER

  1. deploy CountersignCollector and Countersign(owner, escrow, agent, oracle)
  2. move $(usdc "$FUND") USDC into the wallet, allow its collector to use it
  3. open a $(usdc "$CHANNEL_DEPOSIT") USDC tab to your own seller
  4. write the results into countersigner/.env, seller/.env, agent/.env, scripts/.env

PLAN
if ! $ASSUME_YES; then
  read -r -p "  This spends real funds. Type yes to continue: " answer
  [ "$answer" = yes ] || die "stopped, nothing was sent"
fi

# Write the new keys down BEFORE sending anything. A run that dies halfway must never leave a
# deployed wallet whose agent and oracle keys existed only in this process's memory.
env_set agent AGENT_PRIVATE_KEY "$AGENT_KEY"
env_set countersigner ORACLE_PRIVATE_KEY "$ORACLE_KEY"
env_set seller SELLER_AUTHORIZER_KEY "$SELLER_AUTHORIZER_KEY"
log "new keys saved to agent/.env, countersigner/.env, seller/.env"

# Every transaction carries an explicit nonce. mainnet.base.org load-balances across nodes that
# can briefly disagree on the pending nonce, so two quick sends would otherwise collide.
NONCE=$(cast nonce "$OWNER" --rpc-url "$RPC" --block pending)
send() {
  local out
  out=$(cast send "$@" --nonce "$NONCE" --rpc-url "$RPC" --private-key "$OWNER_KEY" --json 2>&1) \
    || die "transaction (nonce $NONCE) failed: $* :: $out"
  NONCE=$((NONCE + 1))
  echo "$out" | jq -r .transactionHash
}

cd "$(dirname "$0")/../contracts"
forge build --silent
# Runs in the main shell (not a subshell) so the nonce it consumes is remembered; the address
# comes back in CREATED.
create() {
  local out
  out=$(forge create --rpc-url "$RPC" --private-key "$OWNER_KEY" --nonce "$NONCE" --broadcast --json "$@" 2>&1) || true
  CREATED=$(echo "$out" | sed -n '/^{/,$p' | jq -r '.deployedTo // empty' 2>/dev/null || true)
  [ -n "$CREATED" ] || die "deploying $1 failed: $out"
  NONCE=$((NONCE + 1))
}

log "deploying..."
create src/Countersign.sol:CountersignCollector --constructor-args "$ESCROW"; COLLECTOR=$CREATED
env_set scripts COUNTERSIGN_COLLECTOR "$COLLECTOR"
create src/Countersign.sol:Countersign --constructor-args "$OWNER" "$ESCROW" "$AGENT" "$ORACLE"; WALLET=$CREATED
env_set scripts COUNTERSIGN_WALLET "$WALLET"
env_set agent COUNTERSIGN_WALLET "$WALLET"
[ "$(cast call "$WALLET" 'riskOracle()(address)' --rpc-url "$RPC")" = "$ORACLE" ] || die "the wallet does not name the new oracle"
log "  collector $SCAN/address/$COLLECTOR"
log "  wallet    $SCAN/address/$WALLET"

log "funding gas for the oracle and the seller's claim key..."
TX=$(send "$ORACLE" --value "$GAS_TOPUP"); NONCE=$((NONCE + 1))
TX=$(send "$SELLER_AUTHORIZER" --value "$GAS_TOPUP"); NONCE=$((NONCE + 1))

log "moving $(usdc "$FUND") USDC into the wallet..."
TX=$(send $USDC 'transfer(address,uint256)' "$WALLET" "$FUND"); NONCE=$((NONCE + 1))
TX=$(send "$WALLET" 'approveToken(address,address,uint256)' $USDC "$COLLECTOR" "$FUND"); NONCE=$((NONCE + 1))

log "opening a $(usdc "$CHANNEL_DEPOSIT") USDC tab to your seller..."
CFG="($WALLET,$ZERO,$SELLER_RECEIVER,$SELLER_AUTHORIZER,$USDC,$WITHDRAW_DELAY,$SALT)"
TX=$(send "$WALLET" 'openChannel((address,address,address,address,address,uint40,bytes32),uint128,address)' "$CFG" "$CHANNEL_DEPOSIT" "$COLLECTOR"); NONCE=$((NONCE + 1))
CHANNEL_ID=$(cast call $ESCROW 'getChannelId((address,address,address,address,address,uint40,bytes32))(bytes32)' "$CFG" --rpc-url "$RPC")
log "  tab $CHANNEL_ID  $SCAN/tx/$TX"

CONTROL_TOKEN=$(env_get countersigner CONTROL_TOKEN)
[ ${#CONTROL_TOKEN} -ge 16 ] || CONTROL_TOKEN=$(openssl rand -hex 16)

env_set countersigner ORACLE_PRIVATE_KEY "$ORACLE_KEY"
env_set countersigner BASE_RPC "$RPC"
env_set countersigner CONTROL_TOKEN "$CONTROL_TOKEN"

env_set seller SELLER_RECEIVER "$SELLER_RECEIVER"
env_set seller SELLER_AUTHORIZER_KEY "$SELLER_AUTHORIZER_KEY"
env_set seller SELLER_WITHDRAW_DELAY "$WITHDRAW_DELAY"
env_set seller BASE_RPC "$RPC"
env_set seller CHAIN_ID 8453
[ -n "$(env_get seller SELLER_ADMIN_TOKEN)" ] || env_set seller SELLER_ADMIN_TOKEN "$(openssl rand -hex 16)"

env_set agent AGENT_PRIVATE_KEY "$AGENT_KEY"
env_set agent COUNTERSIGN_WALLET "$WALLET"
env_set agent RECEIVER_AUTHORIZER "$SELLER_AUTHORIZER"
env_set agent BASE_RPC "$RPC"
env_set agent CHAIN_ID 8453

env_set scripts COUNTERSIGN_WALLET "$WALLET"
env_set scripts COUNTERSIGN_COLLECTOR "$COLLECTOR"
env_set scripts CHANNEL_ID "$CHANNEL_ID"

log ""
log "done. countersigner/.env, seller/.env, agent/.env and scripts/.env now hold this deployment."
log "the new keys exist ONLY in those files: back them up before you delete anything."
