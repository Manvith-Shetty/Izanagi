# Countersign

**An escrow wallet for AI agents where a payment can still be stopped *after* it has been made.**

Every agent-payment guard available today screens *before* the agent signs — because in
`exact`/EIP-3009 and ERC-4337 systems, signing **is** spending. Once the signature exists,
the decision is final.

Countersign is built on x402's `batch-settlement` scheme, where it isn't. The agent signs
vouchers continuously off-chain; the seller only cashes them in at the end; and Coinbase's
deployed escrow asks our contract *"is this valid?"* **at claim time**.

So a seller that turns malicious forty minutes after your agent already paid — but before
settlement — still doesn't get the money.

```
agent signs ────────── 40 minutes of paid API calls ────────── seller claims
     │                                                              │
  everyone else                                               Countersign
  checks here                                                 checks here
  (already committed)                                         (can still say no)
```

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
| **Recover funds** | ✗ | ✗ | ✓ | ✓ after 15 min |

Neither key alone is dangerous. If the oracle disappears forever, the owner still recovers
the escrow unilaterally via `initiateWithdraw`/`finalizeWithdraw` — verified in the tests.

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
contracts:  24 passed, 0 failed     (Base, Optimism, Arbitrum forks)
rust:       73 passed, 0 failed
seller e2e:  1 passed, 0 failed     (Base fork, real escrow, over HTTP)
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
the oracle absent · direct claim by the seller · claim-before-revoke is final.

Plus:
- **Cross-language parity** — signatures produced by the Rust countersigner are accepted by
  the deployed escrow, and become unclaimable once revoked ([`RustParity.t.sol`](contracts/test/RustParity.t.sol)).
- **Real deployment path** — deployed through the canonical CREATE2 deployer, not cheatcodes.
- **A real mainnet seller** — `0xdF1b…43A9`, which receives 354 of the 379 live channels on Base.

### Measured numbers

| | |
|---|---|
| `claimWithSignature` through EIP-1271 | **164,645 gas** (~$0.0027 on Base) |
| `revoke` — the kill switch | ~30,000 gas (~$0.0005) |
| Whole demo, on mainnet | **~$0.0065** |
| Live x402 channels on Base (14 days) | 379, across 61 payer wallets |
| Of those, guarded by a single hot key | **379 / 379** |
| Using EIP-1271 policy | **0 / 379** |

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
scripts/        fork-setup.sh: deploy Countersign on a Base fork and open a channel
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
   (`ATTESTATION_TTL`, 120s). A seller that waits longer than that after the agent's last
   request loses the claim. Our seller reads the expiry from the signature and claims
   `CLAIM_MARGIN_SECS` before it; a stock x402 seller would not. So the kill switch has a
   real cost for sellers, and it's something to negotiate, not hide: a longer TTL means a
   longer revocation window.
4. **The oracle can grief by refusing to sign.** Bounded: the owner recovers unilaterally
   after `withdrawDelay` (15 minutes minimum).
## License

MIT
