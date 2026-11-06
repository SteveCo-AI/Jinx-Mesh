//! Authentication is a capability boundary, never a learned graph decision.
#![forbid(unsafe_code)]
mod app;
mod crypto;
mod engine;
mod gate;
mod graph;
mod healing;
mod journal;
mod language;
mod model;
mod mutation;
mod retrieval;
mod store;
pub use app::{App, Mode, RecallResult, SessionPolicy};
pub use model::Reply;
pub use store::Import;
#[cfg(test)]
mod tests;
