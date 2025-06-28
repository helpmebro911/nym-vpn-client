// Copyright 2023 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

mod any_tunnel_handle;
mod gateway_selector;
pub mod mixnet;
mod tombstone;
pub mod wireguard;

pub use gateway_selector::SelectedGateways;
use nym_gateway_directory::{CachingGatewayClient, EntryPoint, ExitPoint};

use tokio_util::sync::CancellationToken;

use super::TunnelType;
#[cfg(windows)]
use super::route_handler;
use crate::{GatewayDirectoryError, MixnetError};
pub use any_tunnel_handle::AnyTunnelHandle;
pub use tombstone::Tombstone;

pub async fn select_gateways(
    gateway_directory_client: CachingGatewayClient,
    tunnel_type: TunnelType,
    entry_point: Box<EntryPoint>,
    exit_point: Box<ExitPoint>,
    cancel_token: CancellationToken,
) -> Result<SelectedGateways> {
    let select_gateways_fut = gateway_selector::select_gateways(
        gateway_directory_client,
        tunnel_type,
        entry_point,
        exit_point,
    );
    cancel_token
        .run_until_cancelled(select_gateways_fut)
        .await
        .ok_or(Error::Cancelled)?
        .map_err(|err| Error::SelectGateways(Box::new(err)))
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to create gateway client")]
    CreateGatewayClient(#[source] nym_gateway_directory::Error),

    #[error("failed to select gateways")]
    SelectGateways(#[source] Box<GatewayDirectoryError>),

    #[error("start mixnet client timeout")]
    StartMixnetClientTimeout,

    #[error("mixnet tunnel has failed")]
    MixnetClient(#[from] MixnetError),

    #[error("failed to lookup gateway: {}", gateway_id)]
    LookupGatewayIp {
        gateway_id: String,
        #[source]
        source: Box<nym_gateway_directory::Error>,
    },

    #[error("failed to connect to ip packet router")]
    ConnectToIpPacketRouter(#[source] nym_ip_packet_client::Error),

    #[error(
        "wireguard authentication is not possible due to one of the gateways not running the authenticator process: {0}"
    )]
    AuthenticationNotPossible(String),

    #[error("failed to find authenticator address")]
    AuthenticatorAddressNotFound,

    #[error("failed to setup storage paths")]
    SetupStoragePaths(#[source] Box<nym_sdk::Error>),

    #[error("bandwidth controller error")]
    BandwidthController(#[from] crate::bandwidth_controller::Error),

    #[cfg(target_os = "ios")]
    #[error("failed to resolve using dns64")]
    ResolveDns64(#[from] wireguard::dns64::Error),

    #[error("WireGuard error")]
    Wireguard(#[from] nym_wg_go::Error),

    #[error("failed to dup tunnel file descriptor")]
    DupFd(#[source] std::io::Error),

    #[cfg(windows)]
    #[error("failed to add default route listener")]
    AddDefaultRouteListener(#[source] route_handler::Error),

    #[error("connection cancelled")]
    Cancelled,

    /// Indicates that a mixnet client has been moved out of shared reference (`Arc<Mutex<Option<MixnetClient>>`)
    /// Typically this is done for the purpose of disconnecting and disposing a mixnet client.
    ///
    /// If this error occurs, it's likely that two or more parties have been racing for access to mixnet client.
    /// One of parties then moved the mixnet client out of shared reference.
    #[error("mixnet client is already disposed")]
    MixnetClientDisposed,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
