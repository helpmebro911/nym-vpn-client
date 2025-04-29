// Copyleft 2017-2024 meh. <meh@schizofreni.co> | http://meh.schizofreni.co
// Copyright 2025 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

use std::{net::IpAddr, path::PathBuf, sync::Arc};

use wintun::Session;

use crate::{ConfigurableDevice, NetworkConfig};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Failed to load wintun from {0}")]
    LoadWintun(PathBuf, #[source] wintun::Error),

    #[error("Failed to load wintun from current dir")]
    LoadWintunFromCurrentDir(#[source] wintun::Error),

    #[error("Failed to create a tun interface")]
    CreateTunInterface(#[source] wintun::Error),

    #[error("Failed to obtain tun name")]
    GetTunName(#[source] wintun::Error),

    #[error("Failed to get MTU")]
    GetMtu(#[source] wintun::Error),

    #[error("Failed to set MTU")]
    SetMtu(#[source] wintun::Error),

    #[error("Failed to set network interface addresses")]
    SetNetworkInterfaceAddresses(#[source] wintun::Error),

    #[error("Failed to start wintun session")]
    StartSession(#[source] wintun::Error),

    #[error("Failed to convert mtu {0} to u16")]
    MTUOverflowU16(usize),
}

#[derive(Debug)]
pub struct WintunConfig {
    /// Path to `wintun.dll`
    /// Pass `None` to load from current directory
    pub wintun_path: Option<PathBuf>,

    /// Decorative name of the adapter
    pub adapter_name: String,

    /// Tunnel type (i.e WireGuard)
    pub tunnel_type: String,

    /// Tunnel adapter guid
    pub network_guid: Option<u128>,
}

pub struct Device {
    adapter: Arc<wintun::Adapter>,
    name: String,
}

impl Device {
    pub fn new(wintun_config: &WintunConfig) -> Result<Self> {
        let wintun = match wintun_config.wintun_path {
            Some(wintun_path) => unsafe {
                wintun::load_from_path(&wintun_path)
                    .map_err(|e| Error::LoadWintun(wintun_path, e))?
            },
            None => unsafe { wintun::load().map_err(Error::LoadWintunFromCurrentDir)? },
        };

        let adapter =
            wintun::Adapter::open(&wintun, &wintun_config.adapter_name).or_else(|_| {
                wintun::Adapter::create(
                    &wintun,
                    &wintun_config.adapter_name,
                    &wintun_config.tunnel_type,
                    wintun_config.network_guid,
                )
                .map_err(Error::CreateTunInterface)
            })?;
        let name = adapter.get_name().map_err(Error::GetTunName)?;

        Ok(Self { adapter, name })
    }

    pub fn has_packet_information(&self) -> bool {
        false
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn mtu(&self) -> Result<u16> {
        let mtu = self.adapter.get_mtu().map_err(Error::GetMtu)?;
        mtu.try_into().map_err(|_| Error::MTUOverflowU16(mtu))
    }

    pub fn set_mtu(&self, mtu: u16) -> Result<()> {
        self.adapter.set_mtu(mtu as usize).map_err(Error::SetMtu)
    }

    pub fn set_interface_addresses(
        &self,
        address: IpAddr,
        netmask: IpAddr,
        destination: Option<IpAddr>,
    ) -> Result<()> {
        self.adapter
            .set_network_addresses_tuple(address, netmask, destination)
            .map_err(Error::SetNetworkInterfaceAddresses)
    }

    pub(crate) fn start_session(&self) -> Result<Session> {
        self.adapter
            .start_session(wintun::MAX_RING_CAPACITY)
            .map_err(Error::StartSession)
    }
}

impl ConfigurableDevice for Device {
    type Error = self::Error;

    fn set_network_config(&self, config: &NetworkConfig) -> Result<()> {
        self.set_mtu(config.mtu)?;
        self.set_interface_addresses(
            IpAddr::V4(config.ipv4.address),
            IpAddr::V4(config.ipv4.netmask),
            Some(IpAddr::V4(config.ipv4.destination)),
        )?;
        if let Some(ipv6) = config.ipv6.as_ref() {
            self.set_interface_addresses(
                IpAddr::V6(ipv6.address),
                IpAddr::V6(super::ipv6_netmask(ipv6.prefix_length)),
                None,
            )?;
        }
        Ok(())
    }
}
