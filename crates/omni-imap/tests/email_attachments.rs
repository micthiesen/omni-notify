//! Private email PDF retrieval through MCP, with a scripted attachment reader.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures::future::BoxFuture;
use omni_core::clock::{SharedClock, TestClock};
use omni_imap::archive_service::ArchiveService;
use omni_imap::compose::ComposeService;
use omni_imap::mcp_tools::{AttachmentReader, ToolDeps, email_tools};
use omni_imap::protocol::ImapError;
use omni_imap::transport::DownloadedAttachment;
use omni_mcp_kit::{McpTool, ToolContext, ToolError, ToolOutput};
use omni_testkit::TestStore;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

const MESSAGE_ID: &str = "<attachment@example.test>";
const DATA: &[u8] = b"%PDF-1.7\nsynthetic fixture";

struct Reader {
    result: Mutex<Option<DownloadedAttachment>>,
    calls: Mutex<Vec<(String, String, usize)>>,
}

impl AttachmentReader for Reader {
    fn fetch_attachment<'a>(
        &'a self,
        message_id: &'a str,
        attachment_id: &'a str,
        max_bytes: usize,
    ) -> BoxFuture<'a, Result<Option<DownloadedAttachment>, ImapError>> {
        self.calls.lock().unwrap().push((
            message_id.to_owned(),
            attachment_id.to_owned(),
            max_bytes,
        ));
        let result = self.result.lock().unwrap().clone();
        Box::pin(async move { Ok(result) })
    }
}

fn pdf(name: &str, mime: &str, data: &[u8]) -> Option<DownloadedAttachment> {
    Some(DownloadedAttachment {
        name: name.to_owned(),
        mime_type: mime.to_owned(),
        data: data.to_vec(),
    })
}

async fn fixture(result: Option<DownloadedAttachment>) -> (McpTool, Arc<Reader>, TestStore) {
    let clock: SharedClock = TestClock::new(1_790_769_600_000);
    let store = TestStore::new(clock.clone()).await;
    let reader = Arc::new(Reader {
        result: Mutex::new(result),
        calls: Mutex::new(Vec::new()),
    });
    let tools = email_tools(ToolDeps {
        compose: ComposeService::new(store.store.clone(), clock.clone(), None, None),
        archive: ArchiveService::new(store.store.clone(), clock),
        archive_transport: None,
        attachments: Some(reader.clone() as Arc<dyn AttachmentReader>),
        tracker: TaskTracker::new(),
    })
    .unwrap();
    let tool = tools
        .into_iter()
        .find(|t| t.meta.name == "email_attachment_get")
        .unwrap();
    (tool, reader, store)
}

async fn call(tool: &McpTool, input: Value) -> Result<ToolOutput, ToolError> {
    tool.handler
        .call(
            input,
            ToolContext {
                call_id: "t".to_owned(),
                cancel: CancellationToken::new(),
            },
        )
        .await
}

#[tokio::test]
async fn returns_an_embedded_binary_resource_and_safe_metadata_without_a_public_url() {
    let (tool, reader, _store) = fixture(pdf("../private.pdf", "application/pdf", DATA)).await;
    let output = call(
        &tool,
        json!({"messageId": MESSAGE_ID, "attachmentId": "stable-id"}),
    )
    .await
    .unwrap();
    assert_eq!(
        *reader.calls.lock().unwrap(),
        vec![(
            MESSAGE_ID.to_owned(),
            "stable-id".to_owned(),
            5 * 1024 * 1024
        )]
    );
    let ToolOutput::Custom {
        structured,
        content,
    } = output
    else {
        panic!("expected custom content");
    };
    assert_eq!(content[0]["type"], json!("text"));
    assert!(
        !content[0]["text"]
            .as_str()
            .unwrap()
            .contains(&STANDARD.encode(DATA))
    );
    let mut resource = Map::new();
    resource.insert("type".to_owned(), json!("resource"));
    resource.insert(
        "resource".to_owned(),
        json!({"uri": "omni-email-attachment:stable-id", "mimeType": "application/pdf", "blob": STANDARD.encode(DATA)}),
    );
    assert_eq!(content[1], resource);
    let filename = structured["filename"].as_str().unwrap();
    assert!(!filename.contains(['\\', '/']) && !filename.chars().any(|c| (c as u32) < 0x20));
    assert_eq!(structured["size"], json!(DATA.len()));
    let sha = structured["sha256"].as_str().unwrap();
    assert!(sha.len() == 64 && sha.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(structured["blob"], json!(STANDARD.encode(DATA)));
}

#[tokio::test]
async fn rejects_excessive_bounds_and_header_injection_before_touching_imap() {
    let (tool, reader, _store) = fixture(pdf("x.pdf", "application/pdf", DATA)).await;
    for input in [
        json!({"messageId": MESSAGE_ID, "attachmentId": "id", "maxBytes": 5 * 1024 * 1024 + 1}),
        json!({"messageId": "<id>\r\nInjected: header", "attachmentId": "id"}),
        json!({"messageId": MESSAGE_ID, "attachmentId": "id", "destination": "/tmp/file.pdf"}),
    ] {
        assert!(call(&tool, input).await.is_err());
    }
    assert!(reader.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn rejects_missing_parts_mismatched_mime_invalid_pdf_bytes_and_decoded_oversize() {
    for result in [
        None,
        pdf("x.pdf", "text/html", DATA),
        pdf("x.pdf", "application/pdf", b"not pdf"),
    ] {
        let (tool, _, _store) = fixture(result).await;
        assert!(
            call(
                &tool,
                json!({"messageId": MESSAGE_ID, "attachmentId": "id"})
            )
            .await
            .is_err()
        );
    }
    let (tool, _, _store) = fixture(pdf("x.pdf", "application/pdf", DATA)).await;
    let error = call(
        &tool,
        json!({"messageId": MESSAGE_ID, "attachmentId": "id", "maxBytes": 5}),
    )
    .await
    .unwrap_err();
    assert!(error.message.contains("limit"), "{}", error.message);
}
