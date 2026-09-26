//! Validate a World ID for Agents setup on its own, before involving payments.
//!
//!   cargo run -p countersigner --bin world_check
//!
//! Walks the whole journey the prize asks to see -- discovery, a device
//! authorization request, human completion, and a backend-validated result --
//! and reports exactly which step fails if one does.
//!
//! Set `NTFY_TOPIC` to have the device code and the final verdict pushed to a
//! phone, so the approval can be tested away from the terminal. The channel is
//! pinged first, because a device code lives five minutes and finding out the
//! push was broken afterwards wastes one.

use anyhow::Result;
use countersigner_lib::env::{NotifyEnv, WorldEnv};
use countersigner_lib::notify::{human_duration, Notifier, Priority, Push};
use countersigner_lib::world::{DeniedReason, PollOutcome, WorldId};

fn ok(msg: &str) {
    println!("  \x1b[32mOK\x1b[0m    {msg}");
}
fn bad(msg: &str) {
    println!("  \x1b[31mFAIL\x1b[0m  {msg}");
}
fn info(msg: &str) {
    println!("        {msg}");
}

/// Send if a notifier is configured, and never let the result change the check.
async fn push(notifier: &Option<Notifier>, p: Push) {
    if let Some(n) = notifier {
        n.send(p).await;
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // this crate's own .env (wherever cargo was invoked from), then ./.env; refuses to
    // start on a file the parser would only half-read -- see common::utils::load_env
    common::utils::load_env(env!("CARGO_MANIFEST_DIR")).map_err(anyhow::Error::msg)?;
    tracing_subscriber::fmt().with_env_filter("warn").init();

    println!("\nWorld ID for Agents -- setup check\n");

    // ---- 1. phone push (optional, and never gates anything) ----
    // First, deliberately: this is the channel that reports every later failure, and a
    // device code lives five minutes -- finding out the push was broken afterwards
    // wastes one.
    println!("1. notifications");
    let notifier = match NotifyEnv::new() {
        Ok(env) => match Notifier::new(&env) {
            Some(n) => {
                info(&format!("topic  {}", n.describe()));
                let delivered = n
                    .send(
                        Push::new(
                            "Push channel ready",
                            "The device code and the verdict will arrive here.",
                        )
                        .tags("satellite_antenna")
                        .priority(Priority::Low),
                    )
                    .await;
                if delivered {
                    ok("test push delivered -- subscribe on your phone to see it");
                } else {
                    bad("the broker did not accept the test push (see the warning above)");
                    info("Continuing anyway: notifications are observability, never a control.");
                }
                Some(n)
            }
            None => {
                info("NTFY_TOPIC is not set -- no phone push, terminal only");
                info("To enable: pick an unguessable topic, put it in countersigner/.env as");
                info("NTFY_TOPIC=..., and subscribe to it in the ntfy app or on ntfy.sh.");
                None
            }
        },
        Err(e) => {
            bad(&e);
            info("Continuing without notifications.");
            None
        }
    };

    // ---- 2. credentials present? ----
    println!("\n2. credentials");
    let world = match WorldEnv::new() {
        Ok(WorldEnv::Enabled { issuer, client_id, client_secret, require_orb }) => {
            ok(&format!("client id  {client_id}"));
            ok(&format!("issuer     {issuer}"));
            info(&format!("require_orb {require_orb}"));
            WorldId::new(issuer, client_id, client_secret, require_orb)
        }
        Ok(WorldEnv::Disabled) => {
            bad("WORLD_CLIENT_ID is not set");
            info("Register a confidential client at https://sandbox.auth.world.org/portal,");
            info("then put WORLD_CLIENT_ID and WORLD_CLIENT_SECRET in countersigner/.env");
            push(
                &notifier,
                Push::new(
                    "Setup incomplete · credentials",
                    "WORLD_CLIENT_ID is not set, so the human-approval path is off and any \
                     `ask` verdict becomes a refusal.",
                )
                .tags("warning")
                .priority(Priority::High),
            )
            .await;
            return Ok(());
        }
        Err(e) => {
            bad(&e);
            push(
                &notifier,
                Push::new("Setup failed · credentials", e.clone())
                    .tags("rotating_light")
                    .priority(Priority::High),
            )
            .await;
            return Ok(());
        }
    };

    // ---- 3. discovery ----
    println!("\n3. discovery");
    match world.discovery_summary().await {
        Ok(d) => {
            ok("reached the issuer's OIDC discovery document");
            for line in d {
                info(&line);
            }
        }
        Err(e) => {
            let mut m = e.to_string();
            for c in e.chain().skip(1) {
                m.push_str(&format!(" :: {c}"));
            }
            bad(&format!("discovery failed: {m}"));
            push(
                &notifier,
                Push::new("Setup failed · discovery", m.clone())
                    .tags("rotating_light")
                    .priority(Priority::High),
            )
            .await;
            return Ok(());
        }
    }

    // ---- 4. device authorization (proves the client credentials are real) ----
    println!("\n4. device authorization");
    let challenge = match world.start_device_flow().await {
        Ok(c) => {
            ok("the IdP accepted our client credentials and issued a device code");
            c
        }
        Err(e) => {
            // walk the error chain so the underlying OAuth error is visible
            let mut s = e.to_string();
            for cause in e.chain().skip(1) {
                s.push_str(&format!(" :: {cause}"));
            }
            bad(&s);
            if s.contains("invalid_client") || s.contains("401") {
                info("invalid_client => the client id/secret pair is wrong, or the client's");
                info("registered auth method is not client_secret_basic. Check the portal.");
            }
            push(
                &notifier,
                Push::new("Setup failed · device authorization", s.clone())
                    .tags("rotating_light")
                    .priority(Priority::High),
            )
            .await;
            return Ok(());
        }
    };

    println!("\n   ┌──────────────────────────────────────────────");
    println!("   │  open : {}", challenge.verification_uri);
    println!("   │  code : {}", challenge.user_code);
    if let Some(c) = &challenge.verification_uri_complete {
        println!("   │  or   : {c}");
    }
    println!("   └──────────────────────────────────────────────");
    println!("   expires in {}s, polling every {}s\n", challenge.expires_in, challenge.interval);

    // The URI that pre-fills the code is the one worth tapping on a phone.
    let tap_uri = challenge
        .verification_uri_complete
        .clone()
        .unwrap_or_else(|| challenge.verification_uri.clone());

    push(
        &notifier,
        Push::new(
            // The code belongs in the title: it is the one thing that must be legible
            // from a lock screen without opening anything.
            format!("Approve · {}", challenge.user_code),
            format!(
                "Tap Approve below, or enter the code at {host}.\n\
                 Expires in {expiry} · deny it to test the blocked path.",
                host = short_host(&challenge.verification_uri),
                expiry = human_duration(challenge.expires_in),
            ),
        )
        .tags("closed_lock_with_key")
        .priority(Priority::Urgent)
        .click(&tap_uri)
        .view_action("Approve in World", &tap_uri),
    )
    .await;

    // ---- 5. wait for the human, then validate in this backend ----
    println!("5. human approval (approve, or deny to see the unsuccessful path)");
    let started = std::time::Instant::now();
    let budget = challenge.expires_in.min(300);
    let deadline = started + std::time::Duration::from_secs(budget);
    info(&format!("waiting up to {budget}s for you to approve (code lives {}s)", challenge.expires_in));
    let mut interval = challenge.interval.max(1);
    loop {
        if std::time::Instant::now() >= deadline {
            bad("device code expired before approval - that is the `expired` path");
            push(
                &notifier,
                Push::new(
                    "Expired · nobody approved",
                    format!(
                        "No approval within {}. No countersignature exists, so the voucher \
                         carries one signature and an on-chain claim reverts.",
                        human_duration(budget)
                    ),
                )
                .tags("hourglass")
                .priority(Priority::High),
            )
            .await;
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
        match world.poll(&challenge.device_code).await {
            Ok(PollOutcome::Pending) => {
                println!("        pending... {}s elapsed", started.elapsed().as_secs());
            }
            Ok(PollOutcome::SlowDown) => {
                interval += 5;
                info(&format!("slow_down - backing off to {interval}s"));
            }
            Ok(PollOutcome::Denied(r)) => {
                println!();
                ok(&format!(
                    "unsuccessful path reached cleanly after {}s: {r:?}",
                    started.elapsed().as_secs()
                ));
                info("No countersignature would be produced, so no voucher exists");
                info("and a claim submitted anyway reverts on-chain.");
                push(
                    &notifier,
                    Push::new(
                        format!("Denied · {}", reason_code(&r)),
                        format!(
                            "Blocked after {}. No countersignature was produced, so the voucher \
                             carries one signature and the escrow rejects the claim.",
                            human_duration(started.elapsed().as_secs())
                        ),
                    )
                    .tags("no_entry")
                    .priority(Priority::High),
                )
                .await;
                return Ok(());
            }
            Ok(PollOutcome::Approved(a)) => {
                println!();
                ok("id_token signature verified against the issuer's JWKS in THIS backend");
                ok(&format!("pairwise sub  {}", a.sub));
                info(&format!("issuer        {}", a.issuer));
                info(&format!("auth_time     {}", a.auth_time));
                info(&format!("acr           {:?}", a.acr));
                info(&format!("orb verified  {}", a.orb_verified));
                println!("\n  Setup is working. That subject is the budget key: it is private to");
                println!("  this app, stable for this person, and shared across their agents.\n");
                push(
                    &notifier,
                    Push::new(
                        "Approved · identity validated",
                        format!(
                            "Verified in the backend against World's JWKS.\n\
                             Fresh proof {age} · {assurance} · sub {sub}",
                            age = proof_age(a.auth_time),
                            assurance = short_acr(a.acr.as_deref(), a.orb_verified),
                            // Only a prefix: the pairwise sub identifies a person, and
                            // ntfy.sh is someone else's server.
                            sub = short_sub(&a.sub),
                        ),
                    )
                    .tags("white_check_mark"),
                )
                .await;
                return Ok(());
            }
            Err(e) => {
                println!();
                bad(&format!("{e}"));
                push(
                    &notifier,
                    Push::new("World ID check FAILED", format!("{e}"))
                        .tags("rotating_light")
                        .priority(Priority::High),
                )
                .await;
                return Ok(());
            }
        }
        use std::io::Write;
        let _ = std::io::stdout().flush();
    }
}

/// `AccessDenied` -> `access_denied`: the same vocabulary the OAuth error and the
/// service's own JSON use, rather than a Rust type name.
fn reason_code(r: &DeniedReason) -> String {
    serde_json::to_string(r)
        .unwrap_or_default()
        .trim_matches('"')
        .to_string()
}

/// How long ago the human actually proved themselves. This is the claim the whole
/// gate rests on, so it is shown as an age rather than a raw epoch -- and a
/// timestamp from the future is called out rather than rendered as "0s ago".
fn proof_age(auth_time: u64) -> String {
    if auth_time == 0 {
        return "at an unstated time".to_string();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if now >= auth_time {
        format!("{} ago", human_duration(now - auth_time))
    } else {
        format!("{} in the FUTURE (clock skew)", human_duration(auth_time - now))
    }
}

/// `https://world.org/oidc/acr/orb-v3` -> `orb-v3`. The full URI is precise and
/// unreadable on a phone.
fn short_acr(acr: Option<&str>, orb_verified: bool) -> String {
    match acr {
        Some(a) => a.rsplit('/').next().unwrap_or(a).to_string(),
        // An absent acr is not a failure: the device grant guarantees fresh proof
        // regardless, so say what is true rather than implying a downgrade.
        None if orb_verified => "assurance unstated".to_string(),
        None => "no assurance asserted".to_string(),
    }
}

/// `https://sandbox.auth.world.org/device` -> `sandbox.auth.world.org`
fn short_host(uri: &str) -> String {
    uri.trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or(uri)
        .to_string()
}

/// Enough of the pairwise subject to recognise it across runs, not enough to be a
/// useful identifier if the topic leaks.
fn short_sub(sub: &str) -> String {
    let head: String = sub.chars().take(8).collect();
    if sub.chars().count() > 8 {
        format!("{head}…")
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_codes_match_the_oauth_vocabulary() {
        assert_eq!(reason_code(&DeniedReason::AccessDenied), "access_denied");
        assert_eq!(reason_code(&DeniedReason::ExpiredToken), "expired_token");
        assert_eq!(reason_code(&DeniedReason::StaleAuthentication), "stale_authentication");
    }

    #[test]
    fn proof_age_reports_an_age_not_an_epoch() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(proof_age(now - 8), "8s ago");
        assert_eq!(proof_age(now - 90), "1m 30s ago");
        assert_eq!(proof_age(0), "at an unstated time");
        // World's step-up guide says to reject implausibly future timestamps, so a
        // skewed clock must be visible rather than rendered as "0s ago".
        assert!(proof_age(now + 600).contains("FUTURE"));
    }

    #[test]
    fn acr_is_readable_and_an_absent_one_is_not_called_a_failure() {
        assert_eq!(short_acr(Some("https://world.org/oidc/acr/orb-v3"), true), "orb-v3");
        assert_eq!(short_acr(None, true), "assurance unstated");
        assert_eq!(short_acr(None, false), "no assurance asserted");
    }

    #[test]
    fn host_is_stripped_to_something_a_phone_can_show() {
        assert_eq!(short_host("https://sandbox.auth.world.org/device"), "sandbox.auth.world.org");
        assert_eq!(short_host("http://localhost:8787/x"), "localhost:8787");
    }

    #[test]
    fn subject_is_truncated_before_it_leaves_the_machine() {
        let sub = "DBDSY3FKLMNOPQRSTUVWXYZBZDQ";
        let short = short_sub(sub);
        assert_eq!(short, "DBDSY3FK…");
        assert!(!short.contains("BZDQ"), "tail of the subject leaked");
        assert_eq!(short_sub("ABC"), "ABC", "already short enough");
    }
}
