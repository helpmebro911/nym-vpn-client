// Copyleft 2017-2024 meh. <meh@schizofreni.co> | http://meh.schizofreni.co
// Copyright 2025 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

use std::{
    io::{self, Read, Write},
    mem,
    os::unix::io::{AsRawFd, IntoRawFd, RawFd},
};

use nix::libc;

pub struct UnixTun<Fd>
where
    Fd: AsRawFd,
{
    fd: Fd,
}

impl<Fd> UnixTun<Fd>
where
    Fd: AsRawFd,
{
    pub(crate) fn new(fd: Fd) -> Self {
        Self { fd }
    }
}

impl<Fd> Read for UnixTun<Fd>
where
    Fd: AsRawFd + IntoRawFd,
{
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let amount =
            unsafe { libc::read(self.fd.as_raw_fd(), buf.as_mut_ptr() as *mut _, buf.len()) };

        if amount < 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(amount as usize)
    }

    fn read_vectored(&mut self, bufs: &mut [io::IoSliceMut<'_>]) -> io::Result<usize> {
        let mut msg: libc::msghdr = unsafe { mem::zeroed() };
        // msg.msg_name: NULL
        // msg.msg_namelen: 0
        msg.msg_iov = bufs.as_mut_ptr().cast();
        msg.msg_iovlen = bufs.len().min(libc::c_int::MAX as usize) as _;

        let n = unsafe { libc::recvmsg(self.fd.as_raw_fd(), &mut msg, 0) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(n as usize)
    }
}

impl<Fd> Write for UnixTun<Fd>
where
    Fd: AsRawFd + IntoRawFd,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let amount =
            unsafe { libc::write(self.fd.as_raw_fd(), buf.as_ptr() as *const _, buf.len()) };

        if amount < 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(amount as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        let mut msg: libc::msghdr = unsafe { mem::zeroed() };
        // msg.msg_name = NULL
        // msg.msg_namelen = 0
        msg.msg_iov = bufs.as_ptr() as *mut _;
        msg.msg_iovlen = bufs.len().min(libc::c_int::MAX as usize) as _;

        let n = unsafe { libc::sendmsg(self.fd.as_raw_fd(), &msg, 0) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(n as usize)
    }
}

impl<Fd> AsRawFd for UnixTun<Fd>
where
    Fd: AsRawFd,
{
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl<Fd> IntoRawFd for UnixTun<Fd>
where
    Fd: AsRawFd + IntoRawFd,
{
    fn into_raw_fd(self) -> RawFd {
        self.fd.into_raw_fd()
    }
}
