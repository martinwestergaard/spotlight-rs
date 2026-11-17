//! Experiment: divert the Next (CID 0x00da) and Back (CID 0x00dc) hold controls, or the controls
//! named with --cid, so the remote reports them over HID++, and print what arrives. THIS WRITES
//! TO THE REMOTE.
//!
//! Usage: divert_test [seconds] [--no-divert] [--cid HEX ...] (for example --cid d8 diverts only 0x00d8). Diverts, prints every HID++ report and every key
//! event of the keyboard node for the given time (default 12 s), then undoes the diversion.
//! With --no-divert it only listens (control run).
//! The undo also runs on Ctrl+C or SIGTERM. Flags: 0x33 = temporary divert + raw XY divert with
//! their "valid" bits; 0x22 = the same valid bits with the values cleared, which switches the
//! hold reports off again.

use spotlight_rs::{
    logitech_protocol::{self, feature, Device, Request},
    remote_input::{self, Reader},
};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

const TIMEOUT: Duration = Duration::from_millis(1500);

fn set(dev: &mut Device, feat: u8, cid_lo: u8, flags: u8) {
    let req = Request::long(1, feat, 3, &[0x00, cid_lo, flags]);
    match dev.request(&req, TIMEOUT) {
        Ok(r) => println!("  CID 0x00{cid_lo:02x} flags 0x{flags:02x}: acknowledged, reply {}", hex(&r[..8])),
        Err(e) => println!("  CID 0x00{cid_lo:02x} flags 0x{flags:02x}: {e}"),
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|v| format!("{v:02x}")).collect::<Vec<_>>().join(" ")
}

fn listen(dev: &mut Device, kbd: &mut Option<Reader>, mouse: &mut Option<Reader>, secs: u64) {
    let mut rel_events = 0u32;
    let mut other_mouse = 0u32;
    let start = Instant::now();
    let mut moves = 0u32;
    while start.elapsed().as_secs() < secs && !STOP.load(Ordering::SeqCst) {
        if let Some(r) = dev.read_report(Duration::from_millis(50)) {
            let function = r.get(3).map(|b| b >> 4);
            if function == Some(1) {
                moves += 1; // movement reports are only counted, they come at about 100 per second
            } else {
                if moves > 0 {
                    println!("         ({moves} movement reports)");
                    moves = 0;
                }
                println!("  +{:5.2}s logitech_protocol {}", start.elapsed().as_secs_f64(), hex(&r[..8.min(r.len())]));
            }
        }
        if let Some(m) = mouse.as_mut() {
            if let Ok(raws) = m.read_raw() {
                for e in raws {
                    if e.ty == remote_input::EV_REL {
                        rel_events += 1;
                    } else if e.ty == remote_input::EV_KEY {
                        other_mouse += 1;
                        println!("  +{:5.2}s mouse key code {} value {}", start.elapsed().as_secs_f64(), e.code, e.value);
                    }
                }
            }
        }
        if let Some(k) = kbd.as_mut() {
            if let Ok(raws) = k.read_raw() {
                for e in raws {
                    if e.ty == remote_input::EV_KEY {
                        println!("  +{:5.2}s key   code {} value {}", start.elapsed().as_secs_f64(), e.code, e.value);
                    }
                }
            }
        }
    }
    if moves > 0 {
        println!("         ({moves} movement reports)");
    }
    println!("  normal mouse node: {rel_events} motion events, {other_mouse} button events");
}

fn main() {
    // Usage: divert_test [seconds] [--no-divert]
    let mut secs = 12u64;
    let mut divert = true;
    let mut cids: Vec<u8> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--no-divert" {
            divert = false;
        } else if a == "--cid" {
            let v = args.next().expect("--cid needs a hex value");
            cids.push(u8::from_str_radix(v.trim_start_matches("0x"), 16).expect("--cid: hex byte, for example d8"));
        } else {
            secs = a.parse().expect("seconds");
        }
    }
    if cids.is_empty() {
        cids = vec![logitech_protocol::CID_NEXT, logitech_protocol::CID_BACK];
    }
    unsafe {
        libc::signal(libc::SIGINT, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
    }
    let node = logitech_protocol::find_hidpp_nodes(0x046d, 0xc53e).into_iter().next().expect("no HID++ node");
    let mut dev = Device::open(&node).expect("open HID++ node");
    dev.drain();
    logitech_protocol::ping(&mut dev, 1, TIMEOUT).expect("ping");
    let (feat, _) = logitech_protocol::get_feature(&mut dev, 1, feature::REPROG_CONTROLS_V4, TIMEOUT).expect("lookup").expect("no ReprogControlsV4");
    let mut kbd = remote_input::find_event_node_now("Logitech USB Receiver").and_then(|p| Reader::open(&p).ok());
    let mut mouse = remote_input::find_event_node_now("Logitech USB Receiver Mouse").and_then(|p| Reader::open(&p).ok());
    println!("keyboard node readable: {}, mouse node readable: {}", kbd.is_some(), mouse.is_some());

    if divert {
        println!("diverting {}", cids.iter().map(|c| format!("0x00{c:02x}")).collect::<Vec<_>>().join(", "));
        for c in &cids {
            set(&mut dev, feat, *c, 0x33);
        }
    } else {
        println!("no diversion (control run)");
    }
    println!("LISTENING {secs} s: do your one action now");
    listen(&mut dev, &mut kbd, &mut mouse, secs);
    if divert {
        println!("undoing the diversion");
        for c in &cids {
            set(&mut dev, feat, *c, 0x22);
        }
    }
    println!("done");
}
