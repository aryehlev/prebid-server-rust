//! Go: `util/iputil/validator.go`.

use std::net::IpAddr;

use ipnet::IpNet;

use super::{IpVersion, net};

/// Go: `iputil.IPValidator`.
pub trait IpValidator {
    /// Go: `IsValid`: whether the address of the given version is acceptable.
    fn is_valid(&self, ip: &IpAddr, ver: IpVersion) -> bool;
}

impl<T: IpValidator + ?Sized> IpValidator for &T {
    fn is_valid(&self, ip: &IpAddr, ver: IpVersion) -> bool {
        (**self).is_valid(ip, ver)
    }
}

/// Go: `iputil.PublicNetworkIPValidator`: an address is valid when it is in none of the
/// private networks of its version; an address of unknown version is never valid.
///
/// The networks come from `request_validation.ipv4_private_networks` and
/// `ipv6_private_networks`. Go's `IPNet.Contains` compares an IPv4-mapped address in its
/// 4-byte form, so with version [`IpVersion::V6`] (any text containing `:`) such an address is
/// only matched by a network written in IPv4-mapped form (`::ffff:10.0.0.0/104`); see
/// [`net::contains`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PublicNetworkIpValidator {
    /// Go: `IPv4PrivateNetworks`.
    pub ipv4_private_networks: Vec<IpNet>,
    /// Go: `IPv6PrivateNetworks`.
    pub ipv6_private_networks: Vec<IpNet>,
}

impl IpValidator for PublicNetworkIpValidator {
    fn is_valid(&self, ip: &IpAddr, ver: IpVersion) -> bool {
        let private_networks = match ver {
            IpVersion::V4 => &self.ipv4_private_networks,
            IpVersion::V6 => &self.ipv6_private_networks,
            IpVersion::Unknown => return false,
        };
        !private_networks.iter().any(|n| net::contains(n, *ip))
    }
}

/// Go: `iputil.VersionIPValidator`: an address is valid when its version is `version`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct VersionIpValidator {
    /// Go: `Version`.
    pub version: IpVersion,
}

impl IpValidator for VersionIpValidator {
    fn is_valid(&self, _ip: &IpAddr, ver: IpVersion) -> bool {
        ver == self.version
    }
}
