//! Go: `compress/`: the `flate` and `gzip` packages, which share the [`goio`] helpers.

mod goio;
#[cfg(test)]
mod testutil;

pub mod flate;
pub mod gzip;
