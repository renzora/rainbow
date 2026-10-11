//! The web version's link to the browser (web/glue.js): saved files, the songs folder,
//! downloads, and timers.

use std::collections::HashMap;
use web_time::Duration;

use anyhow::{Result, anyhow};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/web/glue.js")]
extern "C" {
    #[wasm_bindgen(js_name = idb_load)]
    async fn js_idb_load() -> JsValue;
    #[wasm_bindgen(js_name = idb_put)]
    fn js_idb_put(key: &str, bytes: &[u8]);
    #[wasm_bindgen(js_name = idb_delete)]
    fn js_idb_delete(key: &str);
    #[wasm_bindgen(js_name = has_folder_api)]
    fn js_has_folder_api() -> bool;
    #[wasm_bindgen(js_name = folder_name)]
    fn js_folder_name() -> Option<String>;
    #[wasm_bindgen(js_name = pick_folder)]
    async fn js_pick_folder() -> JsValue;
    #[wasm_bindgen(js_name = forget_folder)]
    fn js_forget_folder();
    #[wasm_bindgen(js_name = folder_allowed)]
    async fn js_folder_allowed() -> JsValue;
    #[wasm_bindgen(js_name = folder_has)]
    async fn js_folder_has(parts: Vec<String>) -> JsValue;
    #[wasm_bindgen(js_name = folder_write, catch)]
    async fn js_folder_write(parts: Vec<String>, bytes: &[u8], append: bool) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name = folder_read)]
    async fn js_folder_read(parts: Vec<String>) -> JsValue;
    #[wasm_bindgen(js_name = folder_remove)]
    async fn js_folder_remove(parts: Vec<String>) -> JsValue;
    #[wasm_bindgen(js_name = save_file)]
    fn js_save_file(name: &str, bytes: &[u8]);
    #[wasm_bindgen(js_name = sleep)]
    async fn js_sleep(ms: f64) -> JsValue;
    #[wasm_bindgen(js_name = loaded)]
    pub fn loaded();
}

pub async fn idb_load() -> HashMap<String, Vec<u8>> {
    let all = js_idb_load().await;
    let mut out = HashMap::new();
    for pair in js_sys::Array::from(&all).iter() {
        let pair = js_sys::Array::from(&pair);
        if let Some(key) = pair.get(0).as_string() {
            out.insert(key, js_sys::Uint8Array::new(&pair.get(1)).to_vec());
        }
    }
    out
}

pub fn idb_put(key: &str, bytes: &[u8]) {
    js_idb_put(key, bytes);
}

pub fn idb_delete(key: &str) {
    js_idb_delete(key);
}

pub async fn sleep(d: Duration) {
    js_sleep(d.as_secs_f64() * 1000.0).await;
}

/// Saves bytes as an ordinary browser download.
pub fn save_file(name: &str, bytes: &[u8]) {
    js_save_file(name, bytes);
}

// ─────────────────────────────── The songs folder ───────────────────────────────

/// Whether this browser can write to a folder (Chrome and Edge can; Firefox and Safari can't,
/// so charts are saved as downloads there).
pub fn has_folder_api() -> bool {
    js_has_folder_api()
}

/// The chosen songs folder's name, if there is one.
pub fn folder_name() -> Option<String> {
    js_folder_name()
}

/// Asks for a songs folder; returns its name, or None if cancelled.
pub async fn pick_folder() -> Option<String> {
    js_pick_folder().await.as_string()
}

pub fn forget_folder() {
    js_forget_folder();
}

/// Whether we may write to the songs folder, asking the browser if needed.
pub async fn folder_allowed() -> bool {
    js_folder_allowed().await.as_bool().unwrap_or(false)
}

fn parts(path: &str) -> Vec<String> {
    path.split('/').filter(|p| !p.is_empty()).map(str::to_owned).collect()
}

/// Whether a file or folder exists at this path (with `/`s) in the songs folder.
pub async fn folder_has(path: &str) -> bool {
    js_folder_has(parts(path)).await.as_bool().unwrap_or(false)
}

pub async fn folder_write(path: &str, bytes: &[u8], append: bool) -> Result<()> {
    js_folder_write(parts(path), bytes, append)
        .await
        .map(|_| ())
        .map_err(|e| anyhow!("couldn't save {path}: {}", js_sys::Error::from(e).message()))
}

pub async fn folder_read(path: &str) -> Option<Vec<u8>> {
    let v = js_folder_read(parts(path)).await;
    (!v.is_null()).then(|| js_sys::Uint8Array::new(&v).to_vec())
}

pub async fn folder_remove(path: &str) {
    js_folder_remove(parts(path)).await;
}

// ─────────────────────────────── The page ───────────────────────────────

/// A chart id from the page address (`?c=<id>`, as shared links open it).
pub fn link_from_url() -> Option<String> {
    let search = web_sys::window()?.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    let id = params.get("c")?;
    crate::links::chart_id(&format!("rainbow://{id}"))
}

/// An address relative to the page, made absolute (fetch from Rust needs whole addresses).
pub fn page_url(rel: &str) -> String {
    web_sys::window()
        .and_then(|w| w.location().href().ok())
        .and_then(|base| web_sys::Url::new_with_base(rel, &base).ok())
        .map(|u| u.href())
        .unwrap_or_else(|| rel.to_owned())
}
