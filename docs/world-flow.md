# World ID for Agents — the human-approval flow

How a headless agent pulls a real person into a payment decision, and why the money
cannot move without them.

## Who holds what

```
┌──────────────────────┐   ┌────────────────────────────┐   ┌──────────────────────┐
│ AGENT                │   │ COUNTERSIGNER (backend)    │   │ WORLD  (sandbox IdP) │
│                      │   │                            │   │                      │
│ • its own hot key    │   │ • World client id + SECRET  │   │ • the human's account│
│                      │   │ • Intercepta API key        │   │ • signs id_tokens    │
│ holds NO:            │   │ • the countersigning key    │   │ • publishes JWKS     │
│  ✗ client secret     │   │ • device codes              │   │                      │
│  ✗ device code       │   │ • approval records          │   │                      │
│  ✗ human subject     │   │ • per-human budgets         │   │                      │
│  ✗ countersign key   │   │                            │   │                      │
└──────────────────────┘   └────────────────────────────┘   └──────────────────────┘
                                                                       ▲
                                                            ┌──────────┴──────────┐
                                                            │  THE HUMAN's phone  │
                                                            └─────────────────────┘
```

The agent can ask. Only the backend can decide. Neither can move money alone.

## The happy path

```
AGENT                        COUNTERSIGNER                      WORLD            HUMAN
  │                               │                               │                │
  │ 1  POST /v1/countersign       │                               │                │
  │    {channel, chainId,         │                               │                │
  │     ceiling: 500 USDC}        │                               │                │
  ├──────────────────────────────►│                               │                │
  │                               │                               │                │
  │                               │ 2  Intercepta: screen the     │                │
  │                               │    seller, token, price       │                │
  │                               │    verdict = ASK              │                │
  │                               │    (over the $20 autonomous   │                │
  │                               │     limit)                    │                │
  │                               │                               │                │
  │                               │ 3  POST /api/v1/device_       │                │
  │                               │      authorization            │                │
  │                               │    Basic auth, scope=openid   │                │
  │                               ├──────────────────────────────►│                │
  │                               │◄──────────────────────────────┤                │
  │                               │    device_code (SECRET)       │                │
  │                               │    user_code  RL65U-TZ5VS     │                │
  │                               │    verification_uri           │                │
  │                               │                               │                │
  │                               │ 4  bind an approval to THIS   │                │
  │                               │    payment:                   │                │
  │                               │      digest = voucherDigest(  │                │
  │                               │        channelId, ceiling)    │                │
  │                               │    store {device_code, digest}│                │
  │                               │                               │                │
  │ 5  202 Accepted               │                               │                │
  │◄──────────────────────────────┤                               │                │
  │    approvalId  apr_9f3c…      │   ← opaque handle             │                │
  │    userCode    RL65U-TZ5VS    │   ← for the human to read     │                │
  │    verificationUri            │                               │                │
  │    seller, ceiling            │   ← so the agent can show it  │                │
  │    (NO device_code)           │                               │                │
  │                               │                               │                │
  │ 6  prints the code            │                               │                │
  │                               │                               │                │
  │                               │                               │  7  opens URL, │
  │                               │                               │     taps       │
  │                               │                               │     Approve    │
  │                               │                               │◄───────────────┤
  │                               │                               │                │
  │ 8  POST /v1/approve/poll      │                               │                │
  │    {approvalId}               │                               │                │
  ├──────────────────────────────►│                               │                │
  │                               │    POST /api/v1/token         │                │
  │                               │    grant_type=device_code     │                │
  │                               ├──────────────────────────────►│                │
  │                               │◄──────────────────────────────┤                │
  │                               │    authorization_pending      │                │
  │◄──────────────────────────────┤                               │                │
  │    202 {status: pending}      │      ↺ every 5s               │                │
  │                               │                               │                │
  │ 9  POST /v1/approve/poll      │                               │                │
  ├──────────────────────────────►│──────────────────────────────►│                │
  │                               │◄──────────────────────────────┤                │
  │                               │    id_token (JWT, RS256)      │                │
  │                               │                               │                │
  │                               │ 10 VERIFY, in this backend:   │                │
  │                               │    • signature vs JWKS ───────┤ GET /jwks.json │
  │                               │    • issuer, audience         │                │
  │                               │    • auth_time fresh (<300s)  │                │
  │                               │    • acr = orb-v3             │                │
  │                               │    → pairwise sub  DBDSY…BZDQ │                │
  │                               │    store sub on the approval  │                │
  │                               │                               │                │
  │ 11 200 {status: approved,     │                               │                │
  │◄──────────────────────────────┤                               │                │
  │       orbVerified: true}      │   ← still NO subject          │                │
  │                               │                               │                │
  │ 12 POST /v1/countersign       │                               │                │
  │    {channel, chainId,         │                               │                │
  │     ceiling, approvalId}      │                               │                │
  ├──────────────────────────────►│                               │                │
  │                               │ 13 redeem the approval:       │                │
  │                               │    • exists, not denied       │                │
  │                               │    • not expired              │                │
  │                               │    • NOT already used         │                │
  │                               │    • digest matches EXACTLY   │                │
  │                               │    • sub within budget        │                │
  │                               │    → mark consumed            │                │
  │                               │    → countersign the ceiling  │                │
  │ 14 200 {countersignature}     │                               │                │
  │◄──────────────────────────────┤                               │                │
  │                               │                               │                │
  │ 15 agent signs the same digest, and now the voucher carries TWO signatures     │
  │                               │                               │                │
  ▼                                                                                │
┌────────────────────────────────────────────────────────────────────────────┐
│  Coinbase x402BatchSettlement escrow, Base mainnet                          │
│  claimWithSignature → isValidSignature(digest, blob) on Countersign.sol      │
│    agent sig ✓   oracle sig ✓   not expired ✓   seller not revoked ✓         │
│  → the seller is paid                                                       │
└────────────────────────────────────────────────────────────────────────────┘
```

## The unsuccessful paths

Every one of these ends the same way: **no countersignature exists, so the voucher has
only one signature, and the escrow rejects the claim.** Not our code refusing — a contract
we did not write and cannot override.

```
the human taps Deny        → World: access_denied      → approval marked denied
the human walks away       → World: expired_token      → approval marked denied
the code runs out          → expires_at passes         → approval marked denied
approval used twice        → AlreadyUsed               → "a human approves one
                                                          payment, not a budget"
approval moved to another  → WrongPayment              → "that approval was given
  seller or amount                                        for a different payment"
authentication too old     → auth_time > 300s          → StaleAuthentication
assurance too weak         → acr ≠ orb-v3              → InsufficientAssurance
World unreachable          → screening/identity down   → refuse (fail closed)
```

```
                    ┌──────────────────────────┐
  no approval  ───► │  no countersignature     │ ───► claimWithSignature
                    └──────────────────────────┘        reverts on mainnet
```

## The three design choices, and why

**1. Why a device grant rather than a login redirect.**
The agent is a headless process. It has no browser to redirect and nobody sitting at it.
RFC 8628 is the OAuth flow built precisely for that, and World's guide notes device grants
*"always require fresh proof and explicit approval"* — so freshness comes free rather than
being requested.

**2. Why the agent never receives the `device_code` or the subject.**
Both are credentials. If the agent held the subject it could carry one human's approval to
a different payment — approve $500 to a legitimate seller, spend it at a scammer. Binding
the approval to the voucher digest and keeping it server-side makes that structurally
impossible rather than merely discouraged.

**3. Why the budget is keyed on the pairwise `sub`.**
It is a private, per-application identifier for one person, stable across their devices and
agents. So a limit belongs to a *human*, not a wallet: three agents run by the same person
share one budget, and the third is stopped because *they* have spent it.

## What this does and does not prove

| Claim | True? |
|---|---|
| A ceremony was completed for **this payment**, at this moment | ✅ |
| The same person returns as the same `sub` (Sybil resistance) | ✅ in production |
| The credential is orb-issued to a unique human | ✅ in production (`acr: orb-v3`) |
| A **live biometric scan** happened at approval time | ❌ `amr: pop` is proof of possession |
| There is a real human behind the demo | ❌ the sandbox uses **fake identities** |

Say *"authenticated"*, not *"biometrically verified"*. The integration is real; on the
sandbox the identity is simulated, and World's own guide says non-production proof
behaviour *"is not evidence of production verification."*
