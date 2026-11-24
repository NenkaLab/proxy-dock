//! Shared by proxydock.exe (tray + core) and proxydock-ui.exe (window): the line-JSON connection between them,
//! and the controller that runs in the core process.

pub mod controller;
pub mod ipc;
