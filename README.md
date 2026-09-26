# Izanagi

**An escrow wallet for AI agents where a payment can still be stopped *after* it has been made.**

Every agent-payment guard available today screens *before* the agent signs — because in
`exact`/EIP-3009 and ERC-4337 systems, signing **is** spending. Once the signature exists,
the decision is final.

Izanagi is built on x402's `batch-settlement` scheme, where it isn't. The agent signs
vouchers continuously off-chain; the seller only cashes them in at the end; and Coinbase's
deployed escrow asks our contract *"is this valid?"* **at claim time**.

So a seller that turns malicious after your agent already paid — but before it cashes in —
still doesn't get the money. How long that window stays open is the seller's choice: busy
sellers on Base cash in every 1–5 minutes, quiet ones go days without claiming.

```
agent signs ──────── minutes to days of paid API calls ──────── seller claims
     │                                                              │
  everyone else                                               Izanagi
  checks here                                                 checks here
  (already committed)                                         (can still say no)
```

## Try it live

Everything runs on **Base mainnet** with real USDC, against Coinbase's deployed escrow.

| | |
|---|---|
| Website | **[izanagi-black.vercel.app](https://izanagi-black.vercel.app)** |
| Demo shop (goes rogue after 3 calls) | [`seller-production-18f0.up.railway.app/v1/data`](https://seller-production-18f0.up.railway.app/v1/data) |
| Backend health | [`tab-production-5655.up.railway.app/api/health`](https://tab-production-5655.up.railway.app/api/health) |

1. **Get your Tab**: verify you're a unique human with World ID (the event's sandbox, from a
   browser), then create your own Countersign wallet from MetaMask. It's yours from the first
   block: only your MetaMask can ever take money out.
2. **Add money** from MetaMask: a few USDC on Base.
3. **Connect your AI**: the dashboard gives you a personal MCP link. Add it to Claude, then ask
   *"Use Tab to get the latest Bitcoin price from hyperextend."*
4. **Watch it stop**: buy from the demo shop until it turns rogue, then **Close tab**. The shop
   can no longer collect for calls it already served.

---

## How it works

The agent's funds sit in **Coinbase's deployed `x402BatchSettlement` escrow**
(`0x4020074e9dF2ce1deE5A9C1b5c3f541D02a10003`, live on Base, Optimism and Arbitrum). We did
not write that contract, which is exactly why the guarantee is credible.

The channel's `payer` is a `Countersign` wallet and its `payerAuthorizer` is `address(0)`.
That combination makes the escrow validate **every voucher** through
`SignatureChecker.isValidSignatureNow(payer, ...)` — i.e. through our contract.

A voucher is claimable only if **all five** hold *when the seller cashes it in*:

| # | Check | Could a pre-signature guard do this? |
|---|---|---|
| 1 | The agent signed this exact voucher digest | yes |
| 2 | The risk oracle countersigned `(digest, seller, expiry)` | yes |
| 3 | The attestation has not expired | yes |
| 4 | **The seller has not been revoked** | **no** |
| 5 | **The wallet is not paused** | **no** |

Checks 4 and 5 read live storage during validation. That is the whole idea.

### Trust model

The countersigning key holds a **veto, never the funds**:

|  | agent key | oracle key | both | neither (timeout) |
|---|---|---|---|---|
| Move money | ✗ | ✗ | ✓ | ✗ |
| Redirect to another seller | ✗ | ✗ | ✓ | ✗ |
| Block a payment | ✗ | ✓ | ✓ | — |
| **Recover funds** | ✗ | ✗ | ✓ | ✓ after the withdraw delay |

Neither key alone is dangerous. If the oracle disappears forever, the owner still recovers
the escrow unilaterally via `initiateWithdraw`/`finalizeWithdraw` — verified in the tests.
The wait is the channel's `withdrawDelay`, which the seller sets: 15 minutes at the minimum,
a day for the real sellers we paid.

### Your money, your keys

You create your wallet yourself: your MetaMask sends the transaction that deploys it, with your
account as `owner` from the first block. The operator holds no key that owns or funds anyone's
wallet, so there is nothing to hand over and nothing of yours on its servers to steal.

Before building that transaction, the server asks your MetaMask to sign one message naming your
signup, and afterwards it accepts only a deployment sent by that same account, of exactly our
contract wired to our agent, oracle and collector. So nobody can claim a wallet somebody else just
created, and your World ID is tied only to the wallet your own account made.

The agent opens tabs through the wallet's own `openTab`, which approves the deposit collector for
exactly that deposit and resets it to zero. The collector itself only moves funds into a channel
the wallet gates (`payerAuthorizer == 0`). Without that check, anyone could open a channel funded
by a standing allowance, name themselves its authorizer, and claim it: we found that on a fork of
the real escrow, and [`TabFlow.t.sol`](contracts/test/TabFlow.t.sol) keeps it closed.

---

## Architecture

```
  you: browser + MetaMask ──────────► website (Vercel · tab/web)
     │                                    │  /api, proxied: one origin, first-party login cookie
     │ creates, owns and funds            ▼
     │ your own wallet               tab (Railway) ◄──── MCP ──── your AI (Claude)
     │                                    │  builds your transactions; the agent key opens
     │                                    │  tabs and signs vouchers
     │                                    ▼
     │                          countersigner (Railway, private network only)
     │                            Intercepta screening · World ID · the oracle key ·
     │                            re-screens open tabs and revokes on chain
     ▼                                    │
  Countersign wallet (Base) ◄─────────────┘  revoke / pause
     │  payer of every tab; EIP-1271 gate at claim time
     ▼
  Coinbase x402 escrow (Base) ◄──── claims ──── sellers: hyperextend, onesource, our demo shop
```

| Piece | Runs on | Holds | Can it move your money? |
|---|---|---|---|
| Your Countersign wallet | Base | nothing: your MetaMask owns it from the block that created it | only with both signatures, or when you withdraw |
| Website | Vercel | no keys | no |
| `tab` | Railway | the agent key | no: every voucher also needs the oracle |
| `countersigner` | Railway, private | the oracle key, Intercepta and World secrets | no: it can only refuse, or revoke |
| `seller` (demo shop) | Railway | its claim key | only cashes in vouchers your wallet accepts |
| Deposit collector | Base | no keys | only into a tab your own wallet gates |

**Signing up**: World ID proves you are a unique human → your MetaMask signs a message naming
this signup (free) → your MetaMask sends the transaction that creates your wallet → Tab checks
on chain that exactly that account created exactly our contract, and the countersigner ties the
wallet to your World ID. **Paying**: your AI asks `tab` → the countersigner screens the seller
and the voucher, and asks you through World ID above your limit → the agent opens a tab from your
wallet if needed and pays with a countersigned voucher → the seller cashes in later, through the
escrow, which asks your wallet.

---

## Sponsor integrations

### Intercepta — screening *is* the signature

The verdict isn't advice the agent may follow; without it **no claimable voucher exists**.
Three endpoints, each used where it belongs:

| What | Endpoint | Code |
|---|---|---|
| Counterparty (`payTo`) | `GET /account/{addr}/quick-scan` | [`intercepta.rs`](common/src/intercepta.rs) → `scan_address` |
| Token (real USDC vs lookalike) | `GET /token-intelligence/token/{addr}/risks` | `scan_token` |
| The payment authorization itself | `POST /analysis/signature` | `scan_message` |

Their brief says a verdict should let the flow *"pay, refuse, cap the amount or ask a
human"*, so those are **literally the four outcomes** —
[`policy.rs`](countersigner/src/policy.rs) → `Verdict`.

And because we screen at settlement rather than at signing, we consume the part of
Intercepta's product everyone else discards: a score that *keeps moving*. The
[re-screening loop](countersigner/src/state.rs) polls every open session, and when a score
crosses the threshold it revokes — killing vouchers that were already signed.

There is **no mock mode**. Without an API key the engine fails **closed**: every payment is
refused. A guard that silently allows traffic when its data source is down is not a guard.

#### The paid side: the seller screens its payer

A `batch-settlement` seller serves first and is paid later, so **every request extends
credit**. The [demo seller](seller/src/screen.rs) screens each payer with the same
`quick-scan` and turns the score into a credit line, not a yes/no:

| Payer toxicScore | Served? | Unclaimed value carried |
|---|---|---|
| below `PAYER_CAREFUL_AT` (30) | yes | up to `CLAIM_MAX_UNCLAIMED` (1 USDC) |
| below `PAYER_REFUSE_AT` (60) | yes | none — claimed after every request |
| at or above | **no** (403) | — |

Before serving, it asks the payer's wallet the question the escrow will ask at claim time
(`isValidSignature` through EIP-1271). So the seller also sees revocation from its side.
Once it has been revoked, the next voucher is refused at the door, and the vouchers it
already holds come back from the escrow as unclaimable, with the cause read from the
wallet's own registry:

```
CLAIM REJECTED by the escrow: 0.020000 USDC we served for is unclaimable.
the payer's Countersign wallet revoked this seller at 1790424567 (reason: wallet_drainer)
- after we had served the requests
```

### World — the human gate

> Full sequence diagram, trust boundaries and every unsuccessful path:
> **[docs/world-flow.md](docs/world-flow.md)**

When the verdict is `ask`, no signature exists until a real person authenticates.

Built on the official [`openidconnect`](https://crates.io/crates/openidconnect) crate against
World's OIDC provider. Discovery is live and advertises everything the flow needs:

```
device_authorization_endpoint  .../api/v1/device_authorization
grant_types_supported          [authorization_code, urn:ietf:params:oauth:grant-type:device_code]
subject_types_supported        [pairwise]
id_token_signing_alg_values    [RS256]
prompt_values_supported        [none, login]
acr_values_supported           [https://world.org/oidc/acr/orb-v3]
```

- **Device Authorization Grant (RFC 8628)** — the OAuth flow built for clients with no
  browser. The headless agent prints a short code; the human approves on their phone.
- **Fresh proof by construction.** World's OIDC guide states the device grant accepts
  `scope=openid` only — `nonce`, `max_age`, `prompt` and `acr_values` are ignored — because
  *"fresh proof is already required"*. So we send none of them, assert no nonce, and add an
  `auth_time` check (300s) as a second belt rather than the primary control.
- **Assurance without false negatives.** `acr` is enforced against the orb credential when
  the IdP asserts one, and an absent claim is not treated as a failure — the grant itself
  guarantees the proof.
- **The budget belongs to the person, not the wallet.** Keyed by the pairwise `sub`, so one
  human running three agents shares one limit.
- **Five distinct denied paths** — `access_denied`, `expired_token`, `cancelled`,
  `stale_authentication`, `insufficient_assurance` — each producing no countersignature, so
  **the on-chain claim reverts**.
- **One approval authorises one payment.** The approval is bound to the voucher digest,
  which commits to the channel — and therefore the payer, seller and token — plus the exact
  ceiling. So an approval cannot be moved to a different seller, a different amount, or a
  second payment. A human approves *this* transaction, not a budget.
- **Backend-only validation.** The `id_token` signature is verified against the issuer's
  JWKS; the client secret never leaves the countersigner. The agent receives an opaque
  approval handle and a yes/no — never the OAuth `device_code`, and never the subject.

---

## Validation

All tests run against the **real deployed escrow on real mainnet forks** with real USDC.

```
contracts:  40 passed, 0 failed     (Base, Optimism, Arbitrum forks)
rust:      137 passed, 0 failed
seller e2e:  1 passed, 0 failed     (Base fork, real escrow, over HTTP)
tab e2e:     1 passed, 0 failed     (Base fork: your account creates the wallet → tabs → every cent back)
mainnet:     Tab's own code on Base, real USDC, a real seller (below)
```

The seller's end-to-end test drives its real HTTP router and claim loop against a Base fork
with a freshly deployed Countersign wallet. It serves two paid requests, gets a corrective
402 for a skipped cumulative amount, and refuses a voucher with no countersignature. Then the
owner revokes the seller on chain, and the claim for the requests already served is
**rejected by Coinbase's escrow**. After a restore, the same vouchers claim and settle to the
seller's wallet ([`server.rs`](seller/src/server.rs) → `fork_serve_revoke_reject_restore_claim`).

Covered: happy path · agent-key compromise · wrong oracle key · missing oracle signature ·
expired attestation · attestation bound to a different seller · replay · over-ceiling ·
over-balance · revocation before signing · **revocation after signing** · un-revoke ·
agent cannot revoke · agent cannot withdraw · oracle cannot withdraw · owner recovery with
the oracle absent · direct claim by the seller · claim-before-revoke is final · a stranger
routing the wallet's allowance into a channel they authorise · the agent opening an ungated
or long-locked channel · no allowance left standing after a tab · a wallet that works from the
transaction creating it · a stranger claiming someone else's new wallet · a wallet wired to
other keys passed off as ours · ownership moving only when its owner moves it.

**On Base mainnet, through Tab's own code** ([`mainnet_wallet.rs`](tab/tests/mainnet_wallet.rs), Sep 27, 2026):

| Step | Transaction |
|---|---|
| A person's own account creates their wallet ([`0x4881…601b`](https://basescan.org/address/0x48818697fc650875dae7442648e0891fabe8601b)), and Tab's check accepts it | [`0x7814…b85e`](https://basescan.org/tx/0x78145568835bdcd08fc8006b74839218ea29f8adda6be3eb10b399a20cc85b8e) |
| They add 0.02 USDC | [`0xdffa…5d28`](https://basescan.org/tx/0xdffaf3cb427fbd4ebf639837c4fdeb918cdcd7974e2bb0e431cc28f5c0935d28) |
| Tab buys a real BTC candle from hyperextend: screened by Intercepta, countersigned, the tab opened through `openTab`, no allowance left | [`0xc2b0…a86d`](https://basescan.org/tx/0xc2b0aa6554de6e294c57b18c1fe1882de4a30a78faeecca40693bab5737ba86d) |
| Closing the tab revokes hyperextend on the wallet | [`0x3103…1994`](https://basescan.org/tx/0x310343a3cc51ceb658faf8172739b59fcdbee605d011477b300d9aba4dba1994) |

hyperextend served the data and holds a signed voucher for it, but claimed nothing before the
revocation, so it now never can: the tab's whole 0.005 USDC goes back to its owner. The run also
found two bugs, both fixed: a rate-limited RPC used to cancel a revocation (every service now
retries with backoff), and the escrow counts the withdraw delay from when a withdrawal starts,
not from the time it reports.

Plus:
- **Cross-language parity** — signatures produced by the Rust countersigner are accepted by
  the deployed escrow, and become unclaimable once revoked ([`RustParity.t.sol`](contracts/test/RustParity.t.sol)).
- **Real deployment path** — deployed through the canonical CREATE2 deployer, not cheatcodes.
- **A real mainnet seller** — `0xdF1b…43A9`, which received 354 of 379 live channels on Base
  when we measured.
- **Real sellers, real money, on mainnet** — a Countersign wallet
  (`0x81E0FAC8aA64cE0E95Ec43337568c0744aB0C70b`) paid two third-party x402 sellers on Base
  mainnet: hyperextend (Bitcoin 1-minute candles, 0.002 USDC a call) and onesource (the
  Ethereum block number, 0.001 USDC). Neither seller changed anything to accept an EIP-1271
  payer. [`RealSeller.t.sol`](contracts/test/RealSeller.t.sol) replays both on a fork with
  their live channel terms, including a revocation after signing.

### Measured numbers

| | |
|---|---|
| `claimWithSignature` through EIP-1271 | **164,645 gas** (~$0.0027 on Base) |
| `revoke` — the kill switch | ~30,000 gas (~$0.0005) |
| Whole demo, on mainnet | **~$0.0065** |

The escrow on Base, from its logs and state (measured Sep 26, 2026; the website shows the live
figures):

| | |
|---|---|
| Channels ever opened | **1,724**, from 201 payers to 75 sellers |
| Last 14 days | 399 channels, 66 payers |
| Claims in the last 7 days | 4,434, worth $15.18 |
| How often active sellers cash in | every **1–5 minutes** (median); hyperextend and onesource: not once in 7 days |
| Payers that are a policy contract | **none**: one channel ever left `payerAuthorizer` empty, and its payer is a plain key |

---

## Layout

```
contracts/      Countersign.sol — EIP-1271 payer + settlement-time revocation registry
common/         EIP-712 types byte-identical to the escrow, x402 v2 wire types,
                escrow bindings, the Intercepta client, shared utils
countersigner/  the brain: World OIDC, policy, budgets, the signing key, the watcher
agent/          demo agent: profiles the Bazaar, pays x402 sellers through the countersigner
seller/         x402 batch-settlement seller: screens payers, verifies vouchers via
                EIP-1271, cashes them in through the escrow
scripts/        fork-setup.sh: deploy Countersign on a Base fork and open a channel;
                railway-setup.sh: generate and fund the production config
tab/            the Izanagi server: API, MCP endpoint, sign-up, the tabs each person's AI opens
tab/web/        the website (Vercel): sign-up, dashboard, World ID approvals, MetaMask
```

The seller and agent speak x402 v2 as specified in
[`scheme_batch_settlement_evm.md`](https://github.com/x402-foundation/x402/blob/main/specs/schemes/batch-settlement/scheme_batch_settlement_evm.md):
terms in `PAYMENT-REQUIRED`, cumulative vouchers in `PAYMENT-SIGNATURE`, receipts in
`PAYMENT-RESPONSE`, and the spec's error codes, including the corrective 402.

### Configuration

Every crate reads its environment in exactly one file — `src/env.rs` — through
`common::utils::get_from_env_unsafe`, and each ships its own `.env.example`. There is no
root-level env file: each binary loads its own crate's `.env` regardless of where `cargo`
was invoked from, so the per-crate files are the single source of truth.

| File | Holds |
|---|---|
| [`countersigner/.env.example`](countersigner/.env.example) | every secret and every threshold |
| [`agent/.env.example`](agent/.env.example) | one key, and no authority |
| [`seller/.env.example`](seller/.env.example) | the x402 endpoint, its payer thresholds and claim policy |
| [`tab/.env.example`](tab/.env.example) | the agent key, where the countersigner and the website are; no key that owns or funds a wallet |
| [`scripts/.env.example`](scripts/.env.example) | your own keys for the setup scripts, and the production deploy's addresses |

A missing or malformed key names itself at startup rather than failing halfway through a
payment:

```
INFO common::utils: ORACLE_PRIVATE_KEY env not found with error: environment variable not found
Error: ORACLE_PRIVATE_KEY env not found with error: environment variable not found
```

Config structs holding secrets implement `Debug` by redaction, so the oracle key and the
World client secret cannot be logged by accident.

## Running it

On a local fork of Base, against the real escrow and real USDC:

```bash
anvil --fork-url https://mainnet.base.org             # terminal 1
source <(scripts/fork-setup.sh)                        # deploys Countersign, opens a 20 USDC channel
export INTERCEPTA_API_KEY=...                          # both sides screen; without it both refuse

cargo run -p countersigner --bin countersigner         # terminal 2 (+ WORLD_CLIENT_ID/SECRET for `ask`)
SELLER_ADMIN_TOKEN=demo cargo run -p seller            # terminal 3
cargo run -p agent -- fetch http://127.0.0.1:8080/v1/data 3

# the moment: revoke the seller after it has been paid, then watch its claim fail
cast send $COUNTERSIGN_WALLET 'revoke(address,uint32)' $SELLER_RECEIVER 4 --private-key $OWNER_KEY
curl -X POST -H 'Authorization: Bearer demo' http://127.0.0.1:8080/admin/claim
```

Tests:

```bash
cargo test --workspace                                        # unit + HTTP tests, no chain
source <(scripts/fork-setup.sh) && cargo test -p seller -- --ignored   # seller e2e on the fork
export BASE_RPC=https://mainnet.base.org && (cd contracts && forge test)
```

## Deploying

Backend on Railway (three services from one Dockerfile), website on Vercel, everything on Base
mainnet. The website proxies `/api` to the backend, so the login cookie stays first-party; MCP
links point straight at the backend. Step by step: **[docs/deploy.md](docs/deploy.md)**.

## Known limitations

Stated plainly, because they're real:

1. **Revocation binds only before the claim lands.** Once `claimWithSignature` succeeds the
   funds are committed and cannot be clawed back — proven in `test_D_Limitation_ClaimBeforeRevokeIsFinal`.
   Mitigated by short attestation TTLs (declining to re-issue is itself a revocation) and by
   Base's sequencer exposing no public mempool to front-run from.
2. **No API calls inside `isValidSignature`** — it's a static call, so every verdict must be
   pre-baked into a signature or a pre-flipped storage bit.
3. **Standard sellers don't expect vouchers to expire.** The spec says vouchers carry no
   expiry, but a Countersign voucher is only claimable until its attestation lapses
   (`ATTESTATION_TTL`, 120s by default). A seller that waits longer than that after the
   agent's last request loses the claim. Our seller reads the expiry from the signature and
   claims `CLAIM_MARGIN_SECS` before it; a stock x402 seller would not. Real sellers like
   hyperextend and onesource went a week without claiming, so the mainnet deployment sets
   `ATTESTATION_TTL` to 30 days: there, revocation is the brake, not expiry. The kill switch
   has a real cost for sellers, and it's something to negotiate, not hide.
4. **The oracle can grief by refusing to sign.** Bounded: the owner recovers unilaterally
   after `withdrawDelay` (15 minutes minimum, a day for the real sellers we paid).
5. **The phone-code World ID flow runs on World's sandbox only.** The device grant a headless
   agent needs is offered by `sandbox.auth.world.org`, the event's environment, but not by
   production `id.worldcoin.org`. Production would send the person an ordinary login link
   (authorization code + PKCE) instead.
6. **The operator runs both signing keys.** The agent key and the oracle key are two keys, but
   one operator holds both on its servers, so a breach of those servers could sign both halves
   of a voucher. Separate custody, and a spending limit enforced in the contract itself, are
   the fixes before real money at scale.

## License

MIT
