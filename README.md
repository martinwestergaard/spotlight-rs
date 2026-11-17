# spotlight-rs

On-screen pointer for the Logitech Spotlight presentation remote on Linux, written for Pop!_OS with the COSMIC desktop (Wayland)

Hold the top button and the screen shows a pointer that follows the remote. The pointer has these modes

- Highlight: the screen dims, a clear circle stays around the pointer
- Magnify: a live zoom of the active window inside a circle
- Laser: a glowing red dot, shown while Back is held
- Circle: a plain ring, off by default

Other features

- Double press the top button to switch between highlight and magnify
- Taps of the top button do not click, so a double press never advances a slide
- Hold Next to send a key, by default the blank-screen key `b`
- A countdown timer that vibrates the remote before the end and at the end
- A config file that is re-read while the program runs

It was built and tested with one Spotlight (USB receiver 046d:c53e) on COSMIC. Other desktops and remotes are untested

## Requirements

- A Logitech Spotlight with its USB receiver
- COSMIC (Wayland). The overlay needs the layer-shell protocol. Magnify also needs `ext_image_copy_capture` and COSMIC's toplevel info protocol. `spotlight-rs setup` reports what the compositor offers
- A Rust toolchain that supports edition 2024 (it builds with rust 1.95 from apt on the development machine)
- Read access to the receiver's input and hidraw nodes, and write access to `/dev/uinput` (see Install)

## Install

Build

```
git clone <repository url>
cd spotlight-rs
cargo build --release
```

The program is then `target/release/spotlight-rs`. To put it on your path instead, run `cargo install --path .`, which installs to `~/.cargo/bin`, together with the small hardware test programs in `src/bin`

Check the machine

```
target/release/spotlight-rs setup
```

Setup reads the state of the receiver, the input nodes, the compositor and `/dev/uinput`. It prints `ok`, `info` or `MISSING` for each, and the commands to fix what is missing. It never runs sudo. You run the commands yourself

If the receiver is not accessible, the fix is a udev rule that gives the logged-in user access to it

```
sudo tee /etc/udev/rules.d/55-spotlight.rules >/dev/null <<'EOF'
# Logitech Spotlight USB Receiver: user access for the session user
SUBSYSTEMS=="usb", ATTRS{idVendor}=="046d", ATTRS{idProduct}=="c53e", MODE="0660", TAG+="uaccess"
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="046d", ATTRS{idProduct}=="c53e", MODE="0660", TAG+="uaccess"
EOF
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Then unplug and replug the receiver

`/dev/uinput` is needed to swallow taps, to move the cursor during a hold, and to send the hold-action key. Setup reports `info` with `false` when it is not writable. It was already writable on the development machine, and how it got that way was not checked. Without it the program still shows the pointer, but taps click and the hold actions do not work

## Run

```
spotlight-rs
```

Leave it running in a terminal. Hold the top button and move the remote. Stop it with Ctrl+C (closing the terminal or a plain `kill` stops it the same way). On exit it undoes the changes it made to the remote and releases the remote's mouse. There is no autostart yet

Commands

| Command | What it does |
|---|---|
| `spotlight-rs` or `spotlight-rs run` | Run the program |
| `spotlight-rs setup` | Check access and protocols, print the fixes |
| `spotlight-rs example-config` | Print a config file with every default |
| `spotlight-rs demo` | Draw one pointer without the remote, for screenshots |

Options

| Option | Used with | What it does |
|---|---|---|
| `--config PATH` | run, demo, setup | Use another config file |
| `--timer-now` | run | Start the countdown at launch, to try the timer |
| `--mode highlight\|magnify\|circle\|laser` | demo | Which pointer to draw |
| `--at X,Y` | demo | Pointer position in logical pixels |
| `--seconds N` | demo | How long to show it, default 10 |
| `--cycle` | demo | Hide and show the overlay once, to test it |

The demo draws a click-through overlay, so clicks keep working while it is up

### What the program changes on the remote

Two things, and nothing else

- It makes the remote vibrate for the timer
- It asks the remote to report holds of Next, Back and the top button over HID++, so it can see a hold start and end. It switches this off again on exit

It never sends Reset, never changes pointer speed and never writes receiver registers. If the program is killed hard, the reporting stays on until a later run ends cleanly, which undoes it. Whether it survives a power cycle of the remote was not checked

## Using it

- Hold the top button and move: the current mode shows and follows the remote. It stays for as long as you hold, even with a still hand, and goes when you let go
- Double press the top button (two taps within 400 ms): switch mode. The first mode after start is highlight
- Hold Next: runs the action set in `hold.next`, by default a `b` key press. It blanks the screen in Google Slides and does nothing in LibreOffice Impress (the only two tried)
- Hold Back: runs the action set in `hold.back`, by default none. Set it to `laser` for the red dot
- Next and Back taps are plain arrow keys, so slides advance with no software
- Magnify shows the active window only. Where the circle runs past the window, it stays empty

## Config file

Location: `$XDG_CONFIG_HOME/spotlight-rs/config.toml`, which is `~/.config/spotlight-rs/config.toml` when that variable is not set. Use `--config PATH` for another file

Create one from the defaults

```
mkdir -p ~/.config/spotlight-rs
spotlight-rs example-config > ~/.config/spotlight-rs/config.toml
```

Without a file the defaults are used. Every setting is optional, so a file can hold only the lines you change, for example

```toml
[hold]
back = "laser"

[look]
zoom = 3.0
```

### How it works

- The file is TOML
- The program checks the file about twice a second and reloads it when it changes. The log shows `config reloaded`
- If the file has a mistake, the log shows `config error` with the reason and the previous settings stay in use
- If you delete the file, the defaults come back
- Some settings are read only at certain moments. `input.device_name` and `input.keyboard_name` are read once at start, so changing them needs a restart. `input.swallow_clicks` applies fully after the remote reconnects. `look.hide_cursor` applies the next time the overlay appears
- A value outside its range is rejected as a mistake. At least one of the three pointer modes must stay enabled

### Settings

`[modes.highlight]`, `[modes.magnify]` and `[modes.circle]`

| Key | Default | Meaning |
|---|---|---|
| `enabled` | highlight true, magnify true, circle false | Whether the mode is in the double-press cycle |
| `size_percent` | highlight 10, magnify 12, circle 10 | Radius as a percentage of the screen height, 1 to 50 |

`[look]`

| Key | Default | Meaning |
|---|---|---|
| `color` | `"#ffffff"` | Ring and dot color, `#rrggbb` or `#rrggbbaa` |
| `ring_width` | 3.0 | Ring thickness in pixels, 0 to 500 |
| `dim_level` | 0.6 | How dark the screen gets outside the highlight circle, 0.0 to 1.0 |
| `edge_softness` | 6.0 | Soft edge of the highlight circle in pixels, 0 to 500 |
| `dot_radius` | 0.0 | Radius of a center dot in pixels, 0 for none, up to 500 |
| `zoom` | 2.0 | Magnify zoom factor, 1 to 8 |
| `laser_color` | `"#ff0000"` | Laser dot color |
| `laser_radius` | 12.0 | Laser dot radius in pixels, 1 to 200 |
| `hide_cursor` | `["highlight", "magnify", "laser"]` | Modes during which the arrow cursor is hidden. Also accepts `"circle"`. Remove `"magnify"` to show the cursor in the lens |

Pixel sizes are in logical pixels, so they look the same at any display scale

`[hold]`

| Key | Default | Meaning |
|---|---|---|
| `next` | `"blank_screen"` | Action for a hold of Next |
| `back` | `"none"` | Action for a hold of Back |
| `blank_key` | `"b"` | Key sent for `blank_screen` |
| `custom_keystroke` | `""` | Key or combination sent for `custom_keystroke`, for example `"ctrl+shift+b"` |

Actions are `none`, `blank_screen`, `custom_keystroke` and `laser`. In a combination, modifiers come first and the last part is the key. An unknown key name is a config mistake

`[timer]`

| Key | Default | Meaning |
|---|---|---|
| `kind` | `"off"` | `off` or `countdown` |
| `minutes` | 30.0 | Countdown length in minutes, 0.05 to 10000, fractions allowed |
| `warn_minutes` | 5.0 | Warn this long before the end, 0 for no warning, up to 10000 |
| `vibrate` | true | Vibrate the remote at the warning and at the end |
| `vibration_intensity` | 128 | 0 to 255 |
| `warn_length` | 2 | Length of the warning buzz, 1 to 10 |
| `end_length` | 5 | Length of the end buzz, 1 to 10 |

The countdown starts at the first press of Next, or at launch with `--timer-now`

`[input]`

| Key | Default | Meaning |
|---|---|---|
| `device_name` | `"Logitech USB Receiver Mouse"` | Name of the remote's mouse node |
| `keyboard_name` | `"Logitech USB Receiver"` | Name of the remote's keyboard node |
| `idle_ms` | 400 | Hide the pointer this long after the last motion when the top button's hold is not seen over HID++, at least 50 |
| `swallow_clicks` | true | Take over the remote's mouse so taps do not click. Needs `/dev/uinput` |
| `double_press_ms` | 400 | Two taps within this time switch the mode, 10 to 5000 |
| `laser_speed` | 1.0 | Cursor movement per movement report during a laser hold, 0.1 to 10 |
| `pointer_speed` | 1.0 | The same while the top button is held, 0.1 to 10 |

## Troubleshooting

- Run `spotlight-rs setup` first. It names what is missing
- Everything the program does is logged to the terminal: devices found, mode changes, holds, timer events, config reloads and errors
- The pointer shows but taps click: `/dev/uinput` is not writable, or `swallow_clicks` is false
- The pointer is soft or the wrong size: the log line `scale ...` shows the scale the compositor reported. If the line is missing, the compositor did not offer fractional scaling and drawing is stretched
- The remote does not vibrate: the remote may be asleep. The log says `vibration failed` with the reason

## Code layout

| File | What it does |
|---|---|
| `src/main.rs` | Command line, the `setup` check, signals, and the main loop |
| `src/overlay.rs` | The full-screen overlay surface, its drawing and its Wayland handlers |
| `src/remote.rs` | The physical remote: input nodes, the virtual mouse, hold links, hold actions, vibration |
| `src/state_machine.rs` | Modes, holds, idle hiding and the timer, with no I/O |
| `src/render.rs` | Draws highlight, ring and laser into a pixel buffer |
| `src/lens.rs` | The zoomed circle for magnify |
| `src/window_capture.rs` | Live capture of the active window, on its own thread |
| `src/cosmic_windows.rs` | Generated bindings for COSMIC's window-info protocol |
| `src/logitech_protocol.rs` | HID++ messages to the remote: vibrate, divert, hold reports |
| `src/remote_input.rs` | Reading the remote's mouse and keyboard nodes |
| `src/virtual_devices.rs` | The virtual mouse and keyboard created through `/dev/uinput` |
| `src/config.rs` | The config file: settings, validation, hot reload |
| `src/bin/` | Small hardware test programs: `check_remote`, `list_controls`, `divert_test`, `vibrate_test`, `virtual_mouse_test`, `capture_test` |

## Tests

```
cargo test
```

The unit tests cover the drawing, the state machine, the config and the HID++ framing. `TESTS.md` lists the checks that need the remote in hand

## License

None chosen yet
