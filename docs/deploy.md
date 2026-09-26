# Deploying Izanagi on Base mainnet: Railway + Vercel

The backend runs on Railway: three services from one repo and one Dockerfile. The website runs
on Vercel and forwards `/api` to the backend, so to a browser everything is one site.

```
  browser ──► Vercel  (the website: tab/web)
                 │  /api/*  proxied, so the login cookie stays first-party
                 ▼
            Railway  tab ──────────► countersigner    (private network only)
                 ▲                    oracle key, World ID, Intercepta
  Claude ────────┘  /mcp/<token>, straight to Railway
                    seller (the demo shop), public
                        │
                        ▼
                 Base mainnet: Coinbase's x402 escrow, real USDC
```

| Service | Where | What it is | Public? | Port | Volume |
|---|---|---|---|---|---|
| website | Vercel | `tab/web`: landing, sign-up, dashboard, approval pages | yes | | |
| `tab` | Railway | the API and the MCP server; deploys and funds each person's wallet | yes | 3000 | `/data` |
| `countersigner` | Railway | risk screening, World ID, the kill switch; holds the oracle key | **no** | 8787 | `/data` |
| `seller` | Railway | the demo shop that goes bad on purpose | yes | 8080 | none |

The Railway service **must be named `countersigner`**: Tab reaches it at
`countersigner.railway.internal`.

Why the MCP link skips Vercel: an AI client holds its MCP connection open for as long as it
likes, and a proxy in between only adds a way for it to drop. The website's own calls are short,
and its live feed sends a keep-alive every 15 seconds, well inside Vercel's proxy limits.

---

## 1. Before you start

**Push the code to GitHub.** Railway and Vercel both deploy from the repository. Nothing secret
is in it: every `.env` file is gitignored, and `.dockerignore` keeps them out of the image too.

**Have a funder wallet with about 0.002 ETH on Base.** The setup script uses it once, on your
machine, to deploy the deposit collector and give the service keys their gas. Its key never goes
to Railway: Tab holds no key that owns or funds anyone's wallet. Each person creates their wallet
from their own MetaMask, owns it from the first block, and adds their own USDC.

Never reuse a key that has been pasted into a chat or a terminal log.

**Use a dedicated RPC.** `https://mainnet.base.org` rate-limits hard: it throttled a single
test run of ours to HTTP 429 more than once. Every service retries with backoff, but a crowd of
judges will still feel it. A free Alchemy or QuickNode Base endpoint is far steadier; put it in
`RPC` (step 3) and every service uses it.

**Deployed before?** Wallets made by an earlier version use the old contract: they cannot open
tabs the new way, and their collector allowance is exposed. Start from fresh `/data` volumes,
and have whoever owns each old wallet set its collector allowance to zero.

## 2. Railway: create the services and their domains

1. **New Project → Deploy from GitHub repo →** pick this repo. That first service becomes `tab`.
   Railway finds the `Dockerfile` at the root automatically.
2. **+ New → GitHub repo →** the same repo again, twice more. Rename the two new services to
   `countersigner` and `seller` (Settings → Service name).

For each service:

| | `countersigner` | `tab` | `seller` |
|---|---|---|---|
| Settings → Deploy → **Custom Start Command** | `/app/countersigner` | *(leave empty)* | `/app/seller` |
| Settings → Networking | no public domain | **Generate Domain**, port **3000** | **Generate Domain**, port **8080** |
| **+ Volume** (right-click the service) | mount path `/data` | mount path `/data` | none |
| Settings → Deploy → Healthcheck path | `/health` | `/api/health` | `/health` |

The first builds fail until the variables are in (step 4). That's expected.

**Pick the website's name now.** Vercel serves a project at `https://<project name>.vercel.app`.
Choose the name you'll give the Vercel project in step 5 (for example `izanagi-tab`), so every
link can be written once.

## 3. Generate the production config (one command, on your machine)

In `scripts/.env`:

```bash
FUNDER_KEY=0x...                                 # ~0.002 ETH on Base; stays on this machine
RPC=https://mainnet.base.org                     # or your dedicated endpoint
TAB_URL=https://<tab service>.up.railway.app     # from step 2
SHOP_URL=https://<seller service>.up.railway.app # from step 2
SITE_URL=https://<project name>.vercel.app       # the name you picked
```

Then:

```bash
scripts/railway-setup.sh
```

It shows the plan and waits for `yes`. Then it generates the agent, oracle and shop keys and
writes three files, **one per Railway service**. After that it deploys the deposit collector and
sends each key its gas. Keys are never printed.

```
scripts/.env.railway.countersigner
scripts/.env.railway.tab
scripts/.env.railway.seller
```

Back these three files up. The keys in them exist nowhere else.

## 4. Railway: variables

For each service: **Variables → Raw Editor**, paste the whole matching file, **Update Variables**.

| Service | Paste |
|---|---|
| `countersigner` | `scripts/.env.railway.countersigner` |
| `tab` | `scripts/.env.railway.tab` |
| `seller` | `scripts/.env.railway.seller` |

The URLs they must end up with, if you left any out in step 3:

| Service | Variable | Value |
|---|---|---|
| `tab` | `TAB_PUBLIC_URL` | the Vercel site: links and the login cookie belong to it |
| `tab` | `TAB_API_URL` | the `tab` service's own Railway domain: MCP links point here |
| `tab` | `DEMO_SHOP_URL` | the `seller` service's Railway domain |
| `countersigner` | `APPROVAL_PAGE_URL` | the Vercel site: World ID approval links open there |
| `seller` | `SELLER_PUBLIC_URL` | the `seller` service's Railway domain |

Railway redeploys each service when its variables change. Check the backend on its own:

```bash
curl https://<tab service>.up.railway.app/api/health      # status ok, countersigner ok, collector set
curl https://<seller service>.up.railway.app/health        # payerScreening true, rogueAfter 3
```

## 5. Vercel: the website

1. In [`tab/web/vercel.json`](../tab/web/vercel.json), replace both
   `REPLACE-WITH-TAB-BACKEND.up.railway.app` with the `tab` service's Railway domain. Commit
   and push.
2. **Vercel → Add New → Project →** import this repo.
   - **Project Name**: the name you picked in step 2
   - **Root Directory**: `tab/web`
   - Framework, build command (`npm run build`) and output (`dist`) come from `vercel.json`
   - no environment variables: the site holds no secrets, and finds the API through the proxy
3. **Deploy.**

If Vercel gave the project a different domain than you planned, set `TAB_PUBLIC_URL` (tab) and
`APPROVAL_PAGE_URL` (countersigner) on Railway to the real one.

## 6. Check it, end to end

```bash
curl https://<project name>.vercel.app/api/health     # the same answer as step 4: the proxy works
```

Then open `https://<project name>.vercel.app`:

You need MetaMask with a little ETH and a few USDC on Base.

1. **Get your Tab** → verify with World ID → **Connect MetaMask & create your wallet**: one free
   signature, then one transaction. The dashboard shows your MetaMask account as the owner.
2. **Your money → Add money**: $2 from MetaMask.
3. **Try it → Tab demo shop** four times: three real Bitcoin prices, then junk.
4. **Close tab** → the STOPPED stamp; the shop can no longer collect.
5. **Connect your AI**: copy the Claude Code command (it points at Railway), then ask Claude
   *"Use Tab to get the latest Bitcoin price from hyperextend."*
6. **Take money out** → back to your MetaMask (a tab's leftover comes back after the seller's
   withdraw delay).

## Good to know

- **Logs**: each Railway service's *Deployments → View logs*. Tab logs every signup and every
  wallet a person creates; the countersigner logs every decision. Vercel only serves files and
  the proxy.
- **Costs to watch**: only the service keys' gas. The agent pays for each new tab (one
  transaction), the oracle for each revocation. People pay for their own wallet and money.
  `/api/health` reports the agent's gas.
- **Intercepta quota**: the event key allows 1,000 checks. Each purchase uses 1–2 and each open
  tab 1 per 5 minutes (`RESCREEN_SECS=300`). Ask Intercepta for more before a big demo.
- **World ID**: the event runs on World's sandbox, where proofs are mocked. Anyone can verify
  from a browser; no World App is needed.
- **Changing limits**: `AUTONOMOUS_LIMIT` (countersigner) is what an AI may spend per tab before
  asking; `TAB_MAX_PRICE` (tab) is the most it pays for a single call.
- **Changing the backend's domain** means editing `tab/web/vercel.json` and redeploying the
  website; Vercel's rewrites cannot read environment variables.
