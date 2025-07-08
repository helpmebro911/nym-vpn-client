// Copyright 2024 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

pub mod device;
mod error;
pub mod wireguard;

use std::path::Path;

pub use error::KeyStoreError;

pub async fn init_wireguard_on_disk<P: AsRef<Path>>(
    database_dir: P,
) -> Result<wireguard::OnDiskKeys, KeyStoreError> {
    wireguard::OnDiskKeys::init(database_dir)
        .await
        .map_err(|error| KeyStoreError::WireguardOnDisk { error })
}
