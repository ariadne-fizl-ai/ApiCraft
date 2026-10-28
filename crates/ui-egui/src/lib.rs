//! The Fizl ApiCraft user interface, written once with egui. The desktop app
//! runs it natively; the web app compiles it to WebAssembly. Each host
//! provides a [`Platform`] for networking, storage, files and sockets.

mod actions;
mod app;
pub mod brand;
mod highlight;
mod icons;
pub mod platform;
mod sample;
#[cfg(test)]
mod tests;
mod textmenu;
pub mod theme;
mod undo;
mod views;

pub use app::ApiCraftApp;
pub use platform::{run_job, CommandPurpose, FolderContents, FolderPurpose, Inbox, Job, Level, Msg, Platform, SourceCtx, Status, WsConn, WsEvent};

/// The name used for windows, storage keys and file locations.
pub const APP_NAME: &str = "Fizl ApiCraft";
