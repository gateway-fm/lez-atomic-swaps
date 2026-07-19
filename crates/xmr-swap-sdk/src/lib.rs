//! Monero spend-key-share protocol adapter for LEZ atomic swaps.
//!
//! The first progressive M4 slice exposes a byte-stable boundary around a
//! maintained cross-curve DLEQ implementation. It does not implement curve
//! arithmetic and it does not claim production cryptographic acceptance.

mod cross_curve;

pub use cross_curve::{
    CROSS_CURVE_DLEQ_SCHEMA_V1, CrossCurveDleqError, CrossCurveDleqProofV1, CrossCurveScalar,
};
