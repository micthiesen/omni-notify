//! `src/emails/templates.ts`: the log digest email.

use omni_core::LogLevel;

/// One log entry for [`render_log_email`] (mitools `LogItem`).
#[derive(Clone, Debug, PartialEq)]
pub struct LogEmailItem {
    pub level: LogLevel,
    pub message: String,
    /// Structured arguments, each rendered with `JSON.stringify`.
    pub args: Vec<serde_json::Value>,
}

/// HTML and plain-text bodies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmailContent {
    pub html: String,
    pub text: String,
}

/// `renderLogEmail`.
pub fn render_log_email(subject: &str, logs: &[LogEmailItem]) -> EmailContent {
    let html_lines: Vec<String> = logs
        .iter()
        .map(|log| {
            let args = format_args_suffix(&log.args);
            format!(
                "<span style=\"color: {}\">[{}]</span> {}{}",
                log_color(log.level),
                log.level.as_str().to_uppercase(),
                escape_html(&log.message),
                escape_html(&args)
            )
        })
        .collect();
    let text_lines: Vec<String> = logs
        .iter()
        .map(|log| {
            format!(
                "[{}] {}{}",
                log.level.as_str().to_uppercase(),
                log.message,
                format_args_suffix(&log.args)
            )
        })
        .collect();
    let title = escape_html(subject);
    let html = format!(
        "<!DOCTYPE html>
<html>
<head>
  <meta charset=\"utf-8\">
  <title>{title}</title>
</head>
<body style=\"font-family: system-ui, -apple-system, sans-serif; padding: 20px; max-width: 800px;\">
  <h1 style=\"font-size: 18px; margin-bottom: 16px;\">{title}</h1>
  <pre style=\"background: #f5f5f5; padding: 16px; border-radius: 4px; overflow-x: auto; font-family: 'SF Mono', Monaco, Consolas, monospace; font-size: 13px; line-height: 1.4;\">{}</pre>
</body>
</html>",
        html_lines.join("\n")
    );
    let underline = "=".repeat(omni_core::js::utf16_len(subject));
    let text = format!("{subject}\n{underline}\n\n{}", text_lines.join("\n"));
    EmailContent { html, text }
}

fn format_args_suffix(args: &[serde_json::Value]) -> String {
    if args.is_empty() {
        return String::new();
    }
    let rendered: Vec<String> = args.iter().map(omni_core::js::json_stringify).collect();
    format!(" {}", rendered.join(" "))
}

fn log_color(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Debug => "#888",
        LogLevel::Info => "#333",
        LogLevel::Warn => "#b45309",
        LogLevel::Error => "#dc2626",
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#039;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_html_and_text() {
        let content = render_log_email(
            "Logs <today>",
            &[
                LogEmailItem {
                    level: LogLevel::Warn,
                    message: "careful & slow".to_owned(),
                    args: vec![serde_json::json!({ "code": 7 })],
                },
                LogEmailItem {
                    level: LogLevel::Info,
                    message: "fine".to_owned(),
                    args: Vec::new(),
                },
            ],
        );
        assert_eq!(
            content.text,
            "Logs <today>\n============\n\n[WARN] careful & slow {\"code\":7}\n[INFO] fine"
        );
        assert!(content.html.contains("<title>Logs &lt;today&gt;</title>"));
        assert!(content.html.contains(
            "<span style=\"color: #b45309\">[WARN]</span> careful &amp; slow {&quot;code&quot;:7}"
        ));
    }
}
