//! Fast paired-coordinate CPU candidate generation.

mod engine;
mod field;
mod point;

pub(crate) use engine::{OffsetTable, SymmetryEngine};
