//! Code shared by `finder` (UI) and `finderd` (daemon).

pub mod autostart;
pub mod fold;
pub mod hotkey;
pub mod launch;
pub mod paths;
#[cfg(windows)]
pub mod pipe;
pub mod process;
pub mod proto;
pub mod settings;
