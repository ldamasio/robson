//! Robson Projector
//!
//! Applies events from the event log to projection tables for current state.
//! This is the read-side of Event Sourcing - building materialized views
//! from the append-only event log.

pub mod apply;
pub mod error;
pub mod handlers;

mod types;

pub use apply::apply_event_to_projections;
pub use error::{ProjectionError, Result};
