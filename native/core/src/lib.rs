//! ProxyDock core: everything that is not drawing a window. Ported from the Python app (`proxydock/`) module by
//! module; each Rust module names the Python one it replaces, and its behaviour is checked against that one.
#![recursion_limit = "256"] // json! of the outbound defaults

pub mod config;
pub mod net;
pub mod paths;
pub mod secret;
