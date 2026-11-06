//! Authentication is a capability boundary, never a learned graph decision.
#![forbid(unsafe_code)]
mod app;
mod crypto;
mod engine;
mod graph;
mod healing;
mod language;
mod model;
mod retrieval;
mod store;
pub use app::{App, Mode, RecallResult, SessionPolicy};
pub use model::Reply;
pub use store::Import;
#[cfg(test)]
mod tests;
