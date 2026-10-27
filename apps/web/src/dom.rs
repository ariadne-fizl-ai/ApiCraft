//! Small browser helpers.

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::Window;

pub fn window() -> Window {
    web_sys::window().expect("window")
}

pub fn now_ms() -> u64 {
    js_sys::Date::now() as u64
}

pub fn perf_now() -> f64 {
    window().performance().map(|p| p.now()).unwrap_or_else(js_sys::Date::now)
}

pub fn js_err(v: &JsValue) -> String {
    if let Some(e) = v.dyn_ref::<js_sys::Error>() {
        return String::from(e.message());
    }
    v.as_string().unwrap_or_else(|| format!("{v:?}"))
}

pub async fn sleep_ms(ms: u32) {
    let p = js_sys::Promise::new(&mut |resolve, _| {
        let _ = window().set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms as i32);
    });
    let _ = wasm_bindgen_futures::JsFuture::from(p).await;
}

pub fn location_origin() -> String {
    window().location().origin().unwrap_or_default()
}

pub fn storage_get(key: &str) -> Option<String> {
    window().local_storage().ok().flatten()?.get_item(key).ok().flatten()
}

pub fn storage_set(key: &str, value: &str) -> Result<(), String> {
    let s = window().local_storage().ok().flatten().ok_or("browser storage is unavailable")?;
    s.set_item(key, value).map_err(|e| format!("browser storage is full or blocked ({})", js_err(&e)))
}

/// Offers bytes to the user as a file download.
pub fn download(filename: &str, bytes: &[u8]) {
    let arr = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::of1(&arr);
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type("application/octet-stream");
    let Ok(blob) = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts) else { return };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else { return };
    let doc = window().document().expect("document");
    if let Ok(a) = doc.create_element("a") {
        let a: web_sys::HtmlAnchorElement = a.unchecked_into();
        a.set_href(&url);
        a.set_download(filename);
        if let Some(body) = doc.body() {
            let _ = body.append_child(&a);
            a.click();
            let _ = body.remove_child(&a);
        }
    }
    let revoke = Closure::once_into_js(move || {
        let _ = web_sys::Url::revoke_object_url(&url);
    });
    let _ = window().set_timeout_with_callback_and_timeout_and_arguments_0(revoke.unchecked_ref(), 2000);
}
