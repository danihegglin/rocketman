//! Frontend-independent core of rocketman: OpenAPI collections, the workspace
//! folder, request execution and the editing session both the desktop app and
//! the terminal UI drive.

pub mod http;
pub mod openapi;
pub mod session;
pub mod workspace;
