//! Tempest's design system for its web UIs (D-076): themes, number formatting, components,
//! form controls, charts and the chunk map, styled by `style/tempest.css`.
//!
//! An app links the stylesheet and copies `style/fonts` beside it. Components follow the
//! Leptos guidelines in specs/interfaces.md: logic lives in plain functions with tests, and
//! views stay thin.

pub mod chart;
pub mod chunkmap;
pub mod components;
pub mod form;
pub mod format;
pub mod theme;

pub use chunkmap::{ChunkCell, ChunkMap};
pub use theme::{Look, Theme};
