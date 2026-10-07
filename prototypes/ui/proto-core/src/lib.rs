//! Shared core for the M0 UI prototypes.
//!
//! - [`sim`]: a deterministic simulation of one Tempest client's state.
//! - [`format`]: display formatting shared by every prototype.
//! - [`chart`]: chart geometry (scales, ticks, SVG paths), so each framework
//!   only handles rendering and interaction.
//!
//! Nothing here is part of the real Tempest crates.

pub mod chart;
pub mod format;
pub mod sim;

pub use sim::{
    Bundle, BundleState, Cell, Decision, DecisionKind, Path, Peer, Sample, World, GRID_COLS,
    GRID_ROWS,
};
