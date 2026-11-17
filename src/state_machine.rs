//! Pure logic: spotlight on/off, pointer mode cycling, timer. No I/O and no clock of its
//! own; callers pass the time in milliseconds, so tests can drive it with fake events.
//!
//! Behavior:
//! - The overlay turns on at the first remote motion and off after `idle_ms` without
//!   motion, so clicks reach other windows while it is idle.
//! - A double press of the top button is two presses within `double_press_ms`.
//! - While the top button or a laser hold is down, the overlay stays up however still the remote is
//! - The countdown starts at the first Next press

use crate::config::{Config, TimerKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Highlight,
    Magnify,
    Circle,
    /// The digital laser dot. Only shown while a laser hold is active; never part of the double-press cycle.
    Laser,
}

const ORDER: [Mode; 3] = [Mode::Highlight, Mode::Magnify, Mode::Circle];

/// A laser or top-button hold with no report at all for this long is treated as released.
const LASER_FAILSAFE_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    ShowOverlay,
    HideOverlay,
    Redraw,
    ModeChanged(Mode),
    TimerStarted,
    VibrateWarning,
    VibrateEnd,
}

pub struct State {
    cfg: Config,
    mode: Mode,
    active: bool,
    last_motion_ms: u64,
    last_press_ms: Option<u64>,
    pointer: Option<(f64, f64)>,
    timer_started_ms: Option<u64>,
    warned: bool,
    ended: bool,
    laser_held: bool,
    laser_last_report_ms: u64,
    top_held: bool,
    top_last_report_ms: u64,
}

impl State {
    pub fn new(cfg: Config) -> Self {
        let mode = first_enabled(&cfg).unwrap_or(Mode::Circle);
        Self {
            cfg,
            mode,
            active: false,
            last_motion_ms: 0,
            last_press_ms: None,
            pointer: None,
            timer_started_ms: None,
            warned: false,
            ended: false,
            laser_held: false,
            laser_last_report_ms: 0,
            top_held: false,
            top_last_report_ms: 0,
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }
    #[cfg(test)]
    pub fn active(&self) -> bool {
        self.active
    }
    pub fn pointer(&self) -> Option<(f64, f64)> {
        self.pointer
    }
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// Hot reload: keep the running state, move to a valid mode if the current one was disabled.
    pub fn set_config(&mut self, cfg: Config) -> Vec<Effect> {
        let mut fx = Vec::new();
        self.cfg = cfg;
        if !self.mode_enabled(self.mode) {
            if let Some(m) = first_enabled(&self.cfg) {
                self.mode = m;
                fx.push(Effect::ModeChanged(m));
            }
        }
        if self.active {
            fx.push(Effect::Redraw);
        }
        fx
    }

    fn mode_enabled(&self, m: Mode) -> bool {
        match m {
            Mode::Highlight => self.cfg.modes.highlight.enabled,
            Mode::Magnify => self.cfg.modes.magnify.enabled,
            Mode::Circle => self.cfg.modes.circle.enabled,
            Mode::Laser => false, // not part of the cycle
        }
    }

    /// The mode that is being drawn right now: the laser during a laser hold, else the chosen mode.
    pub fn effective_mode(&self) -> Mode {
        if self.laser_held { Mode::Laser } else { self.mode }
    }

    /// Whether the arrow cursor should be hidden for the mode being drawn (look.hide_cursor).
    pub fn hides_cursor(&self) -> bool {
        let name = match self.effective_mode() {
            Mode::Highlight => "highlight",
            Mode::Magnify => "magnify",
            Mode::Circle => "circle",
            Mode::Laser => "laser",
        };
        self.cfg.look.hide_cursor.iter().any(|n| n == name)
    }

    pub fn laser_held(&self) -> bool {
        self.laser_held
    }

    /// The laser hold started: show the overlay and keep it up until the hold ends.
    pub fn laser_press(&mut self, now_ms: u64) -> Vec<Effect> {
        self.laser_held = true;
        self.laser_last_report_ms = now_ms;
        if self.active {
            vec![Effect::Redraw]
        } else {
            self.active = true;
            self.last_motion_ms = now_ms;
            vec![Effect::ShowOverlay]
        }
    }

    pub fn top_held(&self) -> bool {
        self.top_held
    }

    /// The top button's hold started (seen over HID++): show the overlay and keep it up until the
    /// hold ends, however still the remote is.
    pub fn top_hold_press(&mut self, now_ms: u64) -> Vec<Effect> {
        self.top_held = true;
        self.top_last_report_ms = now_ms;
        self.last_motion_ms = now_ms;
        if self.active {
            vec![Effect::Redraw]
        } else {
            self.active = true;
            vec![Effect::ShowOverlay]
        }
    }

    /// A movement report arrived during the top hold (keeps the failsafe from firing).
    pub fn top_hold_report(&mut self, now_ms: u64) {
        self.top_last_report_ms = now_ms;
        self.last_motion_ms = now_ms;
    }

    /// The top button was released. The overlay goes at once unless a laser hold needs it.
    pub fn top_hold_release(&mut self) -> Vec<Effect> {
        if !self.top_held {
            return Vec::new();
        }
        self.top_held = false;
        if self.active && !self.laser_held {
            self.active = false;
            self.pointer = None;
            vec![Effect::HideOverlay]
        } else {
            Vec::new()
        }
    }

    /// A movement report arrived during the laser hold (keeps the failsafe from firing).
    pub fn laser_report(&mut self, now_ms: u64) {
        self.laser_last_report_ms = now_ms;
        self.last_motion_ms = now_ms;
    }

    pub fn laser_release(&mut self) -> Vec<Effect> {
        if !self.laser_held {
            return Vec::new();
        }
        self.laser_held = false;
        if self.active && !self.top_held {
            self.active = false;
            self.pointer = None;
            vec![Effect::HideOverlay]
        } else {
            Vec::new()
        }
    }

    /// Motion from the remote (the gyro pointer, only sent while the top button is held).
    pub fn remote_motion(&mut self, now_ms: u64) -> Vec<Effect> {
        self.last_motion_ms = now_ms;
        if self.active {
            Vec::new()
        } else {
            self.active = true;
            vec![Effect::ShowOverlay]
        }
    }

    /// Absolute pointer position reported by the compositor to the overlay.
    pub fn pointer_at(&mut self, x: f64, y: f64) -> Vec<Effect> {
        self.pointer = Some((x, y));
        if self.active { vec![Effect::Redraw] } else { Vec::new() }
    }

    /// A press of the top button. Two within `double_press_ms` switch the mode.
    pub fn top_press(&mut self, now_ms: u64) -> Vec<Effect> {
        let window = self.cfg.input.double_press_ms;
        let double = matches!(self.last_press_ms, Some(t) if now_ms.saturating_sub(t) <= window);
        if double {
            // Reset so that three quick presses are one switch, not two.
            self.last_press_ms = None;
            self.cycle_mode()
        } else {
            self.last_press_ms = Some(now_ms);
            Vec::new()
        }
    }

    fn cycle_mode(&mut self) -> Vec<Effect> {
        let start = ORDER.iter().position(|m| *m == self.mode).unwrap_or(0);
        for step in 1..=ORDER.len() {
            let candidate = ORDER[(start + step) % ORDER.len()];
            if self.mode_enabled(candidate) {
                if candidate == self.mode {
                    return Vec::new();
                }
                self.mode = candidate;
                let mut fx = vec![Effect::ModeChanged(candidate)];
                if self.active {
                    fx.push(Effect::Redraw);
                }
                return fx;
            }
        }
        Vec::new()
    }

    /// A press of Next. Starts the countdown the first time, when a countdown is configured.
    pub fn next_press(&mut self, now_ms: u64) -> Vec<Effect> {
        if self.cfg.timer.kind == TimerKind::Countdown && self.timer_started_ms.is_none() {
            self.timer_started_ms = Some(now_ms);
            self.warned = false;
            self.ended = false;
            return vec![Effect::TimerStarted];
        }
        Vec::new()
    }


    /// Call regularly (a few times a second is enough).
    pub fn tick(&mut self, now_ms: u64) -> Vec<Effect> {
        let mut fx = Vec::new();

        // Failsafe: a lost release report must not leave the dot on screen forever.
        if self.laser_held && now_ms.saturating_sub(self.laser_last_report_ms) >= LASER_FAILSAFE_MS {
            fx.extend(self.laser_release());
        }

        if self.top_held && now_ms.saturating_sub(self.top_last_report_ms) >= LASER_FAILSAFE_MS {
            fx.extend(self.top_hold_release());
        }

        // The overlay stays up for as long as a laser or top-button hold lasts, however still the remote is.
        if !self.laser_held && !self.top_held && self.active && now_ms.saturating_sub(self.last_motion_ms) >= self.cfg.input.idle_ms {
            self.active = false;
            self.pointer = None;
            fx.push(Effect::HideOverlay);
        }

        if let Some(start) = self.timer_started_ms {
            let total = (self.cfg.timer.minutes * 60_000.0) as u64;
            let warn_at = total.saturating_sub((self.cfg.timer.warn_minutes * 60_000.0) as u64);
            let elapsed = now_ms.saturating_sub(start);
            if self.cfg.timer.vibrate {
                // No warning when the warning time is zero or not before the end.
                if !self.warned && self.cfg.timer.warn_minutes > 0.0 && warn_at > 0 && elapsed >= warn_at && elapsed < total {
                    self.warned = true;
                    fx.push(Effect::VibrateWarning);
                }
                if !self.ended && elapsed >= total {
                    self.ended = true;
                    self.warned = true;
                    fx.push(Effect::VibrateEnd);
                }
            }
        }
        fx
    }
}

fn first_enabled(cfg: &Config) -> Option<Mode> {
    [
        (Mode::Highlight, cfg.modes.highlight.enabled),
        (Mode::Magnify, cfg.modes.magnify.enabled),
        (Mode::Circle, cfg.modes.circle.enabled),
    ]
    .into_iter()
    .find(|(_, on)| *on)
    .map(|(m, _)| m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        let mut c = Config::default();
        c.modes.magnify.enabled = true;
        c.modes.circle.enabled = true; // off by default; the cycle tests want all three
        c
    }

    #[test]
    fn shows_on_motion_and_hides_after_idle() {
        let mut s = State::new(cfg());
        assert_eq!(s.remote_motion(1000), vec![Effect::ShowOverlay]);
        assert!(s.active());
        assert_eq!(s.remote_motion(1100), vec![]); // already active
        assert_eq!(s.tick(1400), vec![]); // 300 ms since last motion, idle_ms is 400
        assert_eq!(s.tick(1500), vec![Effect::HideOverlay]); // 400 ms since 1100
        assert!(!s.active());
        assert_eq!(s.remote_motion(2000), vec![Effect::ShowOverlay]); // shows again
    }

    #[test]
    fn laser_shows_on_press_and_hides_on_release() {
        let mut s = State::new(cfg());
        assert_eq!(s.laser_press(1000), vec![Effect::ShowOverlay]);
        assert!(s.laser_held() && s.active());
        // Still for far longer than idle_ms: the dot stays while held
        assert_eq!(s.tick(5000), vec![]);
        assert!(s.active());
        assert_eq!(s.laser_release(), vec![Effect::HideOverlay]);
        assert!(!s.active() && !s.laser_held());
        assert_eq!(s.laser_release(), vec![]); // second release does nothing
    }

    #[test]
    fn top_hold_stays_up_while_still_and_hides_on_release() {
        let mut s = State::new(cfg());
        assert_eq!(s.top_hold_press(1000), vec![Effect::ShowOverlay]);
        assert!(s.top_held() && s.active());
        assert_eq!(s.tick(9000), vec![], "far longer than idle_ms, still shown");
        assert_eq!(s.top_hold_release(), vec![Effect::HideOverlay]);
        assert!(!s.active() && !s.top_held());
        assert_eq!(s.top_hold_release(), vec![]);
    }

    #[test]
    fn top_hold_press_while_already_active_just_redraws() {
        let mut s = State::new(cfg());
        s.remote_motion(0);
        assert_eq!(s.top_hold_press(10), vec![Effect::Redraw]);
    }

    #[test]
    fn top_hold_failsafe_releases_after_silence() {
        let mut s = State::new(cfg());
        s.top_hold_press(0);
        s.top_hold_report(10_000);
        assert_eq!(s.tick(39_000), vec![]);
        assert_eq!(s.tick(40_000), vec![Effect::HideOverlay]);
        assert!(!s.top_held());
    }

    #[test]
    fn laser_release_keeps_the_overlay_while_the_top_button_is_held() {
        let mut s = State::new(cfg());
        s.top_hold_press(0);
        s.laser_press(10);
        assert_eq!(s.laser_release(), vec![]);
        assert!(s.active() && s.top_held());
        assert_eq!(s.top_hold_release(), vec![Effect::HideOverlay]);
    }

    #[test]
    fn top_release_keeps_the_overlay_while_a_laser_hold_needs_it() {
        let mut s = State::new(cfg());
        s.top_hold_press(0);
        s.laser_press(10);
        assert_eq!(s.top_hold_release(), vec![]);
        assert!(s.active() && s.laser_held());
    }

    #[test]
    fn laser_press_while_already_active_just_redraws() {
        let mut s = State::new(cfg());
        s.remote_motion(0);
        assert_eq!(s.laser_press(10), vec![Effect::Redraw]);
    }

    #[test]
    fn laser_failsafe_releases_after_silence() {
        let mut s = State::new(cfg());
        s.laser_press(0);
        s.laser_report(10_000);
        assert_eq!(s.tick(39_000), vec![]); // 29 s since the last report
        assert_eq!(s.tick(40_000), vec![Effect::HideOverlay]);
        assert!(!s.laser_held());
    }

    #[test]
    fn cursor_hiding_follows_the_mode_and_the_setting() {
        let mut s = State::new(cfg());
        assert!(s.hides_cursor(), "highlight hides by default");
        s.top_press(0);
        s.top_press(100); // magnify (enabled in the test config)
        assert!(s.hides_cursor());
        s.top_press(1000);
        s.top_press(1100); // circle
        assert_eq!(s.mode(), Mode::Circle);
        assert!(!s.hides_cursor(), "circle keeps the cursor by default");
        s.laser_press(2000);
        assert!(s.hides_cursor(), "laser hides by default");
        s.laser_release();
        let mut c = cfg();
        c.look.hide_cursor.clear();
        s.set_config(c);
        s.laser_press(3000);
        assert!(!s.hides_cursor(), "an empty list hides nothing");
    }

    #[test]
    fn laser_is_not_in_the_double_press_cycle() {
        let mut s = State::new(cfg());
        for t in 0..8 {
            s.top_press(t * 1000);
            s.top_press(t * 1000 + 100);
            assert_ne!(s.mode(), Mode::Laser);
        }
    }

    #[test]
    fn motion_keeps_it_alive() {
        let mut s = State::new(cfg());
        s.remote_motion(0);
        for t in (100..3000).step_by(100) {
            s.remote_motion(t);
            assert_eq!(s.tick(t), vec![]);
        }
        assert!(s.active());
    }

    #[test]
    fn pointer_position_is_cleared_on_hide() {
        let mut s = State::new(cfg());
        s.remote_motion(0);
        assert_eq!(s.pointer_at(10.0, 20.0), vec![Effect::Redraw]);
        assert_eq!(s.pointer(), Some((10.0, 20.0)));
        s.tick(500);
        assert_eq!(s.pointer(), None);
    }

    #[test]
    fn pointer_position_while_hidden_does_not_redraw() {
        let mut s = State::new(cfg());
        assert_eq!(s.pointer_at(1.0, 2.0), vec![]);
    }

    #[test]
    fn double_press_cycles_modes() {
        let mut s = State::new(cfg());
        assert_eq!(s.mode(), Mode::Highlight);
        assert_eq!(s.top_press(0), vec![]);
        assert_eq!(s.top_press(200), vec![Effect::ModeChanged(Mode::Magnify)]);
        assert_eq!(s.top_press(1000), vec![]);
        assert_eq!(s.top_press(1200), vec![Effect::ModeChanged(Mode::Circle)]);
        assert_eq!(s.top_press(2000), vec![]);
        assert_eq!(s.top_press(2200), vec![Effect::ModeChanged(Mode::Highlight)]);
    }

    #[test]
    fn slow_presses_do_not_switch() {
        let mut s = State::new(cfg());
        s.top_press(0);
        assert_eq!(s.top_press(401), vec![]);
        assert_eq!(s.mode(), Mode::Highlight);
    }

    #[test]
    fn three_quick_presses_are_one_switch() {
        let mut s = State::new(cfg());
        s.top_press(0);
        s.top_press(100);
        assert_eq!(s.top_press(200), vec![]);
        assert_eq!(s.mode(), Mode::Magnify);
    }

    #[test]
    fn disabled_modes_are_skipped() {
        let mut c = Config::default();
        c.modes.circle.enabled = true;
        c.modes.magnify.enabled = false;
        let mut s = State::new(c);
        s.top_press(0);
        assert_eq!(s.top_press(100), vec![Effect::ModeChanged(Mode::Circle)]);
    }

    #[test]
    fn single_enabled_mode_never_switches() {
        let mut c = Config::default();
        c.modes.circle.enabled = false;
        c.modes.magnify.enabled = false;
        let mut s = State::new(c);
        s.top_press(0);
        assert_eq!(s.top_press(100), vec![]);
        assert_eq!(s.mode(), Mode::Highlight);
    }

    #[test]
    fn mode_change_while_active_redraws() {
        let mut s = State::new(cfg());
        s.remote_motion(0);
        s.top_press(10);
        assert_eq!(s.top_press(20), vec![Effect::ModeChanged(Mode::Magnify), Effect::Redraw]);
    }

    #[test]
    fn hot_reload_moves_off_a_disabled_mode() {
        let mut s = State::new(cfg());
        let mut c = cfg();
        c.modes.highlight.enabled = false;
        assert_eq!(s.set_config(c), vec![Effect::ModeChanged(Mode::Magnify)]);
        assert_eq!(s.mode(), Mode::Magnify);
    }

    #[test]
    fn hot_reload_while_active_redraws() {
        let mut s = State::new(cfg());
        s.remote_motion(0);
        assert_eq!(s.set_config(cfg()), vec![Effect::Redraw]);
    }

    fn timer_cfg() -> Config {
        let mut c = Config::default();
        c.timer.kind = TimerKind::Countdown;
        c.timer.minutes = 30.0;
        c.timer.warn_minutes = 5.0;
        c
    }

    #[test]
    fn timer_does_nothing_unless_countdown() {
        let mut s = State::new(Config::default());
        assert_eq!(s.next_press(0), vec![]);
        assert_eq!(s.tick(100 * 60_000), vec![]);
    }

    #[test]
    fn timer_starts_once_warns_and_ends() {
        let mut s = State::new(timer_cfg());
        assert_eq!(s.next_press(1_000), vec![Effect::TimerStarted]);
        assert_eq!(s.next_press(2_000), vec![]); // already running
        assert_eq!(s.tick(1_000 + 24 * 60_000), vec![]);
        assert_eq!(s.tick(1_000 + 25 * 60_000), vec![Effect::VibrateWarning]);
        assert_eq!(s.tick(1_000 + 26 * 60_000), vec![]); // once only
        assert_eq!(s.tick(1_000 + 30 * 60_000), vec![Effect::VibrateEnd]);
        assert_eq!(s.tick(1_000 + 31 * 60_000), vec![]); // once only
    }

    #[test]
    fn timer_late_tick_skips_the_warning_not_the_end() {
        let mut s = State::new(timer_cfg());
        s.next_press(0);
        // First tick after the end: only the end alert, no stale warning.
        assert_eq!(s.tick(31 * 60_000), vec![Effect::VibrateEnd]);
    }

    #[test]
    fn timer_without_vibrate_stays_silent() {
        let mut c = timer_cfg();
        c.timer.vibrate = false;
        let mut s = State::new(c);
        s.next_press(0);
        assert_eq!(s.tick(40 * 60_000), vec![]);
    }

    #[test]
    fn warning_time_longer_than_timer_gives_no_warning() {
        let mut c = timer_cfg();
        c.timer.minutes = 3.0;
        c.timer.warn_minutes = 5.0;
        let mut s = State::new(c);
        s.next_press(0);
        assert_eq!(s.tick(2 * 60_000), vec![]);
        assert_eq!(s.tick(3 * 60_000), vec![Effect::VibrateEnd]);
    }


    #[test]
    fn default_cycle_is_highlight_then_magnify_only() {
        let mut s = State::new(Config::default());
        assert_eq!(s.mode(), Mode::Highlight);
        s.top_press(0);
        assert_eq!(s.top_press(100), vec![Effect::ModeChanged(Mode::Magnify)]);
        s.top_press(1000);
        assert_eq!(s.top_press(1100), vec![Effect::ModeChanged(Mode::Highlight)]);
    }
}
