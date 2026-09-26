//! The Tab MCP server: a person's AI assistant spends from that person's Tab.
//!
//! Each person has their own link, `/mcp/{token}`. The tools let the assistant find sellers,
//! check them, buy, wait for the person's approval, see the tabs, and close one. There is no
//! tool to reopen a tab: an agent may always stop paying, only a verified human may restart.
//!
//! When a purchase needs the person, the approval reaches them where they already are:
//!   * clients that support MCP URL elicitation get a real prompt to open the approval page;
//!   * every other client gets the link and code in the tool result, with an instruction to
//!     show it verbatim and then call `wait_for_approval`.

use crate::app::Shared;
use crate::buyer::Outcome;
use crate::store::Account;
use alloy::primitives::Address;
use axum::http::request::Parts;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::common::Extension;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, ElicitationAction, Implementation, ProgressNotificationParam, RequestMetaObject,
    ServerCapabilities, ServerConfig,
};
use rmcp::{schemars, tool, tool_handler, tool_router, ErrorData as McpError, Peer, RoleServer, ServerHandler};
use serde::Deserialize;
use std::time::Duration;

const INSTRUCTIONS: &str = "Izanagi lets you pay x402 APIs from the user's own Izanagi account, a spending account on Base. Every payment \
is checked first: the seller is screened for fraud, the amount is held to the user's limits, and anything bigger needs \
the user's approval through World ID. Payments stay stoppable until the seller collects. Use find_services to discover paid APIs, buy to call one. \
When buy or wait_for_approval returns an approval link, show that link and code to the user exactly as written, then \
call wait_for_approval. Never retry a refused purchase with a different seller without telling the user. If a seller \
returns junk or looks wrong, call close_tab: the user keeps the money for anything the seller has not cashed in yet. \
You cannot reopen a closed tab; only the user can, from their dashboard.";

#[derive(Clone)]
pub struct TabMcp {
    app: Shared,
    #[allow(dead_code, reason = "read by the code #[tool_handler] generates")]
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FindArgs {
    /// Words to filter by, e.g. "bitcoin price" or "block". Empty lists everything.
    #[serde(default)]
    pub query: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BuyArgs {
    /// The full URL of the paid endpoint, from find_services or given by the user.
    pub url: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CheckArgs {
    /// A seller's address (0x...) or the URL of a paid endpoint.
    pub seller_or_url: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WaitArgs {
    /// The approval_id a previous buy returned.
    pub approval_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CloseArgs {
    /// The seller's address (0x...), as shown by list_tabs or a receipt.
    pub seller: String,
    /// Why, in a few words. Shown to the user and recorded with the revocation.
    #[serde(default)]
    pub reason: Option<String>,
}

/// Atomic USDC as a person reads it: `$0.002`, `$0.25`, `$1`.
fn usdc(v: u128) -> String {
    let (whole, frac) = (v / 1_000_000, v % 1_000_000);
    if frac == 0 {
        return format!("${whole}");
    }
    format!("${whole}.{}", format!("{frac:06}").trim_end_matches('0'))
}

fn text(s: impl Into<String>) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![ContentBlock::text(s.into())]))
}

impl TabMcp {
    pub fn new(app: Shared) -> Self {
        Self { app, tool_router: Self::tool_router() }
    }

    /// The person this link belongs to: `/mcp/{token}`, or `Authorization: Bearer {token}`.
    async fn account(&self, parts: &Parts) -> Result<Account, McpError> {
        let from_path = parts.uri.path().strip_prefix("/mcp/").map(|t| t.trim_end_matches('/').to_string());
        let from_header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(String::from);
        let token = from_path.or(from_header).unwrap_or_default();
        self.app
            .store
            .account_by_mcp_token(&token)
            .await
            .ok_or_else(|| McpError::invalid_request("unknown Izanagi link: copy yours from your Izanagi dashboard", None))
    }

    fn render(&self, o: &Outcome) -> String {
        match o {
            Outcome::Paid(p) => format!(
                "Paid {} to {} (screened: {}, risk score {}).\n\
                 This seller has charged {} in total; {} of it is still stoppable until they cash in.{}\n\n\
                 Response ({}):\n{}",
                usdc(p.price),
                p.service.clone().unwrap_or_else(|| format!("{:#x}", p.seller)),
                p.verdict,
                p.toxic_score,
                usdc(p.cumulative),
                usdc(p.stoppable),
                p.opened.as_ref().map(|o| format!("\nOpened a {} tab with this seller first (tx {}).", usdc(o.deposit), o.tx)).unwrap_or_default(),
                p.status,
                serde_json::to_string_pretty(&p.data).unwrap_or_default(),
            ),
            Outcome::NeedsApproval(n) => format!(
                "The user needs to approve this before anything is paid.\n\
                 Why: {}\n\
                 Asking to raise the tab with {} to {} (already approved: {}).\n\n\
                 Show the user this, exactly:\n\
                 ──────────────\n\
                 Approve with World ID: {}\n\
                 Code: {}\n\
                 ──────────────\n\
                 It expires in {} minutes. No signature exists until they approve, so a denial or \
                 a timeout costs nothing.\n\
                 Then call wait_for_approval with approval_id \"{}\".",
                n.reason,
                n.service.clone().unwrap_or_else(|| format!("{:#x}", n.seller)),
                usdc(n.limit),
                usdc(n.approved_so_far),
                n.approval_url,
                n.user_code,
                n.expires_in / 60,
                n.approval_id,
            ),
            Outcome::Refused(r) => format!(
                "Not paid: {} ({}; risk score {}). No signature was produced, so nothing can be claimed.",
                r.reason, r.verdict, r.toxic_score
            ),
            Outcome::Unpayable { reason, .. } => format!("Not paid: {reason}"),
            Outcome::Free { status, body, .. } => format!("That endpoint did not ask for payment ({status}):\n{body}"),
            Outcome::Denied { reason, .. } => format!("The user did not approve ({reason}). Nothing was paid."),
            Outcome::StillPending { approval_id, approval_url, user_code } => format!(
                "Still waiting for the user. Remind them: {approval_url} (code {user_code}). \
                 Call wait_for_approval again with approval_id \"{approval_id}\"."
            ),
        }
    }

    /// Wait for the person, sending progress to clients that asked for it.
    async fn wait(&self, a: &Account, id: &str, secs: u64, meta: &RequestMetaObject, peer: &Peer<RoleServer>) -> Outcome {
        let token = meta.get_progress_token();
        let peer = peer.clone();
        self.app
            .buyer
            .wait(a, id, Duration::from_secs(secs), move |waited| {
                if let Some(t) = token.clone() {
                    let p = peer.clone();
                    tokio::spawn(async move {
                        let _ = p
                            .notify_progress(
                                ProgressNotificationParam::new(t, waited as f64)
                                    .with_message(format!("waiting for the user's approval ({waited}s)")),
                            )
                            .await;
                    });
                }
            })
            .await
    }
}

#[tool_router]
impl TabMcp {
    #[tool(description = "List paid APIs this account can buy from (x402 batch-settlement sellers on Base), with price per call. Curated, proven entries first.")]
    async fn find_services(
        &self,
        Parameters(args): Parameters<FindArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, McpError> {
        self.account(&parts).await?;
        let list = self.app.catalog.list(args.query.as_deref()).await;
        if list.is_empty() {
            return text("No matching paid APIs. Try fewer words, or no query.");
        }
        let lines: Vec<String> = list
            .iter()
            .take(25)
            .map(|l| {
                format!(
                    "- {} — {} per call{}{}\n  {}\n  url: {}",
                    l.name,
                    usdc(l.price),
                    if l.proven.is_some() { " · proven on mainnet" } else { "" },
                    if l.demo { " · demo shop (goes bad on purpose)" } else { "" },
                    l.description,
                    l.url
                )
            })
            .collect();
        text(format!("{} paid APIs:\n{}", list.len(), lines.join("\n")))
    }

    #[tool(description = "Check a seller's live fraud-risk score (Intercepta) before paying. Accepts a seller address or a paid endpoint URL.")]
    async fn check_seller(
        &self,
        Parameters(args): Parameters<CheckArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, McpError> {
        self.account(&parts).await?;
        let s = args.seller_or_url.trim();
        let address: Address = if s.starts_with("http") {
            match self.app.catalog.list(None).await.into_iter().find(|l| l.url == s).and_then(|l| l.seller.parse().ok()) {
                Some(a) => a,
                None => return text("That URL is not in the catalog; pass the seller's 0x address instead (buy shows it)."),
            }
        } else {
            s.parse().map_err(|_| McpError::invalid_params("not an address or a URL", None))?
        };
        match self.app.brain.screen(address).await {
            Ok(v) => text(format!(
                "{address:#x}: risk score {} → {} ({}).\nThresholds: pay below {}, cap from {}, ask the user from {}, refuse from {}.",
                v["toxicScore"], v["band"], v["reason"].as_str().unwrap_or(""),
                v["thresholds"]["capAt"], v["thresholds"]["capAt"], v["thresholds"]["askAt"], v["thresholds"]["refuseAt"]
            )),
            Err(e) => text(format!("Screening is unavailable right now ({e:#}), so Izanagi would refuse to pay anyone.")),
        }
    }

    #[tool(description = "Pay for one call to an x402 API from the user's Izanagi account and return the response. Screens the seller first; may need the user's approval (then shows a link).")]
    async fn buy(
        &self,
        Parameters(args): Parameters<BuyArgs>,
        Extension(parts): Extension<Parts>,
        meta: RequestMetaObject,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let a = self.account(&parts).await?;
        let outcome = self.app.buyer.buy(&a, &args.url, None).await;
        if let Outcome::NeedsApproval(n) = &outcome {
            // a real prompt in the client, where it supports one; otherwise the link below
            if let Ok(url) = url::Url::parse(&n.approval_url) {
                let msg = format!(
                    "Your assistant wants to raise its tab with {} to {}. {}",
                    n.service.clone().unwrap_or_else(|| format!("{:#x}", n.seller)),
                    usdc(n.limit),
                    n.reason
                );
                match peer.elicit_url_with_timeout(msg, url, n.approval_id.clone(), Some(Duration::from_secs(120))).await {
                    Ok(ElicitationAction::Accept) => {
                        let o = self.wait(&a, &n.approval_id, 45, &meta, &peer).await;
                        return text(self.render(&o));
                    }
                    Ok(_) => return text("The user declined in the client. Nothing was paid, and no signature exists."),
                    Err(_) => {} // the client has no URL prompts: hand over the link instead
                }
            }
        }
        text(self.render(&outcome))
    }

    #[tool(description = "Wait up to ~45s for the user to approve a pending purchase, then complete it. Call again if still pending.")]
    async fn wait_for_approval(
        &self,
        Parameters(args): Parameters<WaitArgs>,
        Extension(parts): Extension<Parts>,
        meta: RequestMetaObject,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let a = self.account(&parts).await?;
        let o = self.wait(&a, &args.approval_id, 45, &meta, &peer).await;
        text(self.render(&o))
    }

    #[tool(description = "Show the user's tabs: each seller, what it has charged, what is still stoppable, and the wallet balance.")]
    async fn list_tabs(&self, Extension(parts): Extension<Parts>) -> Result<CallToolResult, McpError> {
        let a = self.account(&parts).await?;
        let o = self.app.overview(&a).await;
        let tabs = o["tabs"].as_array().cloned().unwrap_or_default();
        let lines: Vec<String> = tabs
            .iter()
            .map(|t| {
                format!(
                    "- {} ({}) [{}]: {} calls, charged {}, stoppable {}, seller {}",
                    t["service"].as_str().unwrap_or("seller"),
                    t["price"].as_u64().map(|p| format!("{} a call", usdc(p as u128))).unwrap_or_default(),
                    t["status"].as_str().unwrap_or(""),
                    t["requests"],
                    usdc(t["charged"].as_u64().unwrap_or(0) as u128),
                    usdc(t["stoppable"].as_u64().unwrap_or(0) as u128),
                    t["seller"].as_str().unwrap_or("")
                )
            })
            .collect();
        text(format!(
            "Wallet {} holds {} (plus {} escrowed in tabs). Stoppable right now: {}.\n{}",
            a.wallet,
            usdc(o["wallet"]["usdc"].as_u64().unwrap_or(0) as u128),
            usdc(o["totals"]["escrowed"].as_u64().unwrap_or(0) as u128),
            usdc(o["totals"]["stoppable"].as_u64().unwrap_or(0) as u128),
            if lines.is_empty() { "No tabs yet.".to_string() } else { lines.join("\n") }
        ))
    }

    #[tool(description = "Close a tab: stop paying a seller now, including payments already signed but not yet cashed in. Use when a seller sells junk or looks wrong.")]
    async fn close_tab(
        &self,
        Parameters(args): Parameters<CloseArgs>,
        Extension(parts): Extension<Parts>,
    ) -> Result<CallToolResult, McpError> {
        let a = self.account(&parts).await?;
        let seller: Address = args.seller.trim().parse().map_err(|_| McpError::invalid_params("seller must be a 0x address", None))?;
        match self.app.close_tab(&a, seller, args.reason.as_deref()).await {
            Ok(v) => text(format!(
                "Closed. {} already signed to this seller can no longer be cashed in{}. Only the user can reopen this tab, \
                 from their dashboard, with World ID.",
                usdc(v["value_stopped"].as_u64().unwrap_or(0) as u128),
                v["tx"].as_str().map(|t| format!(" (on chain: https://basescan.org/tx/{t})")).unwrap_or_default()
            )),
            Err(e) => text(format!("Could not close it: {e:#}")),
        }
    }
}

#[tool_handler]
impl ServerHandler for TabMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("izanagi", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_read_like_money() {
        assert_eq!(usdc(2_000), "$0.002");
        assert_eq!(usdc(250_000), "$0.25");
        assert_eq!(usdc(1_000_000), "$1");
        assert_eq!(usdc(0), "$0");
    }
}
