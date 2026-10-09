//! Client-side file downloads (`frontend/src/utils/download.ts`).

/// Trigger a download of in-memory `content`; the object URL is revoked after
/// ten seconds.
pub fn download_file(filename: &str, content: &str, mime_type: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast as _;
        let parts = js_sys::Array::of1(&wasm_bindgen::JsValue::from_str(content));
        let options = web_sys::BlobPropertyBag::new();
        options.set_type(mime_type);
        let Ok(blob) = web_sys::Blob::new_with_str_sequence_and_options(&parts, &options) else {
            return;
        };
        let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else {
            return;
        };
        let Some(document) = web_sys::window().and_then(|w| w.document()) else {
            return;
        };
        if let Some(anchor) = document
            .create_element("a")
            .ok()
            .and_then(|el| el.dyn_into::<web_sys::HtmlAnchorElement>().ok())
        {
            anchor.set_href(&url);
            anchor.set_download(filename);
            anchor.click();
        }
        leptos::task::spawn_local(async move {
            gloo_timers::future::TimeoutFuture::new(10_000).await;
            let _ = web_sys::Url::revoke_object_url(&url);
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (filename, content, mime_type);
    }
}
