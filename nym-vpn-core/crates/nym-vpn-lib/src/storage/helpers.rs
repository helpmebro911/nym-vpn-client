// Copyright 2024 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

use std::path::{Path, PathBuf};

use nym_vpn_store::keys::device::{DeviceKeyStore as _, DeviceKeys, OnDiskKeysError};

use crate::storage::error::KeyStoreError;

use super::VpnClientOnDiskStorage;

// Set of helpers to load, create and store device keys for situations where you don't have a long
// running store instance.

#[allow(unused)]
pub async fn load_device_keys<P: AsRef<Path> + Clone>(
    path: P,
) -> Result<DeviceKeys, KeyStoreError> {
    VpnClientOnDiskStorage::init(path.clone())
        .load_keys()
        .await
        .map_err(|error| KeyStoreError::LoadDeviceKeys {
            path: path.as_ref().to_path_buf(),
            error,
        })
}

#[allow(unused)]
pub async fn create_device_keys<P: AsRef<Path> + Clone>(path: P) -> Result<(), KeyStoreError> {
    let vpn_storage = VpnClientOnDiskStorage::init(path.clone());
    let mut rng = rand::rngs::OsRng;
    DeviceKeys::generate_new(&mut rng)
        .persist_keys(&vpn_storage)
        .await
        .map_err(|error| KeyStoreError::CreateDeviceKeys {
            path: path.as_ref().to_path_buf(),
            error,
        })
}

#[allow(unused)]
pub async fn store_device_keys<P: AsRef<Path> + Clone>(
    path: P,
    keys: &DeviceKeys,
) -> Result<(), KeyStoreError> {
    let vpn_storage = VpnClientOnDiskStorage::init(path.clone());
    keys.persist_keys(&vpn_storage)
        .await
        .map_err(|error| KeyStoreError::StoreDeviceKeys {
            path: path.as_ref().to_path_buf(),
            error,
        })
}
