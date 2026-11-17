//! Decoded GPP sections. Go: package `github.com/prebid/go-gpp/sections` and its per-state
//! sub-packages.
//!
//! go-gpp v0.2.0 decodes only the US state sections below; every other section ID (including
//! TCF EU v2 and US Privacy v1) is kept as an undecoded [`GenericSection`](crate::GenericSection).

mod section_util;
pub mod uspca;
pub mod uspco;
pub mod uspct;
pub mod uspnat;
pub mod usput;
pub mod uspva;

pub use section_util::{CommonUsCoreSegment, CommonUsGpcSegment};
