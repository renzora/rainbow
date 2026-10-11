//! Rainbow's own files (settings, the catalogue, caches), on the desktop and the web.
//!
//! The desktop keeps them in the usual folders (see `catalogue::data_dir`). The web keeps them in
//! the browser's IndexedDB: everything is read into memory before the app starts (so reading
//! stays instant and synchronous), and writes are saved in the background.

use std::path::Path;

#[cfg(target_arch = "wasm32")]
thread_local! {
    static FILES: std::cell::RefCell<std::collections::HashMap<String, Vec<u8>>> = Default::default();
}

#[cfg(target_arch = "wasm32")]
fn key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Reads every saved file into memory. Called once on the web before the app starts.
#[cfg(target_arch = "wasm32")]
pub async fn load() {
    let files = crate::web::idb_load().await;
    FILES.with(|f| *f.borrow_mut() = files);
}

pub fn read(path: &Path) -> Option<Vec<u8>> {
    #[cfg(not(target_arch = "wasm32"))]
    return std::fs::read(path).ok();
    #[cfg(target_arch = "wasm32")]
    return FILES.with(|f| f.borrow().get(&key(path)).cloned());
}

pub fn read_string(path: &Path) -> Option<String> {
    String::from_utf8(read(path)?).ok()
}

/// Saves a file whole (on the desktop through a temporary file, so a crash can't leave half of it).
pub fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let key = key(path);
        crate::web::idb_put(&key, bytes);
        FILES.with(|f| f.borrow_mut().insert(key, bytes.to_vec()));
        Ok(())
    }
}

pub fn remove(path: &Path) {
    #[cfg(not(target_arch = "wasm32"))]
    let _ = std::fs::remove_file(path);
    #[cfg(target_arch = "wasm32")]
    {
        let key = key(path);
        crate::web::idb_delete(&key);
        FILES.with(|f| f.borrow_mut().remove(&key));
    }
}

pub fn exists(path: &Path) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    return path.exists();
    #[cfg(target_arch = "wasm32")]
    return FILES.with(|f| f.borrow().contains_key(&key(path)));
}
