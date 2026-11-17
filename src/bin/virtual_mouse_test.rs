//! Creates the virtual mouse, checks the kernel lists it, destroys it, checks it is gone.
//! Emits no events and does not touch any real device.

use spotlight_rs::{remote_input, virtual_devices::VirtualMouse};
use std::{thread::sleep, time::Duration};

const NAME: &str = "spotlight-rs uinput test";

fn listed() -> Option<String> {
    remote_input::find_event_node_now(NAME)
}

fn main() {
    println!("before: {:?}", listed());
    let vm = match VirtualMouse::create(NAME) {
        Ok(v) => v,
        Err(e) => {
            println!("create failed: {e}");
            std::process::exit(1);
        }
    };
    sleep(Duration::from_millis(300));
    println!("after create: {:?}", listed());
    drop(vm);
    sleep(Duration::from_millis(300));
    println!("after destroy: {:?}", listed());
}
