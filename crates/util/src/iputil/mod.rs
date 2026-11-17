//! Go `util/iputil`: IP parsing with version detection and private-network validation, plus
//! the Go `net` and `net/netip` semantics they rest on.
//!
//! ```
//! use pbs_util::iputil::{
//!     IpValidator, IpVersion, PublicNetworkIpValidator,
//!     net::{self, parse_cidr},
//!     parse_ip,
//! };
//!
//! // Go: iputil.ParseIP decides the version by the presence of ':' in the text, so an
//! // IPv4-mapped address is IPv6 although it prints in dotted decimal.
//! let (ip, ver) = parse_ip("::ffff:10.0.0.1").ok_or("not an IP")?;
//! assert_eq!(ver, IpVersion::V6);
//! assert_eq!(ip.to_string(), "10.0.0.1");
//!
//! let validator = PublicNetworkIpValidator {
//!     ipv4_private_networks: vec![parse_cidr("10.0.0.0/8")?.1],
//!     ipv6_private_networks: vec![parse_cidr("fc00::/7")?.1],
//! };
//! // Checked against the IPv6 networks, the mapped address is public (Go's IPNet.Contains
//! // compares it in its 4-byte form, which no 16-byte network contains).
//! assert!(validator.is_valid(&ip, ver));
//! assert!(!validator.is_valid(&ip, IpVersion::V4));
//!
//! // privacy.scrubIP: net.ParseIP(ip).Mask(net.CIDRMask(ones, bits)).String().
//! let masked = net::mask_ip(
//!     net::parse_ip("2001:db8:85a3:8d3:1319:8a2e:370:7348").as_ref(),
//!     net::cidr_mask(56, 128).as_ref(),
//! );
//! assert_eq!(net::ip_string(masked.as_ref()), "2001:db8:85a3:800::");
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod net;
pub mod netip;
mod parse;
mod validator;

pub use parse::{
    IPV4_BIT_SIZE, IPV4_DEFAULT_MASKING_BIT_SIZE, IPV6_BIT_SIZE, IPV6_DEFAULT_MASKING_BIT_SIZE,
    IpVersion, parse_ip, parse_ip_bytes,
};
pub use validator::{IpValidator, PublicNetworkIpValidator, VersionIpValidator};

#[cfg(test)]
mod tests;
