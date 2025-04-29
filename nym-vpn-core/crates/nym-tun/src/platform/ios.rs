// Copyright 2025 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

use std::{
    io::{self, Read, Write},
    os::fd::{AsRawFd, OwnedFd, RawFd},
};

use nix::sys::socket::{getsockopt, sockopt::UtunIfname};

use crate::UnixTun;

// This call takes a pointer to ifreq but must be defined as taking integer.
nix::ioctl_read!(tungetiff, b'T', 210, nix::libc::c_int);

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Failed to execute syscall to {}", _0)]
    SystemCall(&'static str, #[source] nix::Error),

    #[error("Failed to copy interface name")]
    CopyInterfaceName(#[source] std::ffi::FromBytesUntilNulError),

    #[error("Failed to convert interface name to utf-8")]
    ConvertInterfaceNameToUtf8(#[source] std::str::Utf8Error),
}

pub type Result<T> = std::result::Result<T, Error>;

pub struct Device {
    name: String,
    tun: UnixTun<OwnedFd>,
}

impl Device {
    pub fn new(fd: OwnedFd) -> Result<Self> {
        let name = getsockopt(&fd, UtunIfname)
            .map_err(|e| Error::SystemCall("getsockopt(UtunIfname)", e))?
            .to_str()
            .map_err(Error::ConvertInterfaceNameToUtf8)?
            .to_owned();

        Ok(Self {
            name,
            tun: UnixTun::new(fd),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn has_packet_information(&self) -> bool {
        true
    }

    pub fn mtu(&self) -> Result<u16> {
        todo!()
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

impl AsRawFd for Device {
    fn as_raw_fd(&self) -> RawFd {
        self.tun.as_raw_fd()
    }
}
