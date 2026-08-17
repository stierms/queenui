//! Platform-independent QueenUI engine and game building blocks.
//!
//! The desktop shell and the headless runner both depend on this crate. Keep
//! windowing, Tauri commands, and transport concerns outside this boundary.

mod campaign;
pub mod diagnostics;
pub mod enginelog;
pub mod history;
pub mod lichess;
pub mod models;
pub mod opening_book;
pub mod position;
mod runtime;
pub mod storage;
pub mod uci;

#[cfg(test)]
pub(crate) mod test_support;

pub use runtime::*;
