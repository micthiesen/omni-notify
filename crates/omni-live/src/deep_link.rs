//! Native app links for watching a stream.

use crate::platform::Platform;

/// The URL that opens `username`'s stream in the platform's app: Twitch's
/// `twitch://stream/<login>`. Kick and YouTube have none; their https watch
/// URLs are universal links that already open the app.
pub fn app_url(platform: Platform, username: &str) -> Option<String> {
    match platform {
        Platform::Twitch => {
            let valid = !username.is_empty()
                && username
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_');
            valid.then(|| format!("twitch://stream/{}", username.to_ascii_lowercase()))
        }
        Platform::Kick | Platform::YouTube => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn twitch_links_open_the_stream_by_lowercase_login() {
        assert_eq!(
            app_url(Platform::Twitch, "Vinesauce"),
            Some("twitch://stream/vinesauce".into())
        );
        assert_eq!(
            app_url(Platform::Twitch, "a_b9"),
            Some("twitch://stream/a_b9".into())
        );
    }

    #[test]
    fn invalid_twitch_logins_have_no_link() {
        assert_eq!(app_url(Platform::Twitch, ""), None);
        assert_eq!(app_url(Platform::Twitch, "a/b"), None);
        assert_eq!(app_url(Platform::Twitch, "a?x=1"), None);
    }

    #[test]
    fn kick_and_youtube_rely_on_universal_links() {
        assert_eq!(app_url(Platform::Kick, "destiny"), None);
        assert_eq!(app_url(Platform::YouTube, "@destiny"), None);
    }
}
