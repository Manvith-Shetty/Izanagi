#!/usr/bin/env bash
# Prepare a production deployment of Tab on Base mainnet, for Railway.
#
# In scripts/.env (gitignored), set:
#   TAB_TREASURY_KEY=0x...   a NEW wallet that has never been pasted anywhere. It deploys and owns
#                            each person's wallet and funds their trial, so it needs, on Base:
#                            ~0.003 ETH and TRIAL_AMOUNT x TRIAL_MAX_ACCOUNTS USDC (0.25 x 40 = 10).
#   TAB_URL=https://...      your Tab service's Railway domain (or leave for later)
#   SHOP_URL=https://...     your demo shop's Railway domain (or leave for later)
#
#   scripts/railway-setup.sh        (shows the plan, asks before sending anything)
#
# It generates every other key, writes them straight into three files -- one per Railway
# service, ready to paste into that service's "Raw Editor" -- deploys the fixed deposit
# collector, and sends each key the gas it needs. Keys are never printed.
#
#   scripts/.env.railway.countersigner
#   scripts/.env.railway.tab
#   scripts/.env.railway.seller
set -euo pipefail
source "$(dirname "$0")/lib.sh"

ASSUME_YES=false
[ "${1:-}" = "--yes" ] && ASSUME_YES=true

TREASURY_KEY=$(env_get scripts TAB_TREASURY_KEY)
[ -n "$TREASURY_KEY" ] || die "put a NEW funded wallet's key in scripts/.env as TAB_TREASURY_KEY=0x..."
RPC=$(env_get scripts RPC); RPC=${RPC:-https://mainnet.base.org}
TAB_URL=$(env_get scripts TAB_URL); TAB_URL=${TAB_URL:-https://REPLACE-with-your-tab-domain}
SHOP_URL=$(env_get scripts SHOP_URL); SHOP_URL=${SHOP_URL:-https://REPLACE-with-your-shop-domain}
TRIAL_AMOUNT=${TRIAL_AMOUNT:-250000}
TRIAL_MAX_ACCOUNTS=${TRIAL_MAX_ACCOUNTS:-40}

ESCROW=0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003
USDC=0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913
TREASURY=$(addr "$TREASURY_KEY")
OUT="$ENV_ROOT/scripts"

# the old, exposed wallet must never be reused
[ "$TREASURY" != "0x3AaAe578f1F6bBE9705363DE4354d64d6a09C8B7" ] || die "that key was exposed; use a new wallet"

# ---- preflight ----
[ "$(cast chain-id --rpc-url "$RPC")" = 8453 ] || die "$RPC is not Base mainnet"
[ "$(cast code $ESCROW --rpc-url "$RPC")" != 0x ] || die "the escrow has no code on $RPC"
ETH=$(cast balance "$TREASURY" --rpc-url "$RPC")
USDC_HAVE=$(cast call $USDC 'balanceOf(address)(uint256)' "$TREASURY" --rpc-url "$RPC" | cut -d' ' -f1)
USDC_NEED=$((TRIAL_AMOUNT * TRIAL_MAX_ACCOUNTS))
[ "$ETH" -ge 1500000000000000 ] || die "treasury $TREASURY has $(cast from-wei "$ETH") ETH; needs at least 0.0015 (0.003 recommended)"
[ "$USDC_HAVE" -ge "$TRIAL_AMOUNT" ] || die "treasury $TREASURY has no USDC for trials"
[ "$USDC_HAVE" -ge "$USDC_NEED" ] || log "note: treasury holds $USDC_HAVE atomic USDC, enough for $((USDC_HAVE / TRIAL_AMOUNT)) of $TRIAL_MAX_ACCOUNTS trials"

cat >&2 <<PLAN

  Base mainnet via $RPC
  treasury   $TREASURY   $(cast from-wei "$ETH") ETH, $USDC_HAVE atomic USDC
  Tab        $TAB_URL
  demo shop  $SHOP_URL

  1. generate the agent, oracle and shop keys, and write all three service files
  2. deploy the fixed CountersignCollector (only the escrow may use it)
  3. send gas: 0.0005 ETH to the agent (opens tabs), 0.0003 to the oracle (stops them),
     0.0003 to the shop's claim key

PLAN
if ! $ASSUME_YES; then
  read -r -p "  This spends real funds. Type yes to continue: " answer
  [ "$answer" = yes ] || die "stopped, nothing was sent"
fi

# ---- 1. keys, written down before anything is sent ----
AGENT_KEY=$(new_key); ORACLE_KEY=$(new_key); SHOP_KEY=$(new_key)
CONTROL_TOKEN=$(openssl rand -hex 24); SHOP_ADMIN=$(openssl rand -hex 16)
AGENT=$(addr "$AGENT_KEY"); ORACLE=$(addr "$ORACLE_KEY"); SHOP=$(addr "$SHOP_KEY")
INTERCEPTA=$(env_get countersigner INTERCEPTA_API_KEY)
[ -n "$INTERCEPTA" ] || die "INTERCEPTA_API_KEY is empty in countersigner/.env"

write() { # write <file> ; content on stdin
  umask 077
  cat > "$OUT/$1"
}

write .env.railway.countersigner <<EOF
# Railway service "countersigner" (private networking only: generate no public domain).
ORACLE_PRIVATE_KEY=$ORACLE_KEY
CONTROL_TOKEN=$CONTROL_TOKEN
INTERCEPTA_API_KEY=$INTERCEPTA
WORLD_CLIENT_ID=$(env_get countersigner WORLD_CLIENT_ID)
WORLD_CLIENT_SECRET=$(env_get countersigner WORLD_CLIENT_SECRET)
WORLD_ISSUER=https://sandbox.auth.world.org
WORLD_REQUIRE_ORB=true
BASE_RPC=$RPC
BIND=[::]:8787
STATE_DIR=/data
APPROVAL_PAGE_URL=$TAB_URL
ATTESTATION_TTL=2592000
AUTONOMOUS_LIMIT=50000
APPROVAL_STEP=50000
HUMAN_LIMIT=5000000
RESCREEN_SECS=300
NTFY_TOPIC=
EOF

write .env.railway.tab <<EOF
# Railway service "tab" (public: generate a domain with target port 3000).
CONTROL_TOKEN=$CONTROL_TOKEN
AGENT_PRIVATE_KEY=$AGENT_KEY
TAB_TREASURY_KEY=$TREASURY_KEY
COUNTERSIGN_COLLECTOR=PENDING
COUNTERSIGNER_URL=http://countersigner.railway.internal:8787
BASE_RPC=$RPC
CENSUS_RPC=$RPC
CHAIN_ID=8453
TAB_BIND=[::]:3000
TAB_PUBLIC_URL=$TAB_URL
DEMO_SHOP_URL=$SHOP_URL
TAB_TRIAL_AMOUNT=$TRIAL_AMOUNT
TAB_TRIAL_MAX_ACCOUNTS=$TRIAL_MAX_ACCOUNTS
TAB_DEPOSIT=50000
TAB_MAX_PRICE=100000
EOF

write .env.railway.seller <<EOF
# Railway service "seller" (public: generate a domain with target port 8080).
SELLER_RECEIVER=$TREASURY
SELLER_AUTHORIZER_KEY=$SHOP_KEY
SELLER_ADMIN_TOKEN=$SHOP_ADMIN
INTERCEPTA_API_KEY=$INTERCEPTA
BASE_RPC=$RPC
CHAIN_ID=8453
SELLER_BIND=[::]:8080
SELLER_PUBLIC_URL=$SHOP_URL
SELLER_PRICE=1000
SELLER_ROGUE_AFTER=3
SELLER_WITHDRAW_DELAY=900
EOF
log "wrote scripts/.env.railway.{countersigner,tab,seller} (gitignored, mode 600)"

# ---- 2. the fixed collector ----
cd "$(dirname "$0")/../contracts"
forge build --silent
NONCE=$(cast nonce "$TREASURY" --rpc-url "$RPC" --block pending)
out=$(forge create --rpc-url "$RPC" --private-key "$TREASURY_KEY" --nonce "$NONCE" --broadcast --json \
  src/Countersign.sol:CountersignCollector --constructor-args $ESCROW 2>&1) || true
COLLECTOR=$(echo "$out" | sed -n '/^{/,$p' | jq -r '.deployedTo // empty' 2>/dev/null || true)
[ -n "$COLLECTOR" ] || die "deploying the collector failed: $out"
NONCE=$((NONCE + 1))
[ "$(cast call "$COLLECTOR" 'escrow()(address)' --rpc-url "$RPC")" = "$ESCROW" ] || die "the collector does not guard the escrow"
sed -i.bak "s|^COUNTERSIGN_COLLECTOR=PENDING$|COUNTERSIGN_COLLECTOR=$COLLECTOR|" "$OUT/.env.railway.tab" && rm -f "$OUT/.env.railway.tab.bak"
log "collector  https://basescan.org/address/$COLLECTOR"

# ---- 3. gas ----
fund() {
  cast send "$1" --value "$2" --nonce "$NONCE" --rpc-url "$RPC" --private-key "$TREASURY_KEY" --json >/dev/null \
    || die "sending gas to $1 failed"
  NONCE=$((NONCE + 1))
  log "gas        $2 to $1"
}
fund "$AGENT" 0.0005ether
fund "$ORACLE" 0.0003ether
fund "$SHOP" 0.0003ether

log ""
log "done. Paste each file into its Railway service's Variables > Raw Editor:"
log "  countersigner  <- scripts/.env.railway.countersigner"
log "  tab            <- scripts/.env.railway.tab"
log "  seller         <- scripts/.env.railway.seller"
log "the new keys exist only in those files: back them up."
