// Copyleft 2017-2024 meh. <meh@schizofreni.co> | http://meh.schizofreni.co
// Copyright 2025 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

use std::{
    ffi::CStr,
    io::{self, Read, Write},
    mem,
    net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6},
    os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd},
};

use nix::{
    fcntl,
    libc::{self, IFNAMSIZ, c_char, ifreq, in6_ifreq},
    net::if_::InterfaceFlags,
    sys::socket::{self, AddressFamily, SockFlag, SockType, SockaddrIn, SockaddrIn6, SockaddrLike},
};

use crate::{ConfigurableDevice, NetworkConfig, UnixTun};

// This call takes a pointer to ifreq but must be defined as taking integer.
nix::ioctl_write_int!(tunsetiff, b'T', 202);
nix::ioctl_read_bad!(siocgifflags, libc::SIOCGIFFLAGS, ifreq);
nix::ioctl_write_ptr_bad!(siocsifflags, libc::SIOCSIFFLAGS, ifreq);
nix::ioctl_read_bad!(siocgifmtu, libc::SIOCGIFMTU, ifreq);
nix::ioctl_write_ptr_bad!(siocsifmtu, libc::SIOCSIFMTU, ifreq);
nix::ioctl_write_ptr_bad!(siocsifaddr, libc::SIOCSIFADDR, ifreq);
nix::ioctl_write_ptr_bad!(siocsifdstaddr, libc::SIOCSIFDSTADDR, ifreq);
nix::ioctl_write_ptr_bad!(siocsifnetmask, libc::SIOCSIFNETMASK, ifreq);
nix::ioctl_write_ptr_bad!(siogifindex, libc::SIOGIFINDEX, ifreq);
nix::ioctl_write_ptr_bad!(siocsifaddr_in6, libc::SIOCSIFADDR, in6_ifreq);

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Failed to open a tun control socket")]
    OpenTunControlSocket(#[source] nix::Error),

    #[error("Failed to create a tun interface")]
    CreateTunInterface(#[source] nix::Error),

    #[error("Failed to open system control socket for IPv4")]
    OpenCtlInet(#[source] nix::Error),

    #[error("Failed to open system control socket for IPv6")]
    OpenCtlInet6(#[source] nix::Error),

    #[error("Invalid tunnel device name")]
    InvalidTunName,

    #[error("Failed to decode device name as utf8")]
    DecodeTunName,

    #[error("Failed to execute syscall to {}", _0)]
    SystemCall(&'static str, #[source] nix::Error),

    #[error("Failed to convert mtu {} to u16", _0)]
    MTUOverflowU16(i32),
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct Device {
    name: String,
    tun: UnixTun<OnwedFd>,
    ctl_inet: OwnedFd,
    ctl_inet6: OwnedFd,
}

impl Device {
    pub fn new() -> Result<Self> {
        let tun_fd = {
            let fd = fcntl::open(
                "/dev/net/tun",
                fcntl::OFlag::O_RDWR,
                nix::sys::stat::Mode::empty(),
            )
            .map_err(Error::OpenTunControlSocket)?;

            unsafe { OwnedFd::from_raw_fd(fd) }
        };

        let mut req: ifreq = unsafe { mem::zeroed() };
        let flags = InterfaceFlags::IFF_TUN | InterfaceFlags::IFF_NO_PI;

        req.ifr_ifru.ifru_flags = flags.bits() as i16;
        unsafe {
            tunsetiff(tun_fd.as_raw_fd(), &mut req as *mut _ as _)
                .map_err(Error::CreateTunInterface)?
        };

        let name = CStr::from_bytes_until_nul(&req.ifr_name.map(|byte| byte as u8))
            .map_err(|_| Error::InvalidTunName)?
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
        false
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_enabled(&self, value: bool) -> Result<()> {
        let mut req = self.request();

        let _ = unsafe {
            siocgifflags(self.ctl_inet.as_raw_fd(), &mut req)
                .map_err(|e| Error::SystemCall("siocgifflags", e))?
        };

        let mut interface_flags =
            InterfaceFlags::from_bits_retain(unsafe { req.ifr_ifru.ifru_flags as _ });

        if value {
            interface_flags.set(InterfaceFlags::IFF_UP, true);
            interface_flags.set(InterfaceFlags::IFF_RUNNING, true);
        } else {
            interface_flags.set(InterfaceFlags::IFF_UP, false);
        }

        req.ifr_ifru.ifru_flags = interface_flags.bits() as _;

        unsafe {
            siocsifflags(self.ctl_inet.as_raw_fd(), &req)
                .map_err(|e| Error::SystemCall("siocsifflags", e))?
        };

        Ok(())
    }

    pub fn set_ipv4_address(&self, addr: Ipv4Addr) -> Result<()> {
        let sa = SockaddrIn::from(SocketAddrV4::new(addr, 0));
        let mut req = self.request();
        req.ifr_ifru.ifru_addr = unsafe { *sa.as_ptr() };

        unsafe {
            siocsifaddr(self.ctl_inet.as_raw_fd(), &req)
                .map_err(|e| Error::SystemCall("siocsifaddr", e))?
        };

        Ok(())
    }

    pub fn set_ipv4_destination(&self, addr: Ipv4Addr) -> Result<()> {
        let sa = SockaddrIn::from(SocketAddrV4::new(addr, 0));
        let mut req = self.request();
        req.ifr_ifru.ifru_addr = unsafe { *sa.as_ptr() };

        unsafe {
            siocsifdstaddr(self.ctl_inet.as_raw_fd(), &req)
                .map_err(|e| Error::SystemCall("siocsifdstaddr", e))?
        };

        Ok(())
    }

    pub fn interface_index(&self) -> Result<i32> {
        unsafe {
            let req = self.request();
            siogifindex(self.ctl_inet.as_raw_fd(), &req)
                .map_err(|e| Error::SystemCall("siogifindex", e))?;

            Ok(req.ifr_ifru.ifru_ifindex)
        }
    }

    pub fn set_ipv4_netmask(&self, addr: Ipv4Addr) -> Result<()> {
        let sa = SockaddrIn::from(SocketAddrV4::new(addr, 0));
        let mut req = self.request();
        req.ifr_ifru.ifru_addr = unsafe { *sa.as_ptr() };

        unsafe {
            siocsifnetmask(self.ctl_inet.as_raw_fd(), &req)
                .map_err(|e| Error::SystemCall("siocsifnetmask", e))?
        };

        Ok(())
    }

    pub fn mtu(&self) -> Result<u16> {
        let mut req = self.request();
        unsafe {
            siocgifmtu(self.ctl_inet.as_raw_fd(), &mut req)
                .map_err(|e| Error::SystemCall("siocgifmtu", e))?;

            let mtu = req.ifr_ifru.ifru_mtu;
            mtu.try_into().map_err(|_| Error::MTUOverflowU16(mtu))
        }
    }

    pub fn set_mtu(&self, mtu_value: u16) -> Result<()> {
        let mut req = self.request();
        unsafe {
            req.ifr_ifru.ifru_mtu = mtu_value as i32;
            siocsifmtu(self.ctl_inet.as_raw_fd(), &req)
                .map_err(|e| Error::SystemCall("siocsifmtu", e))?;
            Ok(())
        }
    }

    pub fn set_ipv6_address(&self, addr: Ipv6Addr, prefix_len: u8) -> Result<()> {
        let mut req = self.request_ipv6()?;

        let sa = SockaddrIn6::from(SocketAddrV6::new(addr, 0, 0, 0));
        req.ifr6_addr = sa.as_ref().sin6_addr;
        req.ifr6_prefixlen = prefix_len as u32;

        unsafe {
            siocsifaddr_in6(self.ctl_inet6.as_raw_fd(), &req)
                .map_err(|e| Error::SystemCall("siocsifaddr_in6", e))?
        };

        Ok(())
    }

    fn request(&self) -> ifreq {
        let mut req: ifreq = unsafe { mem::zeroed() };
        self.copy_name_into(&mut req.ifr_name);
        req
    }

    fn request_ipv6(&self) -> Result<in6_ifreq> {
        let mut req: in6_ifreq = unsafe { mem::zeroed() };
        req.ifr6_ifindex = self.interface_index()?;
        Ok(req)
    }

    fn copy_name_into(&self, buf: &mut [c_char; IFNAMSIZ]) {
        for (i, ch) in self.name.as_bytes().iter().take(IFNAMSIZ).enumerate() {
            buf[i] = *ch as i8;
        }
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
        self.set_mtu(config.mtu)?;
        self.set_ipv4_address(config.ipv4.address)?;
        self.set_ipv4_destination(config.ipv4.destination)?;
        self.set_ipv4_netmask(config.ipv4.netmask)?;
        if let Some(ipv6) = config.ipv6.as_ref() {
            self.set_ipv6_address(ipv6.address, ipv6.prefix_length)?;
        }
        self.set_enabled(true)?;
        Ok(())
    }
}

impl AsRawFd for Device {
    fn as_raw_fd(&self) -> RawFd {
        self.tun.as_raw_fd()
    }
}
