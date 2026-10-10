//! Album art loading: downloads covers in the background, shrinks them, caches them on disk
//! and keeps a bounded number of textures in memory.
//!
//! Requests are served newest-first, so when you scroll quickly the covers you're looking at
//! now load before the ones you've already scrolled past.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use eframe::egui;
use image::imageops::FilterType;

use crate::api::FILES_URL;

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

type Queue = Arc<(Mutex<Vec<Key>>, Condvar)>;

pub struct Covers {
    slots: HashMap<Key, Slot>,
    /// Ready textures, oldest first, for eviction.
    order: VecDeque<Key>,
    queue: Queue,
    done: Receiver<Done>,
    ctx: egui::Context,
}

fn cache_dir() -> PathBuf {
    crate::catalogue::cache_dir().join("covers")
}

impl Covers {
    pub fn new(ctx: &egui::Context) -> Self {
        let queue: Queue = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
        let (tx, done) = channel();
        let http = reqwest::blocking::Client::builder()
            .user_agent(concat!("rainbow/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("failed to create HTTP client");
        let _ = fs::create_dir_all(cache_dir());
        for _ in 0..WORKERS {
            let (queue, tx, http, ctx) = (queue.clone(), tx.clone(), http.clone(), ctx.clone());
            thread::spawn(move || worker(queue, tx, http, ctx));
        }
        Self { slots: HashMap::new(), order: VecDeque::new(), queue, done, ctx: ctx.clone() }
    }

    /// The cover texture if it's loaded; otherwise starts loading it and returns None.
    pub fn get(&mut self, art: &str, size: u32) -> Option<&egui::TextureHandle> {
        let key = Key { art: art.to_owned(), size };
        if !self.slots.contains_key(&key) {
            self.slots.insert(key.clone(), Slot::Loading);
            let (lock, cvar) = &*self.queue;
            let mut q = lock.lock().unwrap();
            q.push(key);
            if q.len() > MAX_QUEUE {
                let stale = q.remove(0);
                self.slots.remove(&stale); // re-requested if it comes back on screen
            }
            cvar.notify_one();
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

fn worker(queue: Queue, tx: Sender<Done>, http: reqwest::blocking::Client, ctx: egui::Context) {
    loop {
        let key = {
            let (lock, cvar) = &*queue;
            let mut q = lock.lock().unwrap();
            while q.is_empty() {
                q = cvar.wait(q).unwrap();
            }
            q.pop().unwrap() // newest first
        };
        let image = load(&http, &key);
        if tx.send(Done { key, image }).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

fn load(http: &reqwest::blocking::Client, key: &Key) -> Option<egui::ColorImage> {
    // Art hashes come from the API; only allow plain hex so they're safe as file names.
    if key.art.is_empty() || !key.art.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let path = cache_dir().join(format!("{}_{}.jpg", key.art, key.size));
    let img = match fs::read(&path).ok().and_then(|b| image::load_from_memory(&b).ok()) {
        Some(img) => img,
        None => {
            let bytes = http.get(format!("{FILES_URL}/{}.jpg", key.art)).send().ok()?.error_for_status().ok()?.bytes().ok()?;
            let full = image::load_from_memory(&bytes).ok()?;
            let small = full.resize(key.size, key.size, FilterType::Triangle);
            let mut out = Cursor::new(Vec::new());
            if small.to_rgb8().write_to(&mut out, image::ImageFormat::Jpeg).is_ok() {
                let _ = fs::write(&path, out.into_inner());
            }
            small
        }
    };
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}
