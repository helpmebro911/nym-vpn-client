// Copyleft 2017-2024 meh. <meh@schizofreni.co> | http://meh.schizofreni.co
// Copyright 2025 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

mod codec;
pub use codec::{TunPacket, TunPacketCodec};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use unix::AsyncDevice;
#[cfg(windows)]
pub use windows::AsyncDevice;

#[derive(thiserror::Error, Debug)]
pub enum IntoFramedError {
    #[error("failed to retrive MTU")]
    Mtu(#[source] crate::platform::Error),
}
