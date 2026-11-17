//! Code shared by `finder` (UI) and `finderd` (daemon).

pub mod paths;
#[cfg(windows)]
pub mod pipe;
pub mod proto;
pub mod settings;
