// Copyleft 2017-2024 meh. <meh@schizofreni.co> | http://meh.schizofreni.co
// Copyright 2025 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

use nix::{
    libc::{IFNAMSIZ, c_char, c_int, ifreq, in6_ifreq, sockaddr, sockaddr_in6},
    net::if_::{InterfaceFlags, if_nametoindex},
    sys::{
        socket::{
            self, AddressFamily, SockFlag, SockProtocol, SockType, SockaddrIn, SockaddrIn6,
            SockaddrLike, SysControlAddr, sockopt::UtunIfname,
        },
        time::time_t,
    },
};

use std::{
    ffi::CStr,
    io::{self, Read, Write},
    mem,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6},
    os::fd::{AsRawFd, OwnedFd, RawFd},
};

use crate::{ConfigurableDevice, NetworkConfig, UnixTun};

const UTUN_CONTROL_NAME: &str = "com.apple.net.utun_control";

// don't perform DAD on this address (used only at first SIOC* call)
const IN6_IFF_NODAD: i32 = 0x0020;

// A lot of ioctl calls were deprecated or broken.
// The following are being used by ifconfig on macOS so we should stick to them.
// Notably `ifaliasreq` and `in6_aliasreq` are the preferred way to assign IPv4 and IPv6 on the interface.
// However since they are additive, it means that the one has to clear out other IPs.
// See: https://github.com/apple-oss-distributions/network_cmds/tree/main/ifconfig.tproj
nix::ioctl_write_ptr!(siocsifflags, 'i', 16, ifreq);
nix::ioctl_readwrite!(siocgifflags, 'i', 17, ifreq);
nix::ioctl_write_ptr!(siocsifmtu, 'i', 52, ifreq);
nix::ioctl_readwrite!(siocgifmtu, 'i', 51, ifreq);
nix::ioctl_write_ptr!(siocaifaddr, 'i', 26, ifaliasreq);
nix::ioctl_write_ptr!(siocaifaddr_in6, 'i', 26, in6_aliasreq);
nix::ioctl_write_ptr!(siocdifaddr, b'i', 25, ifreq);
nix::ioctl_write_ptr!(siocdifaddr_in6, b'i', 25, in6_ifreq);

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Failed to open tunnel device")]
    OpenTunnelDevice(#[source] nix::Error),

    #[error("Failed to create system control address")]
    CreateSysControlAddr(#[source] nix::Error),

    #[error("Failed to create a tun interface")]
    CreateTunInterface(#[source] nix::Error),

    #[error("Failed to open system control socket for IPv4")]
    OpenCtlInet(#[source] nix::Error),

    #[error("Failed to open system control socket for IPv6")]
    OpenCtlInet6(#[source] nix::Error),

    #[error("Failed to obtain tun name")]
    GetTunName(#[source] nix::Error),

    #[error("Failed to decode device name as utf8")]
    DecodeTunName,

    #[error("Failed to execute syscall to {}", _0)]
    SystemCall(&'static str, #[source] nix::Error),

    #[error("Failed to convert mtu {} to u16", _0)]
    MtuOverflow(i32),

    #[error("Failed to obtain interface index")]
    GetInterfaceIndex(nix::Error),

    #[error("Failed to obtain interface addresses")]
    GetInterfaceAddresses(nix::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct Device {
    name: String,
    tun: UnixTun<OwnedFd>,
    ctl_inet: OwnedFd,
    ctl_inet6: OwnedFd,
}

impl Device {
    pub fn new() -> Result<Self> {
        let tun_fd = socket::socket(
            AddressFamily::System,
            SockType::Datagram,
            SockFlag::empty(),
            SockProtocol::KextControl,
        )
        .map_err(Error::OpenTunnelDevice)?;

        let ctl_addr = SysControlAddr::from_name(tun_fd.as_raw_fd(), UTUN_CONTROL_NAME, 0)
            .map_err(Error::CreateSysControlAddr)?;

        socket::connect(tun_fd.as_raw_fd(), &ctl_addr).map_err(Error::CreateTunInterface)?;

        let name = socket::getsockopt(&tun_fd, UtunIfname)
            .map_err(Error::GetTunName)?
            .to_str()
            .map_err(|_| Error::DecodeTunName)?
            .to_owned();

        let ctl_inet = socket::socket(
            AddressFamily::Inet,
            SockType::Datagram,
            SockFlag::empty(),
            None,
        )
        .map_err(Error::OpenCtlInet)?;

        let ctl_inet6 = socket::socket(
            AddressFamily::Inet6,
            SockType::Datagram,
            SockFlag::empty(),
            None,
        )
        .map_err(Error::OpenCtlInet6)?;

        Ok(Self {
            name,
            tun: UnixTun::new(tun_fd),
            ctl_inet,
            ctl_inet6,
        })
    }

    pub fn has_packet_information(&self) -> bool {
        true
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns interface index
    pub fn interface_index(&self) -> Result<u32> {
        if_nametoindex(self.name.as_str()).map_err(Error::GetInterfaceIndex)
    }

    /// Mark interface as up and running.
    pub fn set_enabled(&self, enabled: bool) -> Result<()> {
        let mut interface_flags = self.get_interface_flags()?;
        interface_flags.set(InterfaceFlags::IFF_UP, enabled);
        if enabled {
            interface_flags.set(InterfaceFlags::IFF_RUNNING, true);
        }
        self.set_interface_flags(interface_flags)
    }

    /// Get device MTU
    pub fn mtu(&self) -> Result<u16> {
        let mut req = self.request();
        unsafe {
            siocgifmtu(self.ctl_inet.as_raw_fd(), &mut req)
                .map_err(|e| Error::SystemCall("siocgifmtu", e))?;

            req.ifr_ifru
                .ifru_mtu
                .try_into()
                .map_err(|_| Error::MtuOverflow(req.ifr_ifru.ifru_mtu))
        }
    }

    /// Set device MTU
    pub fn set_mtu(&self, mtu_value: u16) -> Result<()> {
        let mut req = self.request();
        unsafe {
            req.ifr_ifru.ifru_mtu = mtu_value as i32;
            siocsifmtu(self.ctl_inet.as_raw_fd(), &req)
                .map_err(|e| Error::SystemCall("siocsifmtu", e))?;
            Ok(())
        }
    }

    /// Add or change IPv4 address to the interface.
    /// Note that as of macOS 14.7, calling this method with the IP address that is already assigned on the interface
    /// will properly update destination and netmask.
    pub fn add_ipv4_address(
        &self,
        addr: Ipv4Addr,
        destination: Ipv4Addr,
        netmask: Ipv4Addr,
    ) -> Result<()> {
        let addr_sa = ipv4_address_to_sockaddr_in(addr);
        let destination_sa = ipv4_address_to_sockaddr_in(destination);
        let netmask_sa = ipv4_address_to_sockaddr_in(netmask);

        let mut req: ifaliasreq = unsafe { mem::zeroed() };
        self.copy_name_into(&mut req.ifran);
        req.addr = unsafe { *addr_sa.as_ptr() };
        req.destination = unsafe { *destination_sa.as_ptr() };
        req.mask = unsafe { *netmask_sa.as_ptr() };

        unsafe {
            siocaifaddr(self.ctl_inet.as_raw_fd(), &req)
                .map_err(|e| Error::SystemCall("siocaifaddr", e))?;
        }

        Ok(())
    }

    /// Add IPv6 address to the interface.
    /// Note that as of macOS 14.7, calling this method with the IP address that is already assigned on the interface is a no-op.
    /// If prefix length when differs will not be applied. The IPv6 address needs to be removed first.
    pub fn add_ipv6_address(&self, addr: Ipv6Addr, prefix_length: u8) -> Result<()> {
        let addr_sa = ipv6_address_to_sockaddr_in(addr);
        let prefix_mask_sa = ipv6_address_to_sockaddr_in(crate::net::ipv6_netmask(prefix_length));

        let mut req: in6_aliasreq = unsafe { mem::zeroed() };
        self.copy_name_into(&mut req.name);
        req.addr = *addr_sa.as_ref();
        req.prefixmask = *prefix_mask_sa.as_ref();
        req.lifetime.ia6t_pltime = u32::MAX;
        req.lifetime.ia6t_vltime = u32::MAX;
        req.flags = IN6_IFF_NODAD;

        unsafe {
            siocaifaddr_in6(self.ctl_inet6.as_raw_fd(), &req)
                .map_err(|e| Error::SystemCall("siocaifaddr_in6", e))?;
        }
        Ok(())
    }

    /// Remove IPv4 address from interface.
    pub fn remove_ipv4_addr(&self, addr: Ipv4Addr) -> Result<()> {
        let addr_sa = ipv4_address_to_sockaddr_in(addr);

        let mut req = self.request();
        req.ifr_ifru.ifru_addr = unsafe { *addr_sa.as_ptr() };

        unsafe {
            siocdifaddr(self.ctl_inet.as_raw_fd(), &req)
                .map_err(|e| Error::SystemCall("siocdifaddr", e))?;
        }

        Ok(())
    }

    /// Remove IPv6 address from interface.
    pub fn remove_ipv6_addr(&self, addr: Ipv6Addr) -> Result<()> {
        let addr_sa = SockaddrIn6::from(SocketAddrV6::new(addr, 0, 0, 0));

        let mut req = self.request_v6();
        req.ifr_ifru.ifru_addr = *addr_sa.as_ref();

        unsafe {
            siocdifaddr_in6(self.ctl_inet6.as_raw_fd(), &req)
                .map_err(|e| Error::SystemCall("siocdifaddr_in6", e))?;
        }

        Ok(())
    }

    /// Get all IP addresses assigned on interface.
    pub fn get_addresses(&self) -> Result<Vec<IpAddr>> {
        let ifaddrs = nix::ifaddrs::getifaddrs().map_err(Error::GetInterfaceAddresses)?;
        let mut ips = vec![];

        for ifaddr in ifaddrs {
            if ifaddr.interface_name == self.name {
                if let Some(addr_storage) = ifaddr.address {
                    if let Some(sa) = addr_storage.as_sockaddr_in() {
                        ips.push(IpAddr::V4(sa.ip()))
                    } else if let Some(sa6) = addr_storage.as_sockaddr_in6() {
                        ips.push(IpAddr::V6(sa6.ip()))
                    }
                }
            }
        }

        Ok(ips)
    }

    fn request(&self) -> ifreq {
        let mut req: ifreq = unsafe { mem::zeroed() };
        self.copy_name_into(&mut req.ifr_name);
        req
    }

    fn request_v6(&self) -> in6_ifreq {
        let mut req: in6_ifreq = unsafe { mem::zeroed() };
        self.copy_name_into(&mut req.ifr_name);
        req.ifr_ifru.ifru_flags = IN6_IFF_NODAD;
        req
    }

    fn copy_name_into(&self, buf: &mut [c_char; IFNAMSIZ]) {
        // Take IFNAMESIZ-1 bytes leaving space for nul terminator
        let mut bytes = self
            .name
            .as_bytes()
            .iter()
            .copied()
            .take(IFNAMSIZ - 1)
            .collect::<Vec<u8>>();
        // Add nul terminator
        bytes.push(0);

        // Safety: skip interior nul byte checks since the copy is made to fixed array
        let name_str = unsafe { CStr::from_bytes_with_nul_unchecked(&bytes) };

        // Safety: name_str is guaranteed to not exceed IFNAMESIZ
        unsafe { std::ptr::copy_nonoverlapping(name_str.as_ptr(), buf.as_mut_ptr(), bytes.len()) };
    }

    fn get_interface_flags(&self) -> Result<InterfaceFlags> {
        let mut req = self.request();

        unsafe {
            siocgifflags(self.ctl_inet.as_raw_fd(), &mut req)
                .map_err(|e| Error::SystemCall("siocgifflags", e))?;
        }

        Ok(InterfaceFlags::from_bits_retain(unsafe {
            req.ifr_ifru.ifru_flags as _
        }))
    }

    fn set_interface_flags(&self, interface_flags: InterfaceFlags) -> Result<()> {
        let mut req = self.request();
        req.ifr_ifru.ifru_flags = interface_flags.bits() as _;

        unsafe {
            siocsifflags(self.ctl_inet.as_raw_fd(), &req)
                .map_err(|e| Error::SystemCall("siocsifflags", e))?
        };

        Ok(())
    }
}

impl Read for Device {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.tun.read(buf)
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        self.tun.read_vectored(bufs)
    }
}

impl Write for Device {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.tun.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.tun.flush()
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        self.tun.write_vectored(bufs)
    }
}

impl ConfigurableDevice for Device {
    type Error = self::Error;

    fn set_network_config(&self, config: &NetworkConfig) -> Result<()> {
        // Find and remove interface addresses that are not in network configuration.
        let clear_ifaddrs = self.get_addresses()?.into_iter().filter(|addr| match addr {
            IpAddr::V4(ipv4) => {
                // No need to clear out IPv4 addresses since siocaifaddr supports updating netmask, destination for matching IPv4
                config.ipv4.address != *ipv4
            }
            IpAddr::V6(ipv6) => {
                // Preserve link local IPv6 (fe80:*) for consistency, since macOS assigns one when creating the first IPv6 alias.
                if ipv6.is_unicast_link_local() {
                    false
                } else {
                    // Always clear out IPv6 addresses sibnce siocaifaddr_in6 does not support updating the prefix length
                    true
                }
            }
        });

        for ifaddr in clear_ifaddrs {
            match ifaddr {
                IpAddr::V4(ipv4) => {
                    self.remove_ipv4_addr(ipv4)?;
                }
                IpAddr::V6(ipv6) => {
                    self.remove_ipv6_addr(ipv6)?;
                }
            }
        }

        self.add_ipv4_address(
            config.ipv4.address,
            config.ipv4.destination,
            config.ipv4.netmask,
        )?;
        if let Some(ipv6) = config.ipv6.as_ref() {
            self.add_ipv6_address(ipv6.address, ipv6.prefix_length)?;
        }
        self.set_mtu(config.mtu)?;
        self.set_enabled(true)?;
        Ok(())
    }
}

impl AsRawFd for Device {
    fn as_raw_fd(&self) -> RawFd {
        self.tun.as_raw_fd()
    }
}

#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Copy, Clone)]
struct in6_aliasreq {
    pub name: [c_char; IFNAMSIZ],
    pub addr: sockaddr_in6,
    pub dstaddr: sockaddr_in6,
    pub prefixmask: sockaddr_in6,
    pub flags: c_int,
    pub lifetime: in6_addrlifetime,
}

#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Copy, Clone)]
struct in6_addrlifetime {
    pub ia6t_expire: time_t,
    pub ia6t_preferred: time_t,
    pub ia6t_vltime: u32,
    pub ia6t_pltime: u32,
}

#[allow(non_camel_case_types)]
#[repr(C)]
#[derive(Copy, Clone)]
struct ifaliasreq {
    pub ifran: [c_char; IFNAMSIZ],
    pub addr: sockaddr,
    pub destination: sockaddr,
    pub mask: sockaddr,
}

fn ipv4_address_to_sockaddr_in(addr: Ipv4Addr) -> SockaddrIn {
    SockaddrIn::from(SocketAddrV4::new(addr, 0))
}

fn ipv6_address_to_sockaddr_in(addr: Ipv6Addr) -> SockaddrIn6 {
    SockaddrIn6::from(SocketAddrV6::new(addr, 0, 0, 0))
}

#[cfg(test)]
pub mod tests {

    use super::*;
    use crate::{Ipv4Config, Ipv6Config};

    #[test]
    #[ignore]
    fn setup_tun_interface() {
        let dev = Device::new().unwrap();

        dev.set_network_config(&NetworkConfig {
            ipv4: Ipv4Config {
                address: "10.10.0.10".parse().unwrap(),
                destination: "10.10.0.1".parse().unwrap(),
                netmask: "255.255.255.0".parse().unwrap(),
            },
            ipv6: Some(Ipv6Config {
                address: "fdea:e3cd:1c32::".parse().unwrap(),
                prefix_length: 120,
            }),
            mtu: 1500,
        })
        .unwrap();

        ifconfig(&dev.name());
    }

    fn ifconfig(name: &str) {
        let ch = std::process::Command::new("ifconfig")
            .arg(name)
            .spawn()
            .unwrap();
        let output = String::from_utf8(ch.wait_with_output().unwrap().stdout).unwrap();

        println!("Output: {output}");
    }
}
