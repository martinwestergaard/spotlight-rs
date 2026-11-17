//! Everything tied to the physical remote: its input nodes, the virtual mouse that re-emits its
//! motion, the HID++ link for holds (Next, Back, top button), hold actions, and timer vibration.

use spotlight_rs::{
    config::{Config, HoldAction},
    logitech_protocol, remote_input,
    remote_input::{map_event, Reader, RemoteEvent, BTN_LEFT, EV_KEY, EV_REL},
    state_machine::{Effect, State},
    virtual_devices::{parse_keystroke, VirtualKeyboard, VirtualMouse},
};
use std::time::{Duration, Instant};

/// Vibrate the remote on its own short thread, so a sleeping remote (the request waits up to
/// 1.5 s per step) never freezes the overlay. Opens the HID++ node each time; it is rare.
pub(crate) fn vibrate_async(what: &'static str, length: u8, intensity: u8) {
    std::thread::spawn(move || {
        let mut dev = match logitech_protocol::open_receiver() {
            Ok(d) => d,
            Err(e) => {
                eprintln!("{what}: {e}, no vibration");
                return;
            }
        };
        let timeout = logitech_protocol::REQUEST_TIMEOUT;
        if let Err(e) = logitech_protocol::ping(&mut dev, 1, timeout) {
            eprintln!("{what}: vibration failed: {e}");
            return;
        }
        let index = match logitech_protocol::get_feature(&mut dev, 1, logitech_protocol::feature::PRESENTER_CONTROL, timeout) {
            Ok(Some((index, _))) => index,
            Ok(None) => {
                eprintln!("{what}: the remote has no PresenterControl feature, no vibration");
                return;
            }
            Err(e) => {
                eprintln!("{what}: vibration failed: {e}");
                return;
            }
        };
        match logitech_protocol::vibrate(&mut dev, 1, index, length, intensity, timeout) {
            Ok(()) => eprintln!("{what}: vibrated (length {length}, intensity {intensity})"),
            Err(e) => eprintln!("{what}: vibration failed: {e}"),
        }
    });
}

/// The HID++ link used for the hold actions. While it exists, the remote reports holds of Next and
/// Back to this program. Dropping it undoes the divert, so every way out of `run`, a panic included,
/// gives the remote its normal behavior back.
struct HoldLink {
    dev: logitech_protocol::Device,
    feat: u8,
    state: logitech_protocol::HoldState,
    /// Whether the top button was diverted too (the undo covers the same controls).
    top: bool,
    /// The receiver went away; there is nothing left to undo and talking to it would only time out.
    lost: bool,
}

fn diverted_cids(top: bool) -> Vec<u8> {
    let mut cids = vec![logitech_protocol::CID_NEXT, logitech_protocol::CID_BACK];
    if top {
        cids.push(logitech_protocol::CID_TOP);
    }
    cids
}

fn connect_hold(divert_top: bool) -> Option<HoldLink> {
    let mut dev = match logitech_protocol::open_receiver() {
        Ok(d) => d,
        Err(e) => {
            // No node at all is normal while the receiver is unplugged; report only real failures.
            if !e.starts_with("no HID++ node") {
                eprintln!("hold actions: {e}");
            }
            return None;
        }
    };
    // Short timeout: a sleeping remote does not answer, and this runs on the main loop.
    if logitech_protocol::ping(&mut dev, 1, Duration::from_millis(400)).is_err() {
        return None; // asleep or out of range; the caller retries
    }
    let slow = logitech_protocol::REQUEST_TIMEOUT;
    let feat = match logitech_protocol::get_feature(&mut dev, 1, logitech_protocol::feature::REPROG_CONTROLS_V4, slow) {
        Ok(Some((index, _))) => index,
        other => {
            eprintln!("hold actions: the remote has no ReprogControlsV4 feature ({other:?})");
            return None;
        }
    };
    let cids = diverted_cids(divert_top);
    for cid in &cids {
        if let Err(e) = logitech_protocol::set_divert(&mut dev, 1, feat, *cid, true, slow) {
            eprintln!("hold actions: divert of control 0x00{cid:02x} failed: {e}");
            for undo in &cids {
                let _ = logitech_protocol::set_divert(&mut dev, 1, feat, *undo, false, slow);
            }
            return None;
        }
    }
    eprintln!("hold actions on: holds of {} are reported to this program (undone when it exits)", if divert_top { "Next, Back and the top button" } else { "Next and Back" });
    Some(HoldLink { dev, feat, state: logitech_protocol::HoldState::default(), top: divert_top, lost: false })
}

impl Drop for HoldLink {
    fn drop(&mut self) {
        if self.lost {
            return;
        }
        // `say!` ignores write errors: after the terminal is closed (SIGHUP) eprintln! would panic
        // and skip the remaining undos.
        for cid in diverted_cids(self.top) {
            if let Err(e) = logitech_protocol::set_divert(&mut self.dev, 1, self.feat, cid, false, logitech_protocol::REQUEST_TIMEOUT) {
                say!("hold actions: could not undo the divert of 0x00{cid:02x}: {e}");
            }
        }
        say!("hold actions off: divert undone");
    }
}

fn send_keystroke(vkbd: &mut Option<VirtualKeyboard>, text: &str) {
    let Some(keys) = parse_keystroke(text) else {
        eprintln!("keystroke {text:?} is not valid");
        return;
    };
    if vkbd.is_none() {
        match VirtualKeyboard::create("spotlight-rs virtual keyboard") {
            Ok(v) => {
                *vkbd = Some(v);
                // A new device needs a moment before the compositor listens to it.
                std::thread::sleep(Duration::from_millis(300));
            }
            Err(e) => {
                eprintln!("cannot create the virtual keyboard: {e} (is /dev/uinput writable?)");
                return;
            }
        }
    }
    if let Some(v) = vkbd.as_mut() {
        match v.press_combo(&keys) {
            Ok(()) => eprintln!("hold action: sent {text:?}"),
            Err(e) => eprintln!("hold action: could not send {text:?}: {e}"),
        }
    }
}

fn run_hold_action(action: HoldAction, cfg: &Config, vkbd: &mut Option<VirtualKeyboard>) {
    match action {
        HoldAction::None | HoldAction::Laser => {} // the laser is handled where the hold reports arrive
        HoldAction::BlankScreen => send_keystroke(vkbd, &cfg.hold.blank_key),
        HoldAction::CustomKeystroke => {
            if cfg.hold.custom_keystroke.is_empty() {
                eprintln!("hold action custom_keystroke is set but hold.custom_keystroke is empty");
            } else {
                send_keystroke(vkbd, &cfg.hold.custom_keystroke);
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Button {
    Next,
    Back,
}

/// Everything tied to the physical remote: its input nodes, the virtual mouse that re-emits its
/// motion, the HID++ link for holds, the virtual keyboard for hold actions, and the laser state.
pub(crate) struct Remote {
    // The node names are read once at start; changing them in the config needs a restart.
    mouse_name: String,
    kbd_name: String,
    mouse: Option<Reader>,
    keyboard: Option<Reader>,
    vmouse: Option<VirtualMouse>,
    grabbed: bool,
    hold: Option<HoldLink>,
    vkbd: Option<VirtualKeyboard>,
    /// The button whose hold is currently the laser.
    laser_button: Option<Button>,
    /// Fractions of cursor motion not yet sent.
    laser_acc: (f64, f64),
    /// When the top button's current hold started (for the log).
    top_hold_since: u64,
    last_hold_try: Instant,
    last_device_try: Instant,
}

impl Remote {
    pub(crate) fn new(cfg: &Config) -> Self {
        let long_ago = Instant::now() - Duration::from_secs(10);
        Self {
            mouse_name: cfg.input.device_name.clone(),
            kbd_name: cfg.input.keyboard_name.clone(),
            mouse: None,
            keyboard: None,
            vmouse: None,
            grabbed: false,
            hold: None,
            vkbd: None,
            laser_button: None,
            laser_acc: (0.0, 0.0),
            top_hold_since: 0,
            last_hold_try: long_ago,
            last_device_try: long_ago,
        }
    }

    /// The receiver may be plugged in later or unplugged: look for the nodes every 2 s.
    /// `input.swallow_clicks` is read here for the grab and per event for the swallowing, so a
    /// change in the config applies fully after the next reconnect.
    pub(crate) fn reconnect_devices(&mut self, cfg: &Config) {
        if (self.mouse.is_some() && self.keyboard.is_some()) || self.last_device_try.elapsed() < Duration::from_secs(2) {
            return;
        }
        self.last_device_try = Instant::now();
        if self.mouse.is_none() {
            if let Some(path) = remote_input::find_event_node_now(&self.mouse_name) {
                match Reader::open(&path) {
                    Ok(r) => {
                        eprintln!("remote mouse: {path}");
                        if cfg.input.swallow_clicks {
                            self.take_over(&r);
                        }
                        self.mouse = Some(r);
                    }
                    Err(e) => eprintln!("cannot open {path}: {e} (run `spotlight-rs setup`)"),
                }
            }
        }
        if self.keyboard.is_none() {
            if let Some(path) = remote_input::find_event_node_now(&self.kbd_name) {
                if let Ok(r) = Reader::open(&path) {
                    eprintln!("remote keyboard: {path}");
                    self.keyboard = Some(r);
                }
            }
        }
    }

    /// Grab the remote's mouse and re-emit its events through a virtual mouse, minus the taps.
    pub(crate) fn take_over(&mut self, reader: &Reader) {
        if self.vmouse.is_none() {
            match VirtualMouse::create("spotlight-rs virtual mouse") {
                Ok(v) => self.vmouse = Some(v),
                Err(e) => eprintln!("cannot create the virtual mouse: {e}; taps will still click (is /dev/uinput writable?)"),
            }
        }
        if self.vmouse.is_some() {
            match reader.grab(true) {
                Ok(()) => {
                    self.grabbed = true;
                    eprintln!("remote mouse taken over: motion is passed on, taps are swallowed");
                }
                Err(e) => eprintln!("cannot take over the remote mouse: {e}; taps will still click"),
            }
        }
    }

    /// Divert the holds of Next, Back and (with a virtual mouse) the top button, so the remote reports them over HID++.
    pub(crate) fn connect_hold_if_due(&mut self) {
        // Wait for the mouse node: the virtual mouse is created when it is found, and the top button
        // is diverted only if that mouse exists.
        if self.hold.is_none() && self.mouse.is_some() && self.last_hold_try.elapsed() >= Duration::from_secs(5) {
            self.last_hold_try = Instant::now();
            // The top button's hold moves the cursor through the virtual mouse, so it is diverted only
            // when that exists; without it the plain mouse node keeps working as before.
            self.hold = connect_hold(self.vmouse.is_some());
        }
    }

    /// File descriptors to poll: mouse, keyboard, hold link (-1 when absent).
    pub(crate) fn fds(&self) -> [i32; 3] {
        [
            self.mouse.as_ref().map(Reader::fd).unwrap_or(-1),
            self.keyboard.as_ref().map(Reader::fd).unwrap_or(-1),
            self.hold.as_ref().map(|h| h.dev.fd()).unwrap_or(-1),
        ]
    }

    /// Read the mouse. When taken over, pass everything on to the virtual mouse except the swallowed taps.
    pub(crate) fn read_mouse(&mut self, swallow: bool, events: &mut Vec<RemoteEvent>) {
        let Some(r) = self.mouse.as_mut() else { return };
        match r.read_raw() {
            Ok(raws) => {
                for raw in raws {
                    if self.grabbed {
                        let is_tap = raw.ty == EV_KEY && raw.code == BTN_LEFT;
                        let forward = matches!(raw.ty, 0 | EV_REL | EV_KEY) && !(is_tap && swallow);
                        if forward {
                            if let Some(v) = self.vmouse.as_mut() {
                                if let Err(e) = v.emit(raw.ty, raw.code, raw.value) {
                                    eprintln!("virtual mouse write failed: {e}");
                                }
                            }
                        }
                    }
                    if let Some(ev) = map_event(raw.ty, raw.code, raw.value) {
                        events.push(ev);
                    }
                }
            }
            Err(e) => {
                eprintln!("lost {}: {e}", r.path);
                self.mouse = None;
                self.grabbed = false;
            }
        }
    }

    pub(crate) fn read_keyboard(&mut self, events: &mut Vec<RemoteEvent>) {
        let Some(r) = self.keyboard.as_mut() else { return };
        match r.read_events() {
            Ok(ev) => events.extend(ev),
            Err(e) => {
                eprintln!("lost {}: {e}", r.path);
                self.keyboard = None;
            }
        }
    }

    /// Hold notifications: run the action once when a hold starts. A laser hold also turns the
    /// remote's movement reports into cursor motion, because while a control is diverted the
    /// gyro reaches the computer only as HID++ reports (the mouse node stays silent).
    pub(crate) fn read_hold(&mut self, state: &mut State, now: u64) -> Vec<Effect> {
        let cfg = state.config().clone();
        let mut actions = Vec::new();
        let mut effects = Vec::new();
        while let Some(report) = self.hold.as_mut().and_then(|h| h.dev.read_report(Duration::ZERO)) {
            let feat = self.hold.as_ref().map(|h| h.feat).unwrap_or(0);
            match logitech_protocol::parse_notification(&report, feat) {
                Some(logitech_protocol::Notification::Hold(new)) => {
                    let old = self.hold.as_ref().map(|h| h.state).unwrap_or_default();
                    if new.top && !old.top {
                        eprintln!("top hold on");
                        self.top_hold_since = now;
                        effects.extend(state.top_hold_press(now));
                    }
                    if !new.top && old.top {
                        eprintln!("top hold off (held {} ms)", now.saturating_sub(self.top_hold_since));
                        effects.extend(state.top_hold_release());
                    }
                    for (button, was, pressed, action) in [
                        (Button::Next, old.next, new.next, cfg.hold.next),
                        (Button::Back, old.back, new.back, cfg.hold.back),
                    ] {
                        if pressed && !was {
                            if action == HoldAction::Laser {
                                if self.vmouse.is_none() {
                                    eprintln!("laser: needs the virtual mouse (input.swallow_clicks = true and a writable /dev/uinput)");
                                } else {
                                    self.laser_button = Some(button);
                                    self.laser_acc = (0.0, 0.0);
                                    effects.extend(state.laser_press(now));
                                    eprintln!("laser on");
                                }
                            } else {
                                actions.push(action);
                            }
                        }
                        if !pressed && was && self.laser_button == Some(button) {
                            self.laser_button = None;
                            effects.extend(state.laser_release());
                            eprintln!("laser off");
                        }
                    }
                    if let Some(h) = self.hold.as_mut() {
                        h.state = new;
                    }
                }
                Some(logitech_protocol::Notification::Movement { dx, dy }) => {
                    // The link knows whether the top button is physically held. If the failsafe ended
                    // the hold while it is still down, movement starts it again.
                    let top_down = self.hold.as_ref().is_some_and(|h| h.state.top);
                    if top_down && !state.top_held() {
                        effects.extend(state.top_hold_press(now));
                    }
                    if !state.laser_held() && !state.top_held() {
                        continue;
                    }
                    let speed = if state.laser_held() { cfg.input.laser_speed } else { cfg.input.pointer_speed };
                    if state.laser_held() {
                        state.laser_report(now);
                    }
                    if state.top_held() {
                        state.top_hold_report(now);
                    }
                    self.move_cursor(f64::from(dx) * speed, f64::from(dy) * speed);
                }
                _ => {}
            }
        }
        for action in actions {
            run_hold_action(action, &cfg, &mut self.vkbd);
        }
        effects
    }

    /// The HID++ node went away (receiver unplugged). Drop the link without undoing anything, end any
    /// hold that was in progress, and let `connect_hold_if_due` divert again when the receiver is back.
    pub(crate) fn hold_lost(&mut self, state: &mut State) -> Vec<Effect> {
        if let Some(h) = self.hold.as_mut() {
            h.lost = true;
        }
        self.hold = None;
        self.laser_button = None;
        self.last_hold_try = Instant::now() - Duration::from_secs(5);
        eprintln!("hold actions: lost the receiver's HID++ node; will divert again when it is back");
        let mut effects = state.top_hold_release();
        effects.extend(state.laser_release());
        effects
    }

    /// Move the real cursor through the virtual mouse by a fractional amount; the remainder is kept.
    pub(crate) fn move_cursor(&mut self, dx: f64, dy: f64) {
        self.laser_acc.0 += dx;
        self.laser_acc.1 += dy;
        let (ix, iy) = (self.laser_acc.0.trunc() as i32, self.laser_acc.1.trunc() as i32);
        self.laser_acc.0 -= f64::from(ix);
        self.laser_acc.1 -= f64::from(iy);
        if let Some(v) = self.vmouse.as_mut() {
            if ix != 0 {
                let _ = v.emit(EV_REL, 0, ix);
            }
            if iy != 0 {
                let _ = v.emit(EV_REL, 1, iy);
            }
            if ix != 0 || iy != 0 {
                let _ = v.emit(0, 0, 0);
            }
        }
    }

    /// Undo the hold diversion (a setting on the remote) and release the remote's mouse.
    pub(crate) fn release(&mut self) {
        drop(self.hold.take());
        if let Some(r) = &self.mouse {
            let _ = r.grab(false);
        }
    }
}
