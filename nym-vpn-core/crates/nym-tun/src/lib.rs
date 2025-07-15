// Copyleft 2017-2024 meh. <meh@schizofreni.co> | http://meh.schizofreni.co
// Copyright 2025 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

mod r#async;
#[cfg(any(target_os = "windows", target_os = "macos"))]
mod net;
mod platform;
#[cfg(unix)]
mod unix_tun;

use std::net::{Ipv4Addr, Ipv6Addr};
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(any(target_os = "android"))]
use std::os::fd::FromRawFd;
#[cfg(any(target_os = "ios", target_os = "android"))]
use std::os::fd::OwnedFd;

#[cfg(unix)]
use nix::fcntl;

pub use r#async::{AsyncDevice, TunPacket, TunPacketCodec};
pub use platform::{Device, Error as PlatformError};

#[cfg(target_os = "windows")]
pub use platform::WintunConfig;
#[cfg(unix)]
pub use unix_tun::UnixTun;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Failed to create device")]
    CreateDevice(#[source] PlatformError),

    #[error("Failed to set network configuration")]
    SetNetworkConfig(#[source] PlatformError),

    #[error("Failed to build device")]
    BuildDevice(#[from] DeviceBuilderError),

    #[error("Io error")]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Clone)]
pub struct Ipv4Config {
    pub address: Ipv4Addr,
    pub destination: Ipv4Addr,
    pub netmask: Ipv4Addr,
}

#[derive(Debug, Clone)]
pub struct Ipv6Config {
    pub address: Ipv6Addr,
    pub prefix_length: u8,
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[derive(Debug, Clone)]
struct NetworkConfig {
    ipv4: Ipv4Config,
    ipv6: Option<Ipv6Config>,
    mtu: u16,
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
trait ConfigurableDevice {
    type Error;

    fn set_network_config(&self, config: &NetworkConfig) -> Result<(), Self::Error>;
}

#[derive(Default)]
pub struct DeviceBuilder {
    /// IPv4 configuration
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    ipv4: Option<Ipv4Config>,

    /// IPv6 configuration (optional)
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    ipv6: Option<Ipv6Config>,

    /// MTU
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    mtu: Option<u16>,

    #[cfg(windows)]
    wintun: Option<WintunConfig>,

    /// Raw tunnel device file descriptor
    ///
    /// # Cross-platform considerations
    ///
    /// On iOS the fd is borrowed and never closed on drop. This is because the system owns the fd and takes care of closing it.
    /// On Android the ownership is transferred and the fd is closed on drop.
    #[cfg(any(target_os = "ios", target_os = "android"))]
    tun_fd: Option<OwnedFd>,
}

impl DeviceBuilder {
    /// Set IPv4 configuration
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    pub fn ipv4(&mut self, ipv4_config: Ipv4Config) -> &mut Self {
        self.ipv4 = Some(ipv4_config);
        self
    }

    /// Set IPv6 configuration
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    pub fn ipv6(&mut self, ipv6_config: Ipv6Config) -> &mut Self {
        self.ipv6 = Some(ipv6_config);
        self
    }

    /// Set desired MTU
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    pub fn mtu(&mut self, mtu: u16) -> &mut Self {
        self.mtu = Some(mtu);
        self
    }
    /// Set wintun configuration
    #[cfg(windows)]
    pub fn wintun(&mut self, wintun_config: WintunConfig) -> &mut Self {
        self.wintun = Some(wintun_config);
        self
    }

    /// Set tunnel file descriptor
    /// The descriptor will be properly closed on drop if builder fails to create a device
    #[cfg(any(target_os = "ios", target_os = "android"))]
    pub fn tun_fd(&mut self, tun_fd: OwnedFd) -> &mut Self {
        self.tun_fd = Some(tun_fd);
        self
    }

    /// Build device that can be used in synchronous contexts
    pub fn build(self) -> Result<Device> {
        let device = Device::new(
            #[cfg(any(target_os = "android", target_os = "ios"))]
            self.tun_fd.ok_or(DeviceBuilderError::TunFdUnset)?,
            #[cfg(windows)]
            &self.wintun.ok_or(Error::WintunConfigUnset),
        )
        .map_err(Error::CreateDevice)?;

        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            device
                .set_network_config(&NetworkConfig {
                    ipv4: self.ipv4.ok_or(DeviceBuilderError::Ipv4Unset)?,
                    ipv6: self.ipv6,
                    mtu: self.mtu.ok_or(DeviceBuilderError::MtuUnset)?,
                })
                .map_err(Error::SetNetworkConfig)?;
        }

        Ok(device)
    }

    /// Build device that can be used in asynchronous contexts
    pub fn build_as_async(self) -> Result<AsyncDevice> {
        AsyncDevice::new(self.build()?)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum DeviceBuilderError {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    #[error("Tun fd is not set")]
    TunFdUnset,

    #[cfg(windows)]
    #[error("Wintun config is not set")]
    WintunConfigUnset,

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    #[error("IPv4 config is not set")]
    Ipv4Unset,

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    #[error("MTU is not set")]
    MtuUnset,
}

#[cfg(unix)]
pub trait UnixDevice {
    fn set_nonblock(&self) -> std::io::Result<()>;
}

#[cfg(unix)]
impl UnixDevice for Device {
    fn set_nonblock(&self) -> std::io::Result<()> {
        let arg = fcntl::FcntlArg::F_SETFL(fcntl::OFlag::O_RDWR | fcntl::OFlag::O_NONBLOCK);
        let borrowed_fd = unsafe { std::os::fd::BorrowedFd::borrow_raw(self.as_raw_fd()) };
        fcntl::fcntl(borrowed_fd, arg)?;
        Ok(())
    }
}
