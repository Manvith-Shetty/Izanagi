#!/usr/bin/env bash
# Deploy Countersign on a local fork of Base mainnet and open a funded channel to the demo seller.
#
# Everything runs against Coinbase's REAL deployed x402 escrow and REAL Base USDC; only the
# chain is local. Start the fork first (mainnet.base.org: other public RPCs refuse the archive
# reads a fork makes once it is a few minutes old):
#
#   anvil --fork-url https://mainnet.base.org
#
# Then either write every value into the crates' own .env files (what docs/testing.md uses):
#
#   scripts/fork-setup.sh --write-env
#
# or print them as shell exports (what the seller's fork test reads):
#
#   scripts/fork-setup.sh > fork.env && source fork.env
#
# Keys are derived from fixed labels so reruns are reproducible, and each can be overridden.
# Never reuse these keys on a real chain: they are derivable by anyone who reads this file.
set -euo pipefail
source "$(dirname "$0")/lib.sh"

WRITE_ENV=false
[ "${1:-}" = "--write-env" ] && WRITE_ENV=true

RPC=${RPC:-http://127.0.0.1:8545}
ESCROW=0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003
USDC=0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913
DEPOSIT=${DEPOSIT:-20000000}          # 20 USDC in the channel
FUND=${FUND:-100000000}               # 100 USDC in the wallet
WITHDRAW_DELAY=900
SALT=0x0000000000000000000000000000000000000000000000000000000000000000
ZERO=0x0000000000000000000000000000000000000000

key() { cast keccak "countersign-fork-$1"; }

OWNER_KEY=${OWNER_KEY:-$(key owner)}
AGENT_KEY=${AGENT_KEY:-$(key agent)}
ORACLE_KEY=${ORACLE_KEY:-$(key oracle)}
SELLER_AUTHORIZER_KEY=${SELLER_AUTHORIZER_KEY:-$(key seller-authorizer)}
OWNER=$(addr "$OWNER_KEY"); AGENT=$(addr "$AGENT_KEY"); ORACLE=$(addr "$ORACLE_KEY")
SELLER_AUTHORIZER=$(addr "$SELLER_AUTHORIZER_KEY")
SELLER_RECEIVER=${SELLER_RECEIVER:-$(addr "$(key seller-receiver)")}
CONTROL_TOKEN=${CONTROL_TOKEN:-$(openssl rand -hex 16)}

[ "$(cast chain-id --rpc-url "$RPC")" = 8453 ] || die "not a Base fork at $RPC"
[ "$(cast code $ESCROW --rpc-url "$RPC")" != 0x ] || die "escrow not found - is this a Base fork?"

# gas for everyone who sends transactions: the owner deploys, the seller claims, the oracle
# writes revocations to the wallet, and the agent opens tabs from the owner's allowance
for a in "$OWNER" "$SELLER_AUTHORIZER" "$ORACLE" "$AGENT"; do
  cast rpc anvil_setBalance "$a" 0xDE0B6B3A7640000 --rpc-url "$RPC" >/dev/null   # 1 ETH
done

log "deploying Countersign (owner $OWNER, agent $AGENT, oracle $ORACLE)..."
cd "$(dirname "$0")/../contracts"
forge build --silent
# --constructor-args is variadic, so it must come last. forge may print text before the JSON.
create() {
  local out addr
  out=$(forge create --rpc-url "$RPC" --private-key "$OWNER_KEY" --broadcast --json "$@" 2>&1) || true
  addr=$(echo "$out" | sed -n '/^{/,$p' | jq -r '.deployedTo // empty' 2>/dev/null || true)
  [ -n "$addr" ] || die "deploying $1 failed: $out"
  echo "$addr"
}
COLLECTOR=$(create src/Countersign.sol:CountersignCollector)
WALLET=$(create src/Countersign.sol:Countersign --constructor-args "$OWNER" "$ESCROW" "$AGENT" "$ORACLE")

# USDC (FiatTokenV2_2) keeps balances at storage slot 9
cast rpc anvil_setStorageAt "$USDC" "$(cast index address "$WALLET" 9)" "$(cast to-uint256 "$FUND")" --rpc-url "$RPC" >/dev/null
[ "$(cast call $USDC 'balanceOf(address)(uint256)' "$WALLET" --rpc-url "$RPC" | cut -d' ' -f1)" = "$FUND" ] \
  || die "funding the wallet failed"

send() { cast send "$@" --rpc-url "$RPC" --private-key "$OWNER_KEY" >/dev/null; }
CFG="($WALLET,$ZERO,$SELLER_RECEIVER,$SELLER_AUTHORIZER,$USDC,$WITHDRAW_DELAY,$SALT)"
# the owner's standing allowance: the collector may move up to FUND into this wallet's own
# channels, and nowhere else (it only ever pulls from the wallet into the escrow)
send "$WALLET" 'approveToken(address,address,uint256)' $USDC "$COLLECTOR" "$FUND"
send "$WALLET" 'openChannel((address,address,address,address,address,uint40,bytes32),uint128,address)' "$CFG" "$DEPOSIT" "$COLLECTOR"
CHANNEL_ID=$(cast call $ESCROW 'getChannelId((address,address,address,address,address,uint40,bytes32))(bytes32)' "$CFG" --rpc-url "$RPC")
log "channel $CHANNEL_ID holds $(cast call $ESCROW 'channels(bytes32)(uint128,uint128)' "$CHANNEL_ID" --rpc-url "$RPC" | head -1) of real USDC"

if $WRITE_ENV; then
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

  # not read by any service: the human's key and the deployment, for manual `cast` commands
  env_set scripts OWNER_KEY "$OWNER_KEY"
  env_set scripts RPC "$RPC"
  env_set scripts COUNTERSIGN_WALLET "$WALLET"
  env_set scripts COUNTERSIGN_COLLECTOR "$COLLECTOR"
  env_set scripts CHANNEL_ID "$CHANNEL_ID"

  log "wrote countersigner/.env, seller/.env, agent/.env and scripts/.env (other lines untouched)"
  exit 0
fi

cat <<ENV
export BASE_RPC=$RPC
export CHAIN_ID=8453
# owner: the human. Revokes and restores sellers, recovers funds.
export OWNER_KEY=$OWNER_KEY
export COUNTERSIGN_WALLET=$WALLET
export COUNTERSIGN_COLLECTOR=$COLLECTOR
export CHANNEL_ID=$CHANNEL_ID
# shared by the countersigner (operator API) and Tab
export CONTROL_TOKEN=$CONTROL_TOKEN
# agent/.env
export AGENT_PRIVATE_KEY=$AGENT_KEY
export RECEIVER_AUTHORIZER=$SELLER_AUTHORIZER
# countersigner/.env
export ORACLE_PRIVATE_KEY=$ORACLE_KEY
# seller/.env
export SELLER_RECEIVER=$SELLER_RECEIVER
export SELLER_AUTHORIZER_KEY=$SELLER_AUTHORIZER_KEY
ENV
