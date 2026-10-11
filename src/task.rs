//! Background work that runs the same way on the desktop and on the web.
//!
//! On the desktop each task gets its own thread (so blocking calls inside it are fine). On the
//! web there are no threads: tasks run on the page's event loop between frames, and waiting
//! (for the network or a timer) lets the page carry on.

use std::future::Future;
use web_time::Duration;

/// `Send` on the desktop, where tasks move to other threads; nothing on the web.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send> MaybeSend for T {}
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T> MaybeSend for T {}

/// Runs the future `work` makes in the background. (On the desktop the future is made on its
/// own thread, so it needn't be `Send` itself.)
pub fn spawn<F: Future<Output = ()> + 'static>(work: impl FnOnce() -> F + MaybeSend + 'static) {
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || pollster::block_on(work()));
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(work());
}

/// Waits without holding anything up.
pub async fn sleep(d: Duration) {
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::sleep(d);
    #[cfg(target_arch = "wasm32")]
    crate::web::sleep(d).await;
}

/// Lets the page draw a frame during long work on the web (a no-op on the desktop).
pub async fn breathe() {
    #[cfg(target_arch = "wasm32")]
    crate::web::sleep(Duration::ZERO).await;
}

/// Something being fetched or worked out in the background for the UI.
pub enum Load<T> {
    Loading(std::sync::mpsc::Receiver<Result<T, String>>),
    Ready(T),
    Failed(String),
}

impl<T: MaybeSend + 'static> Load<T> {
    pub fn start<F: Future<Output = anyhow::Result<T>> + 'static>(ctx: &eframe::egui::Context, work: impl FnOnce() -> F + MaybeSend + 'static) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        spawn(move || async move {
            let _ = tx.send(work().await.map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        Load::Loading(rx)
    }

    /// Picks up the result once it's ready.
    pub fn poll(&mut self) -> &mut Self {
        if let Load::Loading(rx) = self
            && let Ok(result) = rx.try_recv()
        {
            *self = match result {
                Ok(v) => Load::Ready(v),
                Err(e) => Load::Failed(e),
            };
        }
        self
    }

    pub fn ready(&mut self) -> Option<&T> {
        match self.poll() {
            Load::Ready(v) => Some(v),
            _ => None,
        }
    }
}
