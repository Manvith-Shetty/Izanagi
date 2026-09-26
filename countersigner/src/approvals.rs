//! Human approvals, each bound to ONE exact action.
//!
//! Four things can need a person:
//!
//!   * a **payment** the policy will not make alone -- bound to the voucher digest, which
//!     commits to the channel (payer, seller, token) and the exact limit;
//!   * **restoring** a seller that was revoked -- bound to (wallet, seller). Stopping payment
//!     is always free and instant; starting it again is the one direction that needs a human;
//!   * **enrolling**: proving you are a unique human before you are given a wallet. Bound to
//!     nothing but itself; what it yields is a fingerprint and a one-time binding grant;
//!   * **handing a trial wallet over** to an account of the person's own -- bound to
//!     (wallet, new owner). Whoever holds the new owner's key can take every cent out, so the
//!     wallet's own human must say which account that is, not whoever holds their session.
//!
//! An approval therefore cannot be moved to a different seller, a different amount, or a
//! second use: the human approved *this*, not a budget.
//!
//! The World ID polling itself happens in the background (`App::follow_approval`), so every
//! reader -- the agent, the approval page, the MCP server -- sees the same answer at the same
//! time, and the IdP is polled once per approval, at the interval it asked for.

use crate::state::now;
use alloy::primitives::{keccak256, Address};
use serde::Serialize;
use std::collections::HashMap;
use tokio::sync::RwLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    Payment,
    Restore,
    Enroll,
    Handover,
}

impl Purpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Purpose::Payment => "payment",
            Purpose::Restore => "restore",
            Purpose::Enroll => "enroll",
            Purpose::Handover => "handover",
        }
    }
}

/// What a human is being asked, as the countersigner knows it.
#[derive(Debug, Clone)]
pub struct Approval {
    pub id: String,
    pub purpose: Purpose,
    pub wallet: Address,
    /// The counterparty: the seller for a payment or a restore, the new owner for a handover.
    pub seller: Address,
    /// The ceiling being approved; zero for a restore.
    pub amount: u128,
    /// Why a human is needed, in words.
    pub reason: String,
    /// The one action this approval is valid for.
    pub bound_digest: String,
    /// OAuth credential. Never serialised, never leaves this process.
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    /// Seconds between World ID polls, as the IdP asked.
    pub interval: u64,
    pub created_at: u64,
    pub expires_at: u64,
    /// Set once World's id_token has been verified. Never serialised.
    pub human_sub: Option<String>,
    pub orb_verified: bool,
    /// Single use: set when the approved action is carried out.
    pub consumed: bool,
    pub denied: Option<String>,
    /// For a restore: the on-chain transaction that carried it out.
    pub tx: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Approved,
    Denied,
    Expired,
    /// Approved, and the approved action has been carried out.
    Used,
}

impl Approval {
    pub fn is_expired(&self) -> bool {
        now() >= self.expires_at
    }

    pub fn status(&self) -> Status {
        if self.denied.is_some() {
            Status::Denied
        } else if self.consumed {
            Status::Used
        } else if self.is_expired() {
            Status::Expired
        } else if self.human_sub.is_some() {
            Status::Approved
        } else {
            Status::Pending
        }
    }

    /// Everything a person or an agent may see: no device code, no subject.
    pub fn view(&self) -> ApprovalView {
        ApprovalView {
            id: self.id.clone(),
            purpose: self.purpose,
            status: self.status(),
            wallet: format!("{:#x}", self.wallet),
            seller: format!("{:#x}", self.seller),
            amount: self.amount,
            reason: self.reason.clone(),
            user_code: self.user_code.clone(),
            verification_uri: self.verification_uri.clone(),
            verification_uri_complete: self.verification_uri_complete.clone(),
            created_at: self.created_at,
            expires_at: self.expires_at,
            denied_reason: self.denied.clone(),
            orb_verified: self.human_sub.as_ref().map(|_| self.orb_verified),
            tx: self.tx.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalView {
    pub id: String,
    pub purpose: Purpose,
    pub status: Status,
    pub wallet: String,
    pub seller: String,
    pub amount: u128,
    pub reason: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_uri_complete: Option<String>,
    pub created_at: u64,
    pub expires_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub denied_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orb_verified: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx: Option<String>,
}

/// Everything needed to open an approval; the World challenge supplies the rest.
pub struct NewApproval {
    pub purpose: Purpose,
    pub wallet: Address,
    pub seller: Address,
    pub amount: u128,
    pub reason: String,
    pub bound_digest: String,
}

/// The challenge World issued for this approval.
pub struct Challenge {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    pub interval: u64,
}

/// What a restore approval is bound to. Distinct from any voucher digest by construction.
pub fn restore_digest(wallet: Address, seller: Address) -> String {
    format!("{:#x}", keccak256(format!("countersign.restore:{wallet:#x}:{seller:#x}")))
}

/// What a handover approval is bound to: this wallet, to this account and no other.
pub fn handover_digest(wallet: Address, new_owner: Address) -> String {
    format!("{:#x}", keccak256(format!("countersign.handover:{wallet:#x}:{new_owner:#x}")))
}

/// Why an approval cannot authorise an action.
#[derive(Debug, PartialEq)]
pub enum ApprovalError {
    Unknown,
    NotYetApproved,
    Denied(String),
    Expired,
    AlreadyUsed,
    /// Approved -- but for a different action than the one now being requested.
    WrongPayment { approved_for: String },
}

impl std::fmt::Display for ApprovalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApprovalError::Unknown => f.write_str("no such approval"),
            ApprovalError::NotYetApproved => f.write_str("the human has not approved yet"),
            ApprovalError::Denied(r) => write!(f, "the human did not approve: {r}"),
            ApprovalError::Expired => f.write_str("the approval expired"),
            ApprovalError::AlreadyUsed => {
                f.write_str("that approval was already used - a human approves one payment, not a budget")
            }
            ApprovalError::WrongPayment { approved_for } => {
                write!(f, "that approval was given for a different payment ({approved_for}), not this one")
            }
        }
    }
}

#[derive(Default)]
pub struct Approvals {
    inner: RwLock<HashMap<String, Approval>>,
}

impl Approvals {
    pub async fn create(&self, n: NewApproval, c: Challenge) -> Approval {
        let a = Approval {
            id: format!("apr_{:032x}", random_u128()),
            purpose: n.purpose,
            wallet: n.wallet,
            seller: n.seller,
            amount: n.amount,
            reason: n.reason,
            bound_digest: n.bound_digest,
            device_code: c.device_code,
            user_code: c.user_code,
            verification_uri: c.verification_uri,
            verification_uri_complete: c.verification_uri_complete,
            interval: c.interval.max(1),
            created_at: now(),
            expires_at: now() + c.expires_in,
            human_sub: None,
            orb_verified: false,
            consumed: false,
            denied: None,
            tx: None,
        };
        self.inner.write().await.insert(a.id.clone(), a.clone());
        a
    }

    pub async fn get(&self, id: &str) -> Option<Approval> {
        self.inner.read().await.get(id).cloned()
    }

    /// Open approvals for one wallet, newest first.
    pub async fn pending_for(&self, wallet: Address) -> Vec<ApprovalView> {
        let mut v: Vec<_> = self
            .inner
            .read()
            .await
            .values()
            .filter(|a| a.wallet == wallet && matches!(a.status(), Status::Pending | Status::Approved))
            .map(Approval::view)
            .collect();
        v.sort_by_key(|a| std::cmp::Reverse(a.created_at));
        v
    }

    pub async fn mark_approved(&self, id: &str, sub: String, orb_verified: bool) {
        if let Some(a) = self.inner.write().await.get_mut(id) {
            a.human_sub = Some(sub);
            a.orb_verified = orb_verified;
        }
    }

    /// Record a denial. The first reason wins: a later poll cannot overwrite why it ended.
    pub async fn mark_denied(&self, id: &str, reason: String) {
        if let Some(a) = self.inner.write().await.get_mut(id) {
            if a.denied.is_none() && !a.consumed {
                a.denied = Some(reason);
            }
        }
    }

    pub async fn record_tx(&self, id: &str, tx: String) {
        if let Some(a) = self.inner.write().await.get_mut(id) {
            a.tx = Some(tx);
        }
    }

    /// Redeem an enrolment: the verified subject, once. The caller never learns it -- it goes
    /// straight into a binding grant (see `App::claim_enrollment`).
    pub async fn take_enrollment(&self, id: &str) -> Result<String, ApprovalError> {
        let mut g = self.inner.write().await;
        let a = g.get_mut(id).ok_or(ApprovalError::Unknown)?;
        if a.purpose != Purpose::Enroll {
            return Err(ApprovalError::Unknown);
        }
        if let Some(r) = &a.denied {
            return Err(ApprovalError::Denied(r.clone()));
        }
        if a.consumed {
            return Err(ApprovalError::AlreadyUsed);
        }
        if a.is_expired() {
            return Err(ApprovalError::Expired);
        }
        let sub = a.human_sub.clone().ok_or(ApprovalError::NotYetApproved)?;
        a.consumed = true;
        Ok(sub)
    }

    /// Redeem an approval for one specific action. Single use, and only for the exact digest
    /// the human approved.
    pub async fn consume(&self, id: &str, digest: &str) -> Result<String, ApprovalError> {
        let mut g = self.inner.write().await;
        let a = g.get_mut(id).ok_or(ApprovalError::Unknown)?;
        if let Some(r) = &a.denied {
            return Err(ApprovalError::Denied(r.clone()));
        }
        if a.consumed {
            return Err(ApprovalError::AlreadyUsed);
        }
        // strict: an approval is redeemable only within the window the human was shown
        if a.is_expired() {
            return Err(ApprovalError::Expired);
        }
        let sub = a.human_sub.clone().ok_or(ApprovalError::NotYetApproved)?;
        if !a.bound_digest.eq_ignore_ascii_case(digest) {
            return Err(ApprovalError::WrongPayment { approved_for: a.bound_digest.clone() });
        }
        a.consumed = true;
        Ok(sub)
    }
}

/// 128 bits from the OS's cryptographic RNG. Approval ids and binding grants are capabilities.
pub fn random_u128() -> u128 {
    // a fresh secp256k1 key is 32 bytes straight from the OS RNG; we keep 16 of them
    let b = alloy::signers::local::PrivateKeySigner::random().to_bytes();
    u128::from_be_bytes(b[..16].try_into().expect("16 bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    const WALLET: Address = address!("1111111111111111111111111111111111111111");
    const SELLER: Address = address!("2222222222222222222222222222222222222222");

    async fn pending() -> (Approvals, Approval) {
        let a = Approvals::default();
        let ap = a
            .create(
                NewApproval {
                    purpose: Purpose::Payment,
                    wallet: WALLET,
                    seller: SELLER,
                    amount: 500_000_000,
                    reason: "over the autonomous limit".into(),
                    bound_digest: "0xdigestA".into(),
                },
                Challenge {
                    device_code: "dev-secret".into(),
                    user_code: "CODE".into(),
                    verification_uri: "https://sandbox.auth.world.org/device".into(),
                    verification_uri_complete: None,
                    expires_in: 600,
                    interval: 5,
                },
            )
            .await;
        (a, ap)
    }

    #[tokio::test]
    async fn cannot_be_used_before_the_human_approves() {
        let (a, ap) = pending().await;
        assert_eq!(a.consume(&ap.id, "0xdigestA").await, Err(ApprovalError::NotYetApproved));
    }

    #[tokio::test]
    async fn approval_is_bound_to_one_exact_payment() {
        let (a, ap) = pending().await;
        a.mark_approved(&ap.id, "sub-123".into(), true).await;
        // a compromised agent tries to spend the approval on a different payment
        let e = a.consume(&ap.id, "0xdigestB").await.unwrap_err();
        assert!(matches!(e, ApprovalError::WrongPayment { .. }), "got {e:?}");
        // the payment the human actually approved still works
        assert_eq!(a.consume(&ap.id, "0xdigestA").await, Ok("sub-123".into()));
    }

    #[tokio::test]
    async fn approval_is_single_use() {
        let (a, ap) = pending().await;
        a.mark_approved(&ap.id, "sub-123".into(), true).await;
        assert!(a.consume(&ap.id, "0xdigestA").await.is_ok());
        assert_eq!(a.consume(&ap.id, "0xdigestA").await, Err(ApprovalError::AlreadyUsed));
        assert_eq!(a.get(&ap.id).await.unwrap().status(), Status::Used);
    }

    #[tokio::test]
    async fn denial_blocks_redemption_and_the_first_reason_sticks() {
        let (a, ap) = pending().await;
        a.mark_denied(&ap.id, "access_denied".into()).await;
        a.mark_denied(&ap.id, "expired_token".into()).await;
        assert_eq!(a.consume(&ap.id, "0xdigestA").await, Err(ApprovalError::Denied("access_denied".into())));
        assert_eq!(a.get(&ap.id).await.unwrap().status(), Status::Denied);
    }

    #[tokio::test]
    async fn unknown_id_is_rejected() {
        let a = Approvals::default();
        assert_eq!(a.consume("apr_nope", "0xdigestA").await, Err(ApprovalError::Unknown));
    }

    #[tokio::test]
    async fn the_view_never_carries_a_credential() {
        let (a, ap) = pending().await;
        a.mark_approved(&ap.id, "private-subject".into(), true).await;
        let j = serde_json::to_string(&a.get(&ap.id).await.unwrap().view()).unwrap();
        assert!(!j.contains("dev-secret"), "device_code leaked: {j}");
        assert!(!j.contains("private-subject"), "subject leaked: {j}");
        assert!(j.contains("CODE"), "the user-facing code should be present");
        assert!(j.contains("\"status\":\"approved\""));
    }

    #[tokio::test]
    async fn pending_for_lists_only_open_approvals_of_that_wallet() {
        let (a, ap) = pending().await;
        assert_eq!(a.pending_for(WALLET).await.len(), 1);
        assert!(a.pending_for(SELLER).await.is_empty());
        a.mark_denied(&ap.id, "access_denied".into()).await;
        assert!(a.pending_for(WALLET).await.is_empty());
    }

    #[tokio::test]
    async fn an_enrollment_yields_its_subject_once_and_only_when_approved() {
        let a = Approvals::default();
        let ap = a
            .create(
                NewApproval {
                    purpose: Purpose::Enroll,
                    wallet: Address::ZERO,
                    seller: Address::ZERO,
                    amount: 0,
                    reason: "get a tab".into(),
                    bound_digest: "enroll:1".into(),
                },
                Challenge {
                    device_code: "dev".into(),
                    user_code: "CODE".into(),
                    verification_uri: "https://sandbox.auth.world.org/device".into(),
                    verification_uri_complete: None,
                    expires_in: 600,
                    interval: 5,
                },
            )
            .await;
        assert_eq!(a.take_enrollment(&ap.id).await, Err(ApprovalError::NotYetApproved));
        a.mark_approved(&ap.id, "sub-1".into(), true).await;
        assert_eq!(a.take_enrollment(&ap.id).await, Ok("sub-1".into()));
        assert_eq!(a.take_enrollment(&ap.id).await, Err(ApprovalError::AlreadyUsed));

        // a payment approval can never be redeemed as an enrolment
        let (b, pay) = pending().await;
        b.mark_approved(&pay.id, "sub-2".into(), true).await;
        assert_eq!(b.take_enrollment(&pay.id).await, Err(ApprovalError::Unknown));
    }

    #[test]
    fn handover_digests_are_bound_to_wallet_and_owner_and_never_a_restore() {
        let (w, o) = (Address::repeat_byte(1), Address::repeat_byte(2));
        assert_eq!(handover_digest(w, o), handover_digest(w, o));
        assert_ne!(handover_digest(w, o), handover_digest(w, Address::repeat_byte(3)));
        assert_ne!(handover_digest(w, o), handover_digest(Address::repeat_byte(3), o));
        assert_ne!(handover_digest(w, o), restore_digest(w, o));
    }

    #[test]
    fn restore_digests_are_bound_to_wallet_and_seller() {
        let d = restore_digest(WALLET, SELLER);
        assert_ne!(d, restore_digest(SELLER, WALLET));
        assert!(d.starts_with("0x") && d.len() == 66);
    }
}
