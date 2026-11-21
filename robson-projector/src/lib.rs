//! Robson Projector
//!
//! Applies events from the event log to projection tables for current state.
//! This is the read-side of Event Sourcing - building materialized views
//! from the append-only event log.

pub mod accounting;
pub mod apply;
pub mod error;
pub mod handlers;

mod types;

pub use accounting::AccountingStep;
pub use apply::{
    apply_event_to_projections, apply_event_to_projections_with, is_repairable_position_event,
};
pub use error::{ProjectionError, Result};
