//! Live window capture for Magnify, on its own thread with its own Wayland connection.
//!
//! Why a window and not the output: an output capture contains the overlay itself, so a zoom of it
//! would feed back into itself. A window capture (ext_image_copy_capture on an
//! ext_foreign_toplevel_image_capture_source) does not contain the overlay. COSMIC's
//! zcosmic_toplevel_info_v1 says which window is active and where it sits on the output.
//!
//! Contract with the rest of the program: `set_active(true)` starts capturing the active window,
//! `latest()` returns the newest frame with the window's rectangle, `wake_fd()` becomes readable
//! after each new frame (call `drain_wake()` after it fires).

use crate::{
    cosmic_windows::{
        zcosmic_toplevel_handle_v1::{self, ZcosmicToplevelHandleV1},
        zcosmic_toplevel_info_v1::{self, ZcosmicToplevelInfoV1},
    },
    lens::{Frame, Geometry},
};
use std::{
    fs::File,
    os::fd::{AsFd, FromRawFd, RawFd},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use wayland_client::{
    globals::{registry_queue_init, GlobalListContents},
    protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool},
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum,
};
use wayland_protocols::ext::{
    foreign_toplevel_list::v1::client::{
        ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1},
        ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1},
    },
    image_capture_source::v1::client::{
        ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1,
        ext_image_capture_source_v1::ExtImageCaptureSourceV1,
    },
    image_copy_capture::v1::client::{
        ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1},
        ext_image_copy_capture_manager_v1::{ExtImageCopyCaptureManagerV1, Options},
        ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
    },
};

/// Newest captured frame and where its window sits.
pub struct Shot {
    pub frame: Arc<Frame>,
    pub geom: Geometry,
}

pub struct Capturer {
    active: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    latest: Arc<Mutex<Option<Shot>>>,
    /// Bumped on every on/off change; a frame is stored only if it started in the current generation.
    generation: Arc<AtomicU64>,
    wake_read: RawFd,
    thread: Option<JoinHandle<()>>,
}

impl Capturer {
    /// Connects and starts the worker. Fails when the compositor lacks a needed protocol.
    pub fn start() -> Result<Capturer, String> {
        let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
        let (globals, mut queue) = registry_queue_init::<Worker>(&conn).map_err(|e| e.to_string())?;
        let qh = queue.handle();
        let bind_err = |name: &str| format!("compositor lacks {name}");
        let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).map_err(|_| bind_err("wl_shm"))?;
        let cap_mgr: ExtImageCopyCaptureManagerV1 = globals.bind(&qh, 1..=1, ()).map_err(|_| bind_err("ext_image_copy_capture_manager_v1"))?;
        let source_mgr: ExtForeignToplevelImageCaptureSourceManagerV1 =
            globals.bind(&qh, 1..=1, ()).map_err(|_| bind_err("ext_foreign_toplevel_image_capture_source_manager_v1"))?;
        let list: ExtForeignToplevelListV1 = globals.bind(&qh, 1..=1, ()).map_err(|_| bind_err("ext_foreign_toplevel_list_v1"))?;
        let info: ZcosmicToplevelInfoV1 = globals.bind(&qh, 2..=3, ()).map_err(|_| bind_err("zcosmic_toplevel_info_v1 (version 2 or 3)"))?;
        let _output: wl_output::WlOutput = globals.bind(&qh, 1..=4, ()).map_err(|_| bind_err("wl_output"))?;

        let mut fds = [0i32; 2];
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) } != 0 {
            return Err("pipe2 failed".into());
        }
        let (wake_read, wake_write) = (fds[0], fds[1]);

        let mut worker = Worker { info, tops: Vec::new(), session: SessionState::default() };
        // Let the initial toplevel, state and geometry events arrive.
        let _ = queue.roundtrip(&mut worker);
        let _ = queue.roundtrip(&mut worker);

        let active = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let latest = Arc::new(Mutex::new(None));
        let generation = Arc::new(AtomicU64::new(0));
        let ctx = Ctx { queue, qh, shm, cap_mgr, source_mgr, _list: list, worker, wake_write };
        let (a, s, l, g) = (active.clone(), stop.clone(), latest.clone(), generation.clone());
        let thread = std::thread::Builder::new()
            .name("capture".into())
            .spawn(move || run_worker(ctx, a, s, l, g))
            .map_err(|e| e.to_string())?;
        Ok(Capturer { active, stop, latest, generation, wake_read, thread: Some(thread) })
    }

    /// Capture only while Magnify is on screen. Any change also forgets the last frame, so the next
    /// show never flashes old content. The worker stores a frame under the same lock after checking
    /// `active`, so a frame from before the change cannot land after it.
    pub fn set_active(&self, on: bool) {
        let mut latest = self.latest.lock().unwrap();
        if self.active.swap(on, Ordering::SeqCst) != on {
            self.generation.fetch_add(1, Ordering::SeqCst);
            *latest = None;
        }
    }

    pub fn latest(&self) -> Option<(Arc<Frame>, Geometry)> {
        self.latest.lock().unwrap().as_ref().map(|s| (s.frame.clone(), s.geom))
    }

    pub fn wake_fd(&self) -> RawFd {
        self.wake_read
    }

    pub fn drain_wake(&self) {
        let mut buf = [0u8; 64];
        while unsafe { libc::read(self.wake_read, buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
    }
}

impl Drop for Capturer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        unsafe { libc::close(self.wake_read) };
    }
}

// ------------------------------------------------------------------------------------------
// Worker

struct Top {
    foreign: ExtForeignToplevelHandleV1,
    cosmic: ZcosmicToplevelHandleV1,
    activated: bool,
    minimized: bool,
    /// Rectangle on the first output that reported one: x, y, width, height in logical pixels.
    rect: Option<(i32, i32, i32, i32)>,
}

#[derive(Default)]
struct SessionState {
    size: Option<(u32, u32)>,
    formats: Vec<u32>,
    done: bool,
    stopped: bool,
    frame_ready: bool,
    frame_failed: Option<String>,
}

struct Worker {
    info: ZcosmicToplevelInfoV1,
    tops: Vec<Top>,
    session: SessionState,
}

struct Ctx {
    queue: EventQueue<Worker>,
    qh: QueueHandle<Worker>,
    shm: wl_shm::WlShm,
    cap_mgr: ExtImageCopyCaptureManagerV1,
    source_mgr: ExtForeignToplevelImageCaptureSourceManagerV1,
    _list: ExtForeignToplevelListV1,
    worker: Worker,
    wake_write: RawFd,
}

/// A capture session for one window with its shared-memory buffer.
struct Live {
    foreign: ExtForeignToplevelHandleV1,
    source: ExtImageCaptureSourceV1,
    session: ExtImageCopyCaptureSessionV1,
    buffer: wl_buffer::WlBuffer,
    map: Mapping,
    w: u32,
    h: u32,
    rgb_order: bool,
}

struct Mapping {
    ptr: *mut u8,
    len: usize,
    _file: File,
}

impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.ptr.cast(), self.len) };
    }
}

fn run_worker(mut ctx: Ctx, active: Arc<AtomicBool>, stop: Arc<AtomicBool>, latest: Arc<Mutex<Option<Shot>>>, generation: Arc<AtomicU64>) {
    let mut live: Option<Live> = None;
    let mut last_error = String::new();
    while !stop.load(Ordering::SeqCst) {
        if !active.load(Ordering::SeqCst) {
            if let Some(l) = live.take() {
                l.destroy();
            }
            pump(&mut ctx, 50, |_| false);
            continue;
        }
        // Keep window state current, then follow the active window.
        pump(&mut ctx, 0, |_| false);
        let Some(top) = ctx.worker.tops.iter().find(|t| t.activated && !t.minimized && t.rect.is_some()) else {
            if let Some(l) = live.take() {
                l.destroy(); // the window is gone; do not keep its session and buffer
            }
            pump(&mut ctx, 50, |_| false);
            continue;
        };
        let (foreign, rect) = (top.foreign.clone(), top.rect.unwrap());
        if live.as_ref().is_some_and(|l| l.foreign != foreign) {
            live.take().unwrap().destroy();
        }
        if live.is_none() {
            match open_live(&mut ctx, &foreign) {
                Ok(l) => live = Some(l),
                Err(e) => {
                    if e != last_error {
                        eprintln!("magnify capture: {e}");
                        last_error = e;
                    }
                    pump(&mut ctx, 200, |_| false);
                    continue;
                }
            }
        }
        let started = Instant::now();
        let started_in = generation.load(Ordering::SeqCst);
        match grab_frame(&mut ctx, live.as_ref().unwrap()) {
            Ok(frame) => {
                last_error.clear();
                let geom = Geometry { x: f64::from(rect.0), y: f64::from(rect.1), w: f64::from(rect.2), h: f64::from(rect.3) };
                let mut slot = latest.lock().unwrap();
                if active.load(Ordering::SeqCst) && generation.load(Ordering::SeqCst) == started_in {
                    *slot = Some(Shot { frame: Arc::new(frame), geom });
                    drop(slot);
                    let one = 1u8;
                    unsafe { libc::write(ctx.wake_write, (&one as *const u8).cast(), 1) };
                }
            }
            Err(e) => {
                if e != last_error {
                    eprintln!("magnify capture: {e}");
                    last_error = e;
                }
                // The window may have resized or closed: start a fresh session next round.
                live.take().unwrap().destroy();
                pump(&mut ctx, 100, |_| false);
            }
        }
        // About 30 frames a second at most.
        let spent = started.elapsed();
        if spent < Duration::from_millis(33) {
            pump(&mut ctx, (Duration::from_millis(33) - spent).as_millis() as u64, |_| false);
        }
    }
    unsafe { libc::close(ctx.wake_write) };
    if let Some(l) = live.take() {
        l.destroy();
    }
}

impl Live {
    fn destroy(self) {
        self.buffer.destroy();
        self.session.destroy();
        self.source.destroy();
    }
}

/// Dispatch events for up to `ms` milliseconds, or until `done` says so.
fn pump(ctx: &mut Ctx, ms: u64, done: impl Fn(&Worker) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_millis(ms);
    loop {
        if ctx.queue.dispatch_pending(&mut ctx.worker).is_err() {
            // The connection is broken; wait instead of letting callers spin on it.
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()).min(Duration::from_millis(50)));
            return done(&ctx.worker);
        }
        if done(&ctx.worker) {
            return true;
        }
        let _ = ctx.queue.flush();
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        if let Some(guard) = ctx.queue.prepare_read() {
            let fd = std::os::fd::AsRawFd::as_raw_fd(&guard.connection_fd());
            let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            unsafe { libc::poll(&mut p, 1, left.as_millis().min(100) as i32) };
            if p.revents & libc::POLLIN != 0 {
                let _ = guard.read();
            }
        }
    }
}

fn open_live(ctx: &mut Ctx, foreign: &ExtForeignToplevelHandleV1) -> Result<Live, String> {
    ctx.worker.session = SessionState::default();
    let source = ctx.source_mgr.create_source(foreign, &ctx.qh, ());
    let session = ctx.cap_mgr.create_session(&source, Options::empty(), &ctx.qh, ());
    match open_buffer(ctx) {
        Ok((buffer, map, w, h, rgb_order)) => Ok(Live { foreign: foreign.clone(), source, session, buffer, map, w, h, rgb_order }),
        Err(e) => {
            // The compositor keeps these until they are destroyed, so do not leak them on failure.
            session.destroy();
            source.destroy();
            Err(e)
        }
    }
}

/// Wait for the session's buffer constraints and create the shared-memory buffer to match.
fn open_buffer(ctx: &mut Ctx) -> Result<(wl_buffer::WlBuffer, Mapping, u32, u32, bool), String> {
    if !pump(ctx, 2000, |w| w.session.done || w.session.stopped) || ctx.worker.session.stopped {
        return Err("the compositor offered no capture buffer for the active window".into());
    }
    let (w, h) = ctx.worker.session.size.ok_or("no buffer size from the compositor")?;
    // wl_shm codes: 0 Argb8888, 1 Xrgb8888, others use the DRM fourcc. Memory order on little-endian:
    // Argb/Xrgb are B,G,R,A; Abgr/Xbgr are R,G,B,A.
    const ABGR8888: u32 = 0x3432_4241;
    const XBGR8888: u32 = 0x3432_4258;
    let code = [0u32, 1, ABGR8888, XBGR8888]
        .into_iter()
        .find(|f| ctx.worker.session.formats.contains(f))
        .ok_or_else(|| format!("no readable shm format among {:?}", ctx.worker.session.formats))?;
    let (format, rgb_order) = match code {
        0 => (wl_shm::Format::Argb8888, false),
        1 => (wl_shm::Format::Xrgb8888, false),
        ABGR8888 => (wl_shm::Format::Abgr8888, true),
        _ => (wl_shm::Format::Xbgr8888, true),
    };
    let stride = w as usize * 4;
    let len = stride * h as usize;
    let fd = unsafe { libc::memfd_create(b"spotlight-capture\0".as_ptr().cast(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err("memfd_create failed".into());
    }
    let file = unsafe { File::from_raw_fd(fd) };
    file.set_len(len as u64).map_err(|e| e.to_string())?;
    let ptr = unsafe { libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ, libc::MAP_SHARED, fd, 0) };
    if ptr == libc::MAP_FAILED {
        return Err("mmap failed".into());
    }
    let pool = ctx.shm.create_pool(file.as_fd(), len as i32, &ctx.qh, ());
    let buffer = pool.create_buffer(0, w as i32, h as i32, stride as i32, format, &ctx.qh, ());
    pool.destroy();
    Ok((buffer, Mapping { ptr: ptr.cast(), len, _file: file }, w, h, rgb_order))
}

fn grab_frame(ctx: &mut Ctx, live: &Live) -> Result<Frame, String> {
    ctx.worker.session.frame_ready = false;
    ctx.worker.session.frame_failed = None;
    let frame = live.session.create_frame(&ctx.qh, ());
    frame.attach_buffer(&live.buffer);
    frame.damage_buffer(0, 0, live.w as i32, live.h as i32);
    frame.capture();
    let answered = pump(ctx, 2000, |w| w.session.frame_ready || w.session.frame_failed.is_some() || w.session.stopped);
    frame.destroy();
    if !answered {
        return Err("no frame within 2 s".into());
    }
    if let Some(reason) = &ctx.worker.session.frame_failed {
        return Err(format!("frame failed ({reason})"));
    }
    if ctx.worker.session.stopped {
        return Err("capture session stopped".into());
    }
    let data = unsafe { std::slice::from_raw_parts(live.map.ptr, live.map.len) }.to_vec();
    Ok(Frame { w: live.w as usize, h: live.h as usize, rgb_order: live.rgb_order, data })
}

// ------------------------------------------------------------------------------------------
// Event handling

impl Dispatch<ExtForeignToplevelListV1, ()> for Worker {
    fn event(s: &mut Self, _: &ExtForeignToplevelListV1, e: ext_foreign_toplevel_list_v1::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } = e {
            let cosmic = s.info.get_cosmic_toplevel(&toplevel, qh, toplevel.id());
            s.tops.push(Top { foreign: toplevel, cosmic, activated: false, minimized: false, rect: None });
        }
    }
    wayland_client::event_created_child!(Worker, ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ExtForeignToplevelHandleV1, ())
    ]);
}

impl Dispatch<ExtForeignToplevelHandleV1, ()> for Worker {
    fn event(s: &mut Self, h: &ExtForeignToplevelHandleV1, e: ext_foreign_toplevel_handle_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let ext_foreign_toplevel_handle_v1::Event::Closed = e {
            if let Some(i) = s.tops.iter().position(|t| &t.foreign == h) {
                let t = s.tops.remove(i);
                t.cosmic.destroy();
                t.foreign.destroy();
            }
        }
    }
}

impl Dispatch<ZcosmicToplevelInfoV1, ()> for Worker {
    fn event(_: &mut Self, _: &ZcosmicToplevelInfoV1, _: zcosmic_toplevel_info_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
    wayland_client::event_created_child!(Worker, ZcosmicToplevelInfoV1, [
        zcosmic_toplevel_info_v1::EVT_TOPLEVEL_OPCODE => (ZcosmicToplevelHandleV1, wayland_client::backend::ObjectId::null())
    ]);
}

/// User data of a cosmic handle is the id of its ext_foreign_toplevel_handle_v1.
impl Dispatch<ZcosmicToplevelHandleV1, wayland_client::backend::ObjectId> for Worker {
    fn event(s: &mut Self, _: &ZcosmicToplevelHandleV1, e: zcosmic_toplevel_handle_v1::Event, key: &wayland_client::backend::ObjectId, _: &Connection, _: &QueueHandle<Self>) {
        let Some(t) = s.tops.iter_mut().find(|t| &t.foreign.id() == key) else { return };
        match e {
            zcosmic_toplevel_handle_v1::Event::State { state } => {
                let states: Vec<u32> = state.chunks_exact(4).map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]])).collect();
                t.minimized = states.contains(&(zcosmic_toplevel_handle_v1::State::Minimized as u32));
                // Losing activation is not recorded: showing the overlay can deactivate the window
                // that was active, and that window is still the one to magnify. Another window
                // becoming active takes over (below).
                if states.contains(&(zcosmic_toplevel_handle_v1::State::Activated as u32)) {
                    t.activated = true;
                    let me = key.clone();
                    for o in s.tops.iter_mut().filter(|o| o.foreign.id() != me) {
                        o.activated = false;
                    }
                }
            }
            zcosmic_toplevel_handle_v1::Event::Geometry { x, y, width, height, .. } => {
                // Only one output is handled: the latest report wins.
                t.rect = Some((x, y, width, height));
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, ()> for Worker {
    fn event(s: &mut Self, _: &ExtImageCopyCaptureSessionV1, e: ext_image_copy_capture_session_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use ext_image_copy_capture_session_v1::Event;
        match e {
            Event::BufferSize { width, height } => s.session.size = Some((width, height)),
            Event::ShmFormat { format } => match format {
                WEnum::Value(f) => s.session.formats.push(f as u32),
                WEnum::Unknown(raw) => s.session.formats.push(raw),
            },
            Event::Done => s.session.done = true,
            Event::Stopped => s.session.stopped = true,
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, ()> for Worker {
    fn event(s: &mut Self, _: &ExtImageCopyCaptureFrameV1, e: ext_image_copy_capture_frame_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use ext_image_copy_capture_frame_v1::Event;
        match e {
            Event::Ready => s.session.frame_ready = true,
            Event::Failed { reason } => s.session.frame_failed = Some(format!("{reason:?}")),
            _ => {}
        }
    }
}

macro_rules! ignore_events {
    ($($iface:ty),* $(,)?) => {$(
        impl Dispatch<$iface, ()> for Worker {
            fn event(_: &mut Self, _: &$iface, _: <$iface as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}

ignore_events!(
    wl_output::WlOutput,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    ExtImageCopyCaptureManagerV1,
    ExtImageCaptureSourceV1,
    ExtForeignToplevelImageCaptureSourceManagerV1,
);

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Worker {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
