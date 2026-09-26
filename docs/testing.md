# Testing Countersign, end to end

Four levels, each more real than the last. Levels 1–3 cost nothing. Level 4 runs on Base
mainnet with real USDC, costing a few cents plus whatever you deposit (recoverable).

| Level | What runs | Chain | Needs |
|---|---|---|---|
| 1 | Unit + HTTP tests | none | Rust |
| 2 | Contract tests against Coinbase's deployed escrow | Base / OP / Arbitrum forks | Foundry |
| 3 | The whole stack: contract, countersigner, seller, agent, World, Intercepta | local fork of Base | Foundry, Intercepta key, World sandbox client |
| 4 | The same, for real | **Base mainnet** | a little ETH and USDC on Base |

> **Status.** This covers everything built so far: the contract, the countersigner (screening,
> World approvals, the on-chain kill switch, restore-with-a-human, the live journal), the
> seller and the CLI agent. The Tab product server (MCP + dashboard) is being built; its
> section will be added here when it lands.

---

## 0. Where configuration lives

Every value lives in a `.env` file next to the code that reads it. Nothing in this guide is
`export`ed.

| File | Read by | Holds |
|---|---|---|
| [`countersigner/.env`](../countersigner/.env.example) | the countersigner | oracle key, Intercepta key, World client, policy thresholds, `BASE_RPC`, `CONTROL_TOKEN` |
| [`seller/.env`](../seller/.env.example) | the demo seller | its receiver and claim key, Intercepta key, admin token |
| [`agent/.env`](../agent/.env.example) | the CLI agent | agent key, wallet address, the seller's authorizer |
| [`contracts/.env`](../contracts/.env.example) | `forge test` | `BASE_RPC` |
| [`scripts/.env`](../scripts/.env.example) | `mainnet-setup.sh`, `ops.sh` | **your** owner key and the deployment's addresses |

Each has a committed `.env.example` documenting every key; the `.env` files themselves are
gitignored. The setup scripts fill in what they generate (keys, addresses, tokens) and leave
every other line alone, so a secret you added by hand is never overwritten.

**Do not also `export` these values.** A service reads its `.env` only for keys that are not
already set in the environment, so an old `export BASE_RPC=...` silently wins over the file.
If you exported anything earlier, open a fresh terminal.

`scripts/ops.sh` reads the same files, so you never copy a token or address by hand:

```bash
scripts/ops.sh            # lists every command
```

---

## 1. One-time setup

```bash
curl -L https://foundry.paradigm.xyz | bash && foundryup      # forge, cast, anvil
brew install jq openssl

for c in countersigner seller agent contracts scripts; do
  [ -f $c/.env ] || cp $c/.env.example $c/.env
done
```

Then fill in by hand:

| File | Key | Value |
|---|---|---|
| `countersigner/.env` | `INTERCEPTA_API_KEY` | free key from https://intercepta.io/ethglobal |
| `seller/.env` | `INTERCEPTA_API_KEY` | the same key (the seller screens its payers) |
| `countersigner/.env` | `WORLD_CLIENT_ID`, `WORLD_CLIENT_SECRET` | a **confidential** client from https://sandbox.auth.world.org/portal (already set) |
| `countersigner/.env` | `NTFY_TOPIC` *(optional)* | an unguessable topic; subscribe to it in the ntfy phone app (already set) |
| `countersigner/.env` | `RESCREEN_SECS` | `60` (older copies say `15`, which burns the Intercepta quota) |

**Intercepta quota.** The key allows 1,000 calls. Each payment costs 2 (seller + token; the
token verdict is cached for an hour), and the watcher costs 1 per open seller per
`RESCREEN_SECS`. One open tab for an hour ≈ 60 calls.

---

## Level 1 — no chain

```bash
cargo test --workspace
```

Expect every suite green (100 tests). They cover: fail-closed screening, approvals bound to
one exact action and used once, the device code and OIDC subject never leaving the
countersigner, a closed tab getting no signature, the operator API not existing without
`CONTROL_TOKEN`, human bindings surviving a restart, the seller's x402 rules and spec error codes.

---

## Level 2 — contracts against the real escrow

`contracts/.env` already holds `BASE_RPC=https://mainnet.base.org`.

```bash
cd contracts && forge test -vv
```

Expect **27 passed**. The three `RealSellerForkTest` tests run the whole journey against the
**live channel parameters of two real, unaffiliated Bazaar sellers** (hyperextend and
onesource): pay → they claim real USDC → revoke after signing → their claim reverts →
restore → claimable again.

---

## Level 3 — the whole stack on a Base fork

Real Coinbase escrow, real Base USDC, real Intercepta, real World sandbox. Only the chain is
local.

### 3.1 Fork, deploy, write the .env files

```bash
# terminal 1 - leave running
anvil --fork-url https://mainnet.base.org --retries 10 --fork-retry-backoff 1000 --compute-units-per-second 100
```

The fork fetches any state it has not seen yet from `mainnet.base.org`, which throttles
bursts. The last three flags pace and retry those fetches; without them an occasional request
fails with `invalid_batch_settlement_evm_rpc_read_failed` and simply works on a retry.

Use `mainnet.base.org`. `base-rpc.publicnode.com` works for a minute and then refuses the
archive reads a fork makes, which shows up as `contract was not deployed`.

```bash
# terminal 2
scripts/fork-setup.sh --write-env
```

This deploys a Countersign wallet, funds it with 100 USDC, lets its collector use them, opens a
20 USDC tab to the demo seller, and writes:

| File | Keys written |
|---|---|
| `countersigner/.env` | `ORACLE_PRIVATE_KEY`, `BASE_RPC=http://127.0.0.1:8545`, `CONTROL_TOKEN` |
| `seller/.env` | `SELLER_RECEIVER`, `SELLER_AUTHORIZER_KEY`, `SELLER_WITHDRAW_DELAY`, `BASE_RPC`, `CHAIN_ID`, `SELLER_ADMIN_TOKEN` |
| `agent/.env` | `AGENT_PRIVATE_KEY`, `COUNTERSIGN_WALLET`, `RECEIVER_AUTHORIZER`, `BASE_RPC`, `CHAIN_ID` |
| `scripts/.env` | `OWNER_KEY`, `RPC`, `COUNTERSIGN_WALLET`, `COUNTERSIGN_COLLECTOR`, `CHANNEL_ID` |

Rerun it whenever you restart anvil: a fresh fork has none of these contracts.

### 3.2 Set the scenario knobs

In `countersigner/.env` (inline comments must not contain `'` or `"`: the `.env` parser treats
them as quotes. The services now refuse to start rather than silently drop keys):

```bash
ATTESTATION_TTL=1800     # long enough to reopen a tab within the attestation window
AUTONOMOUS_LIMIT=20000   # 0.02 USDC: the 3rd paid call will need a person
APPROVAL_STEP=20000      # a person raises a tabs limit in 0.02 steps
```

### 3.3 Start the services

```bash
# terminal 3
cargo run -p countersigner --bin countersigner

# terminal 4
cargo run -p seller

# terminal 5 - the countersigner's decisions, live
scripts/ops.sh stream
```

Check that everything is switched on:

```bash
scripts/ops.sh health     # want worldIdConfigured, onChainGuardian, operatorApi all true
```

### 3.4 Scenario A — a payment that goes through

```bash
cargo run -p agent -- fetch http://127.0.0.1:8080/v1/data 2
```

Expect `verdict pay`, the seller answering `200`, and `charged ... still cancellable until
claimed`. Terminal 5 shows `screened` and `countersigned`. Nothing is on chain yet: the
seller holds signed vouchers it has not cashed.

### 3.5 Scenario B — a payment that is blocked, with the reason

Take a known-risky address from Intercepta's Discord (they pin test addresses):

```bash
cargo run -p agent -- pay 0x<risky-address-from-intercepta> 10000
```

Expect `REFUSED:` with Intercepta's reason (for example `wallet_drainer`). No signature exists,
so there is nothing to claim. Terminal 5 shows `screened` with `verdict: refuse`.

### 3.6 Scenario C — a person raises the tab's limit (World ID)

```bash
cargo run -p agent -- fetch http://127.0.0.1:8080/v1/data 3
```

The first call of this run takes the tab to 0.03 USDC, over the 0.02 autonomous limit:

1. The agent prints `a human has to approve this one`, a **code** and a **link**. With ntfy
   set up, the same arrives on your phone with the code in the title.
2. Open the link and sign in to the World sandbox. Proofs are mocked, so no World App is
   needed. Approve.
3. Within one poll interval the agent continues with `approved by a human`, and the call is paid.
4. The next call (0.04) goes through **without asking**: you approved the tab up to 0.04.
5. The call after asks again. This time press **Deny**, or let it expire. Expect
   `NOT APPROVED: access_denied` (or `expired_token`) and `no voucher exists`.

Terminal 5 shows `approval_requested` → `human_bound` (first approval only) →
`approval_granted`, then later `approval_denied`.

**Wrong-human path (optional).** Approvals are bound to the wallet's first verified human.
Start another approval and approve it signed in with a *different* Google account. Expect
`approval_denied` with reason `wrong_human`, and no voucher.

### 3.7 Scenario D — stop a payment after it was made

The seller has served you and holds signed vouchers. Close the tab:

```bash
scripts/ops.sh close wallet_drainer      # the oracle writes `revoke` to the wallet: note the tx
scripts/ops.sh status                    # revoked true   reason code 4
scripts/ops.sh claim                     # the seller tries to cash in what it already served
```

Expect `"outcome": "rejected"`. The seller's log reads *"CLAIM REJECTED by the escrow ... the
payer's Countersign wallet revoked this seller ... after we had served the requests"*. That is
Coinbase's escrow refusing, not our code. Another `agent fetch` is turned away at the seller's
door, and the countersigner refuses to sign too.

### 3.8 Scenario E — reopen a tab (only a person can)

```bash
scripts/ops.sh reopen                    # prints a code and an approveAt link
```

Nothing is restored yet. Open the link and approve. Within a poll interval, terminal 5 shows
`approval_granted` and then `restored` with a tx. Then:

```bash
scripts/ops.sh status                    # revoked false
scripts/ops.sh claim                     # claimed: the same vouchers now settle to the seller
```

Stopping is instant and free. Starting again takes a verified person.

### 3.9 What this level cannot show on demand

The **watcher's automatic revocation** fires when a seller's live Intercepta score crosses
`REFUSE_AT` *after* you paid, which needs the real world to change a score. It runs the same
code as Scenario D (`App::revoke`, with `by: watcher`), so D proves the effect.

### 3.10 The seller's own fork test

This one still takes exported values, because it is a Rust test and not a service:

```bash
scripts/fork-setup.sh > /tmp/fork.env && (set -a && source /tmp/fork.env && cargo test -p seller -- --ignored)
```

It runs in a subshell, so nothing leaks into your terminal.

---

## Level 4 — Base mainnet, real money

Costs at today's gas (~0.006 gwei): deploy ≈ 2¢, each claim ≈ 0.3¢, each revoke ≈ 0.05¢.
Everything except gas, and what sellers legitimately earn, is recoverable.

### 4.1 Your key

In `scripts/.env`:

```bash
OWNER_KEY=0x...          # your wallet on Base: ~0.001 ETH and at least 2 USDC
# optional: FUND, CHANNEL_DEPOSIT, GAS_TOPUP, SELLER_RECEIVER (see scripts/.env.example)
```

This key becomes the wallet's **owner**: the only key that can open tabs and take money out.

### 4.2 Deploy

```bash
scripts/mainnet-setup.sh
```

It checks it is really Base, that the escrow is live and that you can afford it. It prints the
plan and sends nothing until you type `yes`. Then it:

1. generates **fresh** agent, oracle and seller-claim keys (never the fork's public ones)
2. deploys `CountersignCollector` and `Countersign(owner, escrow, agent, oracle)`
3. sends the oracle and the seller's claim key 0.0003 ETH each for gas
4. moves 2 USDC into the wallet and lets its collector use them
5. opens a 0.50 USDC tab to **your own** demo seller (earnings land back at your address)
6. writes the results into the files below, printing Basescan links as it goes

| File | Keys written |
|---|---|
| `countersigner/.env` | `ORACLE_PRIVATE_KEY`, `BASE_RPC=https://mainnet.base.org`, `CONTROL_TOKEN` |
| `seller/.env` | `SELLER_RECEIVER`, `SELLER_AUTHORIZER_KEY`, `SELLER_WITHDRAW_DELAY`, `BASE_RPC`, `CHAIN_ID`, `SELLER_ADMIN_TOKEN` |
| `agent/.env` | `AGENT_PRIVATE_KEY`, `COUNTERSIGN_WALLET`, `RECEIVER_AUTHORIZER`, `BASE_RPC`, `CHAIN_ID` |
| `scripts/.env` | `COUNTERSIGN_WALLET`, `COUNTERSIGN_COLLECTOR`, `CHANNEL_ID` |

**The new keys exist only in those files.** Back them up before you delete or re-run anything.

### 4.3 The kill-switch demo, for real, on your own seller

Keep the `ATTESTATION_TTL=1800` and limits from 3.2 in `countersigner/.env`, start the same
three terminals as 3.3 (no anvil), and run Scenarios A–E unchanged. Every `tx` is now on
[basescan.org](https://basescan.org): the `revoke`, the World-approved `restore`, and the claim
and settlement that follow. The rejected claim is only ever simulated, never sent, so it costs
nothing.

### 4.4 A real third-party seller

Paying a stranger from the live Bazaar, through a wallet that can still say no. hyperextend
charges 0.002 USDC a call; `ops.sh` reads its terms from its live 402 response:

```bash
scripts/ops.sh open https://api.hyperextend.xyz/v1/candles/BTC/1m/latest 50000    # a 0.05 USDC tab
```

In `countersigner/.env`, set `ATTESTATION_TTL=2592000` (30 days) and restart it. Then:

```bash
cargo run -p agent -- fetch https://api.hyperextend.xyz/v1/candles/BTC/1m/latest 1
scripts/ops.sh tab https://api.hyperextend.xyz/v1/candles/BTC/1m/latest
```

Two rules for real sellers:

- **Keep `ATTESTATION_TTL` longer than the seller's claim cadence.** hyperextend claimed zero
  times in the last 7 days; with a 120s TTL it could never be paid for what it served. A long
  TTL keeps your veto (a revoke still works until the claim lands) and keeps them whole.
- **Do not revoke an honest seller after it served you.** That withholds payment for work
  delivered. Demo the kill switch on your own seller (4.3).

Not yet verified: whether hyperextend's server accepts an EIP-1271 payer. If it answers with a
signature error, the escrow path is still proven by Level 2's `RealSellerForkTest` against its
exact parameters. Say so in the pitch.

### 4.5 Get your money back

```bash
scripts/ops.sh withdraw start 500000                  # your seller's tab; add a URL for another
# wait the tab's withdrawDelay: 15 min for your seller, 24 h for hyperextend
scripts/ops.sh withdraw finish
scripts/ops.sh sweep 2000000                          # wallet -> your owner address
```

No oracle is involved: if the countersigner vanished, this still works.

---

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| a value in a `.env` file seems ignored | an `export`ed variable of the same name wins | open a fresh terminal |
| `contract was not deployed` on the fork | the fork source refuses archive reads | `anvil --fork-url https://mainnet.base.org` |
| seller `503 ... rpc_read_failed`, then fine on retry | the fork's upstream throttled a burst of reads | retry; start anvil with the flags in 3.1 |
| every payment `refused ... failing closed` | no or invalid `INTERCEPTA_API_KEY` | set it in `countersigner/.env` **and** `seller/.env` |
| `a human is required, but World ID is not configured` | `WORLD_CLIENT_ID` unset | `countersigner/.env` |
| `close` returns a `chain_error` | the oracle has no ETH, or the wallet names another oracle | fund the oracle; `cast call $WALLET 'riskOracle()(address)'` |
| `401 bad control token` | the countersigner was started before `CONTROL_TOKEN` changed | restart the countersigner |
| `reopen` then `claim` → `attestation lapsed` | approved after `ATTESTATION_TTL` ran out | raise `ATTESTATION_TTL` |
| `approval_denied: wrong_human` | a different World account approved | expected: only the wallet's own human counts |
| `operator API disabled` | `CONTROL_TOKEN` empty in `countersigner/.env` | rerun a setup script, or set 16+ characters |
| after restarting anvil, everything fails | the fresh fork has no contracts | `scripts/fork-setup.sh --write-env` again |
