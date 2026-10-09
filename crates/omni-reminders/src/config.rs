//! Feature configuration (`src/reminders/config.ts`).

use std::path::PathBuf;

/// Raw `ICLOUD_REMINDERS_*` settings plus the private state directory.
#[derive(Clone, Default)]
pub struct RemindersConfiguration {
    pub enabled: Option<String>,
    pub account: Option<String>,
    pub password: Option<String>,
    pub storage_key: Option<String>,
    pub public_origin: Option<String>,
    pub directory: PathBuf,
}

impl std::fmt::Debug for RemindersConfiguration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemindersConfiguration")
            .field("enabled", &self.enabled)
            .field("account", &self.account.as_ref().map(|_| "<redacted>"))
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field(
                "storage_key",
                &self.storage_key.as_ref().map(|_| "<redacted>"),
            )
            .field("public_origin", &self.public_origin)
            .field("directory", &self.directory)
            .finish()
    }
}

impl RemindersConfiguration {
    /// Reads the settings from the application configuration.
    pub fn from_config(config: &omni_config::Config, directory: PathBuf) -> Self {
        Self {
            enabled: config.icloud_reminders_enabled.clone(),
            account: config.icloud_reminders_account.clone(),
            password: config.icloud_reminders_password.clone(),
            storage_key: config.icloud_reminders_storage_key.clone(),
            public_origin: config.icloud_reminders_public_origin.clone(),
            directory,
        }
    }
}

/// A 64-character hexadecimal storage key.
pub(crate) fn is_storage_key(key: &str) -> bool {
    key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Invalid or incomplete configuration disables this feature, never the application.
pub fn reminders_configured(config: &RemindersConfiguration) -> bool {
    if config.enabled.as_deref() != Some("true")
        || config.account.as_deref().is_none_or(crate::json::js_blank)
        || config.password.as_deref().is_none_or(str::is_empty)
        || !is_storage_key(config.storage_key.as_deref().unwrap_or(""))
    {
        return false;
    }
    let Some(origin) = config.public_origin.as_deref() else {
        return false;
    };
    match url::Url::parse(origin) {
        Ok(url) => {
            url.scheme() == "https"
                && url.origin().ascii_serialization() == origin
                && url.username().is_empty()
                && url.password().is_none()
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete() -> RemindersConfiguration {
        RemindersConfiguration {
            enabled: Some("true".into()),
            account: Some("test@example.com".into()),
            password: Some("secret".into()),
            storage_key: Some("a".repeat(64)),
            public_origin: Some("https://omni.example.test".into()),
            directory: PathBuf::from("/tmp/unused"),
        }
    }

    #[test]
    fn requires_every_setting() {
        assert!(reminders_configured(&complete()));
        for broken in [
            RemindersConfiguration {
                enabled: Some("TRUE".into()),
                ..complete()
            },
            RemindersConfiguration {
                account: Some("  ".into()),
                ..complete()
            },
            RemindersConfiguration {
                password: Some(String::new()),
                ..complete()
            },
            RemindersConfiguration {
                storage_key: Some("g".repeat(64)),
                ..complete()
            },
            RemindersConfiguration {
                storage_key: None,
                ..complete()
            },
            RemindersConfiguration {
                public_origin: Some("http://omni.example.test".into()),
                ..complete()
            },
            RemindersConfiguration {
                public_origin: Some("https://omni.example.test/".into()),
                ..complete()
            },
            RemindersConfiguration {
                public_origin: Some("https://user@omni.example.test".into()),
                ..complete()
            },
            RemindersConfiguration {
                public_origin: Some("https://Omni.example.test".into()),
                ..complete()
            },
        ] {
            assert!(!reminders_configured(&broken), "{broken:?}");
        }
    }
}
