//! Phone push over [ntfy](https://ntfy.sh), so the human-approval path can be
//! exercised without sitting in front of the terminal.
//!
//! Deliberately the opposite of `intercepta`. Screening fails CLOSED, because a
//! guard that opens when its data source is down is not a guard. A notifier makes
//! no decision at all -- it only reports one -- so every send here fails SOFT:
//! it logs and returns. No payment outcome may depend on whether a push broker
//! answered, and an unreachable phone must never become an approval.

use crate::env::NotifyEnv;
use std::time::Duration;

/// Short: a device code lives ~5 minutes, and nothing waits on this.
const TIMEOUT: Duration = Duration::from_secs(5);

/// ntfy priority. `Urgent` bypasses most phone quiet-hours settings, which is what
/// a time-boxed approval wants.
#[derive(Clone, Copy, Debug)]
pub enum Priority {
    Low,
    Default,
    High,
    Urgent,
}

impl Priority {
    fn as_u8(self) -> u8 {
        match self {
            Priority::Low => 2,
            Priority::Default => 3,
            Priority::High => 4,
            Priority::Urgent => 5,
        }
    }
}

/// One notification.
pub struct Push {
    title: String,
    body: String,
    priority: Priority,
    /// ntfy emoji shortcodes, comma separated.
    tags: String,
    /// Opened when the notification is tapped. For a device grant this is the
    /// verification URI, so approving is one tap rather than typing a code.
    click: Option<String>,
    /// A visible button: (label, url).
    action: Option<(String, String)>,
}

impl Push {
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            priority: Priority::Default,
            tags: String::new(),
            click: None,
            action: None,
        }
    }

    pub fn priority(mut self, p: Priority) -> Self {
        self.priority = p;
        self
    }

    pub fn tags(mut self, t: impl Into<String>) -> Self {
        self.tags = t.into();
        self
    }

    pub fn click(mut self, url: impl Into<String>) -> Self {
        self.click = Some(url.into());
        self
    }

    /// Adds a tappable button labelled `label` that opens `url`.
    pub fn view_action(mut self, label: &str, url: &str) -> Self {
        self.action = Some((label.to_string(), url.to_string()));
        self
    }
}

#[derive(Clone)]
pub struct Notifier {
    server: String,
    topic: String,
    token: Option<String>,
    /// Shown as the notification's avatar. ntfy renders PNG and JPEG only.
    icon: Option<String>,
    http: reqwest::Client,
}

impl Notifier {
    /// `None` when `NTFY_TOPIC` is unset -- the caller then simply does not push.
    pub fn new(env: &NotifyEnv) -> Option<Self> {
        match env {
            NotifyEnv::Disabled => None,
            NotifyEnv::Enabled { server, topic, token, icon } => Some(Self {
                server: server.clone(),
                topic: topic.clone(),
                token: token.clone(),
                icon: icon.clone(),
                http: reqwest::Client::builder()
                    .timeout(TIMEOUT)
                    // a push broker is not a place to follow redirects to
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .expect("http client"),
            }),
        }
    }

    /// Where pushes go, with the topic masked. An ntfy topic is a bearer
    /// capability -- anyone who knows it can read every message and publish more --
    /// and setup checks get screen-shared at hackathons.
    pub fn describe(&self) -> String {
        format!("{}/{}", self.server, mask(&self.topic))
    }

    /// Fails soft by design. Returns whether the broker accepted it, for a setup
    /// check that wants to report the channel as working.
    ///
    /// Published as JSON rather than through ntfy's `X-Title`-style headers. Headers
    /// are latin-1, so a title containing `·` or an em dash silently loses the
    /// character; JSON carries UTF-8 intact and makes header injection structurally
    /// impossible rather than something to sanitise for.
    pub async fn send(&self, push: Push) -> bool {
        let mut body = serde_json::json!({
            "topic": self.topic,
            "title": clean(&push.title),
            "message": clean(&push.body),
            "priority": push.priority.as_u8(),
        });
        let map = body.as_object_mut().expect("object");

        if !push.tags.is_empty() {
            map.insert(
                "tags".into(),
                push.tags.split(',').map(|t| t.trim().to_string()).collect(),
            );
        }
        if let Some(c) = &push.click {
            map.insert("click".into(), c.clone().into());
        }
        if let Some(i) = &self.icon {
            map.insert("icon".into(), i.clone().into());
        }
        if let Some((label, url)) = &push.action {
            map.insert(
                "actions".into(),
                serde_json::json!([{
                    "action": "view",
                    "label": label,
                    "url": url,
                    // dismiss once acted on, so a stale approval prompt cannot linger
                    // on the lock screen
                    "clear": true,
                }]),
            );
        }

        let mut req = self.http.post(&self.server).json(&body);
        if let Some(t) = &self.token {
            req = req.bearer_auth(t);
        }

        match req.send().await {
            Ok(r) if r.status().is_success() => true,
            Ok(r) => {
                let status = r.status();
                let text = r.text().await.unwrap_or_default();
                tracing::warn!(
                    %status,
                    "ntfy rejected the push (ignored -- notifications never gate a decision): {}",
                    text.trim()
                );
                false
            }
            Err(e) => {
                tracing::warn!(
                    "ntfy unreachable (ignored -- notifications never gate a decision): {e}"
                );
                false
            }
        }
    }
}

/// Strip control characters, keeping the newlines that lay a notification out, and
/// bound the length so one runaway error string cannot become the whole message.
fn clean(s: &str) -> String {
    s.chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .take(900)
        .collect()
}

/// `300` -> `5m`, `330` -> `5m 30s`, `45` -> `45s`. Notifications are read at a
/// glance, and nobody reads `330s` as five and a half minutes.
pub fn human_duration(secs: u64) -> String {
    match (secs / 60, secs % 60) {
        (0, s) => format!("{s}s"),
        (m, 0) => format!("{m}m"),
        (m, s) => format!("{m}m {s}s"),
    }
}

/// `0x1234…cdef`. A full address is unreadable on a lock screen, and the first and
/// last four digits are what a person actually compares against.
pub fn short_addr(a: &str) -> String {
    let n = a.chars().count();
    if n <= 12 {
        return a.to_string();
    }
    let head: String = a.chars().take(6).collect();
    let tail: String = a.chars().skip(n - 4).collect();
    format!("{head}…{tail}")
}

/// `countersign-9f3c2a` -> `co…2a`
fn mask(topic: &str) -> String {
    let n = topic.chars().count();
    if n <= 4 {
        return "*".repeat(n);
    }
    let first: String = topic.chars().take(2).collect();
    let last: String = topic.chars().skip(n - 2).collect();
    format!("{first}…{last}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_survives_the_publish_body() {
        // The bug this replaced: `·` is U+00B7, dropped by latin-1 headers, which
        // turned "Approve · CODE" into "Approve  CODE".
        assert_eq!(clean("Approve · MM4EV-TQV7S"), "Approve · MM4EV-TQV7S");
    }

    #[test]
    fn newlines_are_kept_but_other_control_characters_are_not() {
        let out = clean("line one\nline two\r\u{7}");
        assert_eq!(out, "line one\nline two");
    }

    #[test]
    fn message_length_is_bounded() {
        assert_eq!(clean(&"a".repeat(5000)).chars().count(), 900);
    }

    #[test]
    fn topic_is_masked_for_screen_sharing() {
        assert_eq!(mask("countersign-9f3c2a"), "co…2a");
        assert_eq!(mask("abc"), "***");
        assert!(!mask("countersign-9f3c2a").contains("9f3c"));
    }

    /// Real delivery against the real broker. Publishes to a single-use random topic
    /// and reads the message back, so "it returned 200" is not mistaken for "it
    /// arrived".
    ///
    ///   cargo test -p countersigner --lib notify -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "network"]
    async fn pushes_to_ntfy_for_real() {
        let topic = format!(
            "countersign-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let env = NotifyEnv::Enabled {
            server: "https://ntfy.sh".into(),
            topic: topic.clone(),
            token: None,
            icon: Some("https://github.com/worldcoin.png?size=200".into()),
        };
        let n = Notifier::new(&env).expect("enabled");

        let delivered = n
            .send(
                Push::new("Approve · TEST1-TEST2", "Expires in 20m · deny to test the block.")
                    .tags("closed_lock_with_key")
                    .priority(Priority::Urgent)
                    .click("https://sandbox.auth.world.org/device")
                    .view_action("Approve in World", "https://sandbox.auth.world.org/device"),
            )
            .await;
        assert!(delivered, "broker did not accept the push");

        // Read it back out of the topic. The broker answers the POST before the message
        // is readable, so a single immediate poll comes back empty perhaps half the time:
        // retry briefly rather than assert against a race.
        let mut body = String::new();
        for _ in 0..10 {
            let r = reqwest::get(format!("https://ntfy.sh/{topic}/json?poll=1"))
                .await
                .expect("poll");
            assert!(r.status().is_success(), "poll returned {}", r.status());
            body = r.text().await.expect("body");
            if !body.trim().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        eprintln!("topic {topic}\n{body}");
        assert!(body.contains("TEST1-TEST2"), "message not in the topic: {body}");
        // The regression this guards: latin-1 headers silently dropped U+00B7, so
        // "Approve · CODE" arrived as "Approve  CODE".
        assert!(body.contains("Approve \u{b7} TEST1-TEST2"), "UTF-8 separator lost: {body}");
        assert!(body.contains("\"priority\":5"), "urgent priority lost: {body}");
        assert!(body.contains("Approve in World"), "action button lost: {body}");
        assert!(body.contains("worldcoin.png"), "icon lost: {body}");
    }

    #[test]
    fn durations_read_like_a_human_wrote_them() {
        assert_eq!(human_duration(45), "45s");
        assert_eq!(human_duration(300), "5m");
        assert_eq!(human_duration(330), "5m 30s");
        assert_eq!(human_duration(0), "0s");
    }

    #[test]
    fn addresses_stay_comparable_when_shortened() {
        let a = "0x1234567890abcdef1234567890abcdef12345678";
        assert_eq!(short_addr(a), "0x1234…5678");
        // short enough to read whole: left alone rather than mangled
        assert_eq!(short_addr("0xdeadbeef"), "0xdeadbeef");
        assert_eq!(short_addr(""), "");
    }

    #[test]
    fn disabled_env_yields_no_notifier() {
        assert!(Notifier::new(&NotifyEnv::Disabled).is_none());
    }
}
