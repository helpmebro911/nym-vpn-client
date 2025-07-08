// Copyright 2025 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

#[derive(Debug, thiserror::Error)]
pub enum KeyStoreError {
    #[error("failed to load device keys")]
    LoadDeviceKeys {
        path: PathBuf,
        error: nym_vpn_store::keys::device::OnDiskKeysError,
    },

    #[error("failed to create device keys")]
    CreateDeviceKeys {
        path: PathBuf,
        error: nym_vpn_store::keys::device::OnDiskKeysError,
    },

    #[error("failed to store device keys")]
    StoreDeviceKeys {
        path: PathBuf,
        error: nym_vpn_store::keys::device::OnDiskKeysError,
    },

    #[error("failed to init wireguard keys database")]
    InitWireguardKeys {
        path: PathBuf,
        error: nym_vpn_store::keys::wireguard::OnDiskKeysError,
    },
}
