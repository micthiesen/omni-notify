//! SMTP setting resolution from explicit SMTP fields or iCloud credentials.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use omni_mailer::{
    ComposeValues, ICLOUD_SMTP_HOST, ICLOUD_SMTP_PORT, SmtpConfig, resolve_compose_values,
};

fn values() -> ComposeValues {
    ComposeValues {
        smtp_host: Some("smtp.example.test".to_owned()),
        smtp_port: 587.0,
        smtp_user: Some("smtp-user".to_owned()),
        smtp_pass: Some("smtp-pass".to_owned()),
        icloud_username: Some("icloud-user".to_owned()),
        icloud_app_password: Some("icloud-pass".to_owned()),
    }
}

fn without_smtp() -> ComposeValues {
    ComposeValues {
        smtp_host: Some(String::new()),
        smtp_user: Some(String::new()),
        smtp_pass: Some(String::new()),
        ..values()
    }
}

#[test]
fn uses_complete_smtp_settings_and_selects_tls_from_the_port() {
    assert_eq!(
        resolve_compose_values(&values()),
        Some(SmtpConfig::Explicit {
            host: "smtp.example.test".to_owned(),
            port: 587,
            user: "smtp-user".to_owned(),
            pass: "smtp-pass".to_owned(),
            implicit_tls: false,
        })
    );
    let implicit = resolve_compose_values(&ComposeValues {
        smtp_port: 465.0,
        ..values()
    });
    assert!(matches!(
        implicit,
        Some(SmtpConfig::Explicit {
            implicit_tls: true,
            port: 465,
            ..
        })
    ));
}

#[test]
fn uses_icloud_credentials_without_using_the_login_as_the_sender() {
    assert_eq!(
        resolve_compose_values(&without_smtp()),
        Some(SmtpConfig::ICloud {
            user: "icloud-user".to_owned(),
            app_password: "icloud-pass".to_owned(),
        })
    );
    assert_eq!(ICLOUD_SMTP_HOST, "smtp.mail.me.com");
    assert_eq!(ICLOUD_SMTP_PORT, 587);
}

#[test]
fn rejects_partial_smtp_configuration_and_incomplete_icloud_credentials() {
    assert_eq!(
        resolve_compose_values(&ComposeValues {
            smtp_pass: Some(String::new()),
            ..values()
        }),
        None
    );
    assert_eq!(
        resolve_compose_values(&ComposeValues {
            icloud_app_password: Some(String::new()),
            ..without_smtp()
        }),
        None
    );
    assert_eq!(
        resolve_compose_values(&ComposeValues {
            icloud_username: Some(String::new()),
            ..without_smtp()
        }),
        None
    );
}

#[test]
fn rejects_an_invalid_port() {
    assert_eq!(
        resolve_compose_values(&ComposeValues {
            smtp_port: 0.0,
            ..values()
        }),
        None
    );
}
