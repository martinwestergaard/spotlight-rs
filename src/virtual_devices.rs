//! Virtual devices through /dev/uinput: a mouse, used to re-emit the remote's mouse events while
//! the real device is grabbed (so the program can swallow the top button's taps), and a keyboard
//! for the hold actions.
//!
//! The ioctl numbers below match the kernel headers (linux/uinput.h and linux/input.h):
//! UI_DEV_SETUP 0x405c5503 (92-byte struct), UI_SET_EVBIT 0x40045564,
//! UI_SET_KEYBIT 0x40045565, UI_SET_RELBIT 0x40045566, UI_DEV_CREATE 0x5501, UI_DEV_DESTROY 0x5502,
//! EVIOCGRAB 0x40044590, BUS_VIRTUAL 6.

use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    os::fd::AsRawFd,
};

const UI_SET_EVBIT: libc::c_ulong = 0x4004_5564;
const UI_SET_KEYBIT: libc::c_ulong = 0x4004_5565;
const UI_SET_RELBIT: libc::c_ulong = 0x4004_5566;
const UI_DEV_SETUP: libc::c_ulong = 0x405c_5503;
const UI_DEV_CREATE: libc::c_ulong = 0x5501;
const UI_DEV_DESTROY: libc::c_ulong = 0x5502;
pub const EVIOCGRAB: libc::c_ulong = 0x4004_4590;

const BUS_VIRTUAL: u16 = 6;
const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_REL: u16 = 2;
// REL_X, REL_Y, REL_HWHEEL, REL_WHEEL, REL_WHEEL_HI_RES, REL_HWHEEL_HI_RES: what the remote's mouse node advertises
const REL_BITS: [u16; 6] = [0, 1, 6, 8, 11, 12];
// BTN_LEFT, BTN_RIGHT, BTN_MIDDLE
const KEY_BITS: [u16; 3] = [272, 273, 274];

#[repr(C)]
struct UinputSetup {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
    name: [u8; 80],
    ff_effects_max: u32,
}

pub struct VirtualMouse {
    file: File,
}

fn ioctl_int(fd: i32, req: libc::c_ulong, value: libc::c_int) -> io::Result<()> {
    let r = unsafe { libc::ioctl(fd, req, value) };
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

/// Open /dev/uinput and create a virtual device with the given event types, keys and relative axes.
fn create_device(name: &str, product: u16, events: &[u16], keys: &[u16], rels: &[u16]) -> io::Result<File> {
    let file = OpenOptions::new().write(true).open("/dev/uinput")?;
    let fd = file.as_raw_fd();
    for ev in events {
        ioctl_int(fd, UI_SET_EVBIT, (*ev).into())?;
    }
    for k in keys {
        ioctl_int(fd, UI_SET_KEYBIT, (*k).into())?;
    }
    for r in rels {
        ioctl_int(fd, UI_SET_RELBIT, (*r).into())?;
    }
    let mut setup = UinputSetup { bustype: BUS_VIRTUAL, vendor: 0x5350, product, version: 1, name: [0; 80], ff_effects_max: 0 };
    let bytes = name.as_bytes();
    let n = bytes.len().min(79);
    setup.name[..n].copy_from_slice(&bytes[..n]);
    if unsafe { libc::ioctl(fd, UI_DEV_SETUP, &setup as *const UinputSetup) } < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::ioctl(fd, UI_DEV_CREATE) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}

impl VirtualMouse {
    pub fn create(name: &str) -> io::Result<Self> {
        Ok(Self { file: create_device(name, 1, &[EV_SYN, EV_KEY, EV_REL], &KEY_BITS, &REL_BITS)? })
    }

    /// Write one input_event (timestamp left at zero, the kernel fills it in).
    pub fn emit(&mut self, ty: u16, code: u16, value: i32) -> io::Result<()> {
        self.file.write_all(&encode_event(ty, code, value))
    }
}

pub fn encode_event(ty: u16, code: u16, value: i32) -> [u8; 24] {
    let mut b = [0u8; 24];
    b[16..18].copy_from_slice(&ty.to_ne_bytes());
    b[18..20].copy_from_slice(&code.to_ne_bytes());
    b[20..24].copy_from_slice(&value.to_ne_bytes());
    b
}

impl Drop for VirtualMouse {
    fn drop(&mut self) {
        unsafe { libc::ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY) };
    }
}

/// Grab or release a device so that only this process receives its events.
/// The kernel also releases the grab when the file is closed, including when the process dies.
pub fn grab(fd: i32, on: bool) -> io::Result<()> {
    ioctl_int(fd, EVIOCGRAB, on as libc::c_int)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_struct_is_92_bytes() {
        assert_eq!(std::mem::size_of::<UinputSetup>(), 92);
    }

    #[test]
    fn key_names_resolve() {
        assert_eq!(key_code("b"), Some(48));
        assert_eq!(key_code("B"), Some(48));
        assert_eq!(key_code("period"), Some(52));
        assert_eq!(key_code("nonsense"), None);
    }

    #[test]
    fn keystrokes_parse_with_modifiers() {
        assert_eq!(parse_keystroke("b"), Some(vec![48]));
        assert_eq!(parse_keystroke("ctrl+shift+b"), Some(vec![29, 42, 48]));
        assert_eq!(parse_keystroke(" ctrl + b "), Some(vec![29, 48]));
        assert_eq!(parse_keystroke(""), None);
        assert_eq!(parse_keystroke("ctrl+"), None);
        assert_eq!(parse_keystroke("ctrl+nonsense"), None);
    }

    #[test]
    fn key_table_has_no_duplicate_names() {
        let mut names: Vec<&str> = KEY_TABLE.iter().map(|(n, _)| *n).collect();
        names.sort();
        let len = names.len();
        names.dedup();
        assert_eq!(names.len(), len);
    }

    #[test]
    fn event_layout() {
        let b = encode_event(2, 1, -3);
        assert_eq!(&b[..16], &[0u8; 16]);
        assert_eq!(u16::from_ne_bytes([b[16], b[17]]), 2);
        assert_eq!(u16::from_ne_bytes([b[18], b[19]]), 1);
        assert_eq!(i32::from_ne_bytes([b[20], b[21], b[22], b[23]]), -3);
    }
}

// ---------------------------------------------------------------------------------------------
// Virtual keyboard, for hold actions (Blank Screen, custom keystroke).

/// Key names to Linux keycodes. Generated from linux/input-event-codes.h by a C program, not typed by hand.
pub const KEY_TABLE: &[(&str, u16)] = &[
    ("a", 30),
    ("b", 48),
    ("c", 46),
    ("d", 32),
    ("e", 18),
    ("f", 33),
    ("g", 34),
    ("h", 35),
    ("i", 23),
    ("j", 36),
    ("k", 37),
    ("l", 38),
    ("m", 50),
    ("n", 49),
    ("o", 24),
    ("p", 25),
    ("q", 16),
    ("r", 19),
    ("s", 31),
    ("t", 20),
    ("u", 22),
    ("v", 47),
    ("w", 17),
    ("x", 45),
    ("y", 21),
    ("z", 44),
    ("0", 11),
    ("1", 2),
    ("2", 3),
    ("3", 4),
    ("4", 5),
    ("5", 6),
    ("6", 7),
    ("7", 8),
    ("8", 9),
    ("9", 10),
    ("comma", 51),
    ("period", 52),
    ("space", 57),
    ("enter", 28),
    ("escape", 1),
    ("tab", 15),
    ("backspace", 14),
    ("left", 105),
    ("right", 106),
    ("up", 103),
    ("down", 108),
    ("pageup", 104),
    ("pagedown", 109),
    ("home", 102),
    ("end", 107),
    ("volumeup", 115),
    ("volumedown", 114),
    ("mute", 113),
    ("ctrl", 29),
    ("shift", 42),
    ("alt", 56),
    ("super", 125),
    ("minus", 12),
    ("equal", 13),
    ("slash", 53),
    ("f1", 59),
    ("f2", 60),
    ("f3", 61),
    ("f4", 62),
    ("f5", 63),
    ("f6", 64),
    ("f7", 65),
    ("f8", 66),
    ("f9", 67),
    ("f10", 68),
    ("f11", 87),
    ("f12", 88),
];

pub fn key_code(name: &str) -> Option<u16> {
    let n = name.to_ascii_lowercase();
    KEY_TABLE.iter().find(|(k, _)| *k == n).map(|(_, c)| *c)
}

/// "ctrl+shift+b" -> keycodes [ctrl, shift, b]. Modifiers come first in the order written; the last
/// part is the key. None if any part is unknown or the text is empty.
pub fn parse_keystroke(text: &str) -> Option<Vec<u16>> {
    let parts: Vec<&str> = text.split('+').map(str::trim).collect();
    if parts.iter().any(|p| p.is_empty()) {
        return None;
    }
    parts.into_iter().map(key_code).collect()
}

pub struct VirtualKeyboard {
    file: File,
}

impl VirtualKeyboard {
    pub fn create(name: &str) -> io::Result<Self> {
        let keys: Vec<u16> = KEY_TABLE.iter().map(|(_, code)| *code).collect();
        Ok(Self { file: create_device(name, 2, &[EV_SYN, EV_KEY], &keys, &[])? })
    }

    /// Press the keys in order, then release them in reverse order.
    pub fn press_combo(&mut self, keys: &[u16]) -> io::Result<()> {
        for k in keys {
            self.file.write_all(&encode_event(EV_KEY, *k, 1))?;
        }
        self.file.write_all(&encode_event(EV_SYN, 0, 0))?;
        for k in keys.iter().rev() {
            self.file.write_all(&encode_event(EV_KEY, *k, 0))?;
        }
        self.file.write_all(&encode_event(EV_SYN, 0, 0))
    }
}

impl Drop for VirtualKeyboard {
    fn drop(&mut self) {
        unsafe { libc::ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY) };
    }
}
