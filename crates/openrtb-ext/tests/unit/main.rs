//! Ports of the Go unit tests of `openrtb_ext` (but `request_wrapper_test.go` and the convert
//! tests, which a separate port covers, and the bidder name tests that register aliases, which
//! `tests/bidder_names` runs alone because they change the package's state).
//!
//! Each test names the Go test it ports. Where Go checks that a copy holds new pointers
//! (`assert.NotSame`), the Rust copy owns its data, and the test checks the values.

#[cfg(test)]
pub mod alternatebiddercodes;
#[cfg(test)]
pub mod bid;
#[cfg(test)]
pub mod bidders;
#[cfg(test)]
pub mod deal_tier;
#[cfg(test)]
pub mod device;
#[cfg(test)]
pub mod floors;
#[cfg(test)]
pub mod imp_appnexus;
#[cfg(test)]
pub mod multibid;
#[cfg(test)]
pub mod regs;
#[cfg(test)]
pub mod request;
#[cfg(test)]
pub mod site;
#[cfg(test)]
pub mod supply_chain;
#[cfg(test)]
pub mod support;
