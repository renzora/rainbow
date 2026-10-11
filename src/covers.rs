//! Album art loading: downloads covers in the background, shrinks them, caches them on disk
//! and keeps a bounded number of textures in memory.
//!
//! Requests are served newest-first, so when you scroll quickly the covers you're looking at
//! now load before the ones you've already scrolled past.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use web_time::Duration;

use eframe::egui;

use crate::api::FILES_URL;
use crate::net::Http;

/// Thumbnail size for table rows, in pixels.
pub const THUMB: u32 = 96;
/// Size for the details panel.
pub const LARGE: u32 = 400;

const WORKERS: usize = 6;
/// Drop queued requests beyond this (the oldest are least likely to still be on screen).
const MAX_QUEUE: usize = 120;
const MAX_TEXTURES: usize = 1500;

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    art: String,
    size: u32,
}

enum Slot {
    Loading,
    Ready(egui::TextureHandle),
    Missing,
}

struct Done {
    key: Key,
    image: Option<egui::ColorImage>,
}

type Queue = Arc<Mutex<Vec<Key>>>;

pub struct Covers {
    slots: HashMap<Key, Slot>,
    /// Ready textures, oldest first, for eviction.
    order: VecDeque<Key>,
    queue: Queue,
    /// How many loaders are running (each takes covers from the queue until it's empty).
    loaders: Arc<AtomicUsize>,
    tx: Sender<Done>,
    done: Receiver<Done>,
    http: Http,
    ctx: egui::Context,
}

fn cache_dir() -> PathBuf {
    crate::catalogue::cache_dir().join("covers")
}

impl Covers {
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, done) = channel();
        let http = Http::new(Some(Duration::from_secs(30)));
        Self {
            slots: HashMap::new(),
            order: VecDeque::new(),
            queue: Queue::default(),
            loaders: Arc::new(AtomicUsize::new(0)),
            tx,
            done,
            http,
            ctx: ctx.clone(),
        }
    }

    /// The cover texture if it's loaded; otherwise starts loading it and returns None.
    pub fn get(&mut self, art: &str, size: u32) -> Option<&egui::TextureHandle> {
        let key = Key { art: art.to_owned(), size };
        if !self.slots.contains_key(&key) {
            self.slots.insert(key.clone(), Slot::Loading);
            let mut q = self.queue.lock().unwrap();
            q.push(key);
            if q.len() > MAX_QUEUE {
                let stale = q.remove(0);
                self.slots.remove(&stale); // re-requested if it comes back on screen
            }
            drop(q);
            if self.loaders.load(Ordering::SeqCst) < WORKERS {
                self.loaders.fetch_add(1, Ordering::SeqCst);
                let (queue, loaders, tx, http, ctx) = (self.queue.clone(), self.loaders.clone(), self.tx.clone(), self.http.clone(), self.ctx.clone());
                crate::task::spawn(move || loader(queue, loaders, tx, http, ctx));
            }
            return None;
        }
        match self.slots.get(&key) {
            Some(Slot::Ready(t)) => Some(t),
            _ => None,
        }
    }

    /// Picks up finished downloads. Call once per frame.
    pub fn poll(&mut self) {
        while let Ok(Done { key, image }) = self.done.try_recv() {
            let slot = match image {
                Some(img) => {
                    let name = format!("cover-{}-{}", key.art, key.size);
                    self.order.push_back(key.clone());
                    Slot::Ready(self.ctx.load_texture(name, img, egui::TextureOptions::LINEAR))
                }
                None => Slot::Missing,
            };
            self.slots.insert(key, slot);
        }
        while self.order.len() > MAX_TEXTURES {
            if let Some(old) = self.order.pop_front() {
                self.slots.remove(&old);
            }
        }
    }
}

/// Loads covers from the queue, newest first, until it's empty.
async fn loader(queue: Queue, loaders: Arc<AtomicUsize>, tx: Sender<Done>, http: Http, ctx: egui::Context) {
    loop {
        let Some(key) = queue.lock().unwrap().pop() else { break };
        let image = load(&http, &key).await;
        if tx.send(Done { key, image }).is_err() {
            break;
        }
        ctx.request_repaint();
    }
    loaders.fetch_sub(1, Ordering::SeqCst);
}

async fn load(http: &Http, key: &Key) -> Option<egui::ColorImage> {
    use image::imageops::FilterType;
    // Art hashes come from the API; only allow plain hex so they're safe as file names.
    if key.art.is_empty() || !key.art.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let path = cache_dir().join(format!("{}_{}.jpg", key.art, key.size));
    let img = match crate::store::read(&path).and_then(|b| image::load_from_memory(&b).ok()) {
        Some(img) => img,
        None => {
            let url = format!("{FILES_URL}/{}.jpg", key.art);
            let bytes = http.get(&url).send().await.ok()?.error_for_status().ok()?.bytes().await.ok()?;
            let full = image::load_from_memory(&bytes).ok()?;
            let small = full.resize(key.size, key.size, FilterType::Triangle);
            // (The browser keeps its own copy of downloads, so the web version doesn't save these.)
            #[cfg(not(target_arch = "wasm32"))]
            {
                let mut out = std::io::Cursor::new(Vec::new());
                if small.to_rgb8().write_to(&mut out, image::ImageFormat::Jpeg).is_ok() {
                    let _ = crate::store::write(&path, &out.into_inner());
                }
            }
            small
        }
    };
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}
