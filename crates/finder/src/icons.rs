//! File icons as the file manager shows them. Windows: the shell's icon for the file (an
//! executable's own icon, a folder's custom icon). Linux: the MIME type's icon from the desktop's
//! icon theme (Breeze on KDE), colored like Dolphin colors it.
//!
//! Icons load on a worker thread, newest request first, so typing never waits for them. A row
//! shows no icon until its own arrives, and the drawn fallback if there is none.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use fltk::app;
use fltk::enums::ColorDepth;
use fltk::image::RgbImage;
use fltk::prelude::ImageExt;

use crate::client::Msg;

#[cfg(target_os = "linux")]
#[path = "icons_linux.rs"]
mod platform;
#[cfg(windows)]
#[path = "icons_windows.rs"]
mod platform;

pub use platform::Colors;

/// Icon size in FLTK units (pixels at 100% scaling). Breeze draws 24 px and smaller icons as
/// flat symbols; from 32 px on they have the colorful look Dolphin shows.
pub const SIZE: i32 = 32;

/// Straight-alpha RGBA pixels.
pub struct Pixels {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// What the worker found for one path: the icon's id (same id, same picture), and its pixels the
/// first time that id is sent. `None`: no icon (use the drawn fallback).
pub type Found = Option<(String, Option<Pixels>)>;

enum State {
    Pending,
    Ready(String),
    Missing,
}

type Queue = Arc<(Mutex<Vec<(Vec<u8>, bool)>>, Condvar)>;

pub struct Icons {
    /// Until the first request: what the worker needs to start.
    start: Option<(app::Sender<Msg>, Colors)>,
    queue: Queue,
    paths: HashMap<Vec<u8>, State>,
    images: HashMap<String, RgbImage>,
}

pub enum Icon<'a> {
    Image(&'a RgbImage),
    Pending,
    Missing,
}

impl Icons {
    /// `colors`: the UI palette, for icons that follow the color scheme. The worker starts with
    /// the first request (the display is open by then), rendering at `SIZE` times its scaling.
    pub fn new(tx: app::Sender<Msg>, colors: Colors) -> Icons {
        Icons { start: Some((tx, colors)), queue: Queue::default(), paths: HashMap::new(), images: HashMap::new() }
    }

    fn spawn(&mut self) {
        let Some((tx, colors)) = self.start.take() else { return };
        let q = self.queue.clone();
        let px = (SIZE as f32 * app::screen_scale(0)).round().max(SIZE as f32) as u32;
        let r = thread::Builder::new().name("icons".into()).spawn(move || worker(&q, &tx, colors, px));
        if r.is_err() {
            // No icon thread: every request stays pending; show the drawn icons instead.
            self.paths.clear();
            self.start = None;
        }
    }

    /// The icon for `path`, requesting it if needed.
    pub fn get(&mut self, path: &[u8], is_dir: bool) -> Icon<'_> {
        match self.paths.get(path) {
            Some(State::Ready(id)) => self.images.get(id).map_or(Icon::Missing, Icon::Image),
            Some(State::Pending) => Icon::Pending,
            Some(State::Missing) => Icon::Missing,
            None => {
                self.spawn();
                self.paths.insert(path.to_vec(), State::Pending);
                let (lock, cv) = &*self.queue;
                lock.lock().unwrap().push((path.to_vec(), is_dir));
                cv.notify_one();
                Icon::Pending
            }
        }
    }

    /// Drop requests not yet started (the rows they were for are gone).
    pub fn cancel_pending(&mut self) {
        self.queue.0.lock().unwrap().clear();
        self.paths.retain(|_, s| !matches!(s, State::Pending));
    }

    /// A result from the worker.
    pub fn loaded(&mut self, path: Vec<u8>, found: Found) {
        let state = match found {
            Some((id, pixels)) => {
                if let Some(p) = pixels
                    && let Ok(mut img) = RgbImage::new(&p.rgba, p.w as i32, p.h as i32, ColorDepth::Rgba8)
                {
                    img.scale(SIZE, SIZE, true, true);
                    self.images.insert(id.clone(), img);
                }
                State::Ready(id)
            }
            None => State::Missing,
        };
        self.paths.insert(path, state);
    }
}

fn worker(queue: &Queue, tx: &app::Sender<Msg>, colors: Colors, px: u32) {
    let (lock, cv) = &**queue;
    let mut loader = platform::Loader::new(colors, px);
    loop {
        let (path, is_dir) = {
            let mut q = lock.lock().unwrap();
            loop {
                if let Some(job) = q.pop() {
                    break job;
                }
                q = cv.wait(q).unwrap();
            }
        };
        let found = loader.load(&path, is_dir);
        tx.send(Msg::Icon { path, found });
    }
}
