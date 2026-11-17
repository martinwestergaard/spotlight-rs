//! Read-only HID++ check of the Logitech Spotlight receiver. Sends only ping, feature
//! lookups and a battery query. Writes nothing to receiver registers and calls no
//! reset, vibration or button-divert function.

use spotlight_rs::logitech_protocol::{self, feature, Device, Error};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_millis(1500);

fn hex(b: &[u8]) -> String {
    b.iter().map(|v| format!("{v:02x}")).collect::<Vec<_>>().join(" ")
}

fn main() {
    let nodes = logitech_protocol::find_hidpp_nodes(0x046d, 0xc53e);
    println!("candidate HID++ nodes (Logitech hidraw without input children): {nodes:?}");
    if nodes.is_empty() {
        println!("none found: is the receiver plugged in, and is the udev rule installed?");
        std::process::exit(1);
    }
    for node in nodes {
        println!("\n== {}", node.display());
        let mut dev = match Device::open(&node) {
            Ok(d) => d,
            Err(e) => {
                println!("open failed: {e}");
                continue;
            }
        };
        dev.drain();

        let mut found_index = None;
        for idx in 1u8..=6 {
            match logitech_protocol::ping(&mut dev, idx, TIMEOUT) {
                Ok((maj, min)) => {
                    println!("ping device index {idx}: HID++ {maj}.{min}");
                    found_index = Some(idx);
                    break;
                }
                Err(Error::Device { code }) => println!("ping device index {idx}: device error {code} ({})", logitech_protocol::error_name(code)),
                Err(e) => println!("ping device index {idx}: {e}"),
            }
        }
        let Some(idx) = found_index else {
            println!("no device index answered");
            continue;
        };

        let wanted = [
            ("Root", feature::ROOT),
            ("FeatureSet", feature::FEATURE_SET),
            ("FirmwareVersion", feature::FIRMWARE),
            ("Reset", feature::RESET),
            ("BatteryStatus", feature::BATTERY_STATUS),
            ("UnifiedBattery", feature::UNIFIED_BATTERY),
            ("PresenterControl", feature::PRESENTER_CONTROL),
            ("Haptic", feature::HAPTIC),
            ("ReprogControlsV4", feature::REPROG_CONTROLS_V4),
            ("PointerSpeed", feature::POINTER_SPEED),
        ];
        let mut battery_feature = None;
        for (name, id) in wanted {
            match logitech_protocol::get_feature(&mut dev, idx, id, TIMEOUT) {
                Ok(Some((index, flags))) => {
                    println!("feature {name} (0x{id:04x}): index {index}, flags 0x{flags:02x}");
                    if id == feature::BATTERY_STATUS {
                        battery_feature = Some((index, false));
                    }
                    if id == feature::UNIFIED_BATTERY && battery_feature.is_none() {
                        battery_feature = Some((index, true));
                    }
                }
                Ok(None) => println!("feature {name} (0x{id:04x}): not present"),
                Err(e) => println!("feature {name} (0x{id:04x}): {e}"),
            }
        }

        if let Some((index, unified)) = battery_feature {
            match logitech_protocol::battery(&mut dev, idx, index, unified, TIMEOUT) {
                Ok(b) => println!("battery (feature index {index}, unified {unified}): level {} next {} status {}", b.level, b.next_level, b.status),
                Err(e) => println!("battery query: {e}"),
            }
        } else {
            println!("no battery feature found");
        }
        let _ = hex(&[]);
    }
}
