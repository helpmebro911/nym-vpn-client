// Copyright 2025 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::net::Ipv4Addr;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use nym_ip_packet_requests::IpPair;
use tun::{AsyncDevice, Device};

#[cfg(target_os = "android")]
use crate::tunnel_provider::android::AndroidTunProvider;
#[cfg(target_os = "ios")]
use crate::tunnel_provider::ios::OSTunProvider;

use super::ipv6;

#[cfg(any(target_os = "ios", target_os = "android"))]
use super::tun_name;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("failed to create tunnel device")]
    CreateTunDevice(#[source] tun::Error),

    #[cfg(target_os = "ios")]
    #[error("failed to locate tun device")]
    LocateTunDevice(#[source] std::io::Error),

    #[error("failed to get tunnel device name")]
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    GetTunDeviceName(#[source] tun::Error),

    #[error("failed to get tunnel device name")]
    #[cfg(any(target_os = "ios", target_os = "android"))]
    GetTunDeviceName(#[source] tun_name::GetTunNameError),

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    #[error("failed to set tunnel device ipv6 address")]
    SetTunDeviceIpv6Addr(#[source] std::io::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Wrapper around ``tun::AsyncDevice`` adding missing functionality.
pub struct TunnelDevice {
    inner: AsyncDevice,
}

impl TunnelDevice {
    /// Create new tunnel device
    ///
    /// On Windows there is no support for assigning IPv6 addresses yet.
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    pub fn new(
        #[cfg(windows)] wintun_device_name: &str,
        interface_addresses: IpPair,
        destination: Option<Ipv4Addr>,
        mtu: u16,
    ) -> Result<Self> {
        let mut tun_config = tun::Configuration::default();

        // rust-tun uses the same name for tunnel type.
        #[cfg(windows)]
        tun_config.name(wintun_device_name);

        tun_config
            .address(interface_addresses.ipv4)
            .netmask(Ipv4Addr::BROADCAST)
            .mtu(i32::from(mtu))
            .up();

        if let Some(destination) = destination {
            tun_config.destination(destination);
        }

        #[cfg(target_os = "linux")]
        tun_config.platform(|platform_config| {
            platform_config.packet_information(false);
        });

        let inner = tun::create_as_async(&tun_config).map_err(Error::CreateTunDevice)?;

        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            let tun_name = inner.get_ref().name().map_err(Error::GetTunDeviceName)?;
            ipv6::set_ipv6_addr(&tun_name, interface_addresses.ipv6)
                .map_err(Error::SetTunDeviceIpv6Addr)?;
        }

        Ok(Self { inner })
    }

    /// Create new tunnel device.
    ///
    /// On iOS only one tunnel device is possible. It's automatically located automatically.
    /// On Android
    #[cfg(any(target_os = "ios", target_os = "android"))]
    pub fn new(
        &self,
        packet_tunnel_settings: tunnel_provider::tunnel_settings::TunnelSettings,
        #[cfg(target_os = "ios")] tun_provider: OsTunProvider,
        #[cfg(target_os = "android")] tun_provider: AndroidTunProvider,
    ) {
        #[cfg(target_os = "ios")]
        let owned_tun_fd =
            tunnel_provider::ios::interface::get_tun_fd().map_err(Error::LocateTunDevice)?;

        #[cfg(target_os = "android")]
        let owned_tun_fd = {
            let raw_tun_fd = self
                .tun_provider
                .configure_tunnel(packet_tunnel_settings.into_tunnel_network_settings())
                .map_err(|e| Error::ConfigureTunnelProvider(e.to_string()))?;
            unsafe { OwnedFd::from_raw_fd(raw_tun_fd) }
        };

        let mut tun_config = tun::Configuration::default();
        tun_config.raw_fd(owned_tun_fd.as_raw_fd());

        #[cfg(target_os = "ios")]
        {
            self.tun_provider
                .set_tunnel_network_settings(packet_tunnel_settings.into_tunnel_network_settings())
                .await
                .map_err(|e| Error::ConfigureTunnelProvider(e.to_string()))?
        }

        let device = tun::create_as_async(&tun_config).map_err(Error::CreateTunDevice)?;

        // Consume the owned fd, since the device is now responsible for closing the underlying raw fd.
        let _ = owned_tun_fd.into_raw_fd();

        Ok(device)
    }

    /// Returns tunnel device interface name
    pub fn name(&self) -> Result<String> {
        #[cfg(any(target_os = "ios", target_os = "android"))]
        {
            let tun_fd = unsafe { BorrowedFd::borrow_raw(self.inner.get_ref().as_raw_fd()) };
            tun_name::get_tun_name(&tun_fd).map_err(Error::GetTunDeviceName)
        }

        #[cfg(not(all(target_os = "ios", target_os = "android")))]
        {
            self.inner.get_ref().name().map_err(Error::GetTunDeviceName)
        }
    }

    pub fn into_inner(self) -> AsyncDevice {
        self.inner
    }
}
