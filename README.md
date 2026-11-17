# spotlight-rs

On-screen pointer for the Logitech Spotlight presentation remote, for Pop!_OS with the COSMIC desktop (Wayland)

Hold the top button and a pointer follows the remote: a highlight, a live magnifier or a red laser dot. Other buttons run actions such as blanking the screen, and a timer can buzz the remote before your time is up

Built and tested with one Spotlight (receiver 046d:c53e) on COSMIC

<img width="640" height="494" alt="modes-draft" src="https://github.com/user-attachments/assets/92bcdf1f-3c39-4e03-95e7-34a6191715e2" />

## Features

| Mode | What you see | Default |
|---|---|---|
| Highlight | Screen dims, clear circle around the pointer | On |
| Magnify | Live zoom of the active window inside a circle, empty outside that window | On |
| Laser | Glowing red dot while Back is held | On (`hold.back = "laser"`) |
| Circle | Plain ring | Off |

- Double press the top button to switch modes
- Taps of the top button are swallowed, so a double press never advances a slide
- Hold Next to send a key, by default `b` (blank screen)
- Countdown timer that buzzes the remote before the end and at the end
- Config file reloaded while the program runs

## Requirements

- Logitech Spotlight with its USB receiver
- COSMIC session (Wayland). `setup` checks the protocols it needs
- Rust 1.85 or newer with `cargo` (edition 2024), and a C linker (`build-essential` on Ubuntu-based systems)
- `sudo` once for a udev rule

No other system libraries are needed (the program links only `libc` and `libgcc_s`). There are no scripts: the commands below work in bash and fish

## Install

```
sudo apt install git build-essential rustc cargo
git clone https://github.com/martinwestergaard/spotlight-rs.git
cd spotlight-rs
cargo build --release
target/release/spotlight-rs setup
```

If your distribution's Rust is older than 1.85 (`rustc --version`), use rustup instead. `cargo install --path .` puts the program on your path

`setup` checks the receiver, input nodes, compositor and `/dev/uinput`, and prints the fix for anything `MISSING`

### Device access

The program needs to read the receiver's mouse and keyboard nodes, write its HID++ node (vibration, hold detection), and write `/dev/uinput` (swallow taps, move the cursor, send keys). Without `/dev/uinput` the pointer still shows, but taps click and advance your slides

Receiver rule, then replug the receiver

```
target/release/spotlight-rs udev-rules | sudo tee /etc/udev/rules.d/55-spotlight.rules >/dev/null
sudo udevadm control --reload-rules && sudo udevadm trigger
```

`/dev/uinput` is already writable on the development machine through a system rule. If `setup` reports it `MISSING`, add this one. It is untested on a machine without the system rule

```
target/release/spotlight-rs udev-rules --uinput | sudo tee /etc/udev/rules.d/55-spotlight-uinput.rules >/dev/null
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Run `setup` again until every line says `ok`

## Usage

```
target/release/spotlight-rs
```

Leave it running in a terminal. Ctrl+C or `kill` stops it and undoes what it changed on the remote

| You do | Result |
|---|---|
| Hold the top button and move | Current mode follows the remote, stays while held even when still, hides on release |
| Double press the top button | Next mode. Two taps within 700 ms, or a tap followed within 700 ms by a press-and-hold, which then shows the new mode. The first mode is highlight |
| Hold Next | Runs `hold.next`, default a `b` key press. Blanks Google Slides, does nothing in LibreOffice Impress (the only two tried) |
| Hold Back | Runs `hold.back`, default the laser |
| Tap Next or Back | Arrow keys, no software involved |

### Commands and options

| Command | Does |
|---|---|
| `spotlight-rs` or `run` | Run the program |
| `setup` | Check access and protocols, print fixes |
| `example-config` | Print a config file with every default |
| `udev-rules [--uinput]` | Print the udev rule for the receiver, or for `/dev/uinput` |
| `demo` | Draw one pointer without the remote |

| Option | For | Does |
|---|---|---|
| `--config PATH` | run, demo, setup | Use another config file |
| `--timer-now` | run | Start the countdown at launch |
| `--trace` | run | Also log taps, holds, raw button events, hold reports and overlay shows, for diagnosing input |
| `--mode highlight\|magnify\|circle\|laser` | demo | Pointer to draw |
| `--at X,Y` | demo | Position in logical pixels |
| `--seconds N` | demo | Duration, default 10 |
| `--cycle` | demo | Hide and show once |

## Configuration

File: `~/.config/spotlight-rs/config.toml` (or under `$XDG_CONFIG_HOME`)

```
mkdir -p ~/.config/spotlight-rs
target/release/spotlight-rs example-config > ~/.config/spotlight-rs/config.toml
```

Every key is optional. A file can hold only what you change

```toml
[hold]
next = "none"

[look]
zoom = 3.0
```

- The file reloads within about half a second of saving (log: `config reloaded`)
- A mistake is rejected with a `config error` line and the previous settings stay. Out-of-range values count as mistakes, and at least one mode must stay enabled
- `input.device_name` and `input.keyboard_name` need a restart. `input.swallow_clicks` applies after the remote reconnects. `look.hide_cursor` applies the next time the overlay appears

**`[modes.highlight]`, `[modes.magnify]`, `[modes.circle]`**

| Key | Default | Meaning |
|---|---|---|
| `enabled` | true, true, false | In the double-press cycle |
| `size_percent` | 10, 12, 10 | Radius as a percentage of screen height, 1 to 50 |

**`[look]`** (pixel sizes are logical pixels)

| Key | Default | Meaning |
|---|---|---|
| `color` | `"#ffffff"` | Ring and dot color, `#rrggbb` or `#rrggbbaa` |
| `ring_width` | 3.0 | Ring thickness, 0 to 500 |
| `dim_level` | 0.6 | Darkness outside the highlight circle, 0 to 1 |
| `edge_softness` | 6.0 | Soft edge of the highlight circle, 0 to 500 |
| `dot_radius` | 0.0 | Center dot, 0 for none, up to 500 |
| `zoom` | 2.0 | Magnify zoom, 1 to 8 |
| `laser_color` | `"#ff0000"` | Laser dot color |
| `laser_radius` | 12.0 | Laser dot radius, 1 to 200 |
| `hide_cursor` | `["highlight", "magnify", "laser"]` | Modes that hide the arrow cursor, also `"circle"`. Remove `"magnify"` to see the cursor in the lens |

**`[hold]`**

| Key | Default | Meaning |
|---|---|---|
| `next`, `back` | `"blank_screen"`, `"laser"` | Action: `none`, `blank_screen`, `custom_keystroke` or `laser` |
| `blank_key` | `"b"` | Key sent for `blank_screen` |
| `custom_keystroke` | `""` | Key or combination, for example `"ctrl+shift+b"` (modifiers first) |

**`[timer]`** (the countdown starts at the first Next press)

| Key | Default | Meaning |
|---|---|---|
| `kind` | `"off"` | `off` or `countdown` |
| `minutes` | 30.0 | Length, 0.05 to 10000 |
| `warn_minutes` | 5.0 | Warn this long before the end, 0 for none |
| `vibrate` | true | Buzz at the warning and the end |
| `vibration_intensity` | 128 | 0 to 255 |
| `warn_length`, `end_length` | 2, 5 | Buzz lengths, 1 to 10 |

**`[input]`**

| Key | Default | Meaning |
|---|---|---|
| `device_name`, `keyboard_name` | `"Logitech USB Receiver Mouse"`, `"Logitech USB Receiver"` | Names of the remote's input nodes |
| `idle_ms` | 400 | Hide delay after the last motion when holds are not seen over HID++, at least 50 |
| `swallow_clicks` | true | Take over the remote's mouse so taps do not click. Needs `/dev/uinput` |
| `double_press_ms` | 700 | Window for a double press (tap, then a tap or a hold), 10 to 5000 |
| `tap_max_ms` | 450 | A press shorter than this counts as a tap, 100 to 2000. The remote reports presses slower than a quick click as short holds, and these count too |
| `laser_speed`, `pointer_speed` | 1.0, 1.0 | Cursor movement per movement report during a laser hold and a top-button hold, 0.1 to 10 |

## Troubleshooting

Run `spotlight-rs setup`, then read the terminal log

| Symptom | Cause |
|---|---|
| Taps click | `/dev/uinput` not writable, or `swallow_clicks` is false |
| Pointer is soft or the wrong size | The log line `scale ...` shows what the compositor reported. If it is missing, fractional scaling is not offered and drawing is stretched |
| No vibration | The remote may be asleep. The log says `vibration failed` with the reason |
| Holds stop working after replugging the receiver | The program should reconnect. This is untried on hardware: restart it and open an issue if it fails |

## Development

| File | Does |
|---|---|
| `src/main.rs` | Command line, `setup`, signals, main loop |
| `src/overlay.rs` | Overlay surface, drawing, Wayland handlers |
| `src/remote.rs` | The remote: input nodes, virtual mouse, hold links, hold actions, vibration |
| `src/state_machine.rs` | Modes, holds, idle hiding, timer (no I/O) |
| `src/render.rs`, `src/lens.rs` | Drawing the modes, the zoomed circle |
| `src/window_capture.rs`, `src/cosmic_windows.rs` | Live capture of the active window, COSMIC window-info bindings |
| `src/logitech_protocol.rs`, `src/remote_input.rs`, `src/virtual_devices.rs` | HID++ messages, evdev input, uinput devices |
| `src/config.rs` | Settings, validation, hot reload |
| `src/bin/` | Hardware test programs |

`cargo test` runs the unit tests. `TESTS.md` lists the checks that need the remote

## License

None chosen yet
