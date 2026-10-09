//! Event extraction with the calendar model (`extractEvents.ts`).

use base64::Engine as _;
use omni_ai::{
    Ai, AiError, ContentPart, CostTag, FinishReason, GenerateRequest, LanguageModel, Message,
    ModelRole, OutputSpec, Role, costs::llm_cost_cents, schema::parse_object,
};
use omni_core::js::{json_stringify_pretty2, utf16_len, utf16_slice};

use super::prompt;
use super::sanitize::{SanitizeResult, is_degenerate_extraction, sanitize_extracted_events};
use super::schema::{CalendarEventExtraction, ExtractedEvent};
use crate::error::CalendarExtractionError;
use crate::logfile::{RunLogFile, code_block};
use crate::support::DownloadedAttachment;

const LOG: &str = "Main:CalendarEvents";
const MAX_BODY_CHARS: usize = 12_000;

/// An existing event shown to the model, tagged with its per-prompt handle.
#[derive(Clone, Debug, PartialEq)]
pub struct ExistingEventContext {
    pub id: String,
    pub title: String,
    pub start_date: String,
    pub start_time: Option<String>,
    pub end_date: Option<String>,
    pub end_time: Option<String>,
    pub all_day: bool,
    pub location: Option<String>,
    pub time_zone: Option<String>,
}

/// What the model sees of the email.
#[derive(Clone, Copy, Debug)]
pub struct EmailContent<'a> {
    pub subject: &'a str,
    pub from: &'a str,
    pub text_body: &'a str,
}

pub struct ExtractionInput<'a> {
    pub email: EmailContent<'a>,
    pub attachments: &'a [DownloadedAttachment],
    /// `config.TZ`; empty means unknown.
    pub local_time_zone: &'a str,
    pub existing_events: &'a [ExistingEventContext],
    pub now_ms: i64,
}

/// Extracted events plus the USD cents of every model call made (`None` when
/// any call ran on an unpriced model).
#[derive(Clone, Debug, PartialEq)]
pub struct ExtractionResult {
    pub events: Vec<ExtractedEvent>,
    pub cost_cents: Option<f64>,
}

fn format_existing_event(e: &ExistingEventContext) -> String {
    let mut parts = vec![format!("- [{}] \"{}\" on {}", e.id, e.title, e.start_date)];
    if e.all_day {
        parts.push("(all day)".to_owned());
    } else if let Some(start) = e.start_time.as_deref().filter(|s| !s.is_empty()) {
        match e.end_time.as_deref().filter(|s| !s.is_empty()) {
            Some(end) => parts.push(format!("{start}–{end}")),
            None => parts.push(format!("at {start}")),
        }
    }
    if let Some(tz) = e.time_zone.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("({tz})"));
    }
    if let Some(location) = e.location.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("@ {location}"));
    }
    parts.join(" ")
}

/// `toLocaleDateString("en-US", { weekday, year, month: "long", day })`.
fn current_date(now_ms: i64, time_zone: &str) -> String {
    let tz = jiff::tz::TimeZone::get(time_zone).unwrap_or(jiff::tz::TimeZone::UTC);
    jiff::Timestamp::from_millisecond(now_ms)
        .map(|ts| ts.to_zoned(tz).strftime("%A, %B %-d, %Y").to_string())
        .unwrap_or_default()
}

/// The exact TS prompt text.
pub fn build_prompt(input: &ExtractionInput<'_>) -> String {
    let body = utf16_slice(input.email.text_body, 0, MAX_BODY_CHARS);
    let mut text = String::with_capacity(8_192 + body.len());
    text.push_str(prompt::HEAD);
    if input.local_time_zone.is_empty() {
        text.push_str(prompt::NO_TZ_CLAUSE);
    } else {
        text.push_str(prompt::LOCAL_TZ_CLAUSE);
        text.push_str(input.local_time_zone);
    }
    text.push_str(prompt::TAIL);
    if !input.existing_events.is_empty() {
        text.push_str("\nExisting calendar events (created by this system):\n");
        let lines: Vec<String> = input
            .existing_events
            .iter()
            .map(format_existing_event)
            .collect();
        text.push_str(&lines.join("\n"));
        text.push('\n');
    }
    text.push_str("\nToday's date: ");
    text.push_str(&current_date(input.now_ms, input.local_time_zone));
    text.push_str("\n\nFrom: ");
    text.push_str(input.email.from);
    text.push_str("\nSubject: ");
    text.push_str(input.email.subject);
    text.push_str("\n\n");
    text.push_str(&body);
    text
}

struct Attempt {
    sanitized: SanitizeResult,
    cost_cents: Option<f64>,
}

fn extraction_error(error: &AiError) -> CalendarExtractionError {
    CalendarExtractionError {
        cause: error.to_string(),
        transient: true,
    }
}

async fn run_once(
    ai: &Ai,
    model: &dyn LanguageModel,
    content: &[ContentPart],
    log_file: Option<&RunLogFile>,
) -> Result<Attempt, CalendarExtractionError> {
    let request = GenerateRequest {
        messages: vec![Message {
            role: Role::User,
            content: content.to_vec(),
        }],
        output: Some(OutputSpec::of::<CalendarEventExtraction>()),
        ..GenerateRequest::default()
    };
    let response = ai
        .generate(
            model,
            &request,
            CostTag::for_role(ModelRole::CalendarExtraction),
        )
        .await
        .map_err(|e| extraction_error(&e))?;
    if response.finish == FinishReason::Length {
        return Err(extraction_error(&AiError::Schema(
            "No object generated: the response was cut off at the output token limit".to_owned(),
        )));
    }
    let output: CalendarEventExtraction =
        parse_object(&response.text).map_err(|e| extraction_error(&e))?;
    if let (Some(reasoning), Some(file)) = (response.reasoning.as_deref(), log_file)
        && !reasoning.is_empty()
    {
        file.section("Reasoning", &code_block(reasoning, None))
            .await;
        tracing::info!(target: LOG, "{}", code_block(reasoning, None));
    }
    let rendered = serde_json::to_value(&output)
        .map(|v| json_stringify_pretty2(&v))
        .unwrap_or_default();
    match log_file {
        Some(file) => {
            let block = code_block(&rendered, Some("json"));
            file.section("Extraction Response", &block).await;
            tracing::info!(target: LOG, "{block}");
        }
        None => tracing::info!(target: LOG, "Extraction response: {rendered}"),
    }
    tracing::info!(
        target: LOG,
        "Token usage: {} prompt, {} completion",
        response.usage.input_tokens,
        response.usage.output_tokens
    );
    let model_id = model.id().to_string();
    let cost_cents = llm_cost_cents(&model.id().model, &response.usage);
    if cost_cents.is_none() {
        tracing::debug!(target: LOG, "No pricing data for extraction model \"{model_id}\"");
    }
    let sanitized = sanitize_extracted_events(output.events);
    for issue in &sanitized.issues {
        tracing::warn!(target: LOG, "Sanitized extraction output: {issue}");
    }
    Ok(Attempt {
        sanitized,
        cost_cents,
    })
}

fn add_cost(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    Some(a? + b?)
}

/// Extracts calendar events from an email. A degenerate first answer gets one
/// fresh retry; the cleaner of the two wins and both calls count toward cost.
/// Model failures are transient (the email is queued for retry).
pub async fn extract_calendar_events(
    ai: &Ai,
    model: &dyn LanguageModel,
    input: &ExtractionInput<'_>,
    log_file: Option<&RunLogFile>,
) -> Result<ExtractionResult, CalendarExtractionError> {
    let model_id = model.id().to_string();
    let prompt_text = build_prompt(input);
    let attachment_names = input
        .attachments
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let summary = if input.attachments.is_empty() {
        format!(
            "Extraction prompt ({model_id}) [{} chars]",
            utf16_len(&prompt_text)
        )
    } else {
        format!(
            "Extraction prompt ({model_id}) [{} chars, attachments: {attachment_names}]",
            utf16_len(&prompt_text)
        )
    };
    if let Some(file) = log_file {
        file.section(
            &format!("Extraction Prompt ({model_id})"),
            &code_block(&prompt_text, None),
        )
        .await;
    }
    tracing::info!(target: LOG, "{summary}");

    let mut content = vec![ContentPart::Text { text: prompt_text }];
    if !input.attachments.is_empty() {
        for attachment in input.attachments {
            content.push(ContentPart::File {
                mime_type: attachment.mime_type.clone(),
                data_b64: base64::engine::general_purpose::STANDARD.encode(&attachment.data),
                filename: None,
            });
        }
        tracing::info!(
            target: LOG,
            "Including {} attachment(s): {attachment_names}",
            input.attachments.len()
        );
    }

    let first = run_once(ai, model, &content, log_file).await?;
    if !is_degenerate_extraction(&first.sanitized) {
        return Ok(ExtractionResult {
            events: first.sanitized.events,
            cost_cents: first.cost_cents,
        });
    }
    tracing::warn!(
        target: LOG,
        "Degenerate extraction output ({}); retrying once",
        first.sanitized.issues.join("; ")
    );
    let second = run_once(ai, model, &content, log_file).await?;
    let cost_cents = add_cost(first.cost_cents, second.cost_cents);
    if second.sanitized.issues.len() < first.sanitized.issues.len() {
        tracing::info!(target: LOG, "Retry produced a cleaner extraction; using the retry result");
        return Ok(ExtractionResult {
            events: second.sanitized.events,
            cost_cents,
        });
    }
    tracing::info!(target: LOG, "Retry did not improve on the first extraction; keeping the first");
    Ok(ExtractionResult {
        events: first.sanitized.events,
        cost_cents,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_events_render_like_ts() {
        let base = ExistingEventContext {
            id: "evt_1".to_owned(),
            title: "🦷 Dentist".to_owned(),
            start_date: "2026-06-17".to_owned(),
            start_time: Some("14:30".to_owned()),
            end_date: None,
            end_time: Some("15:30".to_owned()),
            all_day: false,
            location: Some("Clinic".to_owned()),
            time_zone: Some("America/Vancouver".to_owned()),
        };
        assert_eq!(
            format_existing_event(&base),
            "- [evt_1] \"🦷 Dentist\" on 2026-06-17 14:30–15:30 (America/Vancouver) @ Clinic"
        );
        let all_day = ExistingEventContext {
            all_day: true,
            location: None,
            time_zone: None,
            ..base
        };
        assert_eq!(
            format_existing_event(&all_day),
            "- [evt_1] \"🦷 Dentist\" on 2026-06-17 (all day)"
        );
    }

    #[test]
    fn prompt_has_ts_layout() {
        let input = ExtractionInput {
            email: EmailContent {
                subject: "Appointment",
                from: "clinic@example.com",
                text_body: "Tomorrow at 9",
            },
            attachments: &[],
            local_time_zone: "America/Vancouver",
            existing_events: &[],
            now_ms: 1_791_590_400_000,
        };
        let text = build_prompt(&input);
        assert!(text.starts_with("Extract calendar events from this email"));
        assert!(text.contains(
            ". When there are no geographic clues, use the recipient's local timezone: America/Vancouver\n- If only a date"
        ));
        assert!(text.ends_with(
            "- When in doubt, prefer \"create\"\n\nToday's date: Friday, October 9, 2026\n\nFrom: clinic@example.com\nSubject: Appointment\n\nTomorrow at 9"
        ));
    }
}
