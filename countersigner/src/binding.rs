//! Which human a wallet belongs to.
//!
//! A device-grant code can be approved by whoever holds it. That is fine for a TV login and
//! wrong for money: an approval link that leaks into a chat or a log could be approved by a
//! stranger with their own World ID. So each wallet is bound to ONE unique human -- World's
//! pairwise `sub`, which is stable for a person and private to this application -- and from
//! then on only that human's approvals count.
//!
//! The binding is made by the first verified approval for the wallet (trust on first use),
//! recorded in the journal, and persisted so a restart cannot reopen it. Changing it is a
//! deliberate act on the state file, never something an API call can do.
//!
//! The raw `sub` never leaves this module: everything else sees a short fingerprint.

use alloy::primitives::{keccak256, Address};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::sync::RwLock;

#[derive(Debug, PartialEq)]
pub enum Bind {
    /// This human is now the wallet's human.
    BoundNow,
    /// This human already was.
    Matches,
    /// Someone else is bound to this wallet.
    WrongHuman,
}

pub struct HumanBindings {
    path: Option<PathBuf>,
    bound: RwLock<HashMap<Address, String>>,
}

/// A short, non-reversible label for a subject, safe to show on a dashboard.
pub fn fingerprint(sub: &str) -> String {
    let h = keccak256(format!("countersign.human:{sub}"));
    format!("human-{}", &format!("{h:x}")[..8])
}

impl HumanBindings {
    /// In memory only (tests).
    pub fn ephemeral() -> Self {
        Self { path: None, bound: RwLock::new(HashMap::new()) }
    }

    /// Load from `path`, or start empty if it does not exist yet.
    pub fn load(path: PathBuf) -> Result<Self> {
        let bound = match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).with_context(|| format!("parsing {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        Ok(Self { path: Some(path), bound: RwLock::new(bound) })
    }

    pub async fn check_or_bind(&self, wallet: Address, sub: &str) -> Result<Bind> {
        let mut g = self.bound.write().await;
        let outcome = match g.get(&wallet) {
            Some(existing) if existing == sub => return Ok(Bind::Matches),
            Some(_) => return Ok(Bind::WrongHuman),
            None => {
                g.insert(wallet, sub.to_string());
                Bind::BoundNow
            }
        };
        if let Some(path) = &self.path {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).ok();
            }
            // write-then-rename, so a crash mid-write cannot leave a half-bound wallet
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(&*g)?)?;
            std::fs::rename(&tmp, path)?;
        }
        Ok(outcome)
    }

    /// The bound human's fingerprint, if any.
    pub async fn human_of(&self, wallet: Address) -> Option<String> {
        self.bound.read().await.get(&wallet).map(|s| fingerprint(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    const W: Address = address!("1111111111111111111111111111111111111111");

    #[tokio::test]
    async fn first_human_binds_and_only_they_match_after() {
        let b = HumanBindings::ephemeral();
        assert_eq!(b.check_or_bind(W, "alice").await.unwrap(), Bind::BoundNow);
        assert_eq!(b.check_or_bind(W, "alice").await.unwrap(), Bind::Matches);
        assert_eq!(b.check_or_bind(W, "mallory").await.unwrap(), Bind::WrongHuman);
    }

    #[tokio::test]
    async fn bindings_are_per_wallet() {
        let b = HumanBindings::ephemeral();
        let other = address!("2222222222222222222222222222222222222222");
        b.check_or_bind(W, "alice").await.unwrap();
        assert_eq!(b.check_or_bind(other, "bob").await.unwrap(), Bind::BoundNow);
    }

    #[tokio::test]
    async fn a_binding_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("cs-binding-{}", std::process::id()));
        let path = dir.join("humans.json");
        let _ = std::fs::remove_file(&path);
        HumanBindings::load(path.clone()).unwrap().check_or_bind(W, "alice").await.unwrap();

        let reloaded = HumanBindings::load(path.clone()).unwrap();
        assert_eq!(reloaded.check_or_bind(W, "mallory").await.unwrap(), Bind::WrongHuman);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn fingerprints_are_stable_short_and_do_not_contain_the_subject() {
        let f = fingerprint("DBDSY-very-private-subject");
        assert_eq!(f, fingerprint("DBDSY-very-private-subject"));
        assert_ne!(f, fingerprint("someone-else"));
        assert!(f.starts_with("human-") && f.len() == 14);
        assert!(!f.contains("DBDSY"));
    }
}
