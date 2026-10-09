//! In-memory alert throttle: per-key escalating
//! cooldowns of 15 m, 30 m, 1 h, then 3 h; a key silent for 6 h is a fresh
//! incident; at most 500 keys, least-recently-seen evicted first.

use std::collections::VecDeque;

const DEFAULT_COOLDOWNS_MS: [i64; 4] = [15 * 60_000, 30 * 60_000, 60 * 60_000, 3 * 60 * 60_000];
const DEFAULT_RESET_MS: i64 = 6 * 60 * 60_000;
const DEFAULT_MAX_KEYS: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThrottledAlert {
    pub key: String,
    pub title: String,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmittedAlert {
    pub title: String,
    pub body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlertThrottleOptions {
    pub cooldowns_ms: Vec<i64>,
    pub reset_ms: i64,
    pub max_keys: usize,
}

impl Default for AlertThrottleOptions {
    fn default() -> Self {
        Self {
            cooldowns_ms: DEFAULT_COOLDOWNS_MS.to_vec(),
            reset_ms: DEFAULT_RESET_MS,
            max_keys: DEFAULT_MAX_KEYS,
        }
    }
}

#[derive(Clone, Debug)]
struct AlertEntry {
    key: String,
    last_sent_at: i64,
    deliveries: usize,
    suppressed: u64,
}

/// Per-key escalating cooldown; entries kept in LRU order (front = oldest).
#[derive(Clone, Debug, Default)]
pub struct AlertThrottle {
    options: AlertThrottleOptions,
    entries: VecDeque<AlertEntry>,
}

impl AlertThrottle {
    pub fn new(options: AlertThrottleOptions) -> Self {
        Self {
            options,
            entries: VecDeque::new(),
        }
    }

    /// The alert to send now (with a repeat summary when earlier copies were
    /// suppressed), or `None` while the key is cooling down.
    pub fn admit(&mut self, alert: ThrottledAlert, now: i64) -> Option<AdmittedAlert> {
        let Some(mut entry) = self.touch(&alert.key, now) else {
            self.entries.push_back(AlertEntry {
                key: alert.key,
                last_sent_at: now,
                deliveries: 1,
                suppressed: 0,
            });
            while self.entries.len() > self.options.max_keys {
                self.entries.pop_front();
            }
            return Some(AdmittedAlert {
                title: alert.title,
                body: alert.body,
            });
        };

        let cooldowns = &self.options.cooldowns_ms;
        let index = entry
            .deliveries
            .saturating_sub(1)
            .min(cooldowns.len().saturating_sub(1));
        let cooldown = cooldowns.get(index).copied().unwrap_or(0);
        if now - entry.last_sent_at < cooldown {
            entry.suppressed += 1;
            self.entries.push_back(entry);
            return None;
        }

        let since_ms = now - entry.last_sent_at;
        let suppressed = entry.suppressed;
        entry.last_sent_at = now;
        entry.deliveries += 1;
        entry.suppressed = 0;
        self.entries.push_back(entry);
        if suppressed == 0 {
            return Some(AdmittedAlert {
                title: alert.title,
                body: alert.body,
            });
        }
        Some(AdmittedAlert {
            title: alert.title,
            body: format!(
                "{}\n\nRepeated {} times in the last {}.",
                alert.body,
                suppressed + 1,
                format_elapsed(since_ms)
            ),
        })
    }

    /// Removes the key's entry and returns it unless it has expired.
    fn touch(&mut self, key: &str, now: i64) -> Option<AlertEntry> {
        let position = self.entries.iter().position(|entry| entry.key == key)?;
        let entry = self.entries.remove(position)?;
        (now - entry.last_sent_at < self.options.reset_ms).then_some(entry)
    }
}

/// `alertKey`: logger name plus the whitespace-normalized, lowercased title
/// (first 200 UTF-16 units).
pub fn alert_key(logger_name: &str, title: &str) -> String {
    // JS `\s` is Unicode White_Space plus U+FEFF.
    let normalized = title
        .to_lowercase()
        .split(|c: char| c.is_whitespace() || c == '\u{feff}')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let normalized = omni_core::js::utf16_slice(&normalized, 0, 200);
    format!("{logger_name}|{normalized}")
}

/// `formatElapsed`: "1m", "45m", "2h", "2h15m".
pub fn format_elapsed(ms: i64) -> String {
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    let minutes = (ms as f64 / 60_000.0).round() as i64;
    if minutes < 60 {
        return format!("{}m", minutes.max(1));
    }
    let hours = minutes / 60;
    let rest = minutes % 60;
    if rest > 0 {
        format!("{hours}h{rest}m")
    } else {
        format!("{hours}h")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alert() -> ThrottledAlert {
        ThrottledAlert {
            key: "k".to_owned(),
            title: "t".to_owned(),
            body: "b".to_owned(),
        }
    }

    #[test]
    fn escalates_and_summarizes_repeats() {
        let mut throttle = AlertThrottle::default();
        assert!(throttle.admit(alert(), 0).is_some());
        assert!(throttle.admit(alert(), 60_000).is_none());
        assert!(throttle.admit(alert(), 14 * 60_000).is_none());
        let admitted = throttle.admit(alert(), 15 * 60_000);
        assert_eq!(
            admitted.map(|a| a.body),
            Some("b\n\nRepeated 3 times in the last 15m.".to_owned())
        );
        assert!(throttle.admit(alert(), 40 * 60_000).is_none());
        assert!(
            throttle
                .admit(alert(), 15 * 60_000 + DEFAULT_RESET_MS)
                .is_some()
        );
    }

    #[test]
    fn evicts_least_recent_keys() {
        let mut throttle = AlertThrottle::new(AlertThrottleOptions {
            max_keys: 2,
            ..Default::default()
        });
        for key in ["a", "b", "c"] {
            throttle.admit(
                ThrottledAlert {
                    key: key.to_owned(),
                    ..alert()
                },
                0,
            );
        }
        assert!(
            throttle
                .admit(
                    ThrottledAlert {
                        key: "a".to_owned(),
                        ..alert()
                    },
                    1
                )
                .is_some()
        );
        assert!(
            throttle
                .admit(
                    ThrottledAlert {
                        key: "c".to_owned(),
                        ..alert()
                    },
                    1
                )
                .is_none()
        );
    }

    #[test]
    fn keys_and_elapsed_labels() {
        assert_eq!(
            alert_key("Main", "  Error   Running\tTask "),
            "Main|error running task"
        );
        assert_eq!(format_elapsed(10_000), "1m");
        assert_eq!(format_elapsed(45 * 60_000), "45m");
        assert_eq!(format_elapsed(120 * 60_000), "2h");
        assert_eq!(format_elapsed(135 * 60_000), "2h15m");
    }
}
