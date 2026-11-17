//! Read-only: lists the controls the remote reports through ReprogControlsV4 (feature 0x1b04):
//! function 0 = count, function 1 = info for one index. Writes nothing.

use spotlight_rs::logitech_protocol::{self, feature, Device, Request};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_millis(1500);

fn main() {
    let nodes = logitech_protocol::find_hidpp_nodes(0x046d, 0xc53e);
    let Some(node) = nodes.first() else {
        println!("no HID++ node found");
        std::process::exit(1);
    };
    let mut dev = Device::open(node).expect("open HID++ node");
    dev.drain();
    logitech_protocol::ping(&mut dev, 1, TIMEOUT).expect("ping");
    let Some((feat, _)) = logitech_protocol::get_feature(&mut dev, 1, feature::REPROG_CONTROLS_V4, TIMEOUT).expect("feature lookup") else {
        println!("no ReprogControlsV4 feature");
        std::process::exit(1);
    };
    let r = dev.request(&Request::short(1, feat, 0, &[]), TIMEOUT).expect("count");
    let count = r[4];
    println!("ReprogControlsV4 at feature index {feat}, {count} controls");
    for i in 0..count {
        match dev.request(&Request::short(1, feat, 1, &[i]), TIMEOUT) {
            Ok(r) => println!(
                "control {i}: CID 0x{:04x}, TID 0x{:04x}, flags1 0x{:02x}, position {}, group {}, gmask 0x{:02x}, flags2 0x{:02x}",
                u16::from_be_bytes([r[4], r[5]]),
                u16::from_be_bytes([r[6], r[7]]),
                r[8], r[9], r[10], r[11], r[12]
            ),
            Err(e) => println!("control {i}: {e}"),
        }
    }
}
