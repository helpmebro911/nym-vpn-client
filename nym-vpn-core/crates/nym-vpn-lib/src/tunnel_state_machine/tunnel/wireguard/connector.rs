// Copyright 2023 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

use std::path::PathBuf;

use nym_vpn_network_config::Network;
use tokio::task::JoinHandle;

use nym_authenticator_client::{AuthClientMixnetListener, AuthClientMixnetListenerHandle};
use nym_credentials_interface::TicketType;
use nym_gateway_directory::{AuthAddresses, CachingGatewayClient, Gateway};
use nym_sdk::mixnet::{
    ConnectionStatsEvent, CredentialStorage, EphemeralCredentialStorage, StoragePaths,
};
use nym_task::TaskManager;
use nym_wg_gateway_client::{GatewayData, WgGatewayClient};
use tokio_util::sync::CancellationToken;

use crate::{
    bandwidth_controller::BandwidthController,
    mixnet::SharedMixnetClient,
    tunnel_state_machine::tunnel::{self, Error, Result, gateway_selector::SelectedGateways},
};

pub struct ConnectionData {
    pub entry: GatewayData,
    pub exit: GatewayData,
}

pub struct ConnectOptions {
    pub data_path: Option<PathBuf>,
    pub network: Network,
    pub enable_credentials_mode: bool,
    pub selected_gateways: SelectedGateways,
}

pub async fn register_with_gateways(
    task_manager: &TaskManager,
    mixnet_client: SharedMixnetClient,
    gateway_directory_client: CachingGatewayClient,
    connect_options: ConnectOptions,
    cancel_token: CancellationToken,
) -> Result<ConnectorResult> {
    let auth_addresses = setup_auth_addresses(
        &connect_options.selected_gateways.entry,
        &connect_options.selected_gateways.exit,
    )?;
    let (Some(entry_auth_recipient), Some(exit_auth_recipient)) =
        (auth_addresses.entry().0, auth_addresses.exit().0)
    else {
        return Err(Error::AuthenticationNotPossible(auth_addresses.to_string()));
    };
    let entry_version = connect_options
        .selected_gateways
        .entry
        .version
        .clone()
        .into();
    tracing::debug!("Entry gateway version: {entry_version}");
    let exit_version = connect_options
        .selected_gateways
        .exit
        .version
        .clone()
        .into();
    tracing::debug!("Exit gateway version: {exit_version}");

    // Start the auth client mixnet listener, which will listen for incoming messages from the
    // mixnet and rebroadcast them to the auth clients.
    let mixnet_listener = AuthClientMixnetListener::new(mixnet_client)
        .with_external_cancel_token(cancel_token.clone())
        .start();

    let auth_client = mixnet_listener
        .new_auth_client()
        .await
        .ok_or(Error::MixnetClientDisposed)?;

    let mut wg_entry_gateway_client = if connect_options.enable_credentials_mode {
        WgGatewayClient::new_free_entry(
            &connect_options.data_path,
            auth_client.clone(),
            entry_auth_recipient,
            entry_version,
        )
    } else {
        WgGatewayClient::new_entry(
            &connect_options.data_path,
            auth_client.clone(),
            entry_auth_recipient,
            entry_version,
        )
    };
    let mut wg_exit_gateway_client = if connect_options.enable_credentials_mode {
        WgGatewayClient::new_free_exit(
            &connect_options.data_path,
            auth_client.clone(),
            exit_auth_recipient,
            exit_version,
        )
    } else {
        WgGatewayClient::new_exit(
            &connect_options.data_path,
            auth_client.clone(),
            exit_auth_recipient,
            exit_version,
        )
    };

    let (connection_data, bandwidth_controller_handle) = start_bandwidth_controller(
        task_manager,
        gateway_directory_client,
        &mut wg_entry_gateway_client,
        &mut wg_exit_gateway_client,
        &connect_options,
        cancel_token,
    )
    .await?;

    if let Some(exit_country_code) = connect_options
        .selected_gateways
        .exit
        .two_letter_iso_country_code()
    {
        auth_client.send_stats_event(
            ConnectionStatsEvent::WgCountry(exit_country_code.to_string()).into(),
        );
    }

    Ok(ConnectorResult {
        entry_gateway_client: wg_entry_gateway_client,
        exit_gateway_client: wg_exit_gateway_client,
        connection_data,
        bandwidth_controller_handle,
        auth_client_mixnet_listener_handle: mixnet_listener,
    })
}

async fn start_bandwidth_controller(
    task_manager: &TaskManager,
    gateway_directory_client: CachingGatewayClient,
    wg_entry_gateway_client: &mut WgGatewayClient,
    wg_exit_gateway_client: &mut WgGatewayClient,
    connect_options: &ConnectOptions,
    cancel_token: CancellationToken,
) -> Result<(ConnectionData, JoinHandle<()>)> {
    let client_task = task_manager.subscribe_named("bandwidth_controller");

    if let Some(data_path) = connect_options.data_path.as_ref() {
        let paths = StoragePaths::new_from_dir(data_path)
            .map_err(|err| Error::SetupStoragePaths(Box::new(err)))?;
        let storage = paths
            .persistent_credential_storage()
            .await
            .map_err(|err| Error::SetupStoragePaths(Box::new(err)))?;
        let bandwidth_controller = BandwidthController::new(
            storage,
            &connect_options.network,
            wg_entry_gateway_client.light_client(),
            wg_exit_gateway_client.light_client(),
            client_task,
        )?;

        start_bandwidth_controller_inner(
            bandwidth_controller,
            connect_options.enable_credentials_mode,
            gateway_directory_client,
            wg_entry_gateway_client,
            wg_exit_gateway_client,
            cancel_token,
        )
        .await
    } else {
        let bandwidth_controller = BandwidthController::new(
            EphemeralCredentialStorage::default(),
            &connect_options.network,
            wg_entry_gateway_client.light_client(),
            wg_exit_gateway_client.light_client(),
            client_task,
        )?;

        start_bandwidth_controller_inner(
            bandwidth_controller,
            connect_options.enable_credentials_mode,
            gateway_directory_client,
            wg_entry_gateway_client,
            wg_exit_gateway_client,
            cancel_token,
        )
        .await
    }
}

async fn start_bandwidth_controller_inner<S>(
    bandwidth_controller: BandwidthController<S>,
    enable_credentials_mode: bool,
    gateway_directory_client: CachingGatewayClient,
    wg_entry_gateway_client: &mut WgGatewayClient,
    wg_exit_gateway_client: &mut WgGatewayClient,
    cancel_token: CancellationToken,
) -> Result<(ConnectionData, JoinHandle<()>)>
where
    S: CredentialStorage + Send + Sync + 'static,
    S::StorageError: Send + Sync + 'static,
{
    let entry_fut = bandwidth_controller.get_initial_bandwidth(
        enable_credentials_mode,
        TicketType::V1WireguardEntry,
        gateway_directory_client.clone(),
        wg_entry_gateway_client,
    );
    let exit_fut = bandwidth_controller.get_initial_bandwidth(
        enable_credentials_mode,
        TicketType::V1WireguardExit,
        gateway_directory_client.clone(),
        wg_exit_gateway_client,
    );

    let (entry, exit) = cancel_token
        .run_until_cancelled(async { tokio::try_join!(entry_fut, exit_fut) })
        .await
        .ok_or(tunnel::Error::Cancelled)??;

    let bandwidth_controller_handle = tokio::spawn(bandwidth_controller.run());

    Ok((ConnectionData { entry, exit }, bandwidth_controller_handle))
}

fn setup_auth_addresses(entry: &Gateway, exit: &Gateway) -> Result<AuthAddresses> {
    let entry_authenticator_address = entry
        .authenticator_address
        .ok_or(Error::AuthenticatorAddressNotFound)?;
    let exit_authenticator_address = exit
        .authenticator_address
        .ok_or(Error::AuthenticatorAddressNotFound)?;
    Ok(AuthAddresses::new(
        entry_authenticator_address,
        exit_authenticator_address,
    ))
}

pub struct ConnectorResult {
    pub entry_gateway_client: WgGatewayClient,
    pub exit_gateway_client: WgGatewayClient,
    pub connection_data: ConnectionData,
    pub bandwidth_controller_handle: JoinHandle<()>,
    pub auth_client_mixnet_listener_handle: AuthClientMixnetListenerHandle,
}
