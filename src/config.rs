//! Settings loaded from a TOML file and hot-reloaded while the program runs.
//!
//! Every field has a default, so an empty or partial file is valid. A file that
//! fails to parse keeps the previous settings and reports the error.

use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

/// One pointer mode (highlight, magnify or circle).
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct ModeConfig {
    pub enabled: bool,
    /// Radius as a percentage of the screen height (like the size slider in the Logitech app).
    pub size_percent: f64,
}

impl Default for ModeConfig {
    fn default() -> Self {
        Self { enabled: true, size_percent: 10.0 }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Modes {
    pub highlight: ModeConfig,
    pub magnify: ModeConfig,
    pub circle: ModeConfig,
}

impl Default for Modes {
    fn default() -> Self {
        Self {
            highlight: ModeConfig::default(),
            magnify: ModeConfig { enabled: true, size_percent: 12.0 },
            circle: ModeConfig { enabled: false, size_percent: 10.0 },
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Look {
    /// Dot or ring color as "#rrggbb" or "#rrggbbaa".
    pub color: String,
    /// Ring thickness in pixels.
    pub ring_width: f64,
    /// How dark the screen gets outside the highlight circle, 0.0 to 1.0.
    pub dim_level: f64,
    /// Soft edge of the highlight circle in pixels.
    pub edge_softness: f64,
    /// Radius of the center dot in pixels, 0 for no dot.
    pub dot_radius: f64,
    /// Magnify: zoom factor inside the circle.
    pub zoom: f64,
    /// Digital laser pointer (the `laser` hold action): dot color, #rrggbb or #rrggbbaa.
    pub laser_color: String,
    /// Digital laser pointer: dot radius in pixels.
    pub laser_radius: f64,
    /// Modes during which the arrow cursor is hidden: any of "highlight", "magnify", "circle", "laser".
    pub hide_cursor: Vec<String>,
}

pub const MODE_NAMES: [&str; 4] = ["highlight", "magnify", "circle", "laser"];

impl Default for Look {
    fn default() -> Self {
        Self {
            color: "#ffffff".into(),
            ring_width: 3.0,
            dim_level: 0.6,
            edge_softness: 6.0,
            dot_radius: 0.0,
            zoom: 2.0,
            laser_color: "#ff0000".into(),
            laser_radius: 12.0,
            hide_cursor: vec!["highlight".into(), "magnify".into(), "laser".into()],
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TimerKind {
    Off,
    Countdown,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Timer {
    pub kind: TimerKind,
    /// Countdown length in minutes (Logitech's presets are 30 and 60, plus custom). Fractions allowed.
    pub minutes: f64,
    /// Warn this many minutes before the end. Fractions allowed.
    pub warn_minutes: f64,
    pub vibrate: bool,
    /// Vibration strength 0 to 255.
    pub vibration_intensity: u8,
    /// Vibration length 1 to 10 for the warning and for the end (units as the device defines them).
    pub warn_length: u8,
    pub end_length: u8,
}

impl Default for Timer {
    fn default() -> Self {
        Self {
            kind: TimerKind::Off,
            minutes: 30.0,
            warn_minutes: 5.0,
            vibrate: true,
            vibration_intensity: 128,
            warn_length: 2,
            end_length: 5,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HoldAction {
    None,
    BlankScreen,
    CustomKeystroke,
    /// A dot (digital laser pointer) that follows the remote while the button is held.
    Laser,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Hold {
    pub next: HoldAction,
    pub back: HoldAction,
    /// Used when an action is custom_keystroke, e.g. "b" or "ctrl+shift+b".
    pub custom_keystroke: String,
    /// Key sent for blank_screen. B blanks the screen in Google Slides; LibreOffice Impress did not
    /// react to it in testing.
    pub blank_key: String,
}

impl Default for Hold {
    fn default() -> Self {
        Self { next: HoldAction::BlankScreen, back: HoldAction::None, custom_keystroke: String::new(), blank_key: "b".into() }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Input {
    /// Name of the receiver's mouse interface as listed in /proc/bus/input/devices.
    pub device_name: String,
    /// Name of the receiver's keyboard interface (Next and Back).
    pub keyboard_name: String,
    /// The overlay hides after this many milliseconds without remote motion.
    pub idle_ms: u64,
    /// Take over the remote's mouse and do not pass the top button's taps on as clicks, so a double
    /// press does not click (and advance a slide). Cursor motion is passed on unchanged.
    pub swallow_clicks: bool,
    /// While a `laser` hold is active the cursor is moved by the remote's movement reports times this.
    pub laser_speed: f64,
    /// Cursor movement per movement report while the top button is held (the pointer).
    pub pointer_speed: f64,
    /// Two presses of the top button within this many milliseconds switch the pointer mode.
    pub double_press_ms: u64,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            device_name: "Logitech USB Receiver Mouse".into(),
            keyboard_name: "Logitech USB Receiver".into(),
            idle_ms: 400,
            swallow_clicks: true,
            laser_speed: 1.0,
            pointer_speed: 1.0,
            double_press_ms: 400,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub modes: Modes,
    pub look: Look,
    pub timer: Timer,
    pub hold: Hold,
    pub input: Input,
}

/// RGBA, each 0 to 255.
pub fn parse_color(s: &str) -> Option<[u8; 4]> {
    let h = s.strip_prefix('#')?;
    if !h.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    match h.len() {
        6 => Some([byte(0)?, byte(2)?, byte(4)?, 255]),
        8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => None,
    }
}

impl Config {
    pub fn parse(text: &str) -> Result<Config, String> {
        let cfg: Config = toml::from_str(text).map_err(|e| e.to_string())?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), String> {
        if parse_color(&self.look.color).is_none() {
            return Err(format!("look.color {:?} is not #rrggbb or #rrggbbaa", self.look.color));
        }
        if parse_color(&self.look.laser_color).is_none() {
            return Err(format!("look.laser_color {:?} is not #rrggbb or #rrggbbaa", self.look.laser_color));
        }
        for name in &self.look.hide_cursor {
            if !MODE_NAMES.contains(&name.as_str()) {
                return Err(format!("look.hide_cursor has unknown mode {name:?}; use {MODE_NAMES:?}"));
            }
        }
        for (name, v) in [
            ("look.ring_width", self.look.ring_width),
            ("look.edge_softness", self.look.edge_softness),
            ("look.dot_radius", self.look.dot_radius),
        ] {
            if !(0.0..=500.0).contains(&v) {
                return Err(format!("{name} must be between 0 and 500"));
            }
        }
        if !(10..=5000).contains(&self.input.double_press_ms) {
            return Err("input.double_press_ms must be between 10 and 5000".into());
        }
        if !(1.0..=200.0).contains(&self.look.laser_radius) {
            return Err("look.laser_radius must be between 1 and 200".into());
        }
        if !(0.1..=10.0).contains(&self.input.pointer_speed) {
            return Err("input.pointer_speed must be between 0.1 and 10".into());
        }
        if !(0.1..=10.0).contains(&self.input.laser_speed) {
            return Err("input.laser_speed must be between 0.1 and 10".into());
        }
        if !(1.0..=8.0).contains(&self.look.zoom) {
            return Err("look.zoom must be between 1 and 8".into());
        }
        if !(0.0..=1.0).contains(&self.look.dim_level) {
            return Err("look.dim_level must be between 0.0 and 1.0".into());
        }
        for (name, m) in [
            ("highlight", &self.modes.highlight),
            ("magnify", &self.modes.magnify),
            ("circle", &self.modes.circle),
        ] {
            if !(1.0..=50.0).contains(&m.size_percent) {
                return Err(format!("modes.{name}.size_percent must be between 1 and 50"));
            }
        }
        if !(self.modes.highlight.enabled || self.modes.magnify.enabled || self.modes.circle.enabled) {
            return Err("at least one of modes.highlight, modes.magnify, modes.circle must be enabled".into());
        }
        if crate::virtual_devices::parse_keystroke(&self.hold.blank_key).is_none() {
            return Err(format!("hold.blank_key {:?} is not a known key or combination", self.hold.blank_key));
        }
        if !self.hold.custom_keystroke.is_empty() && crate::virtual_devices::parse_keystroke(&self.hold.custom_keystroke).is_none() {
            return Err(format!("hold.custom_keystroke {:?} is not a known key or combination", self.hold.custom_keystroke));
        }
        if !(0.05..=10_000.0).contains(&self.timer.minutes) {
            return Err("timer.minutes must be between 0.05 and 10000".into());
        }
        if !(0.0..=10_000.0).contains(&self.timer.warn_minutes) {
            return Err("timer.warn_minutes must be between 0 and 10000".into());
        }
        for (name, v) in [("warn_length", self.timer.warn_length), ("end_length", self.timer.end_length)] {
            if !(1..=10).contains(&v) {
                return Err(format!("timer.{name} must be between 1 and 10"));
            }
        }
        if self.input.idle_ms < 50 {
            return Err("input.idle_ms must be at least 50".into());
        }
        Ok(())
    }
}

/// Default config location: $XDG_CONFIG_HOME/spotlight-rs/config.toml or ~/.config/...
pub fn default_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("spotlight-rs").join("config.toml")
}

/// Watches the file by modification time. Polling is cheap at a few Hz and needs no extra crate.
pub struct ConfigWatcher {
    path: PathBuf,
    modified: Option<SystemTime>,
}

impl ConfigWatcher {
    pub fn new(path: &Path) -> Self {
        Self { path: path.to_path_buf(), modified: None }
    }

    /// Ok(Some(cfg)) when the file changed and parsed, Ok(None) when unchanged,
    /// Err(message) when it changed but is invalid (caller keeps the old config).
    pub fn poll(&mut self) -> Result<Option<Config>, String> {
        let modified = fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        if modified == self.modified {
            return Ok(None);
        }
        self.modified = modified;
        match fs::read_to_string(&self.path) {
            Ok(text) => Config::parse(&text).map(Some),
            // File removed: fall back to defaults.
            Err(_) => Ok(Some(Config::default())),
        }
    }
}

pub const EXAMPLE: &str = r##"# spotlight-rs configuration. Every setting is optional; this file lists the defaults.
# The file is re-read while the program runs. A mistake is reported and the previous settings stay.

[modes.highlight]
enabled = true
size_percent = 10.0      # radius as a percentage of the screen height

[modes.magnify]
enabled = true           # live zoom of the active window inside the circle
size_percent = 12.0

[modes.circle]
enabled = false         # plain ring, no dimming or zoom
size_percent = 10.0

[look]
color = "#ffffff"        # ring and dot color, #rrggbb or #rrggbbaa
ring_width = 3.0
dim_level = 0.6          # 0.0 to 1.0
edge_softness = 6.0
dot_radius = 0.0         # 0 for no dot
zoom = 2.0               # magnify: zoom factor, 1 to 8
laser_color = "#ff0000"  # the digital laser pointer dot
laser_radius = 12.0
hide_cursor = ["highlight", "magnify", "laser"]  # modes that hide the arrow cursor; also "circle"

[timer]
kind = "off"             # off or countdown
minutes = 30.0           # fractions allowed, for example 0.5
warn_minutes = 5.0
vibrate = true
vibration_intensity = 128  # 0 to 255
warn_length = 2          # 1 to 10
end_length = 5           # 1 to 10

[hold]
next = "blank_screen"    # none, blank_screen, custom_keystroke or laser
back = "none"
custom_keystroke = ""     # for example "b" or "ctrl+shift+b"
blank_key = "b"           # key sent for blank_screen

[input]
device_name = "Logitech USB Receiver Mouse"
keyboard_name = "Logitech USB Receiver"
idle_ms = 400
swallow_clicks = true    # take over the remote's mouse so taps do not click
laser_speed = 1.0        # cursor movement per movement report while a laser hold is active
pointer_speed = 1.0      # the same while the top button is held (highlight, magnify)
double_press_ms = 400
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn example_matches_defaults() {
        assert_eq!(Config::parse(EXAMPLE).unwrap(), Config::default());
    }

    #[test]
    fn partial_override() {
        let c = Config::parse("[look]\ncolor = \"#ff0000\"\n[timer]\nkind = \"countdown\"\nminutes = 45\n").unwrap();
        assert_eq!(c.look.color, "#ff0000");
        assert_eq!(c.timer.kind, TimerKind::Countdown);
        assert_eq!(c.timer.minutes, 45.0);
        assert_eq!(c.timer.warn_minutes, 5.0);
    }

    #[test]
    fn removed_hold_actions_are_rejected() {
        for a in ["fast_forward", "scroll", "volume_control"] {
            assert!(Config::parse(&format!("[hold]\nnext = \"{a}\"\n")).is_err(), "{a}");
        }
    }

    #[test]
    fn bad_values_are_rejected() {
        assert!(Config::parse("[look]\ncolor = \"red\"\n").is_err());
        assert!(Config::parse("[look]\ndim_level = 2.0\n").is_err());
        assert!(Config::parse("[modes.circle]\nsize_percent = 0.0\n").is_err());
        assert!(Config::parse("[timer]\nminutes = 0\n").is_err());
        assert!(Config::parse("[timer]\nminutes = 0.5\nwarn_minutes = -1\n").is_err());
        assert!(Config::parse("[timer]\nwarn_length = 0\n").is_err());
        assert!(Config::parse("[hold]\nblank_key = \"nonsense\"\n").is_err());
        assert!(Config::parse("[look]\nlaser_color = \"red\"\n").is_err());
        assert!(Config::parse("[look]\nlaser_radius = 0.0\n").is_err());
        assert!(Config::parse("[look]\nzoom = 0.5\n").is_err());
        assert!(Config::parse("[look]\nzoom = 9.0\n").is_err());
        assert!(Config::parse("[look]\nzoom = 3.0\n").is_ok());
        assert!(Config::parse("[look]\nhide_cursor = [\"spotlight\"]\n").is_err());
        assert!(Config::parse("[look]\nhide_cursor = []\n").is_ok());
        assert!(Config::parse("[look]\nhide_cursor = [\"circle\", \"laser\"]\n").is_ok());
        assert!(Config::parse("[input]\nlaser_speed = 0.0\n").is_err());
        assert!(Config::parse("[input]\npointer_speed = 0.0\n").is_err());
        for bad in ["ring_width = inf", "ring_width = -1.0", "edge_softness = nan", "dot_radius = 1e9"] {
            assert!(Config::parse(&format!("[look]\n{bad}\n")).is_err(), "{bad}");
        }
        assert!(Config::parse("[input]\ndouble_press_ms = 0\n").is_err());
        assert!(Config::parse("[timer]\nminutes = inf\n").is_err());
        assert!(Config::parse("[timer]\nminutes = 1.0\nwarn_minutes = inf\n").is_err());
        assert!(Config::parse("[hold]\nback = \"laser\"\n").is_ok());
        assert!(Config::parse("[hold]\ncustom_keystroke = \"ctrl+\"\n").is_err());
        assert!(Config::parse("[hold]\ncustom_keystroke = \"ctrl+shift+b\"\n").is_ok());
        assert!(Config::parse("[timer]\nend_length = 11\n").is_err());
        assert!(Config::parse("[timer]\nminutes = 0.5\n").is_ok());
        assert!(
            Config::parse("[modes.highlight]\nenabled = false\n[modes.magnify]\nenabled = false\n[modes.circle]\nenabled = false\n").is_err()
        );
        assert!(Config::parse("[input]\nidle_ms = 1\n").is_err());
        assert!(Config::parse("not toml at all =").is_err());
    }

    #[test]
    fn colors() {
        assert_eq!(parse_color("#ff8000"), Some([255, 128, 0, 255]));
        assert_eq!(parse_color("#ff800080"), Some([255, 128, 0, 128]));
        assert_eq!(parse_color("ff8000"), None);
        assert_eq!(parse_color("#ff80"), None);
        assert_eq!(parse_color("#gg0000"), None);
    }
}
