// Copyright 2025 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

mod device;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod ipv6;
#[cfg(any(target_os = "ios", target_os = "android"))]
mod tun_name;

pub use device::{Error, TunnelDevice};

#[cfg(any(target_os = "ios", target_os = "android"))]
pub use tun_name::GetTunNameError;
