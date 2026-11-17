//! Feasibility test of screen capture through ext-image-copy-capture-v1.
//! Captures one frame of the first output into shared memory and prints statistics and a few
//! sample pixels. It writes no image file. Usage: capture_test [--cursor] [x,y ...] (pixels in the capture)

use std::{
    fs::File,
    os::{
        fd::{AsFd, FromRawFd},
        unix::fs::FileExt,
    },
    time::{Duration, Instant},
};
use wayland_client::{
    globals::{registry_queue_init, GlobalListContents},
    protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool},
    Connection, Dispatch, EventQueue, QueueHandle, WEnum,
};
use wayland_protocols::ext::{
    foreign_toplevel_list::v1::client::{
        ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1},
        ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1},
    },
    image_capture_source::v1::client::{
        ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1,
        ext_image_capture_source_v1::ExtImageCaptureSourceV1,
        ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
    },
    image_copy_capture::v1::client::{
        ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1},
        ext_image_copy_capture_manager_v1::{ExtImageCopyCaptureManagerV1, Options},
        ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
    },
};

#[derive(Default)]
struct State {
    size: Option<(u32, u32)>,
    formats: Vec<u32>,
    session_done: bool,
    session_stopped: bool,
    frame_ready: bool,
    frame_failed: Option<String>,
    toplevels: Vec<(ExtForeignToplevelHandleV1, String)>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(_: &mut Self, _: &wl_output::WlOutput, _: wl_output::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_shm::WlShm, ()> for State {
    fn event(_: &mut Self, _: &wl_shm::WlShm, _: wl_shm::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_shm_pool::WlShmPool, ()> for State {
    fn event(_: &mut Self, _: &wl_shm_pool::WlShmPool, _: wl_shm_pool::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_buffer::WlBuffer, ()> for State {
    fn event(_: &mut Self, _: &wl_buffer::WlBuffer, _: wl_buffer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ExtOutputImageCaptureSourceManagerV1, ()> for State {
    fn event(_: &mut Self, _: &ExtOutputImageCaptureSourceManagerV1, _: <ExtOutputImageCaptureSourceManagerV1 as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ExtImageCaptureSourceV1, ()> for State {
    fn event(_: &mut Self, _: &ExtImageCaptureSourceV1, _: <ExtImageCaptureSourceV1 as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ExtImageCopyCaptureManagerV1, ()> for State {
    fn event(_: &mut Self, _: &ExtImageCopyCaptureManagerV1, _: <ExtImageCopyCaptureManagerV1 as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ExtForeignToplevelListV1, ()> for State {
    fn event(s: &mut Self, _: &ExtForeignToplevelListV1, e: ext_foreign_toplevel_list_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } = e {
            s.toplevels.push((toplevel, String::new()));
        }
    }
    wayland_client::event_created_child!(State, ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ExtForeignToplevelHandleV1, ())
    ]);
}
impl Dispatch<ExtForeignToplevelHandleV1, ()> for State {
    fn event(s: &mut Self, h: &ExtForeignToplevelHandleV1, e: ext_foreign_toplevel_handle_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let ext_foreign_toplevel_handle_v1::Event::AppId { app_id } = e {
            if let Some(t) = s.toplevels.iter_mut().find(|t| &t.0 == h) {
                t.1 = app_id;
            }
        }
    }
}
impl Dispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ()> for State {
    fn event(_: &mut Self, _: &ExtForeignToplevelImageCaptureSourceManagerV1, _: <ExtForeignToplevelImageCaptureSourceManagerV1 as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ExtImageCopyCaptureSessionV1, ()> for State {
    fn event(s: &mut Self, _: &ExtImageCopyCaptureSessionV1, e: ext_image_copy_capture_session_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use ext_image_copy_capture_session_v1::Event;
        match e {
            Event::BufferSize { width, height } => s.size = Some((width, height)),
            Event::ShmFormat { format } => {
                if let WEnum::Value(f) = format {
                    s.formats.push(f as u32);
                } else if let WEnum::Unknown(raw) = format {
                    s.formats.push(raw);
                }
            }
            Event::Done => s.session_done = true,
            Event::Stopped => s.session_stopped = true,
            _ => {}
        }
    }
}
impl Dispatch<ExtImageCopyCaptureFrameV1, ()> for State {
    fn event(s: &mut Self, _: &ExtImageCopyCaptureFrameV1, e: ext_image_copy_capture_frame_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use ext_image_copy_capture_frame_v1::Event;
        match e {
            Event::Ready => s.frame_ready = true,
            Event::Failed { reason } => s.frame_failed = Some(format!("{reason:?}")),
            _ => {}
        }
    }
}

/// Dispatch events until `done(state)` or the deadline.
fn pump(q: &mut EventQueue<State>, st: &mut State, secs: u64, done: impl Fn(&State) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        q.dispatch_pending(st).expect("dispatch pending");
        if done(st) {
            return true;
        }
        q.flush().expect("flush");
        if let Some(guard) = q.prepare_read() {
            let fd = std::os::fd::AsRawFd::as_raw_fd(&guard.connection_fd());
            let mut p = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            let left = deadline.saturating_duration_since(Instant::now()).as_millis().min(200) as i32;
            unsafe { libc::poll(&mut p, 1, left) };
            if p.revents & libc::POLLIN != 0 {
                let _ = guard.read();
            }
        }
    }
    q.dispatch_pending(st).expect("dispatch pending");
    done(st)
}

struct Captured {
    w: u32,
    h: u32,
    file: File,
    rgb_order: bool,
}

/// Capture one frame of `source` into shared memory.
fn capture(
    q: &mut EventQueue<State>,
    st: &mut State,
    qh: &QueueHandle<State>,
    cap_mgr: &ExtImageCopyCaptureManagerV1,
    shm: &wl_shm::WlShm,
    source: &ExtImageCaptureSourceV1,
    options: Options,
) -> Result<Captured, String> {
    st.size = None;
    st.formats.clear();
    st.session_done = false;
    st.session_stopped = false;
    st.frame_ready = false;
    st.frame_failed = None;

    let session = cap_mgr.create_session(source, options, qh, ());
    if !pump(q, st, 10, |s| s.session_done || s.session_stopped) {
        return Err("session: no answer within 10 s (a permission dialog may be waiting)".into());
    }
    if st.session_stopped {
        return Err("session: stopped by the compositor before any buffer was offered".into());
    }
    let (w, h) = st.size.ok_or("no buffer_size")?;
    // wl_shm codes: 0 = Argb8888, 1 = Xrgb8888; other formats use their DRM fourcc value.
    // Memory byte order on little-endian: Argb/Xrgb = B,G,R,A ; Abgr/Xbgr = R,G,B,A.
    const ABGR8888: u32 = 0x3432_4241;
    const XBGR8888: u32 = 0x3432_4258;
    let fmt_code = [0u32, 1, ABGR8888, XBGR8888]
        .into_iter()
        .find(|f| st.formats.contains(f))
        .ok_or(format!("no readable shm format among {:?}", st.formats))?;
    let (format, rgb_order) = match fmt_code {
        0 => (wl_shm::Format::Argb8888, false),
        1 => (wl_shm::Format::Xrgb8888, false),
        ABGR8888 => (wl_shm::Format::Abgr8888, true),
        _ => (wl_shm::Format::Xbgr8888, true),
    };

    let stride = w * 4;
    let size = u64::from(stride) * u64::from(h);
    let fd = unsafe { libc::memfd_create(b"capture\0".as_ptr().cast(), libc::MFD_CLOEXEC) };
    assert!(fd >= 0, "memfd_create failed");
    let file = unsafe { File::from_raw_fd(fd) };
    file.set_len(size).map_err(|e| e.to_string())?;
    let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
    let buffer = pool.create_buffer(0, w as i32, h as i32, stride as i32, format, qh, ());

    let frame = session.create_frame(qh, ());
    frame.attach_buffer(&buffer);
    frame.damage_buffer(0, 0, w as i32, h as i32);
    frame.capture();
    if !pump(q, st, 10, |s| s.frame_ready || s.frame_failed.is_some()) {
        return Err("frame: no answer within 10 s".into());
    }
    if let Some(reason) = &st.frame_failed {
        return Err(format!("frame: failed ({reason})"));
    }
    Ok(Captured { w, h, file, rgb_order })
}

/// Returns (average rgb, non-black pixel count, count of pixels exactly equal to `find`).
fn stats(c: &Captured, find: Option<[u8; 3]>) -> ([u64; 3], u64, u64) {
    let stride = u64::from(c.w) * 4;
    let size = stride * u64::from(c.h);
    let (ri, bi) = if c.rgb_order { (0, 2) } else { (2, 0) };
    let mut sum = [0u64; 3];
    let mut nonzero = 0u64;
    let mut found = 0u64;
    let mut chunk = vec![0u8; (stride as usize) * 16];
    let mut off = 0u64;
    while off < size {
        let n = chunk.len().min((size - off) as usize);
        c.file.read_exact_at(&mut chunk[..n], off).expect("read");
        for px in chunk[..n].chunks_exact(4) {
            sum[0] += u64::from(px[ri]);
            sum[1] += u64::from(px[1]);
            sum[2] += u64::from(px[bi]);
            if px[0] | px[1] | px[2] != 0 {
                nonzero += 1;
            }
            if find == Some([px[ri], px[1], px[bi]]) {
                found += 1;
            }
        }
        off += n as u64;
    }
    let pixels = u64::from(c.w) * u64::from(c.h);
    ([sum[0] / pixels, sum[1] / pixels, sum[2] / pixels], nonzero, found)
}

fn main() {
    let mut cursor = false;
    let mut toplevels = false;
    let mut find: Option<[u8; 3]> = None;
    let mut samples: Vec<(u64, u64)> = Vec::new();
    for a in std::env::args().skip(1) {
        if a == "--cursor" {
            cursor = true;
        } else if a == "--toplevels" {
            toplevels = true;
        } else if let Some(v) = a.strip_prefix("--find=") {
            let p: Vec<u8> = v.split(',').map(|x| x.parse().expect("--find=R,G,B")).collect();
            find = Some([p[0], p[1], p[2]]);
        } else if let Some((x, y)) = a.split_once(',') {
            samples.push((x.parse().expect("x"), y.parse().expect("y")));
        }
    }
    let options = if cursor { Options::PaintCursors } else { Options::empty() };

    let conn = Connection::connect_to_env().expect("no wayland connection");
    let (globals, mut q) = registry_queue_init::<State>(&conn).expect("registry");
    let qh = q.handle();
    let mut st = State::default();

    let output: wl_output::WlOutput = globals.bind(&qh, 1..=4, ()).expect("wl_output");
    let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).expect("wl_shm");
    let cap_mgr: ExtImageCopyCaptureManagerV1 = globals.bind(&qh, 1..=1, ()).expect("ext_image_copy_capture_manager_v1 missing");

    if !toplevels {
        let src_mgr: ExtOutputImageCaptureSourceManagerV1 = globals.bind(&qh, 1..=1, ()).expect("ext_output_image_capture_source_manager_v1 missing");
        let source = src_mgr.create_source(&output, &qh, ());
        let t0 = Instant::now();
        match capture(&mut q, &mut st, &qh, &cap_mgr, &shm, &source, options) {
            Err(e) => {
                println!("{e}");
                std::process::exit(1);
            }
            Ok(c) => {
                let (avg, nonzero, found) = stats(&c, find);
                println!("output capture: {}x{} in {} ms, cursor painted {cursor}", c.w, c.h, t0.elapsed().as_millis());
                println!("average RGB {avg:?}; non-black pixels {nonzero} of {}", u64::from(c.w) * u64::from(c.h));
                if find.is_some() {
                    println!("pixels exactly {:?}: {found}", find.unwrap());
                }
                let (ri, bi) = if c.rgb_order { (0, 2) } else { (2, 0) };
                for (x, y) in samples {
                    let mut px = [0u8; 4];
                    c.file.read_exact_at(&mut px, y * u64::from(c.w) * 4 + x * 4).expect("read sample");
                    println!("pixel ({x},{y}): R{} G{} B{} A{}", px[ri], px[1], px[bi], px[3]);
                }
            }
        }
        return;
    }

    let list: ExtForeignToplevelListV1 = globals.bind(&qh, 1..=1, ()).expect("ext_foreign_toplevel_list_v1 missing");
    let tl_mgr: ExtForeignToplevelImageCaptureSourceManagerV1 = globals.bind(&qh, 1..=1, ()).expect("ext_foreign_toplevel_image_capture_source_manager_v1 missing");
    pump(&mut q, &mut st, 2, |_| false); // let the toplevel events arrive
    println!("toplevels listed: {}", st.toplevels.len());
    let handles: Vec<(ExtForeignToplevelHandleV1, String)> = st.toplevels.clone();
    for (i, (handle, app_id)) in handles.iter().enumerate() {
        let source = tl_mgr.create_source(handle, &qh, ());
        let t0 = Instant::now();
        match capture(&mut q, &mut st, &qh, &cap_mgr, &shm, &source, options) {
            Err(e) => println!("toplevel {i} (app_id {app_id:?}): {e}"),
            Ok(c) => {
                let (avg, nonzero, found) = stats(&c, find);
                println!(
                    "toplevel {i} (app_id {app_id:?}): {}x{} in {} ms, average RGB {avg:?}, non-black {nonzero} of {}, exact-color pixels {found}",
                    c.w, c.h, t0.elapsed().as_millis(), u64::from(c.w) * u64::from(c.h)
                );
            }
        }
    }
    drop(list);
}
