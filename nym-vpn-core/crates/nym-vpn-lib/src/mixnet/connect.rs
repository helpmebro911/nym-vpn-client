// Copyright 2023 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

use std::{path::PathBuf, result::Result, time::Duration};

use nym_client_core::config::{RememberMe, StatsReporting};
use nym_gateway_directory::Recipient;
use nym_sdk::{
    DebugConfig, UserAgent,
    mixnet::{
        CredentialStorage, GatewaysDetailsStore, KeyStore, MixnetClient, MixnetClientBuilder,
        MixnetClientStorage, NodeIdentity, ReplyStorageBackend, StoragePaths,
    },
};
use nym_vpn_network_config::Network;
use nym_vpn_store::mnemonic::MnemonicStorage as _;

use super::{MixnetError, topology_provider::VpnTopologyProvider};
use crate::{MixnetClientConfig, storage::VpnClientOnDiskStorage};

const VPN_AVERAGE_PACKET_DELAY: Duration = Duration::from_millis(15);
const MOBILE_LOOP_COVER_STREAM_AVERAGE_DELAY: Duration = Duration::from_secs(10);

pub struct MixnetRuntimeConfig {
    pub network_env: Network,
    pub mixnet_entry_gateway: NodeIdentity,
    pub mixnet_client_key_storage_path: Option<PathBuf>,
    pub mixnet_client_config: MixnetClientConfig,
    pub enable_credentials_mode: bool,
    pub stats_recipient_address: Option<Recipient>,
    pub two_hop_mode: bool,
    pub custom_topology_provider: VpnTopologyProvider,
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub connection_fd_callback: Arc<dyn Fn(RawFd) + Send + Sync>,
}

pub async fn connect_mixnet_client(
    mut task_client: nym_task::TaskClient,
    mixnet_runtime_config: MixnetRuntimeConfig,
) -> Result<MixnetClient, MixnetError> {
    let mut mixnet_client_debug_config = nym_client_core::config::DebugConfig::default();

    let mixnet_client = if let Some(path) = mixnet_runtime_config
        .mixnet_client_key_storage_path
        .as_ref()
    {
        tracing::debug!("Using custom key storage path: {}", path.display());

        let storage = VpnClientOnDiskStorage::new(path);
        match storage.is_mnemonic_stored().await {
            Ok(is_stored) if !is_stored => {
                tracing::error!("No account stored");
                task_client.disarm();
                return Err(MixnetError::InvalidCredential);
            }
            Ok(_) => {}
            Err(err) => {
                tracing::error!("failed to check credential: {:?}", err);
                task_client.disarm();
                return Err(MixnetError::InvalidCredential);
            }
        }

        // We want fresh SURB sender tags on each session
        mixnet_client_debug_config.reply_surbs.fresh_sender_tags = true;

        let key_storage_path = StoragePaths::new_from_dir(path)
            .map_err(|err| MixnetError::SetupMixnetStoragePaths(Box::new(err)))?;

        let storage = key_storage_path
            .initialise_persistent_storage(&mixnet_client_debug_config)
            .await
            .map_err(|err| MixnetError::CreateMixnetClientWithDefaultStorage(Box::new(err)))?;

        build_and_connect_mixnet_client(
            MixnetClientBuilder::new_with_storage(storage),
            mixnet_runtime_config,
            mixnet_client_debug_config,
            task_client,
        )
        .await?
    } else {
        tracing::debug!("Using ephemeral key storage");

        build_and_connect_mixnet_client(
            MixnetClientBuilder::new_ephemeral(),
            mixnet_runtime_config,
            mixnet_client_debug_config,
            task_client,
        )
        .await?
    };

    Ok(mixnet_client)
}

async fn build_and_connect_mixnet_client<S>(
    mut builder: MixnetClientBuilder<S>,
    mixnet_runtime_config: MixnetRuntimeConfig,
    mut mixnet_client_debug_config: DebugConfig,
    task_client: nym_task::TaskClient,
) -> Result<MixnetClient, MixnetError>
where
    S: MixnetClientStorage + Clone + 'static,
    S::ReplyStore: Send + Sync,
    S::GatewaysDetailsStore: Sync,
    <S::ReplyStore as ReplyStorageBackend>::StorageError: Sync + Send,
    <S::CredentialStore as CredentialStorage>::StorageError: Send + Sync,
    <S::KeyStore as KeyStore>::StorageError: Send + Sync,
    <S::GatewaysDetailsStore as GatewaysDetailsStore>::StorageError: Send + Sync,
{
    mixnet_client_debug_config.traffic.average_packet_delay = VPN_AVERAGE_PACKET_DELAY;
    if mixnet_runtime_config.two_hop_mode {
        // for mobile platforms, in two hop mode, we do less frequent cover traffic, to preserve battery
        if cfg!(any(target_os = "android", target_os = "ios")) {
            mixnet_client_debug_config
                .cover_traffic
                .loop_cover_traffic_average_delay = MOBILE_LOOP_COVER_STREAM_AVERAGE_DELAY;
        }

        // If operating in two hop mode, we disable mix hops for the mixnet connection.
        mixnet_client_debug_config.traffic.disable_mix_hops = true;
    }
    apply_mixnet_client_config(
        &mixnet_runtime_config.mixnet_client_config,
        &mut mixnet_client_debug_config,
    );
    mixnet_runtime_config
        .custom_topology_provider
        .update_config(
            mixnet_runtime_config
                .mixnet_client_config
                .min_mixnode_performance,
            mixnet_runtime_config
                .mixnet_client_config
                .min_gateway_performance,
        )
        .await;

    let stats_reporting = StatsReporting {
        provider_address: mixnet_runtime_config.stats_recipient_address,
        ..Default::default()
    };
    let remember_me = if mixnet_runtime_config.two_hop_mode {
        RememberMe::new_vpn()
    } else {
        RememberMe::new_mixnet()
    };
    let user_agent: UserAgent = nym_bin_common::bin_info_owned!().into();

    builder = builder
        .with_user_agent(user_agent)
        .request_gateway(mixnet_runtime_config.mixnet_entry_gateway.to_string())
        .network_details(
            mixnet_runtime_config
                .network_env
                .nym_network
                .network
                .clone(),
        )
        .debug_config(mixnet_client_debug_config)
        .custom_shutdown(task_client)
        .credentials_mode(mixnet_runtime_config.enable_credentials_mode)
        .with_remember_me(remember_me)
        .custom_topology_provider(Box::new(mixnet_runtime_config.custom_topology_provider))
        .with_statistics_reporting(stats_reporting);

    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        builder = builder.with_connection_fd_callback(mixnet_runtime_config.connection_fd_callback);
    }

    builder
        .build()
        .map_err(|err| MixnetError::BuildMixnetClient(Box::new(err)))?
        .connect_to_mixnet()
        .await
        .map_err(map_mixnet_connect_error)
}

// Map some specific mixnet errors to more specific ones
fn map_mixnet_connect_error(err: nym_sdk::Error) -> MixnetError {
    match err {
        nym_sdk::Error::ClientCoreError(
            nym_client_core::error::ClientCoreError::GatewayClientError { gateway_id, source },
        ) => MixnetError::EntryGateway {
            gateway_id: gateway_id.to_string(),
            source: Box::new(source),
        },
        _ => MixnetError::ConnectToMixnet(Box::new(err)),
    }
}

fn apply_mixnet_client_config(
    mixnet_client_config: &MixnetClientConfig,
    debug_config: &mut nym_client_core::config::DebugConfig,
) {
    let MixnetClientConfig {
        disable_poisson_rate,
        disable_background_cover_traffic,
        min_mixnode_performance,
        min_gateway_performance,
    } = mixnet_client_config;

    tracing::info!(
        "mixnet client poisson rate limiting: {}",
        true_to_disabled(*disable_poisson_rate)
    );
    debug_config
        .traffic
        .disable_main_poisson_packet_distribution = *disable_poisson_rate;

    tracing::info!(
        "mixnet client background loop cover traffic stream: {}",
        true_to_disabled(*disable_background_cover_traffic)
    );
    debug_config.cover_traffic.disable_loop_cover_traffic_stream =
        *disable_background_cover_traffic;

    if let Some(min_mixnode_performance) = min_mixnode_performance {
        debug_config.topology.minimum_mixnode_performance = *min_mixnode_performance;
    }
    tracing::info!(
        "mixnet client minimum mixnode performance: {}",
        debug_config.topology.minimum_mixnode_performance,
    );

    if let Some(min_gateway_performance) = min_gateway_performance {
        debug_config.topology.minimum_gateway_performance = *min_gateway_performance;
    }
    tracing::info!(
        "mixnet client minimum gateway performance: {}",
        debug_config.topology.minimum_gateway_performance,
    );
}

fn true_to_disabled(val: bool) -> &'static str {
    if val { "disabled" } else { "enabled" }
}
