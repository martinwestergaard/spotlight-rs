//! Reading the remote's evdev nodes (the receiver shows up as a mouse and a keyboard):
//! - the mouse node gives REL_X and REL_Y while the top button is held and not diverted, and BTN_LEFT for a tap
//! - the keyboard node gives KEY_RIGHT for Next and KEY_LEFT for Back (taps only)
//! Holds never show up here; they come over HID++ (see logitech_protocol.rs and remote.rs).

use std::{
    fs::{File, OpenOptions},
    io::{self, Read},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
};

pub const EV_KEY: u16 = 1;
pub const EV_REL: u16 = 2;
pub const REL_X: u16 = 0;
pub const REL_Y: u16 = 1;
pub const KEY_LEFT: u16 = 105;
pub const KEY_RIGHT: u16 = 106;
pub const BTN_LEFT: u16 = 272;
const EVENT_SIZE: usize = 24; // struct input_event on 64-bit Linux

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteEvent {
    Motion,
    TopPress,
    NextPress,
}

/// Map one raw event (type, code, value) to something the state machine cares about.
pub fn map_event(ty: u16, code: u16, value: i32) -> Option<RemoteEvent> {
    match (ty, code, value) {
        (EV_REL, REL_X | REL_Y, v) if v != 0 => Some(RemoteEvent::Motion),
        (EV_KEY, BTN_LEFT, 1) => Some(RemoteEvent::TopPress),
        (EV_KEY, KEY_RIGHT, 1) => Some(RemoteEvent::NextPress),
        _ => None,
    }
}

/// Split a read into raw events and map them. A trailing partial event is ignored.
pub fn parse_chunk(chunk: &[u8]) -> Vec<RemoteEvent> {
    parse_raw(chunk).into_iter().filter_map(|e| map_event(e.ty, e.code, e.value)).collect()
}

/// Find "/dev/input/eventN" for the device whose name is exactly `name`, given the text of
/// /proc/bus/input/devices.
pub fn find_event_node(proc_devices: &str, name: &str) -> Option<String> {
    let want = format!("N: Name=\"{name}\"");
    for block in proc_devices.split("\n\n") {
        if block.lines().any(|l| l == want) {
            for line in block.lines() {
                if let Some(rest) = line.strip_prefix("H: Handlers=") {
                    if let Some(ev) = rest.split_whitespace().find(|w| w.starts_with("event")) {
                        return Some(format!("/dev/input/{ev}"));
                    }
                }
            }
        }
    }
    None
}

pub fn find_event_node_now(name: &str) -> Option<String> {
    let text = std::fs::read_to_string("/proc/bus/input/devices").ok()?;
    find_event_node(&text, name)
}

/// An event as the kernel sent it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawEvent {
    pub ty: u16,
    pub code: u16,
    pub value: i32,
}

pub fn parse_raw(chunk: &[u8]) -> Vec<RawEvent> {
    chunk
        .chunks_exact(EVENT_SIZE)
        .map(|c| RawEvent {
            ty: u16::from_ne_bytes([c[16], c[17]]),
            code: u16::from_ne_bytes([c[18], c[19]]),
            value: i32::from_ne_bytes([c[20], c[21], c[22], c[23]]),
        })
        .collect()
}

pub struct Reader {
    file: File,
    pub path: String,
}

impl Reader {
    pub fn open(path: &str) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(path)?;
        Ok(Self { file, path: path.to_string() })
    }

    pub fn fd(&self) -> i32 {
        self.file.as_raw_fd()
    }

    pub fn grab(&self, on: bool) -> io::Result<()> {
        crate::virtual_devices::grab(self.fd(), on)
    }

    /// Everything currently readable, unmapped. Err means the device is gone.
    pub fn read_raw(&mut self) -> io::Result<Vec<RawEvent>> {
        let mut out = Vec::new();
        let mut buf = [0u8; EVENT_SIZE * 64];
        loop {
            match self.file.read(&mut buf) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "device closed")),
                Ok(n) => out.extend(parse_raw(&buf[..n])),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(out),
                Err(e) => return Err(e),
            }
        }
    }

    /// Everything currently readable, mapped. Err means the device is gone and the reader should be dropped.
    pub fn read_events(&mut self) -> io::Result<Vec<RemoteEvent>> {
        Ok(self.read_raw()?.into_iter().filter_map(|e| map_event(e.ty, e.code, e.value)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(ty: u16, code: u16, value: i32) -> Vec<u8> {
        let mut b = vec![0u8; 16];
        b.extend(ty.to_ne_bytes());
        b.extend(code.to_ne_bytes());
        b.extend(value.to_ne_bytes());
        b
    }

    #[test]
    fn raw_parsing_keeps_every_event() {
        let mut chunk = raw(EV_REL, REL_X, 4);
        chunk.extend(raw(0, 0, 0));
        chunk.extend(raw(4, 4, 90001)); // scan code, not mapped but still raw
        let r = parse_raw(&chunk);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0], RawEvent { ty: EV_REL, code: REL_X, value: 4 });
        assert_eq!(r[2], RawEvent { ty: 4, code: 4, value: 90001 });
    }

    #[test]
    fn maps_the_observed_events() {
        assert_eq!(map_event(EV_REL, REL_X, 5), Some(RemoteEvent::Motion));
        assert_eq!(map_event(EV_REL, REL_Y, -3), Some(RemoteEvent::Motion));
        assert_eq!(map_event(EV_KEY, BTN_LEFT, 1), Some(RemoteEvent::TopPress));
        assert_eq!(map_event(EV_KEY, KEY_RIGHT, 1), Some(RemoteEvent::NextPress));
        assert_eq!(map_event(EV_KEY, KEY_LEFT, 1), None, "Back is not used as an event");
    }

    #[test]
    fn ignores_releases_zero_motion_and_other_events() {
        assert_eq!(map_event(EV_KEY, BTN_LEFT, 0), None);
        assert_eq!(map_event(EV_KEY, KEY_RIGHT, 0), None);
        assert_eq!(map_event(EV_KEY, KEY_RIGHT, 2), None, "autorepeat is not a new press");
        assert_eq!(map_event(EV_REL, REL_X, 0), None);
        assert_eq!(map_event(EV_REL, 8, 1), None, "wheel");
        assert_eq!(map_event(0, 0, 0), None, "sync");
    }

    #[test]
    fn parses_a_chunk_with_several_events_and_a_partial_tail() {
        let mut chunk = raw(EV_REL, REL_X, 4);
        chunk.extend(raw(0, 0, 0));
        chunk.extend(raw(EV_KEY, BTN_LEFT, 1));
        chunk.extend(&raw(EV_KEY, BTN_LEFT, 0)[..10]); // cut off
        assert_eq!(parse_chunk(&chunk), vec![RemoteEvent::Motion, RemoteEvent::TopPress]);
    }

    const PROC: &str = "I: Bus=0003 Vendor=046d Product=c53e Version=0111\n\
N: Name=\"Logitech USB Receiver\"\n\
P: Phys=usb-0000:0c:00.0-2/input0\n\
H: Handlers=sysrq kbd event3 leds \n\
B: PROP=0\n\
\n\
I: Bus=0003 Vendor=046d Product=c53e Version=0111\n\
N: Name=\"Logitech USB Receiver Mouse\"\n\
H: Handlers=mouse0 event4 \n\
\n\
I: Bus=0003 Vendor=046d Product=c53e Version=0111\n\
N: Name=\"Logitech USB Receiver Consumer Control\"\n\
H: Handlers=kbd event5 \n";

    #[test]
    fn finds_nodes_by_exact_name() {
        assert_eq!(find_event_node(PROC, "Logitech USB Receiver").as_deref(), Some("/dev/input/event3"));
        assert_eq!(find_event_node(PROC, "Logitech USB Receiver Mouse").as_deref(), Some("/dev/input/event4"));
        assert_eq!(find_event_node(PROC, "Logitech USB Receiver Consumer Control").as_deref(), Some("/dev/input/event5"));
        assert_eq!(find_event_node(PROC, "Logitech USB"), None, "no prefix matching");
        assert_eq!(find_event_node("", "x"), None);
    }
}
