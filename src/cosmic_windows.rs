//! Client bindings for COSMIC's zcosmic_toplevel_info_v1 (version 3), generated from
//! protocols/cosmic-toplevel-info-unstable-v1.xml. It tells which window is active and where
//! it sits on the output.

#![allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables, non_upper_case_globals, non_snake_case, unused_imports, missing_docs, clippy::all)]

use wayland_client;
use wayland_client::protocol::*;
use wayland_protocols::ext::foreign_toplevel_list::v1::client::*;

pub mod __interfaces {
    use wayland_client::protocol::__interfaces::*;
    use wayland_protocols::ext::foreign_toplevel_list::v1::client::__interfaces::*;
    wayland_scanner::generate_interfaces!("protocols/cosmic-toplevel-info-unstable-v1.xml");
}
use self::__interfaces::*;

wayland_scanner::generate_client_code!("protocols/cosmic-toplevel-info-unstable-v1.xml");
