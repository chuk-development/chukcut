//! The preview's render thread.
//!
//! The UI asks for "the picture of this project at this time, this big" and
//! carries on; the thread renders the newest request and drops any it was
//! too slow to reach. Latest-wins is the whole policy: during playback and
//! scrubbing only the most recent position matters, and a queue would only
//! ever show the user the past.
//!
//! Frames come back as `RenderImage`s ready for gpui. The engine renders
//! into its own wgpu device and reads the frame back; gpui uploads it into
//! its atlas. The next step is to share one device and skip the round trip
//! (see `docs/research/GPUI_SPIKE.md`, path (a)).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use chukcut_engine::modules::media::MediaSourceProvider;
use chukcut_engine::modules::project::{Micros, Project};
use chukcut_engine::modules::render::Compositor;
use gpui::RenderImage;
use parking_lot::{Condvar, Mutex};

/// One rendered preview frame.
pub struct Frame {
    pub time: Micros,
    pub image: Arc<RenderImage>,
}

struct Request {
    project: Arc<Project>,
    time: Micros,
    size: (u32, u32),
}

#[derive(Default)]
struct Shared {
    request: Mutex<Option<Request>>,
    wake: Condvar,
    result: Mutex<Option<Frame>>,
    stop: AtomicBool,
    /// Set when the GPU could not be opened; the UI shows it instead of a
    /// black viewer.
    failure: Mutex<Option<String>>,
}

pub struct Player {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Player {
    pub fn new() -> Self {
        let shared = Arc::new(Shared::default());
        let thread = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("chukcut-preview".into())
                .spawn(move || render_loop(shared))
                .expect("spawn the preview thread")
        };
        Self {
            shared,
            thread: Some(thread),
        }
    }

    /// Ask for a frame. Replaces any request the thread has not started yet.
    pub fn request(&self, project: Arc<Project>, time: Micros, size: (u32, u32)) {
        *self.shared.request.lock() = Some(Request {
            project,
            time,
            size: (size.0.max(2) & !1, size.1.max(2) & !1),
        });
        self.shared.wake.notify_one();
    }

    /// The newest finished frame, if one arrived since the last call.
    pub fn take(&self) -> Option<Frame> {
        self.shared.result.lock().take()
    }

    pub fn failure(&self) -> Option<String> {
        self.shared.failure.lock().clone()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.shared.wake.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Which media a provider was built for. A provider holds open decoders, so
/// it is rebuilt only when the set of files changes, not on every edit.
///
/// Text materials are in the key too, content and style: the provider keeps
/// its own copy of each, so a text added or edited after it was built would
/// otherwise draw as missing media, or as its old words.
fn material_key(project: &Project) -> Vec<(String, String)> {
    let pool = &project.materials;
    let mut key: Vec<(String, String)> = pool
        .videos
        .iter()
        .map(|m| (m.id.clone(), m.path.clone()))
        .chain(pool.images.iter().map(|m| (m.id.clone(), m.path.clone())))
        // Titles too: the provider snapshots text materials, so a title added
        // or edited after it was built drew as missing media.
        .chain(pool.texts.iter().map(|m| (m.id.clone(), format!("{m:?}"))))
        .collect();
    key.sort();
    key
}

fn render_loop(shared: Arc<Shared>) {
    let Some(context) = chukcut_engine::modules::gpu::render_context() else {
        *shared.failure.lock() = Some("No usable GPU adapter".into());
        return;
    };
    let compositor = Compositor::new(context);
    let mut provider: Option<(Vec<(String, String)>, MediaSourceProvider)> = None;

    loop {
        let request = {
            let mut slot = shared.request.lock();
            loop {
                if shared.stop.load(Ordering::Acquire) {
                    return;
                }
                if let Some(request) = slot.take() {
                    break request;
                }
                shared.wake.wait(&mut slot);
            }
        };

        let key = material_key(&request.project);
        if provider
            .as_ref()
            .map(|(known, _)| known != &key)
            .unwrap_or(true)
        {
            provider = Some((key, MediaSourceProvider::from_project(&request.project)));
        }
        // Titles and captions change without the files changing.
        if let Some((_, sources)) = provider.as_mut() {
            sources.sync_texts(&request.project);
        }
        let (_, sources) = provider.as_ref().expect("provider was just set");

        let (width, height) = request.size;
        let mut pixels =
            match compositor.render_frame(&request.project, request.time, request.size, sources) {
                Ok(pixels) => pixels,
                Err(error) => {
                    tracing::warn!(%error, time = request.time, "preview frame failed");
                    continue;
                }
            };

        // gpui's images are BGRA.
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        let Some(buffer) = image::RgbaImage::from_raw(width, height, pixels) else {
            tracing::warn!("preview frame has the wrong length");
            continue;
        };
        let image = Arc::new(RenderImage::new(vec![image::Frame::new(buffer)]));
        *shared.result.lock() = Some(Frame {
            time: request.time,
            image,
        });
    }
}
