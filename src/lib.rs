//! spotlight-rs as a library: the parts of the program that do not need a Wayland connection of
//! their own (config, state machine, drawing, lens, remote protocol, virtual devices, capture), so
//! they can be tested and reused by the small programs in src/bin.

pub mod config;
pub mod cosmic_windows;
pub mod lens;
pub mod logitech_protocol;
pub mod remote_input;
pub mod render;
pub mod state_machine;
pub mod virtual_devices;
pub mod window_capture;
