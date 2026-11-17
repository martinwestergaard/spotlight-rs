# Hands-on tests

Build once: cd ~/dev/git/spotlight-rs && cargo build
Program: ./target/debug/spotlight-rs   Other tools: check_remote, list_controls, divert_test, capture_test, virtual_mouse_test, vibrate_test (same folder)

Already checked without the remote in hand: 84 unit tests (cargo test), setup check, run and stop on Ctrl+C,
config hot reload (color change), hide and show of the overlay, window and output capture, HID++ reads.

## 1 Health
./target/debug/spotlight-rs setup
Expect: every line ok or info

## 2 Run and show the pointer
./target/debug/spotlight-rs
Expect the log lines: remote mouse and remote keyboard connected
Hold the top button and move the remote
- The screen dims with a clear circle on the cursor (Highlight is the default mode)
- It follows the cursor and stays for as long as you hold the button, even with your hand still
- It disappears when you release the button (the log shows no error)
- Clicks work normally while it is hidden
Tell me: lag, whether the circle sits exactly on the cursor, anything odd at the screen edges

## 3 Double press switches mode
Tap the top button twice quickly, then hold and move
Expect: log line "mode: Magnify" and the zoomed active window inside the ring (highlight and magnify are on by default, circle is off); a second double press goes back to Highlight
Known: the taps are swallowed (input.swallow_clicks), so they do not click
Tell me: how fast you can double press before it stops counting (the window is 400 ms in the config)

## 4 Timer
mkdir -p ~/.config/spotlight-rs && ./target/debug/spotlight-rs example-config > ~/.config/spotlight-rs/config.toml
Edit the file: [timer] kind = "countdown", minutes = 2, warn_minutes = 1
Press Next once with the program running, or start it with --timer-now
Expect: "timer started", then "timer: warning" after 1 minute and "timer: end" after 2 minutes
The remote vibrates at the warning and at the end (see step 7)

## 5 Hot reload
While the program runs, change look.color, look.dim_level or modes.highlight.size_percent and save
Expect: "config reloaded" and the next frame uses the new value
Then break the file on purpose (a typo)
Expect: a "config error" line and the previous settings stay

## 6 Stop
Ctrl+C in the terminal
Expect: "stopped" and no overlay left on screen

## 7 Vibration (this writes to the remote)
./target/debug/vibrate_test 1 128
Expect: "device acknowledged" and a buzz. If you feel nothing try: ./target/debug/vibrate_test 3 255
Tell me what you felt for each pair of numbers. The vibrate command is confirmed to work at 1 and 128; the other values are untested

## 8 Unplug and replug the receiver while the program runs
Expect: "lost /dev/input/..." and "hold actions: lost the receiver's HID++ node", then within a few seconds of replugging the remote lines again and "hold actions on"
Then hold the top button, Back and Next: all three work again without restarting the program
Not yet tried on the hardware: this reconnect path is new

## 9 Still hold, speed and release (top button)
Hold the top button without moving for 5 seconds, then release
Expect: the overlay stays up the whole time and goes at once on release
If the cursor feels too fast or slow while holding, change input.pointer_speed (default 1.0)

## 10 Cursor in the lens
Remove "magnify" from look.hide_cursor, save, hold the top button in magnify mode
Expect: the normal arrow shows in the middle of the lens (it is not zoomed)

## Other machine: first run
cargo build (the crate uses edition 2024; this machine builds it with rust 1.95 from apt. If the build fails on a missing library, note which)
./target/debug/spotlight-rs setup   prints what is missing and the commands to fix it (udev rule needs sudo, run it yourself)
Then steps 1, 2, 3, 7 and 9. Check the log line "scale ..." matches the screen's scale setting

## Checked with you in the room
- 2 run and show: works (idle hide set to 400 ms)
- 3 double press: switches mode
- 5 hot reload while running: works
- Double press with the slideshow up: slides no longer advance (remote mouse taken over, taps swallowed)
- Hold Next (blank screen): works in Google Slides; LibreOffice Impress does not react to the B key
- Digital laser on a hold of Back (hold.back = "laser"): red dot follows, arrow cursor hidden, gone on release
- Cursor hidden during highlight and laser, kept in circle (look.hide_cursor)
- 7 vibration: buzzes; timer warning and end buzz (run with --timer-now and a short [timer] config to try it): one short buzz, then one long
- Top button hold: stays up while the hand is still and hides on release (diverted over HID++, control 0x00d8); taps still arrive as BTN_LEFT
- Hold Next and Back diverts are undone on SIGTERM ("hold actions off: divert undone")
- Fractional scale 1.25: ring edges are crisp in a screenshot (checked by reading pixels)

## Not done
- Magnify follows the active window only; other windows are not shown in the lens, and the lens looks soft because the capture is at logical size
- Multi-monitor, the laptop
- Whether the diverts survive a power cycle of the remote
