//! Asynchronous, read-only preview for incoming image files and real sequence members.
use std::{
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    incoming::{IncomingItem, SingleKind},
    jobs::{Checkpoint, JobContext, JobOutcome, JobProgress, Jobs},
};
use eframe::egui::{ColorImage, TextureHandle, TextureOptions};

const MAX_PIXELS: usize = 16_000_000;
const MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone)]
struct Member {
    path: PathBuf,
    frame: Option<i64>,
}
struct Decoded {
    request: u64,
    path: PathBuf,
    frame: Option<i64>,
    result: Result<(usize, usize, Vec<u8>), String>,
}

/// State for the incoming image/sequence inspector. All decoding runs in a tracked job.
pub struct IncomingMedia {
    key: Option<(u64, Vec<PathBuf>)>,
    members: Vec<Member>,
    index: usize,
    receiver: Option<Receiver<Decoded>>,
    job: Option<u64>,
    texture: Option<TextureHandle>,
    cancel: Option<Arc<AtomicBool>>,
    request: u64,
    context: Option<eframe::egui::Context>,
    loaded: Option<(usize, usize)>,
    error: Option<String>,
    zoom: f32,
    pan: eframe::egui::Vec2,
}

impl Default for IncomingMedia {
    fn default() -> Self {
        Self {
            key: None,
            members: vec![],
            index: 0,
            receiver: None,
            job: None,
            texture: None,
            cancel: None,
            request: 0,
            context: None,
            loaded: None,
            error: None,
            zoom: 0.0,
            pan: eframe::egui::Vec2::ZERO,
        }
    }
}

impl IncomingMedia {
    pub fn set_item(&mut self, item: Option<&IncomingItem>, generation: u64) {
        let members: Vec<Member> = match item {
            Some(IncomingItem::Single(s))
                if matches!(s.kind, SingleKind::Image | SingleKind::Video) =>
            {
                vec![Member {
                    path: s.file.path.clone(),
                    frame: None,
                }]
            }
            Some(IncomingItem::Sequence(s)) => s
                .members
                .iter()
                .map(|m| Member {
                    path: m.file.path.clone(),
                    frame: Some(m.frame),
                })
                .collect(),
            _ => vec![],
        };
        let key = (!members.is_empty())
            .then(|| (generation, members.iter().map(|m| m.path.clone()).collect()));
        if self.key == key {
            return;
        }
        self.clear();
        self.key = key;
        self.members = members;
        self.index = self.members.len() / 2;
    }

    pub fn clear(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        self.request = self.request.wrapping_add(1);
        self.key = None;
        self.members.clear();
        self.texture = None;
        self.loaded = None;
        self.error = None;
        self.index = 0;
        self.zoom = 0.0;
        self.pan = eframe::egui::Vec2::ZERO;
    }

    pub fn poll(&mut self) {
        let Some(rx) = &self.receiver else {
            return;
        };
        let decoded = match rx.try_recv() {
            Ok(decoded) => decoded,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.receiver = None;
                self.job = None;
                if !self.members.is_empty() {
                    self.error = Some(
                        "Image decoder stopped without a result. Use Refresh to retry.".into(),
                    );
                }
                return;
            }
        };
        self.receiver = None;
        self.job = None;
        if decoded.request != self.request
            || !self
                .members
                .get(self.index)
                .is_some_and(|m| m.path == decoded.path && m.frame == decoded.frame)
        {
            return;
        }
        match decoded.result {
            Ok((w, h, pixels)) => {
                self.texture = self.context.as_ref().map(|ctx| {
                    ctx.load_texture(
                        "incoming-preview",
                        ColorImage::from_rgb([w, h], &pixels),
                        TextureOptions::LINEAR,
                    )
                });
                self.loaded = Some((w, h));
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
    }

    pub fn is_busy(&self) -> bool {
        self.receiver.is_some()
    }

    pub fn ui(&mut self, ui: &mut eframe::egui::Ui, jobs: &mut Jobs) {
        self.poll();
        if self.members.is_empty() {
            ui.label("Select an image or image sequence to preview.");
            return;
        }
        if self.members.len() == 1 && is_video(&self.members[0].path) {
            ui.label("Right-click the file to open video in DJV, MRV2 or Nuke Player.");
            return;
        }
        ui.horizontal_wrapped(|ui| {
            if ui.button("|◀").clicked() {
                self.select(0);
            }
            if ui.button("◀").clicked() {
                self.select(self.index.saturating_sub(1));
            }
            ui.label(format!("{} / {}", self.index + 1, self.members.len()));
            if ui.button("▶").clicked() {
                self.select((self.index + 1).min(self.members.len() - 1));
            }
            if ui.button("▶|").clicked() {
                self.select(self.members.len() - 1);
            }
            if ui.button("Refresh").clicked() {
                self.invalidate_decode();
            }
        });
        if self.members.len() > 1 {
            let mut n = self.index;
            if ui
                .add(eframe::egui::Slider::new(
                    &mut n,
                    0..=self.members.len() - 1,
                ))
                .changed()
            {
                self.select(n);
            }
        }
        let member = &self.members[self.index];
        ui.label(format!(
            "{}{}",
            member
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            member
                .frame
                .map(|f| format!(" — frame {f}"))
                .unwrap_or_default()
        ))
        .on_hover_text(member.path.display().to_string());
        if let Some((w, h)) = self.loaded {
            ui.label(format!("{w} × {h}"));
        }
        ui.horizontal_wrapped(|ui| {
            if ui.button("Fit").clicked() {
                self.zoom = 0.0;
                self.pan = eframe::egui::Vec2::ZERO;
            }
            if ui.button("100%").clicked() {
                self.zoom = 1.0;
            }
            if ui.button("−").clicked() {
                self.zoom = (if self.zoom == 0.0 { 1.0 } else { self.zoom } - 0.1).max(0.1);
            }
            if ui.button("+").clicked() {
                self.zoom = (if self.zoom == 0.0 { 1.0 } else { self.zoom } + 0.1).min(5.0);
            }
        });
        if self.texture.is_none() && self.receiver.is_none() && self.error.is_none() {
            self.queue(ui, jobs);
        }
        if let Some(t) = &self.texture {
            let avail = ui.available_size().max(eframe::egui::vec2(1.0, 1.0));
            let size = t.size_vec2();
            let scale = if self.zoom == 0.0 {
                (avail.x / size.x).min(avail.y / size.y).min(1.0)
            } else {
                self.zoom
            };
            let response = ui.allocate_response(avail, eframe::egui::Sense::drag());
            if response.dragged_by(eframe::egui::PointerButton::Middle) {
                self.pan += response.drag_delta();
            }
            let rect = eframe::egui::Rect::from_center_size(
                response.rect.center() + self.pan,
                size * scale,
            );
            ui.painter().image(
                t.id(),
                rect,
                eframe::egui::Rect::from_min_max(
                    eframe::egui::pos2(0.0, 0.0),
                    eframe::egui::pos2(1.0, 1.0),
                ),
                eframe::egui::Color32::WHITE,
            );
        }
        if self.receiver.is_some() {
            ui.label("Loading image…");
        }
        if let Some(e) = &self.error {
            ui.colored_label(eframe::egui::Color32::LIGHT_RED, e);
        }
    }
}

// Internal deferred cancellation lets clear() retain its frozen public signature.
impl IncomingMedia {
    fn invalidate_decode(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        self.request = self.request.wrapping_add(1);
        self.texture = None;
        self.loaded = None;
        self.error = None;
    }
    fn select(&mut self, n: usize) {
        if n != self.index {
            self.index = n;
            self.invalidate_decode();
            self.texture = None;
            self.loaded = None;
            self.error = None;
            self.zoom = 0.0;
            self.pan = eframe::egui::Vec2::ZERO;
        }
    }
}
impl IncomingMedia {
    fn queue(&mut self, ui: &eframe::egui::Ui, jobs: &mut Jobs) {
        if self.receiver.is_some() || jobs.is_shutting_down() {
            return;
        }
        let Some(m) = self.members.get(self.index).cloned() else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let ctx = ui.ctx().clone();
        self.context = Some(ctx.clone());
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        let request = self.request;
        match jobs.start(&ctx, "Decode incoming image", move |jc| {
            let result = decode(&m.path, &jc, &cancel);
            let outcome = if cancel.load(Ordering::Relaxed) || jc.control.is_cancelled() {
                JobOutcome::Cancelled
            } else if result.is_ok() {
                JobOutcome::Succeeded
            } else {
                JobOutcome::Failed
            };
            let _ = tx.send(Decoded {
                request,
                path: m.path,
                frame: m.frame,
                result,
            });
            jc.events.finish(
                outcome,
                JobProgress {
                    completed: 1,
                    total: Some(1),
                    ..Default::default()
                },
            );
        }) {
            Ok(id) => {
                self.job = Some(id);
                self.receiver = Some(rx)
            }
            Err(e) => self.error = Some(format!("Could not start image decoder: {e}")),
        }
    }
}

fn is_video(p: &std::path::Path) -> bool {
    p.extension().and_then(|s| s.to_str()).is_some_and(|s| {
        matches!(
            s.to_ascii_lowercase().as_str(),
            "mov" | "mp4" | "mkv" | "avi" | "webm" | "mxf" | "r3d" | "braw"
        )
    })
}
fn decode(
    path: &std::path::Path,
    control: &JobContext,
    cancel: &AtomicBool,
) -> Result<(usize, usize, Vec<u8>), String> {
    if control.control.checkpoint() == Checkpoint::Cancelled {
        return Err("Preview cancelled.".into());
    }
    if let Ok(reader) = image::ImageReader::open(path).and_then(|r| r.with_guessed_format()) {
        if matches!(
            reader.format(),
            Some(
                image::ImageFormat::Png
                    | image::ImageFormat::Jpeg
                    | image::ImageFormat::Tiff
                    | image::ImageFormat::Hdr
            )
        ) {
            let (w, h) = reader
                .into_dimensions()
                .map_err(|e| format!("Could not read image dimensions: {e}"))?;
            let pixels = (w as usize)
                .checked_mul(h as usize)
                .filter(|n| *n > 0 && *n <= MAX_PIXELS)
                .ok_or("Image exceeds the 16M pixel preview limit.")?;
            if pixels.checked_mul(3).is_none_or(|bytes| bytes > MAX_BYTES) {
                return Err("Image exceeds the preview byte limit.".into());
            }
            let mut reader = image::ImageReader::open(path)
                .and_then(|r| r.with_guessed_format())
                .map_err(|e| e.to_string())?;
            let mut limits = image::Limits::default();
            limits.max_alloc = Some(MAX_BYTES as u64);
            reader.limits(limits);
            let image = reader
                .decode()
                .map_err(|e| format!("Image could not be decoded within its memory limit: {e}"))?;
            if cancel.load(Ordering::Relaxed) || control.control.is_cancelled() {
                return Err("Preview cancelled.".into());
            }
            if image.width() != w || image.height() != h {
                return Err("Image dimensions changed while loading.".into());
            }
            return Ok((w as usize, h as usize, image.to_rgb8().into_raw()));
        }
    }
    let (w, h) = probe_dimensions(path, &control.control, cancel)?;
    let (w, h) = (w as usize, h as usize);
    let bytes = w
        .checked_mul(h)
        .and_then(|v| v.checked_mul(3))
        .filter(|v| {
            w > 0 && h > 0 && w.checked_mul(h).is_some_and(|p| p <= MAX_PIXELS) && *v <= MAX_BYTES
        })
        .ok_or("Image exceeds the 16M pixel / 64 MiB preview limit.")?;
    let raw = run_bounded(
        "ffmpeg",
        &["-v", "error", "-i"],
        path,
        &[
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "pipe:1",
        ],
        &control.control,
        cancel,
    )?;
    if raw.len() != bytes {
        return Err("ffmpeg returned an incomplete RGB frame.".into());
    }
    Ok((w, h, raw))
}
fn run_bounded(
    exe: &str,
    args: &[&str],
    path: &std::path::Path,
    after: &[&str],
    control: &crate::jobs::JobControl,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, String> {
    let mut cmd = Command::new(exe);
    crate::launch::sanitize_environment(&mut cmd);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let mut child = cmd
        .args(["-protocol_whitelist", "file,pipe"])
        .args(args)
        .arg(path)
        .args(after)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{exe} unavailable: {e}"))?;
    let out = child
        .stdout
        .take()
        .ok_or("Could not read decoder output.")?;
    let limit = if exe == "ffprobe" {
        16 * 1024
    } else {
        MAX_BYTES
    };
    let (tx, rx) = mpsc::sync_channel(1);
    let reader = thread::spawn(move || {
        let mut v = Vec::new();
        let r = out.take((limit + 1) as u64).read_to_end(&mut v);
        let _ = tx.send(r.map(|_| v));
    });
    let start = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed)
            || control.checkpoint() == Checkpoint::Cancelled
            || start.elapsed() > Duration::from_secs(10)
        {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err("Image decoding cancelled or timed out.".into());
        }
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(format!("Could not wait for decoder: {error}"));
            }
        };
        if let Some(status) = status {
            let v = rx
                .recv_timeout(Duration::from_secs(1))
                .map_err(|_| "Decoder output reader failed.".to_owned())?
                .map_err(|e| e.to_string())?;
            reader.join().ok();
            if !status.success() {
                return Err(format!(
                    "{exe} could not decode this image; check ffmpeg support and file access."
                ));
            }
            if v.len() > limit {
                return Err("Decoder output exceeded its bound.".into());
            }
            return Ok(v);
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Read dimensions once per analyzed item, using four bounded workers and no media decoding.
pub(crate) fn scan_resolutions(
    items: &[IncomingItem],
    control: &crate::jobs::JobControl,
    events: &crate::jobs::JobEvents,
) -> (Vec<Option<String>>, Vec<String>) {
    use std::sync::{Mutex, atomic::AtomicUsize};
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let result = Mutex::new((vec![None; items.len()], Vec::new()));
    std::thread::scope(|scope| {
        for _ in 0..items.len().min(4) {
            let next = &next;
            let done = &done;
            let result = &result;
            scope.spawn(move || loop {
                if control.checkpoint() == Checkpoint::Cancelled { break; }
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(item) = items.get(index) else { break; };
                let media = matches!(item, IncomingItem::Sequence(_))
                    || matches!(item, IncomingItem::Single(single) if matches!(single.kind, SingleKind::Image | SingleKind::Video));
                if media {
                    if let Some(file) = item.files().first() {
                        let dimensions = image::ImageReader::open(&file.path).ok()
                            .and_then(|reader| reader.with_guessed_format().ok())
                            .and_then(|reader| reader.into_dimensions().ok())
                            .filter(|(w,h)| *w > 0 && *h > 0)
                            .map(|(w,h)| format!("{w}x{h}"))
                            .map(Ok).unwrap_or_else(|| probe_resolution(&file.path, control));
                        let mut result = result.lock().expect("metadata results");
                        match dimensions {
                            Ok(resolution) => result.0[index] = Some(resolution),
                            Err(error) => result.1.push(format!("{}: {error}", file.path.display())),
                        }
                    }
                }
                // Serialize the completion count with its progress event.
                let _result = result.lock().expect("metadata results");
                let completed = done.fetch_add(1, Ordering::Relaxed) + 1;
                events.progress(crate::jobs::JobPhase::MatchingFolders, crate::jobs::JobProgress {
                    completed: completed as u64, total: Some(items.len() as u64),
                    ..Default::default()
                });
            });
        }
    });
    result.into_inner().expect("metadata results")
}

fn probe_resolution(
    path: &std::path::Path,
    control: &crate::jobs::JobControl,
) -> Result<String, String> {
    probe_dimensions(path, control, &AtomicBool::new(false)).map(|(w, h)| format!("{w}x{h}"))
}

fn probe_dimensions(
    path: &std::path::Path,
    control: &crate::jobs::JobControl,
    cancel: &AtomicBool,
) -> Result<(u32, u32), String> {
    // JSON keeps dimensions separate from side-data emitted for camera formats.
    let output = run_bounded(
        "ffprobe",
        &[
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "json",
            "-i",
        ],
        path,
        &[],
        control,
        cancel,
    )?;
    let value: serde_json::Value = serde_json::from_slice(&output)
        .map_err(|error| format!("Invalid dimension output: {error}"))?;
    let dimensions = value
        .get("streams")
        .and_then(|value| value.as_array())
        .and_then(|streams| streams.first())
        .and_then(|stream| {
            let w = u32::try_from(stream.get("width")?.as_u64()?).ok()?;
            let h = u32::try_from(stream.get("height")?.as_u64()?).ok()?;
            (w > 0 && h > 0).then_some((w, h))
        });
    dimensions
        .ok_or_else(|| "No readable video/image dimensions; resolution tag unavailable.".into())
}
