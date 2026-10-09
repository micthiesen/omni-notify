//! The print pipeline's external edges: the public PDF download and the
//! `pdfinfo` / `cupsfilter` / `rastertobrlaser` subprocesses.

use std::time::Duration;

use bytes::Bytes;
use futures::future::BoxFuture;
use omni_core::process::run_bounded;
use omni_http::public::PublicHttpClient;
use omni_http::{HttpError, Method, RedirectRule, Url};

use super::service::{MAX_PDF_BYTES, PRINTER_DOWNLOAD_USER_AGENT, assert_https_document_url};

const MAX_DOWNLOAD_REDIRECTS: u8 = 3;
const PROCESS_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_STDERR_BYTES: usize = 64 * 1024;

/// A downloaded document.
#[derive(Clone, Debug, PartialEq)]
pub struct DownloadedFile {
    pub body: Bytes,
    pub content_type: Option<String>,
}

/// Fetches the document to print (a seam for tests).
pub trait PdfDownloader: Send + Sync {
    fn download<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<DownloadedFile, String>>;
}

/// Public-internet download: every hop must be a public HTTPS URL without
/// credentials, at most three redirects, 20 MiB.
pub struct PublicPdfDownloader {
    http: PublicHttpClient,
    timeout: Duration,
}

impl PublicPdfDownloader {
    pub fn new(http: PublicHttpClient, timeout: Duration) -> Self {
        Self { http, timeout }
    }
}

fn http_message(error: &HttpError) -> String {
    match error {
        HttpError::TooLarge { .. } => {
            format!("PDF must be between 1 byte and {MAX_PDF_BYTES} bytes")
        }
        other => other.to_string(),
    }
}

impl PdfDownloader for PublicPdfDownloader {
    fn download<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<DownloadedFile, String>> {
        Box::pin(async move {
            let work = async {
                let mut current = url.clone();
                let mut hops = 0u8;
                loop {
                    let response = self
                        .http
                        .request(Method::GET, current.clone())
                        .header("user-agent", PRINTER_DOWNLOAD_USER_AGENT)
                        .header("accept", "application/pdf, application/octet-stream")
                        .redirect(RedirectRule::None)
                        .send_bounded(MAX_PDF_BYTES)
                        .await
                        .map_err(|e| http_message(&e))?;
                    let status = response.status.as_u16();
                    let location = response
                        .headers
                        .get("location")
                        .and_then(|v| v.to_str().ok());
                    if matches!(status, 301 | 302 | 303 | 307 | 308)
                        && let Some(location) = location
                    {
                        if hops >= MAX_DOWNLOAD_REDIRECTS {
                            return Err(format!(
                                "Redirected more than {MAX_DOWNLOAD_REDIRECTS} times"
                            ));
                        }
                        let next = current
                            .join(location)
                            .map_err(|e| format!("Invalid redirect location: {e}"))?;
                        current = assert_https_document_url(next.as_str(), "Document redirects")?;
                        hops += 1;
                        continue;
                    }
                    if !response.status.is_success() {
                        return Err(format!(
                            "Response code {status} ({})",
                            response.status.canonical_reason().unwrap_or("")
                        ));
                    }
                    let content_type = response
                        .headers
                        .get("content-type")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned);
                    return Ok(DownloadedFile {
                        body: response.body,
                        content_type,
                    });
                }
            };
            tokio::time::timeout(self.timeout, work)
                .await
                .map_err(|_| "Timeout awaiting the document download".to_owned())?
        })
    }
}

/// Runs the conversion tools (a seam for tests).
pub trait PrintProcesses: Send + Sync {
    /// Captures stdout as text (`pdfinfo`).
    fn run_text<'a>(
        &'a self,
        executable: &'a str,
        args: &'a [String],
    ) -> BoxFuture<'a, Result<String, String>>;
    /// Captures stdout as bytes, with extra environment variables.
    fn run_binary<'a>(
        &'a self,
        executable: &'a str,
        args: &'a [String],
        environment: &'a [(String, String)],
        max_bytes: usize,
    ) -> BoxFuture<'a, Result<Vec<u8>, String>>;
}

/// Real subprocesses: 60 s timeout, bounded output, killed on drop.
pub struct SystemProcesses;

async fn run(
    executable: &str,
    args: &[String],
    environment: &[(String, String)],
    max_stdout: usize,
) -> Result<Vec<u8>, String> {
    let mut command = tokio::process::Command::new(executable);
    command.args(args);
    for (key, value) in environment {
        command.env(key, value);
    }
    let output = run_bounded(command, None, max_stdout, MAX_STDERR_BYTES, PROCESS_TIMEOUT)
        .await
        .map_err(|e| format!("{executable} failed: {e}"))?;
    if output.status != 0 {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = crate::js::trim(&detail);
        return Err(if detail.is_empty() {
            format!("{executable} failed")
        } else {
            format!("{executable} failed: {detail}")
        });
    }
    Ok(output.stdout)
}

impl PrintProcesses for SystemProcesses {
    fn run_text<'a>(
        &'a self,
        executable: &'a str,
        args: &'a [String],
    ) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let stdout = run(executable, args, &[], 1024 * 1024).await?;
            Ok(String::from_utf8_lossy(&stdout).into_owned())
        })
    }

    fn run_binary<'a>(
        &'a self,
        executable: &'a str,
        args: &'a [String],
        environment: &'a [(String, String)],
        max_bytes: usize,
    ) -> BoxFuture<'a, Result<Vec<u8>, String>> {
        Box::pin(run(executable, args, environment, max_bytes))
    }
}
