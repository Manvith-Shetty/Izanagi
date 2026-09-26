//! The countersigner's state, and the operations every entry point shares.
//!
//! The HTTP API (`api.rs`), the re-screening watcher and the World ID approval followers all
//! act through `App`, so a revocation is the same revocation -- in memory, on chain, in the
//! journal, on the phone -- whoever triggered it.

use crate::approvals::{restore_digest, Approval, Approvals, Challenge, NewApproval, Purpose, Status};
use crate::binding::{fingerprint, Bind, HumanBindings};
use crate::env::{self, CountersignerEnv};
use crate::guardian::Guardian;
use crate::intercepta::Intercepta;
use crate::journal::{Actor, Entry, Event, Journal};
use crate::notify::{human_duration, short_addr, Notifier, Priority, Push};
use crate::policy::{fmt_usdc, reason_code, PolicyConfig, PolicyEngine};
use crate::screener::Screener;
use crate::signer::OracleSigner;
use crate::state::Sessions;
use crate::world::{DeniedReason, PollOutcome, WorldId};
use alloy::primitives::Address;
use anyhow::{anyhow, Result};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

pub struct Settings {
    /// Spend per human (OIDC pairwise `sub`) across all their agents.
    pub human_limit: u128,
    pub attestation_ttl: u64,
    pub refuse_at: f64,
    pub rescreen: Duration,
    pub control_token: Option<String>,
    pub approval_page_url: Option<String>,
    /// A person approves a tab's limit in steps of this much (see `state::approval_limit`).
    pub approval_step: u128,
    /// Phone pushes go to one operator topic. With many users' wallets guarded here, only
    /// these wallets' events are pushed; `None` pushes everything.
    pub push_wallets: Option<Vec<Address>>,
}

pub struct App {
    pub policy: PolicyEngine,
    pub screener: Arc<Screener>,
    pub oracle: OracleSigner,
    pub sessions: Sessions,
    pub world: Option<WorldId>,
    pub approvals: Approvals,
    pub bindings: HumanBindings,
    pub journal: Journal,
    /// Writes revocations to the wallet itself. `None` => in-memory only (see `guardian.rs`).
    pub guardian: Option<Guardian>,
    /// Phone push: observability, never a control -- see `notify.rs`.
    pub notify: Option<Notifier>,
    pub settings: Settings,
    budgets: RwLock<HashMap<String, u128>>,
    /// One-time binding grants from enrolments: grant -> (subject, expires at). The subject
    /// never leaves this process; the grant lets the operator bind exactly one wallet to it.
    grants: RwLock<HashMap<String, (String, u64)>>,
}

/// How long an enrolment's binding grant stays usable: long enough to deploy a wallet.
const GRANT_TTL: u64 = 1800;

pub type Shared = Arc<App>;

impl App {
    pub fn from_env(cfg: &CountersignerEnv) -> Result<Self> {
        let screener = Arc::new(Screener::new(
            Intercepta::new(cfg.intercepta.api_key.clone()),
            Duration::from_secs(cfg.policy.address_cache_secs),
            Duration::from_secs(cfg.policy.token_cache_secs),
        ));
        let policy = PolicyEngine::new(
            screener.clone(),
            PolicyConfig {
                autonomous_limit: cfg.policy.autonomous_limit,
                refuse_at: cfg.policy.refuse_at,
                ask_at: cfg.policy.ask_at,
                cap_at: cfg.policy.cap_at,
                capped_ceiling: cfg.policy.capped_ceiling,
                max_price: cfg.policy.max_price,
            },
        );
        let world = match &cfg.world {
            env::WorldEnv::Enabled { issuer, client_id, client_secret, require_orb } => Some(
                WorldId::new(issuer.clone(), client_id.clone(), client_secret.clone(), *require_orb),
            ),
            env::WorldEnv::Disabled => None,
        };
        let guardian = match &cfg.service.rpc {
            Some(rpc) => Some(Guardian::new(rpc, &cfg.oracle_private_key)?),
            None => None,
        };
        Ok(Self {
            policy,
            screener,
            oracle: OracleSigner::from_hex_key(&cfg.oracle_private_key)?,
            sessions: Sessions::default(),
            world,
            approvals: Approvals::default(),
            bindings: HumanBindings::load(cfg.service.state_dir.join("humans.json"))?,
            journal: Journal::default(),
            guardian,
            notify: Notifier::new(&cfg.notify),
            settings: Settings {
                human_limit: cfg.policy.human_limit,
                attestation_ttl: cfg.policy.attestation_ttl,
                refuse_at: cfg.policy.refuse_at,
                rescreen: Duration::from_secs(cfg.policy.rescreen_secs.max(1)),
                control_token: cfg.service.control_token.clone(),
                approval_page_url: cfg.service.approval_page_url.clone(),
                approval_step: match cfg.policy.approval_step {
                    0 => cfg.policy.autonomous_limit,
                    s => s,
                },
                push_wallets: cfg.service.push_wallets.clone(),
            },
            budgets: RwLock::new(HashMap::new()),
            grants: RwLock::new(HashMap::new()),
        })
    }

    /// Whether this wallet's events should reach the operator's phone.
    pub fn pushes_for(&self, wallet: Address) -> bool {
        match &self.settings.push_wallets {
            None => true,
            Some(list) => list.contains(&wallet),
        }
    }

    /// Fire and forget, deliberately. No response waits on a push broker, and no decision
    /// depends on whether one answered.
    pub fn push(&self, p: Push) {
        if let Some(n) = self.notify.clone() {
            tokio::spawn(async move {
                n.send(p).await;
            });
        }
    }

    /// Where a person should go to see what they are approving.
    pub fn approval_link(&self, a: &Approval) -> String {
        match &self.settings.approval_page_url {
            Some(base) => format!("{base}/approve/{}", a.id),
            None => a.verification_uri_complete.clone().unwrap_or_else(|| a.verification_uri.clone()),
        }
    }

    pub async fn within_budget(&self, sub: &str, add: u128) -> bool {
        let spent = *self.budgets.read().await.get(sub).unwrap_or(&0);
        spent.saturating_add(add) <= self.settings.human_limit
    }

    pub async fn charge(&self, sub: &str, amount: u128) {
        *self.budgets.write().await.entry(sub.to_string()).or_insert(0) += amount;
    }

    /* ----------------------------- the kill switch ----------------------------- */

    /// Stop paying `seller` from `wallet`, now: no further countersignatures, and -- with a
    /// guardian -- every voucher already signed becomes unclaimable on chain.
    pub async fn revoke(
        &self,
        wallet: Address,
        seller: Address,
        reason: String,
        code: u32,
        by: Actor,
        score_now: Option<f64>,
    ) -> Entry {
        let stopped = self.sessions.revoke(wallet, seller, &reason).await;

        let (tx, chain_error) = match &self.guardian {
            None => (None, Some("no on-chain guardian configured (BASE_RPC unset): countersigning stopped, \
                                 and outstanding attestations lapse on their own".to_string())),
            Some(g) => match g.standing(wallet, seller).await {
                Ok(s) if s.revoked => (None, None), // already written; nothing to send
                _ => match g.revoke(wallet, seller, code).await {
                    Ok(tx) => (Some(format!("{tx:#x}")), None),
                    Err(e) => (None, Some(format!("{e:#}"))),
                },
            },
        };

        match &chain_error {
            None => tracing::warn!(
                wallet = %wallet, seller = %seller, by = ?by, tx = ?tx, value = %fmt_usdc(stopped.value),
                "REVOKED - vouchers already signed for this seller are no longer claimable"
            ),
            Some(e) => tracing::error!(
                wallet = %wallet, seller = %seller, by = ?by,
                "revoked in memory, but NOT on chain: {e}"
            ),
        }

        // Only the watcher's revocations are news to the person: they did not ask for them.
        if by == Actor::Watcher && self.pushes_for(wallet) {
            self.push(
                Push::new(
                    format!("Tab closed · {}", short_addr(&format!("{seller:#x}"))),
                    format!(
                        "{reason}\n{scores}{vouchers} voucher(s) worth {value}, already signed, \
                         can no longer be claimed.",
                        scores = match (stopped.score_before, score_now) {
                            (Some(b), Some(n)) => format!("Risk score {b:.0} → {n:.0}\n"),
                            _ => String::new(),
                        },
                        vouchers = stopped.vouchers,
                        value = fmt_usdc(stopped.value),
                    ),
                )
                .tags("rotating_light")
                .priority(Priority::High),
            );
        }

        self.journal
            .record(Event::Revoked {
                wallet: format!("{wallet:#x}"),
                seller: format!("{seller:#x}"),
                reason,
                reason_code: code,
                by,
                value_stopped: stopped.value,
                vouchers_stopped: stopped.vouchers,
                score_before: stopped.score_before,
                score_now,
                tx,
                chain_error,
            })
            .await
    }

    /* ------------------------------ human approvals ------------------------------ */

    /// Ask a person, through World ID, to approve one exact action. The device code stays
    /// here; the caller gets an approval it can show, and a background follower resolves it.
    pub async fn request_approval(self: &Arc<Self>, n: NewApproval) -> Result<Approval> {
        let world = self
            .world
            .as_ref()
            .ok_or_else(|| anyhow!("a human is required, but World ID is not configured"))?;
        let c = world.start_device_flow().await?;
        let a = self
            .approvals
            .create(
                n,
                Challenge {
                    device_code: c.device_code,
                    user_code: c.user_code,
                    verification_uri: c.verification_uri,
                    verification_uri_complete: c.verification_uri_complete,
                    expires_in: c.expires_in,
                    interval: c.interval,
                },
            )
            .await;

        let link = self.approval_link(&a);
        let (title, what) = match a.purpose {
            Purpose::Payment => (
                format!("Approve {} · {}", fmt_usdc(a.amount), a.user_code),
                format!("to {}", short_addr(&format!("{:#x}", a.seller))),
            ),
            Purpose::Restore => (
                format!("Reopen a closed tab · {}", a.user_code),
                format!("resume paying {}", short_addr(&format!("{:#x}", a.seller))),
            ),
            Purpose::Enroll => (format!("Sign up · {}", a.user_code), "a new Tab account".to_string()),
            Purpose::Handover => (
                format!("Make the wallet yours · {}", a.user_code),
                format!("only {} can take money out", short_addr(&format!("{:#x}", a.seller))),
            ),
        };
        // a stranger signing up is not news to the operator's phone
        if a.purpose != Purpose::Enroll && self.pushes_for(a.wallet) {
          self.push(
            // the code goes in the title: it must be legible from a lock screen
            Push::new(
                title,
                format!(
                    "{what}\n{reason}\nExpires in {expiry} · ignoring it blocks the action.",
                    reason = a.reason,
                    expiry = human_duration(a.expires_at.saturating_sub(a.created_at)),
                ),
            )
            .tags("closed_lock_with_key")
            .priority(Priority::Urgent)
            .click(&link)
            .view_action("Review and approve", &link),
          );
        }

        self.journal
            .record(Event::ApprovalRequested {
                approval_id: a.id.clone(),
                purpose: a.purpose.as_str().into(),
                wallet: format!("{:#x}", a.wallet),
                seller: format!("{:#x}", a.seller),
                amount: a.amount,
                reason: a.reason.clone(),
                expires_at: a.expires_at,
            })
            .await;

        tokio::spawn(self.clone().follow_approval(a.id.clone()));
        Ok(a)
    }

    /// Poll World for one approval until it resolves, at the interval World asked for.
    async fn follow_approval(self: Arc<Self>, id: String) {
        let Some(world) = self.world.clone() else { return };
        let mut interval = match self.approvals.get(&id).await {
            Some(a) => a.interval,
            None => return,
        };
        loop {
            tokio::time::sleep(Duration::from_secs(interval)).await;
            let Some(a) = self.approvals.get(&id).await else { return };
            match a.status() {
                Status::Pending => {}
                Status::Expired => return self.deny(&a, "expired_token").await,
                _ => return, // resolved elsewhere
            }
            match world.poll(&a.device_code).await {
                Ok(PollOutcome::Pending) => {}
                Ok(PollOutcome::SlowDown) => interval += 5,
                Ok(PollOutcome::Denied(r)) => return self.deny(&a, denied_str(&r)).await,
                Ok(PollOutcome::Approved(v)) => return self.approved(&a, &v.sub, v.orb_verified).await,
                // transient: the device code's own expiry bounds how long we keep trying
                Err(e) => tracing::warn!(approval = %id, "World ID poll failed, retrying: {e:#}"),
            }
        }
    }

    async fn deny(&self, a: &Approval, reason: &str) {
        self.approvals.mark_denied(&a.id, reason.to_string()).await;
        tracing::info!(approval = %a.id, purpose = a.purpose.as_str(), "not approved: {reason}");
        self.journal
            .record(Event::ApprovalDenied {
                approval_id: a.id.clone(),
                purpose: a.purpose.as_str().into(),
                reason: reason.to_string(),
            })
            .await;
    }

    /// World verified a person. Only the wallet's own human counts.
    async fn approved(&self, a: &Approval, sub: &str, orb_verified: bool) {
        // An enrolment has no wallet yet: the person is bound to one later, through a grant.
        let binding = if a.purpose == Purpose::Enroll {
            Ok(Bind::Matches)
        } else {
            self.bindings.check_or_bind(a.wallet, sub).await
        };
        match binding {
            Ok(Bind::WrongHuman) => {
                tracing::warn!(approval = %a.id, wallet = %a.wallet, "approved by a different human than the wallet's");
                return self.deny(a, "wrong_human").await;
            }
            Ok(Bind::BoundNow) => {
                self.journal
                    .record(Event::HumanBound { wallet: format!("{:#x}", a.wallet), human: fingerprint(sub) })
                    .await;
            }
            Ok(Bind::Matches) => {}
            // a binding we cannot persist is a binding a restart would forget: fail closed
            Err(e) => {
                tracing::error!("could not persist the human binding: {e:#}");
                return self.deny(a, "binding_unavailable").await;
            }
        }

        self.approvals.mark_approved(&a.id, sub.to_string(), orb_verified).await;
        tracing::info!(approval = %a.id, orb = orb_verified, "a verified human approved (subject stays here)");
        self.journal
            .record(Event::ApprovalGranted {
                approval_id: a.id.clone(),
                purpose: a.purpose.as_str().into(),
                orb_verified,
            })
            .await;

        // A payment approval is redeemed by the agent's next countersign request. A restore
        // needs no agent: carry it out now.
        if a.purpose == Purpose::Restore {
            self.complete_restore(a).await;
        }
    }

    async fn complete_restore(&self, a: &Approval) {
        if let Err(e) = self.approvals.consume(&a.id, &restore_digest(a.wallet, a.seller)).await {
            tracing::error!(approval = %a.id, "restore approval could not be redeemed: {e}");
            return;
        }
        let (tx, chain_error) = match &self.guardian {
            None => (None, None),
            Some(g) => match g.standing(a.wallet, a.seller).await {
                Ok(s) if !s.revoked => (None, None), // nothing on chain to undo
                _ => match g.restore(a.wallet, a.seller).await {
                    Ok(tx) => (Some(format!("{tx:#x}")), None),
                    Err(e) => (None, Some(format!("{e:#}"))),
                },
            },
        };
        // Reopen in memory only if the chain agrees: countersigning vouchers the wallet will
        // still refuse would have the seller serve requests it can never be paid for.
        if chain_error.is_none() {
            self.sessions.restore(a.wallet, a.seller).await;
        }
        if let Some(t) = &tx {
            self.approvals.record_tx(&a.id, t.clone()).await;
        }
        self.journal
            .record(Event::Restored {
                wallet: format!("{:#x}", a.wallet),
                seller: format!("{:#x}", a.seller),
                by: Actor::Human,
                tx,
                chain_error,
            })
            .await;
    }

    /* ------------------------------ enrolment and binding ------------------------------ */

    /// Redeem an approved enrolment. Returns the person's fingerprint (stable, private to
    /// this app) and a one-time grant for binding a wallet to them.
    pub async fn claim_enrollment(&self, approval_id: &str) -> Result<(String, String), crate::approvals::ApprovalError> {
        let sub = self.approvals.take_enrollment(approval_id).await?;
        let grant = format!("grant_{:032x}", crate::approvals::random_u128());
        let now = crate::state::now();
        let mut g = self.grants.write().await;
        g.retain(|_, (_, exp)| *exp > now);
        g.insert(grant.clone(), (sub.clone(), now + GRANT_TTL));
        Ok((fingerprint(&sub), grant))
    }

    /// Bind `wallet` to the person behind `grant`. The grant is used up either way.
    pub async fn bind_with_grant(&self, grant: &str, wallet: Address) -> Result<String> {
        let (sub, exp) = self.grants.write().await.remove(grant).ok_or_else(|| anyhow!("unknown or used grant"))?;
        if exp <= crate::state::now() {
            return Err(anyhow!("the grant expired"));
        }
        match self.bindings.check_or_bind(wallet, &sub).await? {
            Bind::WrongHuman => Err(anyhow!("that wallet already belongs to a different person")),
            Bind::BoundNow => {
                self.journal.record(Event::HumanBound { wallet: format!("{wallet:#x}"), human: fingerprint(&sub) }).await;
                Ok(fingerprint(&sub))
            }
            Bind::Matches => Ok(fingerprint(&sub)),
        }
    }

    /* --------------------------------- the watcher --------------------------------- */

    /// Keep screening every seller a wallet is still paying, and revoke the moment a live
    /// score crosses the refusal threshold -- even though the agent has already paid.
    pub async fn watch(self: Arc<Self>) {
        let mut ticker = tokio::time::interval(self.settings.rescreen);
        loop {
            ticker.tick().await;
            let pairs = self.sessions.open_pairs().await;
            // one metered call per distinct seller, however many wallets pay it
            let sellers: HashSet<Address> = pairs.iter().map(|(_, s)| *s).collect();
            for seller in sellers {
                let scan = match self.screener.address_fresh(&format!("{seller:#x}")).await {
                    Ok(s) => s,
                    // an outage does not revoke: vouchers already signed were signed for a
                    // seller that passed screening, and an honest seller keeps what it earned
                    Err(e) => {
                        tracing::warn!(seller = %seller, "re-screen failed: {e:#}");
                        continue;
                    }
                };
                self.sessions.record_score(seller, scan.toxic_score).await;
                if scan.toxic_score < self.settings.refuse_at {
                    continue;
                }
                let code = scan.worst_trait().map(|t| reason_code(&t.name)).unwrap_or(99);
                for (wallet, _) in pairs.iter().filter(|(_, s)| *s == seller) {
                    self.revoke(*wallet, seller, scan.reason(), code, Actor::Watcher, Some(scan.toxic_score))
                        .await;
                }
            }
        }
    }
}

fn denied_str(r: &DeniedReason) -> &'static str {
    match r {
        DeniedReason::AccessDenied => "access_denied",
        DeniedReason::ExpiredToken => "expired_token",
        DeniedReason::Cancelled => "cancelled",
        DeniedReason::StaleAuthentication => "stale_authentication",
        DeniedReason::InsufficientAssurance => "insufficient_assurance",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denied_reasons_use_the_wire_names() {
        for (r, want) in [
            (DeniedReason::AccessDenied, "access_denied"),
            (DeniedReason::StaleAuthentication, "stale_authentication"),
        ] {
            assert_eq!(serde_json::to_value(&r).unwrap(), want);
            assert_eq!(denied_str(&r), want);
        }
    }
}
