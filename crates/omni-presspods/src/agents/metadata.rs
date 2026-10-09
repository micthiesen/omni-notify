//! Article validation and metadata extraction.

use std::sync::LazyLock;

use omni_ai::{CostTag, GenerateRequest, ModelRole};
use regex::Regex;
use serde::Deserialize;

use super::Agents;
use crate::costs::{CompletionUsage, CostCounter};
use crate::dates::{js_date_string, parse_js_date};
use crate::error::PressPodsError;
use crate::model::AuthorGender;
use crate::types::{Article, MetadataInfo};

const OPERATION: &str = "generate PressPods article metadata";

const SYSTEM: &str = r#"You are an expert at extracting article metadata for podcast generation. Your goal is to extract useful metadata that enhances the listening experience. When uncertain, make reasonable inferences rather than leaving fields empty - approximate data is better than none for our use case.

Given webpage content, determine if it's valid and extract metadata. In the case of multiple authors, choose one as the primary.

Mark as INVALID only if:
  - Error page (404, 500, access denied, etc.)
  - Login or paywall with no content preview
  - Completely empty or blank page
  - Non-article media page (just a video/audio player with no text)

Content Rating (0-10) - Rate how successfully we extracted the article:
  - 10: Complete article captured perfectly
  - 7-9: Main article content captured, minor ads/nav elements included
  - 4-6: Partial content, significant sections missing or truncated
  - 0-3: Extraction mostly failed (got mainly ads/nav instead of article)
  - This measures extraction quality, NOT article quality

For author extraction, try in order:
  1. Byline (e.g., "By Jane Smith")
  2. Author bio section
  3. URL pattern (e.g., /author/jane-smith/)
  4. Copyright or attribution notice
  5. Reasonable inference from domain/publication

For author gender:
  - Infer from pronouns in bio (she/her → female, he/him → male)
  - Statistical inference from first name is acceptable
  - Only use 'unknown' if truly ambiguous - educated guesses preferred

For shortSummary: Create a one-sentence description of what this article is about (for podcast descriptions). Do NOT include URLs in the summary.

Other webpages (blog posts, wiki pages, forum threads, documentation) should be treated as valid articles.
X Articles and connected same-author X threads are valid articles. Judge completeness against the article body or connected self-thread, not against unrelated replies. Prefer explicit source metadata for their title, author, publication (X), publication date, and lead image instead of inferring replacements."#;

/// `rawMetadataInfoSchema`: every field required, most nullable.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawMetadataInfo {
    pub is_valid_article: bool,
    pub title: Option<String>,
    pub author: Option<String>,
    pub author_gender: Option<AuthorGender>,
    pub coauthors: Option<Vec<String>>,
    pub publication: Option<String>,
    #[serde(rename = "publishedAtISO")]
    pub published_at_iso: Option<String>,
    pub lead_image_url: Option<String>,
    pub short_summary: Option<String>,
    pub content_rating: Option<f64>,
}

static URL_RE: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"https?://\S+").ok());
static BLANK_LINES_RE: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"\n{2,}").ok());

fn strip_urls(text: &str) -> String {
    let without = URL_RE
        .as_ref()
        .map(|re| re.replace_all(text, "").into_owned())
        .unwrap_or_else(|| text.to_owned());
    BLANK_LINES_RE
        .as_ref()
        .map(|re| re.replace_all(&without, "\n").into_owned())
        .unwrap_or(without)
        .trim()
        .to_owned()
}

/// `transformMetadataInfo`.
pub(crate) fn transform_metadata(raw: RawMetadataInfo, tz: &jiff::tz::TimeZone) -> MetadataInfo {
    MetadataInfo {
        is_valid_article: raw.is_valid_article,
        title: raw.title,
        author: raw
            .author
            .filter(|author| !author.is_empty() && !author.contains("unknown")),
        author_gender: raw.author_gender,
        coauthors: raw.coauthors,
        publication: raw.publication,
        published_at: raw
            .published_at_iso
            .filter(|s| !s.is_empty())
            .and_then(|s| parse_js_date(&s, tz)),
        lead_image_url: raw.lead_image_url,
        short_summary: raw
            .short_summary
            .filter(|s| !s.is_empty())
            .map(|s| strip_urls(&s)),
        content_rating: raw.content_rating.unwrap_or(0.0),
    }
}

impl Agents {
    /// Validates the article and extracts its metadata, including the 0-10
    /// extraction-quality rating that picks the best retriever.
    pub async fn article_metadata(
        &self,
        article: &Article,
        costs: &CostCounter,
    ) -> Result<MetadataInfo, PressPodsError> {
        let role = ModelRole::PressPodsMetadata;
        let model = self
            .ai
            .model_for(&self.config, role)
            .map_err(|e| PressPodsError::ai(OPERATION, e))?;
        let published = article
            .published_at
            .map(|ms| js_date_string(ms, &self.tz))
            .unwrap_or_default();
        let prompt = format!(
            "Please validate the following article and extract its metadata if it is valid.\n\n\
             Potential Article Info:\n\
             Title: {}\n\
             Author: {}\n\
             Domain: {}\n\
             Article URL: {}\n\
             Published At: {published}\n\
             Lead Image URL: {}\n\n\
             Webpage Content (HTML converted to plain text):\n{}",
            article.title.as_deref().unwrap_or(""),
            article.author.as_deref().unwrap_or(""),
            article.domain.as_deref().unwrap_or(""),
            article.url,
            article.lead_image_url.as_deref().unwrap_or(""),
            article.text,
        );
        let request = GenerateRequest {
            system: Some(SYSTEM.to_owned()),
            ..GenerateRequest::prompt(prompt)
        };
        let (raw, usage) = self
            .ai
            .generate_object::<RawMetadataInfo>(model.as_ref(), request, CostTag::for_role(role))
            .await
            .map_err(|e| PressPodsError::ai(OPERATION, e))?;
        costs.record_llm_usage(
            self.config.model(role),
            "meta",
            CompletionUsage::from(usage),
        );
        Ok(transform_metadata(raw, &self.tz))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw() -> RawMetadataInfo {
        RawMetadataInfo {
            is_valid_article: true,
            title: Some("T".into()),
            author: Some("unknown author".into()),
            author_gender: Some(AuthorGender::Female),
            coauthors: None,
            publication: None,
            published_at_iso: Some("2026-07-14T14:04:31Z".into()),
            lead_image_url: None,
            short_summary: Some("See https://x.test/a now\n\n\nok ".into()),
            content_rating: None,
        }
    }

    #[test]
    fn transforms_raw_model_output_like_ts() {
        let tz = jiff::tz::TimeZone::UTC;
        let info = transform_metadata(raw(), &tz);
        assert_eq!(info.author, None);
        assert_eq!(info.short_summary.as_deref(), Some("See  now\nok"));
        assert_eq!(info.published_at, Some(1_784_037_871_000));
        assert_eq!(info.content_rating, 0.0);
        let invalid_date = transform_metadata(
            RawMetadataInfo {
                published_at_iso: Some("whenever".into()),
                ..raw()
            },
            &tz,
        );
        assert_eq!(invalid_date.published_at, None);
    }
}
