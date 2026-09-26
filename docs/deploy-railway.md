# Deploying Tab on Railway (console)

Three services from one repo and one Dockerfile, on Base mainnet.

| Service | What it is | Public? | Port | Volume |
|---|---|---|---|---|
| `countersigner` | risk screening, World ID, the kill switch; holds the oracle key | **no**, private network only | 8787 | `/data` |
| `tab` | the website, dashboard and MCP server | yes | 3000 | `/data` |
| `seller` | the demo shop that goes bad on purpose | yes | 8080 | none |

The service **must be named `countersigner`**: Tab reaches it at `countersigner.railway.internal`.

---

## 1. Before Railway

**Push the code to GitHub.** Railway deploys from a GitHub repository. Nothing secret is in the
repo: every `.env` file is gitignored, and `.dockerignore` keeps them out of the image too.

**Use a new wallet for the treasury.** It deploys and owns each person's wallet and pays their
$0.25 trial. On Base it needs:
- about **0.003 ETH** (deploying the collector, gas for the service keys, ~0.00002 ETH per new person)
- **0.25 USDC per person**: 10 USDC covers the default 40 free tabs

Never reuse a key that has been pasted into a chat or a terminal log.

**Use a dedicated RPC if you can.** `https://mainnet.base.org` rate-limits; a free Alchemy or
QuickNode Base endpoint is far steadier under a crowd of judges.

## 2. Generate the production config (one command, on your machine)

In `scripts/.env`:

```bash
TAB_TREASURY_KEY=0x...                  # the NEW wallet
RPC=https://mainnet.base.org            # or your dedicated endpoint
TAB_URL=https://<tab domain>            # fill in after step 3, or edit the files later
SHOP_URL=https://<shop domain>
```

Then:

```bash
scripts/railway-setup.sh
```

It shows the plan and waits for `yes`. Then it generates the agent, oracle and shop keys and writes three
files, **one per Railway service**. After that it deploys the fixed deposit collector and sends each
key its gas. Keys are never printed.

```
scripts/.env.railway.countersigner
scripts/.env.railway.tab
scripts/.env.railway.seller
```

Back these three files up. The keys in them exist nowhere else.

## 3. Create the project

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

## 4. Variables

For each service: **Variables → Raw Editor**, paste the whole matching file, **Update Variables**.

| Service | Paste |
|---|---|
| `countersigner` | `scripts/.env.railway.countersigner` |
| `tab` | `scripts/.env.railway.tab` |
| `seller` | `scripts/.env.railway.seller` |

If you generated the files before you had domains, now set these, using the domains from step 3:

- `tab`: `TAB_PUBLIC_URL=https://<tab domain>` and `DEMO_SHOP_URL=https://<shop domain>`
- `countersigner`: `APPROVAL_PAGE_URL=https://<tab domain>`
- `seller`: `SELLER_PUBLIC_URL=https://<shop domain>`

Railway redeploys each service when its variables change.

## 5. Check it

```bash
curl https://<tab domain>/api/health      # status ok, countersigner ok, trialsLeft 40
curl https://<shop domain>/health          # payerScreening true, rogueAfter 3
```

Then open `https://<tab domain>`:

1. **Get your free Tab** → verify with World ID → you land on your dashboard with $0.25.
2. **Try it → Tab demo shop** four times: three real Bitcoin prices, then junk.
3. **Close tab** → the STOPPED stamp; the shop can no longer collect.
4. **Connect your AI**: copy the Claude Code command, then ask Claude
   *"Use Tab to get the latest Bitcoin price from hyperextend."*

## Good to know

- **Logs**: each service's *Deployments → View logs*. Tab logs every signup and deployment;
  the countersigner logs every decision.
- **Costs to watch**: the treasury (each new person costs ~$0.25 + a little gas) and the agent's
  gas (each new tab is one transaction). `/api/health` reports both gas balances.
- **Intercepta quota**: the event key allows 1,000 checks. Each purchase uses 1–2 and each open
  tab 1 per 5 minutes (`RESCREEN_SECS=300`). Ask Intercepta for more before a big demo.
- **World ID**: the event runs on World's sandbox, where proofs are mocked. Anyone can verify
  from a browser; no World App is needed.
- **Changing limits**: `AUTONOMOUS_LIMIT` (countersigner) is what an AI may spend per tab before
  asking; `TAB_TRIAL_MAX_ACCOUNTS` (tab) caps free tabs.
