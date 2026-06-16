//! The agent engine and its supporting pieces: the 4-step run loop, per-step
//! configuration, real-time events, and the background worker.

pub mod choreography;
pub mod engine;
pub mod events;
pub mod inner_life;
pub mod steps;
pub mod worker;

pub use events::{EventBus, JobEvent};
