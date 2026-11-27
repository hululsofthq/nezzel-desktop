//! Candidate admission, manifest inspection, download hand-off and capture windows.

pub mod candidate;
pub mod handoff;
pub mod manifest;
pub(crate) mod route;

pub(crate) mod adblock;
pub(crate) mod auto;
pub(crate) mod commands;
mod geo;
pub(crate) mod hidden_signin;
mod keys;
mod layering;
#[cfg(feature = "catch-matrix")]
pub mod matrix;
mod native;
mod page_video;
mod proxy;
pub(crate) mod site_tools;
mod state;
mod tabs;
mod thumbnail;
pub(crate) mod window;

pub use commands::{Handoff, HandoffError, HandoffRequest};
pub use handoff::{CatchFormat, FormatLabel};
