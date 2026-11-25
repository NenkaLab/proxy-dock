//! ProxyDock core: everything that is not drawing a window. Ported from the Python app (`proxydock/`) module by
//! module; each Rust module names the Python one it replaces, and its behaviour is checked against that one.
#![recursion_limit = "256"] // json! of the outbound defaults

pub mod checks;
pub mod clash;
pub mod config;
pub mod edit;
pub mod form;
pub mod graph;
pub mod http;
pub mod i18n;
pub mod ids;
pub mod links;
pub mod log;
pub mod manager;
pub mod net;
pub mod paths;
pub mod py;
pub mod secret;
pub mod singbox;
pub mod subscription;
pub mod tls;
pub mod wg;
