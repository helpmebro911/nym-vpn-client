// Copyleft 2017-2024 meh. <meh@schizofreni.co> | http://meh.schizofreni.co
// Copyright 2025 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

use std::net::Ipv6Addr;

/// Returns IPv6 netmask with the prefix length.
pub fn ipv6_netmask(prefix_length: u8) -> Ipv6Addr {
    const IPV6_BITS: u8 = 128;
    let bits = if prefix_length >= IPV6_BITS {
        u128::MAX
    } else {
        u128::MAX
            .checked_shl((IPV6_BITS - prefix_length) as u32)
            .unwrap_or_default()
    };
    Ipv6Addr::from(bits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_ipv6_netmask_prefix_24() {
        assert_eq!(ipv6_netmask(24), Ipv6Addr::from_str("ffff:ff00::").unwrap());
    }

    #[test]
    fn test_ipv6_netmask_prefix_0() {
        assert_eq!(ipv6_netmask(0), Ipv6Addr::from_str("::").unwrap());
    }

    #[test]
    fn test_ipv6_netmask_prefix_128_or_greater() {
        assert_eq!(
            ipv6_netmask(128),
            Ipv6Addr::from_str("ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff").unwrap()
        );
        assert_eq!(
            ipv6_netmask(255),
            Ipv6Addr::from_str("ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff").unwrap()
        );
    }
}
