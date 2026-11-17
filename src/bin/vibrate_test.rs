//! Makes the Spotlight vibrate once. This is the only tool here that writes to the device.
//! Usage: vibrate_test [length 1-10] [intensity 0-255]     (defaults: 1 and 128)
//! Start gently; raise the numbers if you feel nothing.

use spotlight_rs::logitech_protocol::{self, feature, Device};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_millis(1500);

fn main() {
    let mut args = std::env::args().skip(1);
    let length: u8 = args.next().map(|a| a.parse().expect("length 1-10")).unwrap_or(1);
    let intensity: u8 = args.next().map(|a| a.parse().expect("intensity 0-255")).unwrap_or(128);

    let nodes = logitech_protocol::find_hidpp_nodes(0x046d, 0xc53e);
    let Some(node) = nodes.first() else {
        println!("no HID++ node found: is the receiver plugged in, and is the udev rule installed?");
        std::process::exit(1);
    };
    let mut dev = Device::open(node).expect("open HID++ node");
    dev.drain();
    let idx = 1;
    match logitech_protocol::ping(&mut dev, idx, TIMEOUT) {
        Ok((maj, min)) => println!("device answered, HID++ {maj}.{min}"),
        Err(e) => {
            println!("no answer from the remote: {e} (is it awake? press a button first)");
            std::process::exit(1);
        }
    }
    let feat = match logitech_protocol::get_feature(&mut dev, idx, feature::PRESENTER_CONTROL, TIMEOUT) {
        Ok(Some((i, _))) => i,
        Ok(None) => {
            println!("this device has no PresenterControl feature (0x1a00)");
            std::process::exit(1);
        }
        Err(e) => {
            println!("feature lookup failed: {e}");
            std::process::exit(1);
        }
    };
    println!("sending vibrate: length {length}, intensity {intensity} (PresenterControl at feature index {feat})");
    match logitech_protocol::vibrate(&mut dev, idx, feat, length, intensity, TIMEOUT) {
        Ok(()) => println!("device acknowledged. Did it vibrate?"),
        Err(e) => println!("device did not acknowledge: {e}"),
    }
}
