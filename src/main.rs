//! spotlight-rs: on-screen pointer for the Logitech Spotlight on COSMIC (Wayland).
//!
//! Commands:
//!   spotlight-rs [run] [--config PATH] [--timer-now] [--trace]
//!                                        run the program (default); --timer-now starts the countdown at launch
//!   spotlight-rs setup                   check access and protocols, print what to fix
//!   spotlight-rs example-config          print a config file with every default
//!   spotlight-rs udev-rules [--uinput]   print the udev rule for the receiver (or for /dev/uinput)
//!   spotlight-rs demo [--mode M] [--at X,Y] [--seconds N] [--cycle] [--config PATH]
//!                                        draw one pointer, click-through, for screenshots
//!
//! The overlay is a full-screen layer-shell surface. It exists only while the remote moves the
//! cursor or a laser hold is on (it accepts input then, so the compositor reports the cursor
//! position). It is destroyed after `idle_ms` without motion so clicks reach other windows, and
//! created anew on the next motion.

use smithay_client_toolkit::{
    compositor::{CompositorState, Region},
    output::OutputState,
    registry::RegistryState,
    seat::SeatState,
    shell::wlr_layer::LayerShell,
    shm::{slot::SlotPool, Shm},
};
use spotlight_rs::{
    window_capture::Capturer,
    config::{self, Config, ConfigWatcher},
    logitech_protocol, remote_input,
    remote_input::{Reader, RemoteEvent},
    state_machine::{Mode, State},
};
use std::{
    io::ErrorKind,
    os::fd::AsRawFd,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use wayland_client::{
    globals::registry_queue_init,
    Connection,
};

/// Like eprintln!, but only with `--trace` (input and overlay detail that is too chatty for normal use).
macro_rules! trace {
    ($($arg:tt)*) => {
        if crate::TRACE.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!($($arg)*);
        }
    };
}

/// Like eprintln! but never panics when stderr is gone (a closed terminal).
macro_rules! say {
    ($($arg:tt)*) => {{
        use std::io::Write;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

mod overlay;
mod remote;

use overlay::App;
use remote::Remote;

/// `--trace`: also log taps, holds, raw button events, HID++ hold reports and overlay shows.
pub(crate) static TRACE: AtomicBool = AtomicBool::new(false);
static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Milliseconds since the program started (for the trace).
pub(crate) fn trace_ms() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

pub(crate) static STOP: AtomicBool = AtomicBool::new(false);
static STOP_SIGNAL: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

extern "C" fn on_signal(sig: libc::c_int) {
    STOP_SIGNAL.store(sig, Ordering::SeqCst);
    STOP.store(true, Ordering::SeqCst);
}

fn install_signal_handlers() {
    unsafe {
        libc::signal(libc::SIGINT, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
        // Closing the terminal sends SIGHUP; stop cleanly so the remote's divert is undone.
        libc::signal(libc::SIGHUP, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
    }
}

// ------------------------------------------------------------------------------------------
// Command line

struct Demo {
    mode: Option<Mode>,
    at: Option<(f64, f64)>,
    seconds: u64,
    cycle: bool,
}

enum Command {
    Run { config: PathBuf, demo: Option<Demo>, timer_now: bool },
    ExampleConfig,
    UdevRules { uinput: bool },
    Setup { config: PathBuf },
}

fn parse_args(args: &[String]) -> Result<Command, String> {
    let mut config = config::default_path();
    let mut iter = args.iter().peekable();
    let mut cmd = "run".to_string();
    if let Some(first) = iter.peek() {
        if !first.starts_with('-') {
            cmd = iter.next().unwrap().clone();
        }
    }
    let mut demo = Demo { mode: None, at: None, seconds: 10, cycle: false };
    let mut timer_now = false;
    let mut uinput_rule = false;
    while let Some(a) = iter.next() {
        let mut value = |name: &str| iter.next().cloned().ok_or(format!("{name} needs a value"));
        match a.as_str() {
            "--config" => config = PathBuf::from(value("--config")?),
            "--mode" => {
                demo.mode = Some(match value("--mode")?.as_str() {
                    "highlight" => Mode::Highlight,
                    "magnify" => Mode::Magnify,
                    "circle" => Mode::Circle,
                    "laser" => Mode::Laser,
                    other => return Err(format!("unknown mode {other:?}")),
                })
            }
            "--at" => {
                let v = value("--at")?;
                let (x, y) = v.split_once(',').ok_or("--at needs X,Y")?;
                demo.at = Some((x.parse().map_err(|_| "bad X")?, y.parse().map_err(|_| "bad Y")?));
            }
            "--seconds" => demo.seconds = value("--seconds")?.parse().map_err(|_| "bad --seconds")?,
            "--cycle" => demo.cycle = true,
            "--timer-now" => timer_now = true,
            "--trace" => TRACE.store(true, Ordering::SeqCst),
            "--uinput" => uinput_rule = true,
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    match cmd.as_str() {
        "run" => Ok(Command::Run { config, demo: None, timer_now }),
        "demo" => Ok(Command::Run { config, demo: Some(demo), timer_now: false }),
        "example-config" => Ok(Command::ExampleConfig),
        "udev-rules" => Ok(Command::UdevRules { uinput: uinput_rule }),
        "setup" => Ok(Command::Setup { config }),
        other => Err(format!("unknown command {other:?}")),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&args) {
        Ok(Command::ExampleConfig) => print!("{}", config::EXAMPLE),
        Ok(Command::UdevRules { uinput }) => print!("{}", if uinput { UINPUT_RULE } else { UDEV_RULE }),
        Ok(Command::Setup { config }) => setup(&config),
        Ok(Command::Run { config, demo, timer_now }) => run(config, demo, timer_now),
        Err(e) => {
            eprintln!("error: {e}\nsee the top of src/main.rs for the commands");
            std::process::exit(2);
        }
    }
}

// ------------------------------------------------------------------------------------------
// Setup check: reads state and prints commands; it never runs sudo.

const UDEV_RULE: &str = "# Logitech Spotlight USB Receiver: user access for the session user\n\
SUBSYSTEMS==\"usb\", ATTRS{idVendor}==\"046d\", ATTRS{idProduct}==\"c53e\", MODE=\"0660\", TAG+=\"uaccess\"\n\
SUBSYSTEM==\"hidraw\", ATTRS{idVendor}==\"046d\", ATTRS{idProduct}==\"c53e\", MODE=\"0660\", TAG+=\"uaccess\"\n";

/// Gives the logged-in user write access to /dev/uinput. Many systems already ship this rule.
const UINPUT_RULE: &str = "# /dev/uinput: user access for the session user\n\
KERNEL==\"uinput\", SUBSYSTEM==\"misc\", TAG+=\"uaccess\", OPTIONS+=\"static_node=uinput\"\n";

fn setup(config_path: &std::path::Path) {
    let problems = std::cell::Cell::new(0);
    let note = |ok: bool, good: &str, bad: &str| {
        if ok {
            println!("ok       {good}");
        } else {
            println!("MISSING  {bad}");
            problems.set(problems.get() + 1);
        }
    };

    let nodes = logitech_protocol::find_hidpp_nodes(logitech_protocol::RECEIVER_VENDOR, logitech_protocol::RECEIVER_PRODUCT);
    note(!nodes.is_empty(), "receiver found (046d:c53e)", "receiver not found: plug in the Spotlight receiver");
    let hid_access = nodes.iter().any(|n| logitech_protocol::Device::open(n).is_ok());
    note(hid_access || nodes.is_empty(), "HID++ node is accessible", "no access to the receiver's HID++ node: install the udev rule (below)");

    let cfg = Config::default();
    for (label, name) in [("mouse", &cfg.input.device_name), ("keyboard", &cfg.input.keyboard_name)] {
        match remote_input::find_event_node_now(name) {
            Some(path) => note(
                Reader::open(&path).is_ok(),
                &format!("{label} node {path} is readable"),
                &format!("cannot read {label} node {path}: install the udev rule (below)"),
            ),
            None => note(false, "", &format!("{label} node {name:?} not found in /proc/bus/input/devices")),
        }
    }

    match Connection::connect_to_env() {
        Ok(conn) => match registry_queue_init::<App>(&conn) {
            Ok((globals, _q)) => {
                let have: Vec<String> = globals.contents().with_list(|l| l.iter().map(|g| g.interface.clone()).collect());
                for need in ["wl_compositor", "wl_shm", "wl_seat", "zwlr_layer_shell_v1"] {
                    note(have.iter().any(|i| i == need), &format!("compositor offers {need}"), &format!("compositor lacks {need}: the overlay cannot work"));
                }
                let capture = have.iter().any(|i| i == "ext_image_copy_capture_manager_v1");
                println!("{}ext_image_copy_capture (needed for magnify): {}", if capture { "ok       " } else { "info     " }, if capture { "offered" } else { "not offered" });
            }
            Err(e) => note(false, "", &format!("Wayland registry failed: {e}")),
        },
        Err(e) => note(false, "", &format!("no Wayland connection: {e} (run this inside the desktop session)")),
    }

    let receiver_problems = problems.get();
    let uinput_ok = std::fs::OpenOptions::new().write(true).open("/dev/uinput").is_ok();
    note(
        uinput_ok,
        "/dev/uinput is writable (taps are swallowed, holds move the cursor)",
        "/dev/uinput is not writable: taps will click and hold actions will not work (see the fix list)",
    );

    let have_config = config_path.exists();
    println!(
        "{}config file {} ({})",
        if have_config { "ok       " } else { "info     " },
        config_path.display(),
        if have_config { "found" } else { "not found, defaults are used" }
    );

    if !have_config {
        println!(
            "\nOptional config file: mkdir -p {} && spotlight-rs example-config > {}",
            config_path.parent().map(|p| p.display().to_string()).unwrap_or_default(),
            config_path.display()
        );
    }
    if problems.get() == 0 {
        println!("\nNothing required to fix. Run: spotlight-rs");
        return;
    }
    println!("\nFix list. These need sudo, so run them yourself:");
    // The commands use a pipe, not a here-document, so they work in bash and in fish.
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "spotlight-rs".into());
    if receiver_problems > 0 {
        println!("\n  {exe} udev-rules | sudo tee /etc/udev/rules.d/55-spotlight.rules >/dev/null");
    }
    if !uinput_ok {
        println!("\n  {exe} udev-rules --uinput | sudo tee /etc/udev/rules.d/55-spotlight-uinput.rules >/dev/null");
    }
    println!("  sudo udevadm control --reload-rules && sudo udevadm trigger");
    if receiver_problems > 0 {
        println!("\nThen unplug and replug the receiver.");
    }
    println!("Then run setup again to check. If /dev/uinput is still not writable, log out and in again.");
}

fn run(config_path: PathBuf, demo: Option<Demo>, timer_now: bool) {
    install_signal_handlers();
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("no Wayland connection: {e}");
            std::process::exit(1);
        }
    };
    let (globals, mut event_queue) = registry_queue_init(&conn).expect("registry init");
    let qh = event_queue.handle();
    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor missing");
    let layer_shell = LayerShell::bind(&globals, &qh).expect("zwlr_layer_shell_v1 missing: run `spotlight-rs setup`");
    let shm = Shm::bind(&globals, &qh).expect("wl_shm missing");

    let mut watcher = ConfigWatcher::new(&config_path);
    let cfg = match watcher.poll() {
        Ok(Some(c)) => {
            eprintln!("config: {}", config_path.display());
            c
        }
        Ok(None) => Config::default(),
        Err(e) => {
            eprintln!("config error in {}: {e}\nusing defaults", config_path.display());
            Config::default()
        }
    };
    if !config_path.exists() {
        eprintln!("no config file at {} (defaults in use; `spotlight-rs example-config` prints one)", config_path.display());
    }

    let empty_region = Region::new(&compositor).expect("create region");
    let pool = SlotPool::new(256 * 256 * 4, &shm).expect("shm pool");

    let is_demo = demo.is_some();
    let mut app = App {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        shm,
        pool,
        layer: None,
        compositor,
        layer_shell,
        qh: qh.clone(),
        empty_region,
        pointer: None,
        width: 0,
        height: 0,
        configured: false,
        click_through: is_demo,
        pointer_override: demo.as_ref().and_then(|d| d.at),
        mode_override: demo.as_ref().and_then(|d| d.mode),
        state: State::new(cfg),
        pending: Vec::new(),
        fractional: match (globals.bind(&qh, 1..=1, ()), globals.bind(&qh, 1..=1, ())) {
            (Ok(m), Ok(v)) => Some((m, v)),
            _ => {
                eprintln!("fractional scaling: not offered by the compositor; drawing is stretched at fractional scales");
                None
            }
        },
        surface_scale: None,
        scale: 1.0,
        capturer: match Capturer::start() {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("magnify: window capture is not available ({e}); Magnify will show only its ring");
                None
            }
        },
    };

    let start = *START.get_or_init(Instant::now);
    let now_ms = || start.elapsed().as_millis() as u64;
    let mut remote = Remote::new(app.state.config());
    let mut last_config_poll = Instant::now();
    let mut last_tap_ms: Option<u64> = None;
    let mut demo_phase = 0;

    if is_demo {
        eprintln!("demo: click-through, no remote needed");
        app.show();
    } else {
        eprintln!("running; Ctrl+C stops it");
        if timer_now {
            let fx = app.state.next_press(now_ms());
            if fx.is_empty() {
                eprintln!("--timer-now: the timer is not set to countdown in the config, nothing started");
            }
            app.apply(fx);
        }
    }

    while !STOP.load(Ordering::SeqCst) {
        if let Some(d) = &demo {
            if demo_step(&mut app, d, start, &mut demo_phase) {
                break;
            }
        }
        if !is_demo {
            remote.reconnect_devices(app.state.config());
            remote.connect_hold_if_due();
        }

        // Hot reload. Only settings read each time they are used change; see `Remote::new` for the ones that do not.
        if last_config_poll.elapsed() >= Duration::from_millis(500) {
            last_config_poll = Instant::now();
            match watcher.poll() {
                Ok(Some(c)) => {
                    eprintln!("config reloaded");
                    let fx = app.state.set_config(c);
                    app.apply(fx);
                    app.draw(); // no-op unless the overlay is showing
                }
                Ok(None) => {}
                Err(e) => eprintln!("config error: {e} (keeping the previous settings)"),
            }
        }

        // Wait for Wayland, the remote's nodes, the hold reports and new magnify frames.
        dispatch(&mut app, &mut event_queue);
        let _ = event_queue.flush();
        let guard = event_queue.prepare_read();
        let [mouse_fd, keyboard_fd, hold_fd] = remote.fds();
        let mut fds = [
            libc::pollfd { fd: guard.as_ref().map(|g| g.connection_fd().as_raw_fd()).unwrap_or(-1), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: mouse_fd, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: keyboard_fd, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: hold_fd, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: app.capturer.as_ref().map(Capturer::wake_fd).unwrap_or(-1), events: libc::POLLIN, revents: 0 },
        ];
        let timeout = if guard.is_none() { 0 } else { 100 };
        unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
        if let Some(g) = guard {
            if fds[0].revents & libc::POLLIN != 0 {
                match g.read() {
                    Ok(_) => {}
                    Err(wayland_client::backend::WaylandError::Io(e)) if e.kind() == ErrorKind::WouldBlock => {}
                    Err(e) => {
                        eprintln!("wayland connection lost: {e}");
                        break;
                    }
                }
            }
        }
        dispatch(&mut app, &mut event_queue);

        // A new window frame for Magnify: redraw with it.
        if fds[4].revents & libc::POLLIN != 0 {
            if let Some(c) = &app.capturer {
                c.drain_wake();
            }
            if app.current_mode() == Mode::Magnify {
                app.draw();
            }
        }

        let ready = libc::POLLIN | libc::POLLERR | libc::POLLHUP;
        let mut events = Vec::new();
        if fds[1].revents & ready != 0 {
            remote.read_mouse(app.state.config().input.swallow_clicks, &mut events);
        }
        if fds[2].revents & ready != 0 {
            remote.read_keyboard(&mut events);
        }
        if fds[3].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            let fx = remote.hold_lost(&mut app.state);
            app.apply(fx);
        } else if fds[3].revents & libc::POLLIN != 0 {
            let fx = remote.read_hold(&mut app.state, now_ms());
            app.apply(fx);
        }
        for ev in events {
            let now = now_ms();
            let fx = match ev {
                RemoteEvent::Motion => app.state.remote_motion(now),
                RemoteEvent::TopPress => {
                    match last_tap_ms.replace(now) {
                        Some(before) => trace!("tap (+{} ms since the previous tap)", now - before),
                        None => trace!("tap"),
                    }
                    app.state.top_press(now)
                }
                RemoteEvent::NextPress => app.state.next_press(now),
            };
            app.apply(fx);
        }
        if !is_demo {
            let fx = app.state.tick(now_ms());
            app.apply(fx);
        }
    }

    // Leaving: undo the hold diversion first (a setting on the remote), then release the remote's
    // mouse (the kernel does this on exit too), then unmap the overlay.
    remote.release();
    app.hide();
    let _ = event_queue.flush();
    match STOP_SIGNAL.load(Ordering::SeqCst) {
        0 => say!("stopped"),
        libc::SIGINT => say!("stopped (Ctrl+C)"),
        libc::SIGTERM => say!("stopped (terminated by SIGTERM, for example a timeout)"),
        libc::SIGHUP => say!("stopped (terminal closed, SIGHUP)"),
        n => say!("stopped (signal {n})"),
    }
}

/// Handle the Wayland events that have arrived, then the effects they queued.
fn dispatch(app: &mut App, queue: &mut wayland_client::EventQueue<App>) {
    queue.dispatch_pending(app).expect("dispatch pending");
    let queued = std::mem::take(&mut app.pending);
    app.apply(queued);
}

/// Demo script: optionally unmap and re-map once. Returns true when the demo time is up.
fn demo_step(app: &mut App, d: &Demo, start: Instant, phase: &mut u8) -> bool {
    let t = start.elapsed().as_secs();
    if d.cycle {
        if *phase == 0 && t >= 3 {
            eprintln!("demo: hide");
            app.hide();
            *phase = 1;
        } else if *phase == 1 && t >= 5 {
            eprintln!("demo: show again");
            app.show();
            *phase = 2;
        }
    }
    t >= d.seconds
}
