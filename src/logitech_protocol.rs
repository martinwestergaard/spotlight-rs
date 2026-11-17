//! Talking to the Spotlight over HID++ 2.0 through the receiver's hidraw node: message framing,
//! feature lookup, vibration, and the hold reports (Next, Back, top button). Message layout:
//!
//! ```text
//! short = report 0x10, 7 bytes; long = report 0x11, 20 bytes
//! [0]=report id, [1]=device index, [2]=feature index, [3]=(function<<4)|software id, [4..]=params
//! error: short with [2]==0x8f, long with [2]==0xff; [3]=feature index, [4]=function byte, [5]=code
//! ```
//!
//! Reads: ping, feature lookup, battery. Writes, each only on request: `vibrate` and `set_divert`.
//! It never writes receiver registers and never sends Reset.

use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// The Spotlight's USB receiver.
pub const RECEIVER_VENDOR: u16 = 0x046d;
pub const RECEIVER_PRODUCT: u16 = 0xc53e;
/// How long to wait for the remote's answer to one request.
pub const REQUEST_TIMEOUT: Duration = Duration::from_millis(1500);

/// Open the receiver's HID++ node and drop anything already waiting on it.
/// The error text says what to do about it.
pub fn open_receiver() -> Result<Device, String> {
    let node = find_hidpp_nodes(RECEIVER_VENDOR, RECEIVER_PRODUCT)
        .into_iter()
        .next()
        .ok_or("no HID++ node found for the receiver")?;
    let mut dev = Device::open(&node).map_err(|e| format!("cannot open {}: {e} (run `spotlight-rs setup`)", node.display()))?;
    dev.drain();
    Ok(dev)
}

pub const SHORT: u8 = 0x10;
pub const LONG: u8 = 0x11;
pub const SHORT_LEN: usize = 7;
pub const LONG_LEN: usize = 20;
pub const SW_ID: u8 = 7;

pub mod feature {
    pub const ROOT: u16 = 0x0000;
    pub const FEATURE_SET: u16 = 0x0001;
    pub const FIRMWARE: u16 = 0x0003;
    pub const RESET: u16 = 0x0020;
    pub const BATTERY_STATUS: u16 = 0x1000;
    pub const UNIFIED_BATTERY: u16 = 0x1004;
    pub const PRESENTER_CONTROL: u16 = 0x1a00;
    pub const HAPTIC: u16 = 0x19b0;
    pub const REPROG_CONTROLS_V4: u16 = 0x1b04;
    pub const POINTER_SPEED: u16 = 0x2205;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub long: bool,
    pub device_index: u8,
    pub feature_index: u8,
    pub function: u8,
    pub params: Vec<u8>,
}

impl Request {
    pub fn short(device_index: u8, feature_index: u8, function: u8, params: &[u8]) -> Self {
        Self { long: false, device_index, feature_index, function, params: params.to_vec() }
    }
    pub fn long(device_index: u8, feature_index: u8, function: u8, params: &[u8]) -> Self {
        Self { long: true, device_index, feature_index, function, params: params.to_vec() }
    }

    pub fn encode(&self) -> Vec<u8> {
        let len = if self.long { LONG_LEN } else { SHORT_LEN };
        let mut b = vec![0u8; len];
        b[0] = if self.long { LONG } else { SHORT };
        b[1] = self.device_index;
        b[2] = self.feature_index;
        b[3] = (self.function << 4) | SW_ID;
        for (i, p) in self.params.iter().take(len - 4).enumerate() {
            b[4 + i] = *p;
        }
        b
    }

    /// Byte 2 and 3 of the answer to this request.
    fn answer_key(&self) -> (u8, u8) {
        (self.feature_index, (self.function << 4) | SW_ID)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// Answer to the request: the full report.
    Ok(Vec<u8>),
    /// Error report for the request.
    Error { code: u8, feature_index: u8, function_byte: u8 },
}

pub fn error_name(code: u8) -> &'static str {
    match code {
        0 => "NoError",
        1 => "Unknown",
        2 => "InvalidArgument",
        3 => "OutOfRange",
        4 => "HardwareError",
        5 => "LogitechInternal",
        6 => "InvalidFeatureIndex",
        7 => "InvalidFunction",
        8 => "Busy",
        9 => "Unsupported",
        _ => "?",
    }
}

/// Classify a report read from the device against the request that was sent.
/// Returns None for anything that is not the answer (other reports, notifications).
pub fn match_reply(req: &Request, report: &[u8]) -> Option<Reply> {
    if report.len() < SHORT_LEN || (report[0] != SHORT && report[0] != LONG) {
        return None;
    }
    if report[1] != req.device_index {
        return None;
    }
    let is_error = (report[0] == SHORT && report[2] == 0x8f) || (report[0] == LONG && report[2] == 0xff);
    let (fi, fb) = req.answer_key();
    if is_error {
        if report.len() >= 6 && report[3] == fi && report[4] == fb {
            return Some(Reply::Error { code: report[5], feature_index: report[3], function_byte: report[4] });
        }
        return None;
    }
    if report[2] == fi && report[3] == fb {
        return Some(Reply::Ok(report.to_vec()));
    }
    None
}

pub struct Device {
    file: File,
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Timeout,
    Device { code: u8 },
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io error: {e}"),
            Error::Timeout => write!(f, "no answer before the timeout"),
            Error::Device { code } => write!(f, "device error {code} ({})", error_name(*code)),
        }
    }
}

impl Device {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).custom_flags(libc::O_NONBLOCK).open(path)?;
        Ok(Self { file })
    }

    /// Throw away anything waiting to be read.
    pub fn drain(&mut self) {
        let mut buf = [0u8; 64];
        while let Ok(n) = self.file.read(&mut buf) {
            if n == 0 {
                break;
            }
        }
    }

    fn wait_readable(&self, timeout: Duration) -> bool {
        let mut p = libc::pollfd { fd: self.file.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        let r = unsafe { libc::poll(&mut p, 1, ms) };
        r > 0 && p.revents & libc::POLLIN != 0
    }

    pub fn fd(&self) -> i32 {
        self.file.as_raw_fd()
    }

    /// Wait up to `timeout` for one report (a notification, say). None when nothing arrives.
    pub fn read_report(&mut self, timeout: Duration) -> Option<Vec<u8>> {
        if !self.wait_readable(timeout) {
            return None;
        }
        let mut buf = [0u8; 64];
        match self.file.read(&mut buf) {
            Ok(n) if n > 0 => Some(buf[..n].to_vec()),
            _ => None,
        }
    }

    pub fn request(&mut self, req: &Request, timeout: Duration) -> Result<Vec<u8>, Error> {
        self.file.write_all(&req.encode())?;
        let deadline = Instant::now() + timeout;
        let mut buf = [0u8; 64];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::Timeout);
            }
            if !self.wait_readable(left) {
                return Err(Error::Timeout);
            }
            match self.file.read(&mut buf) {
                Ok(n) => match match_reply(req, &buf[..n]) {
                    Some(Reply::Ok(r)) => return Ok(r),
                    Some(Reply::Error { code, .. }) => return Err(Error::Device { code }),
                    None => continue, // another report, e.g. a notification
                },
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                Err(e) => return Err(e.into()),
            }
        }
    }
}

/// IRoot.Ping: returns (major, minor) of the HID++ protocol.
pub fn ping(dev: &mut Device, device_index: u8, timeout: Duration) -> Result<(u8, u8), Error> {
    let req = Request::short(device_index, 0, 1, &[0, 0, 0xa5]);
    let r = dev.request(&req, timeout)?;
    Ok((r[4], r[5]))
}

/// IRoot.GetFeature: returns Some((feature index, type flags)) or None if the feature is absent.
pub fn get_feature(dev: &mut Device, device_index: u8, id: u16, timeout: Duration) -> Result<Option<(u8, u8)>, Error> {
    let req = Request::long(device_index, 0, 0, &[(id >> 8) as u8, id as u8]);
    let r = dev.request(&req, timeout)?;
    Ok(if r[4] == 0 && id != feature::ROOT { None } else { Some((r[4], r[5])) })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Battery {
    pub level: u8,
    pub next_level: u8,
    pub status: u8,
}

/// Read-only battery query. `unified` selects feature 0x1004 (function 1) over 0x1000 (function 0).
pub fn battery(dev: &mut Device, device_index: u8, feature_index: u8, unified: bool, timeout: Duration) -> Result<Battery, Error> {
    let function = if unified { 1 } else { 0 };
    let r = dev.request(&Request::short(device_index, feature_index, function, &[]), timeout)?;
    Ok(parse_battery(&r, unified))
}

pub fn parse_battery(report: &[u8], unified: bool) -> Battery {
    Battery {
        level: report[4],
        next_level: if unified { report[4] } else { report[5] },
        status: report[6],
    }
}

/// Vibrate the remote (PresenterControl, function 1). This WRITES to the device, unlike everything
/// else in this module (except `set_divert`). Long request, params {length (max 10), 0xe8, intensity}.
/// The meaning of the 0xe8 byte is not documented; the remote accepts it and buzzes.
pub fn vibrate(dev: &mut Device, device_index: u8, feature_index: u8, length: u8, intensity: u8, timeout: Duration) -> Result<(), Error> {
    let req = Request::long(device_index, feature_index, 1, &[length.min(10), 0xe8, intensity]);
    dev.request(&req, timeout).map(|_| ())
}

pub const CID_NEXT: u8 = 0xda;
pub const CID_BACK: u8 = 0xdc;
/// The top (pointer) button's hold. Diverted, it reports press and release and the remote's
/// movement over HID++; a tap still arrives as BTN_LEFT on the mouse node.
pub const CID_TOP: u8 = 0xd8;

/// ReprogControlsV4 function 3 (set control reporting) for the hold control `cid_lo` (0xda Next,
/// 0xdc Back, 0xd8 top button). WRITES to the device. Flags: on = 0x33 (the remote reports the
/// hold over HID++ instead of acting on it), off = 0x22 (back to normal).
pub fn set_divert(dev: &mut Device, device_index: u8, feature_index: u8, cid_lo: u8, on: bool, timeout: Duration) -> Result<(), Error> {
    let flags = if on { 0x33 } else { 0x22 };
    let req = Request::long(device_index, feature_index, 3, &[0x00, cid_lo, flags]);
    dev.request(&req, timeout).map(|_| ())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HoldState {
    pub next: bool,
    pub back: bool,
    pub top: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notification {
    /// Which hold controls are currently pressed (function 0).
    Hold(HoldState),
    /// Movement while a diverted control is held (function 1): signed 16-bit dx, dy.
    Movement { dx: i16, dy: i16 },
    Other,
}

/// Parse a report from the remote as a notification of the ReprogControlsV4 feature:
/// `11 01 07 00 00 da ...` a control is pressed, `11 01 07 00 00 00 ...` all released,
/// `11 01 07 10 ff ff ff fd ...` movement while a diverted control is held. None when the report
/// belongs to another feature.
pub fn parse_notification(report: &[u8], feature_index: u8) -> Option<Notification> {
    if report.len() < 8 || report[0] != LONG || report[2] != feature_index {
        return None;
    }
    Some(match report[3] >> 4 {
        0 => {
            // The pressed controls are listed as 16-bit ids in the rest of the report, two bytes each.
            let cids: Vec<u16> = report[4..].chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
            Notification::Hold(HoldState {
                next: cids.contains(&u16::from(CID_NEXT)),
                back: cids.contains(&u16::from(CID_BACK)),
                top: cids.contains(&u16::from(CID_TOP)),
            })
        }
        1 => Notification::Movement {
            dx: i16::from_be_bytes([report[4], report[5]]),
            dy: i16::from_be_bytes([report[6], report[7]]),
        },
        _ => Notification::Other,
    })
}

/// The hidraw node of the receiver's HID++ interface: the Logitech hidraw whose parent HID
/// device has no input children (the other interfaces carry the keyboard, mouse and consumer
/// controls). Returns every candidate, so the caller can see what was considered.
pub fn find_hidpp_nodes(vendor: u16, product: u16) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(dir) = std::fs::read_dir("/sys/class/hidraw") else { return found };
    for entry in dir.flatten() {
        let dev = entry.path().join("device");
        let Ok(uevent) = std::fs::read_to_string(dev.join("uevent")) else { continue };
        let want = format!("HID_ID=0003:{vendor:08X}:{product:08X}");
        if !uevent.lines().any(|l| l.eq_ignore_ascii_case(&want)) {
            continue;
        }
        let has_input = std::fs::read_dir(dev.join("input")).map(|mut d| d.next().is_some()).unwrap_or(false);
        if !has_input {
            found.push(Path::new("/dev").join(entry.file_name()));
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_request_layout() {
        let b = Request::short(1, 0, 1, &[0, 0, 0xa5]).encode();
        assert_eq!(b, vec![0x10, 1, 0, 0x17, 0, 0, 0xa5]);
    }

    #[test]
    fn long_request_layout_is_zero_padded() {
        let b = Request::long(1, 0, 0, &[0x1b, 0x04]).encode();
        assert_eq!(b.len(), 20);
        assert_eq!(&b[..6], &[0x11, 1, 0, 0x07, 0x1b, 0x04]);
        assert!(b[6..].iter().all(|v| *v == 0));
    }

    #[test]
    fn function_goes_in_the_high_nibble() {
        let b = Request::short(1, 5, 3, &[]).encode();
        assert_eq!(b[3], 0x37);
    }

    #[test]
    fn extra_params_are_truncated_not_overflowed() {
        let b = Request::short(1, 0, 0, &[1, 2, 3, 4, 5, 6]).encode();
        assert_eq!(b.len(), 7);
        assert_eq!(&b[4..], &[1, 2, 3]);
    }

    #[test]
    fn matches_answer_by_device_feature_and_function() {
        let req = Request::short(1, 0, 1, &[0, 0, 0xa5]);
        let mut rep = vec![0x10, 1, 0, 0x17, 4, 2, 0xa5];
        assert!(matches!(match_reply(&req, &rep), Some(Reply::Ok(_))));
        rep[1] = 2; // wrong device index
        assert_eq!(match_reply(&req, &rep), None);
        let other = vec![0x10, 1, 3, 0x17, 4, 2, 0xa5]; // other feature: a notification
        assert_eq!(match_reply(&req, &other), None);
        assert_eq!(match_reply(&req, &[0x02, 0, 0, 0, 0, 0, 0, 0]), None); // plain mouse report
    }

    #[test]
    fn decodes_short_and_long_errors() {
        let req = Request::short(1, 0, 1, &[]);
        let short = vec![0x10, 1, 0x8f, 0, 0x17, 9, 0];
        assert_eq!(match_reply(&req, &short), Some(Reply::Error { code: 9, feature_index: 0, function_byte: 0x17 }));
        let mut long = vec![0u8; 20];
        long[..6].copy_from_slice(&[0x11, 1, 0xff, 0, 0x17, 2]);
        assert_eq!(match_reply(&req, &long), Some(Reply::Error { code: 2, feature_index: 0, function_byte: 0x17 }));
        // an error for some other request is not ours
        let foreign = vec![0x10, 1, 0x8f, 4, 0x27, 9, 0];
        assert_eq!(match_reply(&req, &foreign), None);
    }

    #[test]
    fn battery_layouts() {
        let r = [0x10, 1, 6, 0x07, 80, 50, 3];
        assert_eq!(parse_battery(&r, false), Battery { level: 80, next_level: 50, status: 3 });
        assert_eq!(parse_battery(&r, true), Battery { level: 80, next_level: 80, status: 3 });
    }

    fn long_report(bytes: &[u8]) -> Vec<u8> {
        let mut r = vec![0u8; 20];
        r[..bytes.len()].copy_from_slice(bytes);
        r
    }

    #[test]
    fn parses_the_hold_reports() {
        // reports as sent by a real Spotlight
        let pressed = long_report(&[0x11, 0x01, 0x07, 0x00, 0x00, 0xda]);
        let released = long_report(&[0x11, 0x01, 0x07, 0x00, 0x00, 0x00]);
        assert_eq!(parse_notification(&pressed, 7), Some(Notification::Hold(HoldState { next: true, back: false, top: false })));
        assert_eq!(parse_notification(&released, 7), Some(Notification::Hold(HoldState::default())));
    }

    #[test]
    fn parses_back_and_both() {
        let back = long_report(&[0x11, 0x01, 0x07, 0x00, 0x00, 0xdc]);
        assert_eq!(parse_notification(&back, 7), Some(Notification::Hold(HoldState { next: false, back: true, top: false })));
        let both = long_report(&[0x11, 0x01, 0x07, 0x00, 0x00, 0xda, 0x00, 0xdc]);
        assert_eq!(parse_notification(&both, 7), Some(Notification::Hold(HoldState { next: true, back: true, top: false })));
    }

    #[test]
    fn parses_the_top_button_hold() {
        let pressed = long_report(&[0x11, 0x01, 0x07, 0x00, 0x00, 0xd8]);
        assert_eq!(parse_notification(&pressed, 7), Some(Notification::Hold(HoldState { next: false, back: false, top: true })));
    }

    #[test]
    fn parses_three_held_controls() {
        let all = long_report(&[0x11, 0x01, 0x07, 0x00, 0x00, 0xd8, 0x00, 0xda, 0x00, 0xdc]);
        assert_eq!(parse_notification(&all, 7), Some(Notification::Hold(HoldState { next: true, back: true, top: true })));
    }

    #[test]
    fn parses_the_observed_movement_reports() {
        let m = long_report(&[0x11, 0x01, 0x07, 0x10, 0xff, 0xff, 0xff, 0xfd]);
        assert_eq!(parse_notification(&m, 7), Some(Notification::Movement { dx: -1, dy: -3 }));
        let m = long_report(&[0x11, 0x01, 0x07, 0x10, 0x00, 0x06, 0xff, 0xf3]);
        assert_eq!(parse_notification(&m, 7), Some(Notification::Movement { dx: 6, dy: -13 }));
    }

    #[test]
    fn ignores_other_features_and_short_reports() {
        let other = long_report(&[0x11, 0x01, 0x06, 0x00, 0x00, 0xda]);
        assert_eq!(parse_notification(&other, 7), None);
        assert_eq!(parse_notification(&[0x10, 1, 7, 0, 0, 0xda, 0], 7), None);
        assert_eq!(parse_notification(&[0x02, 0, 0, 0], 7), None);
    }

    #[test]
    fn error_names() {
        assert_eq!(error_name(9), "Unsupported");
        assert_eq!(error_name(200), "?");
    }
}
