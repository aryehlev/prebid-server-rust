//! Prebid Server's host configuration.
//!
//! The [`loader`] reads the configuration as `config.SetupViper` and the decode of `config.New`
//! do in Go (config/config.go:936-1415 and :756-760).

pub mod loader;
