// Copyright 2024 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

use std::{
    cmp,
    net::IpAddr,
    path::PathBuf,
    time::{Duration, Instant},
};

#[cfg(any(target_os = "ios", target_os = "android"))]
use ipnetwork::{IpNetwork, Ipv4Network, Ipv6Network};

#[cfg(windows)]
use super::wintun::{self, WintunAdapterConfig};
use nym_gateway_directory::{
    CachingGatewayClient, GatewayClient, GatewayMinPerformance, ResolvedConfig,
};

use nym_sdk::UserAgent;
use nym_task::{TaskManager, TaskStatus};
use nym_vpn_account_controller::AccountCommandSender;
use nym_vpn_network_config::start_background_file_refresh;
use time::OffsetDateTime;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use super::route_handler::{RouteHandler, RoutingConfig};
use super::{
    Error, NymConfig, Result, TunnelInterface, TunnelMetadata, TunnelSettings,
    tunnel::{self, AnyTunnelHandle, SelectedGateways, Tombstone},
};
use nym_common::trace_err_chain;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use nym_ip_packet_requests::IpPair;
use nym_vpn_lib_types::{
    ConnectionData, ErrorStateReason, Gateway, MixnetConnectionData, MixnetEvent, NymAddress,
    RequestZkNymError, TunnelConnectionData, TunnelType, WireguardConnectionData, WireguardNode,
};

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use super::tunnel::wireguard::connected_tunnel::{
    NetstackTunnelOptions, TunTunTunnelOptions, TunnelOptions,
};
#[cfg(any(target_os = "ios", target_os = "android"))]
use crate::tunnel_provider;
#[cfg(target_os = "ios")]
use crate::tunnel_provider::ios::OSTunProvider;
#[cfg(any(target_os = "android", target_os = "linux"))]
use crate::tunnel_state_machine::socket_bypass;
use crate::{
    MixnetClientConfig, VpnTopologyProvider,
    mixnet::{MixnetRuntimeConfig, SharedMixnetClient},
    tunnel_device::TunnelDevice,
    tunnel_state_machine::{WireguardMultihopMode, account, status_listener::StatusListener},
};

const TASK_MANAGER_SHUTDOWN_TIMER_SECS: u64 = 10;

/// Default MTU for mixnet tun device.
const DEFAULT_TUN_MTU: u16 = if cfg!(any(target_os = "ios", target_os = "android")) {
    1280
} else {
    1500
};

/// User-facing tunnel type identifier.
#[cfg(windows)]
const WINTUN_TUNNEL_TYPE: &str = "Nym";

/// The user-facing name of wintun adapter.
///
/// Note that it refers to tunnel type because rust-tun uses the same name for adapter and
/// tunnel type and there is no way to change that.
#[cfg(windows)]
const MIXNET_WINTUN_NAME: &str = WINTUN_TUNNEL_TYPE;

/// The user-facing name of wintun adapter used as entry tunnel.
#[cfg(windows)]
const WG_ENTRY_WINTUN_NAME: &str = "WireGuard (entry)";

/// The user-facing name of wintun adapter used as exit tunnel.
#[cfg(windows)]
const WG_EXIT_WINTUN_NAME: &str = "WireGuard (exit)";

/// WireGuard entry adapter GUID.
#[cfg(windows)]
const WG_ENTRY_WINTUN_GUID: &str = "{AFE43773-E1F8-4EBB-8536-176AB86AFE9B}";

/// WireGuard exit adapter GUID.
#[cfg(windows)]
const WG_EXIT_WINTUN_GUID: &str = "{AFE43773-E1F8-4EBB-8536-176AB86AFE9C}";

pub type TunnelMonitorEventSender = mpsc::UnboundedSender<TunnelMonitorEvent>;
pub type TunnelMonitorEventReceiver = mpsc::UnboundedReceiver<TunnelMonitorEvent>;

/// Initial delay between retry attempts.
const INITIAL_WAIT_DELAY: Duration = Duration::from_secs(2);

/// Wait delay multiplier used for each subsequent retry attempt.
const DELAY_MULTIPLIER: u32 = 2;

/// Max wait delay between retry attempts.
const MAX_WAIT_DELAY: Duration = Duration::from_secs(15);

/// Timeout when waiting for reply from the event handler.
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub enum TunnelMonitorEvent {
    /// Awaiting a cooldown period before reconnecting
    #[allow(dead_code)]
    ReconnectCooldown {
        /// Cooldown begin time
        begin_time: Instant,

        /// Cooldown duration
        duration: Duration,
    },

    /// Initializing mixnet client
    InitializingClient,

    /// Syncronizing account with vpn-api
    SyncingAccount,

    /// Registering device with vpn-api
    RegisteringDevice,

    /// Requesting and downloading zknym credentials from vpn-api
    RequestingZkNyms,

    /// Selecting gateways
    SelectingGateways,

    /// Selected gateways
    SelectedGateways {
        gateways: Box<SelectedGateways>,
        /// Back channel to acknowledge that the event has been processed
        reply_tx: oneshot::Sender<()>,
    },

    /// Tunnel interface is up.
    InterfaceUp {
        /// Tunnel interface
        tunnel_interface: TunnelInterface,
        /// Connection data
        connection_data: Box<ConnectionData>,
        /// Back channel to acknowledge that the event has been processed
        reply_tx: oneshot::Sender<()>,
    },

    /// Tunnel is up and functional.
    Up {
        /// Tunnel interface
        tunnel_interface: TunnelInterface,
        /// Connection data
        connection_data: Box<ConnectionData>,
    },

    /// Tunnel went down
    Down {
        /// Error state reason.
        /// When set indicates that the state machine should transition to error state.
        error_state_reason: Option<ErrorStateReason>,
        /// Back channel to acknowledge that the event has been processed
        reply_tx: oneshot::Sender<()>,
    },
}

pub struct TunnelMonitorHandle {
    shutdown_token: CancellationToken,
    join_handle: JoinHandle<Tombstone>,
}

impl TunnelMonitorHandle {
    pub fn cancel(&self) {
        tracing::info!("Cancelling tunnel monitor handle");
        self.shutdown_token.cancel();
    }

    pub async fn wait(self) -> Tombstone {
        self.join_handle
            .await
            .inspect_err(|e| {
                tracing::error!("Failed to join on tunnel monitor handle: {}", e);
            })
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct TunnelParameters {
    pub nym_config: NymConfig,
    pub resolved_gateway_config: ResolvedConfig,
    pub tunnel_settings: TunnelSettings,
    pub selected_gateways: Option<SelectedGateways>,
    pub retry_attempt: u32,
}

pub struct TunnelMonitor {
    tunnel_parameters: TunnelParameters,
    monitor_event_sender: mpsc::UnboundedSender<TunnelMonitorEvent>,
    mixnet_event_sender: mpsc::UnboundedSender<MixnetEvent>,
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    route_handler: RouteHandler,
    #[cfg(target_os = "ios")]
    tun_provider: Arc<dyn OSTunProvider>,
    #[cfg(target_os = "android")]
    tun_provider: Arc<dyn AndroidTunProvider>,
    account_commands: AccountCommandSender,
    gateway_directory_client: CachingGatewayClient,
    custom_topology_provider: VpnTopologyProvider,
    task_manager: TaskManager,
    shared_mixnet_client: SharedMixnetClient,
    shutdown_token: CancellationToken,
}

impl TunnelMonitor {
    pub fn start(
        tunnel_parameters: TunnelParameters,
        account_commands: AccountCommandSender,
        gateway_directory_client: CachingGatewayClient,
        custom_topology_provider: VpnTopologyProvider,
        monitor_event_sender: mpsc::UnboundedSender<TunnelMonitorEvent>,
        mixnet_event_sender: mpsc::UnboundedSender<MixnetEvent>,
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        route_handler: RouteHandler,
        #[cfg(target_os = "ios")] tun_provider: Arc<dyn OSTunProvider>,
        #[cfg(target_os = "android")] tun_provider: Arc<dyn AndroidTunProvider>,
    ) -> TunnelMonitorHandle {
        let shutdown_token = CancellationToken::new();
        let task_manager = TaskManager::new(TASK_MANAGER_SHUTDOWN_TIMER_SECS);
        let tunnel_monitor = Self {
            tunnel_parameters,
            monitor_event_sender,
            mixnet_event_sender,
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            route_handler,
            #[cfg(any(target_os = "ios", target_os = "android"))]
            tun_provider,
            account_commands,
            gateway_directory_client,
            custom_topology_provider,
            task_manager,
            shutdown_token: shutdown_token.clone(),
            shared_mixnet_client: SharedMixnetClient::default(),
        };
        let join_handle = tokio::spawn(tunnel_monitor.run());

        TunnelMonitorHandle {
            shutdown_token,
            join_handle,
        }
    }

    async fn run(mut self) -> Tombstone {
        let status_listener_token = CancellationToken::new();
        let status_listener_handle = self
            .start_status_listener(status_listener_token.child_token())
            .await;

        let (tombstone, reason) = match self.run_inner().await {
            Ok(tombstone) => (tombstone, None),
            Err(e) => {
                trace_err_chain!(e, "Tunnel monitor exited with error");
                (Tombstone::default(), e.error_state_reason())
            }
        };

        if let Err(e) = self.task_manager.signal_shutdown() {
            tracing::error!("Failed to signal task manager shutdown: {}", e);
        }

        if let Some(mixnet_client) = self.shared_mixnet_client.lock().await.take() {
            tracing::debug!("Disconnect mixnet client");
            mixnet_client.disconnect().await;
        }

        tracing::debug!("Waiting for task manager to shutdown");
        self.task_manager.wait_for_graceful_shutdown().await;

        status_listener_token.cancel();
        if let Err(e) = status_listener_handle.await {
            tracing::error!("Failed to join on status listener: {e}")
        }

        let (reply_tx, reply_rx) = oneshot::channel();
        self.send_event(TunnelMonitorEvent::Down {
            error_state_reason: reason,
            reply_tx,
        });
        if tokio::time::timeout(REPLY_TIMEOUT, reply_rx).await.is_err() {
            tracing::warn!("Tunnel down reply timeout.");
        }

        tombstone
    }

    async fn run_inner(&mut self) -> Result<Tombstone> {
        if self.tunnel_parameters.retry_attempt > 0 {
            let delay = wait_delay(self.tunnel_parameters.retry_attempt);
            tracing::debug!("Waiting for {}s before connecting.", delay.as_secs());
            self.send_event(TunnelMonitorEvent::ReconnectCooldown {
                begin_time: Instant::now(),
                duration: delay,
            });

            self.shutdown_token
                .run_until_cancelled(tokio::time::sleep(delay))
                .await
                .ok_or(Error::Tunnel(Box::new(tunnel::Error::Cancelled)))?;
        }

        self.send_event(TunnelMonitorEvent::InitializingClient);
        self.setup_account().await?;
        self.send_event(TunnelMonitorEvent::SelectingGateways);

        let user_agent = self.get_user_agent();
        let gateway_config = self.get_gateway_config();
        self.setup_gateway_directory_client(gateway_config, user_agent)
            .await?;

        let selected_gateways = self.select_gateways().await?;
        let mixnet_runtime_config = self.get_mixnet_runtime_config(&selected_gateways);
        let mixnet_client = self
            .shutdown_token
            .run_until_cancelled(crate::mixnet::connect_mixnet_client(
                self.task_manager.subscribe_named("mixnet_client_main"),
                mixnet_runtime_config,
            ))
            .await
            .ok_or(Error::Tunnel(Box::new(
                tunnel::Error::StartMixnetClientTimeout,
            )))?
            .map_err(|e| Error::Tunnel(Box::new(tunnel::Error::MixnetClient(e))))?;
        *self.shared_mixnet_client.lock().await = Some(mixnet_client);

        let StartTunnelResult {
            tunnel_interface,
            tunnel_conn_data,
            mut tunnel_handle,
        } = match self.tunnel_parameters.tunnel_settings.tunnel_type {
            TunnelType::Mixnet => self.start_mixnet_tunnel(&selected_gateways).await?,
            TunnelType::Wireguard => {
                match self
                    .tunnel_parameters
                    .tunnel_settings
                    .wireguard_tunnel_options
                    .multihop_mode
                {
                    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
                    WireguardMultihopMode::TunTun => {
                        self.start_wireguard_tunnel(selected_gateways.clone())
                            .await?
                    }
                    WireguardMultihopMode::Netstack => {
                        self.start_wireguard_netstack_tunnel(selected_gateways.clone())
                            .await?
                    }
                }
            }
        };

        let connection_data = ConnectionData {
            entry_gateway: Gateway::from(*selected_gateways.entry),
            exit_gateway: Gateway::from(*selected_gateways.exit),
            connected_at: None,
            tunnel: tunnel_conn_data,
        };

        let (reply_tx, reply_rx) = oneshot::channel();
        self.send_event(TunnelMonitorEvent::InterfaceUp {
            tunnel_interface: tunnel_interface.clone(),
            connection_data: Box::new(connection_data.clone()),
            reply_tx,
        });

        if tokio::time::timeout(REPLY_TIMEOUT, reply_rx).await.is_err() {
            tracing::warn!("Interface up reply timeout");
        }

        // todo: do initial ping

        let (background_error_tx, background_error_rx) = mpsc::channel(1);

        let discovery_refresher_handle = self
            .tunnel_parameters
            .nym_config
            .config_path
            .as_ref()
            .and_then(|config_path: &PathBuf| config_path.parent())
            .map(|config_dir| {
                start_background_file_refresh(
                    config_dir.to_path_buf(),
                    self.tunnel_parameters.nym_config.network_env.clone(),
                    background_error_tx,
                    self.shutdown_token.child_token(),
                )
            });

        let connection_data = ConnectionData {
            connected_at: Some(OffsetDateTime::now_utc()),
            ..connection_data
        };
        self.send_event(TunnelMonitorEvent::Up {
            tunnel_interface,
            connection_data: Box::new(connection_data),
        });

        self.wait_for_shutdown_event(background_error_rx).await;

        tracing::info!("Wait for tunnel to exit");
        tunnel_handle.cancel().await;

        let tun_devices = tunnel_handle
            .wait()
            .await
            .inspect_err(|e| {
                trace_err_chain!(e, "Failed to gracefully shutdown the tunnel");
            })
            .unwrap_or_default();

        if let Some(discovery_refresher_handle) = discovery_refresher_handle {
            tracing::debug!("Wait for discovery refresher to exit");
            if let Err(e) = discovery_refresher_handle.await {
                tracing::error!("Failed to join on discovery refresher: {}", e);
            }
        }
        tracing::info!("Tunnel monitor finished");

        Ok(tun_devices)
    }

    async fn start_status_listener(&mut self, cancel_token: CancellationToken) -> JoinHandle<()> {
        let (status_tx, status_rx) = futures::channel::mpsc::channel(10);
        self.task_manager
            .start_status_listener(status_tx, TaskStatus::Ready)
            .await;

        StatusListener::spawn(status_rx, self.mixnet_event_sender.clone(), cancel_token)
    }

    async fn wait_for_shutdown_event(
        &mut self,
        mut background_error_rx: tokio::sync::mpsc::Receiver<()>,
    ) {
        tokio::select! {
            _ = self.shutdown_token.cancelled() => {}
            task_error = self.task_manager.wait_for_error() => {
                match task_error {
                    Some(task_error) => {
                        tracing::error!("Task manager quit with error: {}", task_error);
                    }
                    None => {
                        tracing::error!("Task manager quit without error");
                    }
                }
            }
            ret = background_error_rx.recv() => {
                if ret.is_some() {
                    tracing::error!("Discovery refresher quit with inconsistent network error");
                } else {
                    tracing::debug!("Discovery refresher quit without error");
                }
            }
        }

        // Trigger cancellation since many other tasks depend on shutdown token
        self.shutdown_token.cancel();
    }

    async fn select_gateways(&self) -> Result<SelectedGateways> {
        if let Some(selected_gateways) = self.tunnel_parameters.selected_gateways.as_ref() {
            Ok(selected_gateways.clone())
        } else {
            let new_gateways = tunnel::select_gateways(
                self.gateway_directory_client.clone(),
                self.tunnel_parameters.tunnel_settings.tunnel_type,
                self.tunnel_parameters.tunnel_settings.entry_point.clone(),
                self.tunnel_parameters.tunnel_settings.exit_point.clone(),
                self.shutdown_token.child_token(),
            )
            .await
            .map_err(Box::new)?;

            let (reply_tx, reply_rx) = oneshot::channel();
            self.send_event(TunnelMonitorEvent::SelectedGateways {
                gateways: Box::new(new_gateways.clone()),
                reply_tx,
            });

            // Wait for reply before proceeding to connect to let state machine configure firewall.
            if tokio::time::timeout(REPLY_TIMEOUT, reply_rx).await.is_err() {
                tracing::warn!("Failed to receive selected gateways reply in time");
            }

            Ok(new_gateways)
        }
    }

    fn send_event(&self, event: TunnelMonitorEvent) {
        if let Err(e) = self.monitor_event_sender.send(event) {
            if !self.shutdown_token.is_cancelled() {
                tracing::error!("Failed to send monitor event: {}", e);
            }
        }
    }

    fn get_user_agent(&self) -> UserAgent {
        self.tunnel_parameters
            .tunnel_settings
            .user_agent
            .clone()
            .unwrap_or(UserAgent::from(nym_bin_common::bin_info_local_vergen!()))
    }

    fn get_gateway_config(&self) -> nym_gateway_directory::Config {
        let mut gateway_config = self.tunnel_parameters.nym_config.gateway_config.clone();
        let gateway_performance_options = self
            .tunnel_parameters
            .tunnel_settings
            .gateway_performance_options;
        let gateway_min_performance = GatewayMinPerformance::from_percentage_values(
            gateway_performance_options
                .mixnet_min_performance
                .map(u64::from),
            gateway_performance_options
                .vpn_min_performance
                .map(u64::from),
        );

        match gateway_min_performance {
            Ok(gateway_min_performance) => {
                gateway_config =
                    gateway_config.with_min_gateway_performance(gateway_min_performance);
            }
            Err(e) => {
                tracing::error!(
                    "Invalid gateway performance values. Will carry on with initial values. Error: {}",
                    e
                );
            }
        }

        gateway_config
    }

    fn get_mixnet_runtime_config(
        &self,
        selected_gateways: &SelectedGateways,
    ) -> MixnetRuntimeConfig {
        let mixnet_client_config = self
            .tunnel_parameters
            .tunnel_settings
            .mixnet_client_config
            .clone()
            .unwrap_or_default();
        let mixnet_client_config = match self.tunnel_parameters.tunnel_settings.tunnel_type {
            TunnelType::Mixnet => mixnet_client_config,
            TunnelType::Wireguard => {
                MixnetClientConfig {
                    // Always disable poisson process for outbound traffic in wireguard.
                    disable_poisson_rate: true,
                    // Always disable background cover traffic in wireguard.
                    disable_background_cover_traffic: true,
                    ..mixnet_client_config
                }
            }
        };

        MixnetRuntimeConfig {
            network_env: self.tunnel_parameters.nym_config.network_env.clone(),
            mixnet_entry_gateway: selected_gateways.entry.identity(),
            mixnet_client_key_storage_path: self.tunnel_parameters.nym_config.data_path.clone(),
            mixnet_client_config,
            enable_credentials_mode: self
                .tunnel_parameters
                .tunnel_settings
                .enable_credentials_mode,
            stats_recipient_address: self
                .tunnel_parameters
                .tunnel_settings
                .statistics_recipient
                .as_deref()
                .copied(),
            two_hop_mode: self.tunnel_parameters.tunnel_settings.tunnel_type
                == TunnelType::Wireguard,
            custom_topology_provider: self.custom_topology_provider.clone(),
            #[cfg(any(target_os = "android", target_os = "linux"))]
            connection_fd_callback: socket_bypass::get_socket_bypass_fn(
                #[cfg(target_os = "android")]
                self.tun_provider.clone(),
            ),
        }
    }

    async fn setup_gateway_directory_client(
        &mut self,
        gateway_config: nym_gateway_directory::Config,
        user_agent: UserAgent,
    ) -> Result<()> {
        let gateway_directory_client = GatewayClient::new_with_resolver_overrides(
            gateway_config,
            user_agent,
            self.tunnel_parameters
                .resolved_gateway_config
                .nym_vpn_api_socket_addrs
                .as_deref(),
        )
        .map_err(|e| Box::new(tunnel::Error::CreateGatewayClient(e)))?;

        self.gateway_directory_client
            .update_client(gateway_directory_client)
            .await;
        self.gateway_directory_client.refresh_all().await;

        Ok(())
    }

    async fn setup_account(&mut self) -> Result<()> {
        // Check that the device time is synced as a precondition to continuing
        account::check_device_time_sync(
            self.account_commands.clone(),
            self.shutdown_token.child_token(),
        )
        .await?;

        // Check if we have ticketbooks already stored, then we can sidestep the account and device
        // sync
        let is_already_tickets_stored = self
            .account_commands
            .get_available_tickets()
            .await
            .map_err(|err| {
                account::Error::from(RequestZkNymError::CredentialStorage(err.to_string()))
            })?
            .is_all_ticket_types_above_soft_threshold();

        if is_already_tickets_stored {
            // If we have tickets stored, trigger sync and register in the background while we
            // proceed anyway.
            self.send_event(TunnelMonitorEvent::SyncingAccount);
            self.account_commands.background_sync_account_state();
            self.account_commands.background_sync_device_state();
        } else {
            // If we don't have ticket stored, go through the steps one by one, syncing and
            // registering and getting credentials.
            self.send_event(TunnelMonitorEvent::SyncingAccount);
            account::wait_for_account_sync(
                self.account_commands.clone(),
                self.shutdown_token.child_token(),
            )
            .await?;

            account::wait_for_device_sync(
                self.account_commands.clone(),
                self.shutdown_token.child_token(),
            )
            .await?;

            self.send_event(TunnelMonitorEvent::RegisteringDevice);
            account::wait_for_device_register(
                self.account_commands.clone(),
                self.shutdown_token.child_token(),
            )
            .await?;
        }

        if self
            .tunnel_parameters
            .tunnel_settings
            .enable_credentials_mode
        {
            self.send_event(TunnelMonitorEvent::RequestingZkNyms);
            account::wait_for_credentials_ready(
                self.account_commands.clone(),
                self.shutdown_token.child_token(),
            )
            .await?;
        }

        Ok(())
    }

    async fn start_mixnet_tunnel(
        &mut self,
        selected_gateways: &SelectedGateways,
    ) -> Result<StartTunnelResult> {
        let assigned_addresses = tunnel::mixnet::connector::register_with_ipr(
            selected_gateways,
            self.shared_mixnet_client.clone(),
            self.gateway_directory_client.clone(),
            self.shutdown_token.child_token(),
        )
        .await
        .map_err(Box::new)?;

        let mtu: u16 = self
            .tunnel_parameters
            .tunnel_settings
            .mixnet_tunnel_options
            .mtu
            .unwrap_or(DEFAULT_TUN_MTU);

        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        let tun_device = TunnelDevice::new(
            #[cfg(windows)]
            WINTUN_TUNNEL_TYPE,
            assigned_addresses.interface_addresses,
            None,
            mtu,
        )
        .map_err(Error::CreateTunDevice)?;

        #[cfg(any(target_os = "ios", target_os = "android"))]
        let tun_device = {
            let packet_tunnel_settings = tunnel_provider::tunnel_settings::TunnelSettings {
                dns_servers: self
                    .tunnel_parameters
                    .tunnel_settings
                    .dns
                    .ip_addresses(&crate::DEFAULT_DNS_SERVERS)
                    .to_vec(),
                interface_addresses: vec![
                    IpNetwork::V4(Ipv4Network::from(
                        assigned_addresses.interface_addresses.ipv4,
                    )),
                    IpNetwork::V6(Ipv6Network::from(
                        assigned_addresses.interface_addresses.ipv6,
                    )),
                ],
                remote_addresses: vec![assigned_addresses.entry_mixnet_gateway_ip],
                mtu,
            };

            TunnelDevice::new(packet_tunnel_settings).await?
        };

        let tun_name = tun_device.name().map_err(Error::GetTunDeviceName)?;

        tracing::info!("Created tun device: {}", tun_name);

        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        {
            let routing_config = RoutingConfig::Mixnet {
                tun_name: tun_name.clone(),
                #[cfg(not(target_os = "linux"))]
                entry_gateway_address: assigned_addresses.entry_mixnet_gateway_ip,
            };

            self.set_routes(routing_config).await?;
        }

        let tunnel_conn_data = TunnelConnectionData::Mixnet(MixnetConnectionData {
            nym_address: NymAddress::from(assigned_addresses.mixnet_client_address),
            exit_ipr: NymAddress::from(assigned_addresses.exit_mix_addresses),
            entry_ip: assigned_addresses.entry_mixnet_gateway_ip,
            exit_ip: assigned_addresses.exit_mixnet_gateway_ip,
            ipv4: assigned_addresses.interface_addresses.ipv4,
            ipv6: assigned_addresses.interface_addresses.ipv6,
        });

        let tunnel_metadata = TunnelMetadata {
            interface: tun_name,
            ips: vec![
                IpAddr::V4(assigned_addresses.interface_addresses.ipv4),
                IpAddr::V6(assigned_addresses.interface_addresses.ipv6),
            ],
            ipv4_gateway: None,
            ipv6_gateway: None,
        };

        let tunnel_handle = tunnel::mixnet::connected_tunnel::start_mixnet_tunnel(
            &self.task_manager,
            self.shared_mixnet_client.clone(),
            assigned_addresses,
            tun_device.into_inner(),
            self.shutdown_token.child_token(),
        )
        .await
        .map_err(Box::new)?;

        Ok(StartTunnelResult {
            tunnel_interface: TunnelInterface::One(tunnel_metadata),
            tunnel_handle: AnyTunnelHandle::from(tunnel_handle),
            tunnel_conn_data,
        })
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    async fn start_wireguard_netstack_tunnel(
        &mut self,
        selected_gateways: SelectedGateways,
    ) -> Result<StartTunnelResult> {
        let connect_options = tunnel::wireguard::connector::ConnectOptions {
            data_path: self.tunnel_parameters.nym_config.data_path.clone(),
            network: self.tunnel_parameters.nym_config.network_env.clone(),
            enable_credentials_mode: self
                .tunnel_parameters
                .tunnel_settings
                .enable_credentials_mode,
            selected_gateways: selected_gateways,
        };

        let connector_result = tunnel::wireguard::connector::register_with_gateways(
            &self.task_manager,
            self.shared_mixnet_client.clone(),
            self.gateway_directory_client.clone(),
            connect_options,
            self.shutdown_token.child_token(),
        )
        .await
        .map_err(Box::new)?;

        let connected_tunnel = tunnel::wireguard::connected_tunnel::ConnectedTunnel::new(
            connector_result.entry_gateway_client,
            connector_result.exit_gateway_client,
            connector_result.connection_data,
            connector_result.bandwidth_controller_handle,
            connector_result.auth_client_mixnet_listener_handle,
        );
        let conn_data = connected_tunnel.connection_data();

        let exit_tun = TunnelDevice::new(
            IpPair {
                ipv4: conn_data.exit.private_ipv4,
                ipv6: conn_data.exit.private_ipv6,
            },
            Some(conn_data.entry.private_ipv4),
            connected_tunnel.exit_mtu(),
        )
        .map_err(Error::CreateTunDevice)?;
        let exit_tun_name = exit_tun.name().map_err(Error::GetTunDeviceName)?;
        tracing::info!("Created exit tun device: {}", exit_tun_name);

        let routing_config = RoutingConfig::WireguardNetstack {
            exit_tun_name: exit_tun_name.clone(),
            #[cfg(not(target_os = "linux"))]
            entry_gateway_address: conn_data.entry.endpoint.ip(),
        };

        self.set_routes(routing_config).await?;

        let tunnel_conn_data = TunnelConnectionData::Wireguard(WireguardConnectionData {
            entry: WireguardNode::from(conn_data.entry.clone()),
            exit: WireguardNode::from(conn_data.exit.clone()),
        });

        let dns_config = self
            .tunnel_parameters
            .tunnel_settings
            .dns
            .to_dns_config()
            .resolve(
                &crate::DEFAULT_DNS_SERVERS,
                #[cfg(target_os = "macos")]
                53,
            );
        let tunnel_options = TunnelOptions::Netstack(NetstackTunnelOptions {
            exit_tun: exit_tun.into_inner(),
            dns: dns_config.tunnel_config().to_vec(),
        });

        let tunnel_metadata = TunnelMetadata {
            interface: exit_tun_name,
            ips: vec![
                IpAddr::V4(conn_data.exit.private_ipv4),
                IpAddr::V6(conn_data.exit.private_ipv6),
            ],
            ipv4_gateway: Some(conn_data.entry.private_ipv4),
            ipv6_gateway: Some(conn_data.entry.private_ipv6),
        };

        let tunnel_handle = AnyTunnelHandle::from(
            connected_tunnel
                .run(tunnel_options)
                .await
                .map_err(Box::new)?,
        );

        Ok(StartTunnelResult {
            tunnel_interface: TunnelInterface::One(tunnel_metadata),
            tunnel_conn_data,
            tunnel_handle,
        })
    }

    #[cfg(windows)]
    async fn start_wireguard_netstack_tunnel(
        &mut self,
        connected_mixnet: ConnectedMixnet,
    ) -> Result<StartTunnelResult> {
        let connected_tunnel = connected_mixnet
            .connect_wireguard_tunnel(
                &self.tunnel_parameters.nym_config.network_env,
                self.tunnel_parameters
                    .tunnel_settings
                    .enable_credentials_mode,
                self.shutdown_token.child_token(),
            )
            .await
            .map_err(Box::new)?;
        let conn_data = connected_tunnel.connection_data();
        let entry_gateway_address = conn_data.entry.endpoint.ip();

        let exit_adapter_config = WintunAdapterConfig {
            interface_ipv4: conn_data.exit.private_ipv4,
            interface_ipv6: conn_data.exit.private_ipv6,
            gateway_ipv4: Some(conn_data.entry.private_ipv4),
            gateway_ipv6: Some(conn_data.entry.private_ipv6),
        };
        let mut tunnel_metadata = TunnelMetadata {
            interface: "".to_owned(),
            ips: vec![
                IpAddr::V4(conn_data.exit.private_ipv4),
                IpAddr::V6(conn_data.exit.private_ipv6),
            ],
            ipv4_gateway: Some(conn_data.entry.private_ipv4),
            ipv6_gateway: Some(conn_data.entry.private_ipv6),
        };

        let tunnel_conn_data = TunnelConnectionData::Wireguard(WireguardConnectionData {
            entry: WireguardNode::from(conn_data.entry.clone()),
            exit: WireguardNode::from(conn_data.exit.clone()),
        });

        let dns_config = self
            .tunnel_parameters
            .tunnel_settings
            .dns
            .to_dns_config()
            .resolve(&crate::DEFAULT_DNS_SERVERS);
        let tunnel_options = TunnelOptions::Netstack(NetstackTunnelOptions {
            exit_tun_name: WG_EXIT_WINTUN_NAME.to_owned(),
            exit_tun_guid: WG_EXIT_WINTUN_GUID.to_owned(),
            wintun_tunnel_type: WINTUN_TUNNEL_TYPE.to_owned(),
            dns: dns_config.tunnel_config().to_vec(),
        });

        let tunnel_handle = connected_tunnel
            .run(
                #[cfg(windows)]
                self.route_handler.clone(),
                tunnel_options,
            )
            .await
            .map_err(Box::new)?;

        let wintun_exit_interface = tunnel_handle
            .exit_wintun_interface()
            .expect("failed to obtain wintun exit interface");

        tracing::info!("Created wintun device: {}", wintun_exit_interface.name);

        wintun::setup_wintun_adapter(wintun_exit_interface.windows_luid(), exit_adapter_config)?;

        let routing_config = RoutingConfig::WireguardNetstack {
            exit_tun_name: wintun_exit_interface.name.clone(),
            entry_gateway_address,
        };
        // todo: make sure to shutdown tunnel_handle on failure!
        self.set_routes(routing_config).await?;

        // Update interface name in tunnel metadata
        tunnel_metadata.interface = wintun_exit_interface.name.clone();

        Ok(StartTunnelResult {
            tunnel_interface: TunnelInterface::One(tunnel_metadata),
            tunnel_handle: AnyTunnelHandle::from(tunnel_handle),
            tunnel_conn_data,
        })
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    async fn start_wireguard_tunnel(
        &mut self,
        selected_gateways: SelectedGateways,
    ) -> Result<StartTunnelResult> {
        let connect_options = tunnel::wireguard::connector::ConnectOptions {
            data_path: self.tunnel_parameters.nym_config.data_path.clone(),
            network: self.tunnel_parameters.nym_config.network_env.clone(),
            enable_credentials_mode: self
                .tunnel_parameters
                .tunnel_settings
                .enable_credentials_mode,
            selected_gateways: selected_gateways,
        };

        let connector_result = tunnel::wireguard::connector::register_with_gateways(
            &self.task_manager,
            self.shared_mixnet_client.clone(),
            self.gateway_directory_client.clone(),
            connect_options,
            self.shutdown_token.child_token(),
        )
        .await
        .map_err(Box::new)?;

        let connected_tunnel = tunnel::wireguard::connected_tunnel::ConnectedTunnel::new(
            connector_result.entry_gateway_client,
            connector_result.exit_gateway_client,
            connector_result.connection_data,
            connector_result.bandwidth_controller_handle,
            connector_result.auth_client_mixnet_listener_handle,
        );

        let conn_data = connected_tunnel.connection_data();

        let entry_tun = TunnelDevice::new(
            IpPair {
                ipv4: conn_data.entry.private_ipv4,
                ipv6: conn_data.entry.private_ipv6,
            },
            None,
            connected_tunnel.entry_mtu(),
        )
        .map_err(Error::CreateTunDevice)?;
        let entry_tun_name = entry_tun.name().map_err(Error::GetTunDeviceName)?;
        tracing::info!("Created entry tun device: {}", entry_tun_name);

        let entry_tunnel_metadata = TunnelMetadata {
            interface: entry_tun_name,
            ips: vec![
                IpAddr::V4(conn_data.entry.private_ipv4),
                IpAddr::V6(conn_data.entry.private_ipv6),
            ],
            ipv4_gateway: None,
            ipv6_gateway: None,
        };

        let exit_tun = TunnelDevice::new(
            IpPair {
                ipv4: conn_data.exit.private_ipv4,
                ipv6: conn_data.exit.private_ipv6,
            },
            // todo: this needs to be able to set both destinations?
            Some(conn_data.entry.private_ipv4),
            connected_tunnel.exit_mtu(),
        )
        .map_err(Error::CreateTunDevice)?;
        let exit_tun_name = exit_tun.name().map_err(Error::GetTunDeviceName)?;
        tracing::info!("Created exit tun device: {}", exit_tun_name);

        let exit_tunnel_metadata = TunnelMetadata {
            interface: exit_tun_name.clone(),
            ips: vec![
                IpAddr::V4(conn_data.exit.private_ipv4),
                IpAddr::V6(conn_data.exit.private_ipv6),
            ],
            ipv4_gateway: Some(conn_data.entry.private_ipv4),
            ipv6_gateway: Some(conn_data.entry.private_ipv6),
        };

        let routing_config = RoutingConfig::Wireguard {
            entry_tun_name: entry_tunnel_metadata.interface.clone(),
            exit_tun_name: exit_tunnel_metadata.interface.clone(),
            #[cfg(not(target_os = "linux"))]
            entry_gateway_address: conn_data.entry.endpoint.ip(),
            exit_gateway_address: conn_data.exit.endpoint.ip(),
        };
        self.set_routes(routing_config).await?;

        let tunnel_conn_data = TunnelConnectionData::Wireguard(WireguardConnectionData {
            entry: WireguardNode::from(conn_data.entry.clone()),
            exit: WireguardNode::from(conn_data.exit.clone()),
        });

        let dns_config = self
            .tunnel_parameters
            .tunnel_settings
            .dns
            .to_dns_config()
            .resolve(
                &crate::DEFAULT_DNS_SERVERS,
                #[cfg(target_os = "macos")]
                53,
            );
        let tunnel_options = TunnelOptions::TunTun(TunTunTunnelOptions {
            entry_tun: entry_tun.into_inner(),
            exit_tun: exit_tun.into_inner(),
            dns: dns_config.tunnel_config().to_vec(),
        });

        let tunnel_handle = AnyTunnelHandle::from(
            connected_tunnel
                .run(tunnel_options)
                .await
                .map_err(Box::new)?,
        );

        Ok(StartTunnelResult {
            tunnel_interface: TunnelInterface::Two {
                entry: entry_tunnel_metadata,
                exit: exit_tunnel_metadata,
            },
            tunnel_conn_data,
            tunnel_handle,
        })
    }

    #[cfg(windows)]
    async fn start_wireguard_tunnel(
        &mut self,
        connected_mixnet: ConnectedMixnet,
    ) -> Result<StartTunnelResult> {
        let connected_tunnel = connected_mixnet
            .connect_wireguard_tunnel(
                &self.tunnel_parameters.nym_config.network_env,
                self.tunnel_parameters
                    .tunnel_settings
                    .enable_credentials_mode,
                self.shutdown_token.child_token(),
            )
            .await
            .map_err(Box::new)?;
        let conn_data = connected_tunnel.connection_data();

        let entry_gateway_address = conn_data.entry.endpoint.ip();
        let exit_gateway_address = conn_data.exit.endpoint.ip();

        let entry_adapter_config = WintunAdapterConfig {
            interface_ipv4: conn_data.entry.private_ipv4,
            interface_ipv6: conn_data.entry.private_ipv6,
            gateway_ipv4: None,
            gateway_ipv6: None,
        };
        let mut entry_tunnel_metadata = TunnelMetadata {
            interface: "".to_owned(),
            ips: vec![
                IpAddr::V4(conn_data.entry.private_ipv4),
                IpAddr::V6(conn_data.entry.private_ipv6),
            ],
            ipv4_gateway: None,
            ipv6_gateway: None,
        };

        let exit_adapter_config = WintunAdapterConfig {
            interface_ipv4: conn_data.exit.private_ipv4,
            interface_ipv6: conn_data.exit.private_ipv6,
            gateway_ipv4: Some(conn_data.entry.private_ipv4),
            gateway_ipv6: Some(conn_data.entry.private_ipv6),
        };
        let mut exit_tunnel_metadata = TunnelMetadata {
            interface: "".to_owned(),
            ips: vec![
                IpAddr::V4(conn_data.exit.private_ipv4),
                IpAddr::V6(conn_data.exit.private_ipv6),
            ],
            ipv4_gateway: Some(conn_data.entry.private_ipv4),
            ipv6_gateway: Some(conn_data.entry.private_ipv6),
        };

        let tunnel_conn_data = TunnelConnectionData::Wireguard(WireguardConnectionData {
            entry: WireguardNode::from(conn_data.entry.clone()),
            exit: WireguardNode::from(conn_data.exit.clone()),
        });

        let dns_config = self
            .tunnel_parameters
            .tunnel_settings
            .dns
            .to_dns_config()
            .resolve(&crate::DEFAULT_DNS_SERVERS);
        let tunnel_options = TunnelOptions::TunTun(TunTunTunnelOptions {
            entry_tun_name: WG_ENTRY_WINTUN_NAME.to_owned(),
            entry_tun_guid: WG_ENTRY_WINTUN_GUID.to_owned(),
            exit_tun_name: WG_EXIT_WINTUN_NAME.to_owned(),
            exit_tun_guid: WG_EXIT_WINTUN_GUID.to_owned(),
            wintun_tunnel_type: WINTUN_TUNNEL_TYPE.to_owned(),
            dns: dns_config.tunnel_config().to_vec(),
        });

        let tunnel_handle = connected_tunnel
            .run(
                #[cfg(windows)]
                self.route_handler.clone(),
                tunnel_options,
            )
            .await
            .map_err(Box::new)?;

        let wintun_entry_interface = tunnel_handle
            .entry_wintun_interface()
            .expect("failed to obtain wintun entry interface");
        let wintun_exit_interface = tunnel_handle
            .exit_wintun_interface()
            .expect("failed to obtain wintun exit interface");

        tracing::info!(
            "Created entry wintun device: {}",
            wintun_entry_interface.name
        );
        tracing::info!("Created exit wintun device: {}", wintun_exit_interface.name);

        wintun::setup_wintun_adapter(wintun_entry_interface.windows_luid(), entry_adapter_config)?;
        wintun::setup_wintun_adapter(wintun_exit_interface.windows_luid(), exit_adapter_config)?;

        // Update interface names in tunnel metadata
        entry_tunnel_metadata.interface = wintun_entry_interface.name.clone();
        exit_tunnel_metadata.interface = wintun_exit_interface.name.clone();

        let tunnel_interface = TunnelInterface::Two {
            entry: entry_tunnel_metadata,
            exit: exit_tunnel_metadata,
        };

        let routing_config = RoutingConfig::Wireguard {
            entry_tun_name: wintun_entry_interface.name.clone(),
            exit_tun_name: wintun_exit_interface.name.clone(),
            entry_gateway_address,
            exit_gateway_address,
        };
        // todo: make sure to shutdown tunnel_handle on failure!
        self.set_routes(routing_config).await?;

        Ok(StartTunnelResult {
            tunnel_interface,
            tunnel_handle: AnyTunnelHandle::from(tunnel_handle),
            tunnel_conn_data,
        })
    }

    #[cfg(any(target_os = "ios", target_os = "android"))]
    async fn start_wireguard_netstack_tunnel(
        &self,
        connected_mixnet: ConnectedMixnet,
    ) -> Result<StartTunnelResult> {
        let connected_tunnel = connected_mixnet
            .connect_wireguard_tunnel(
                &self.tunnel_parameters.nym_config.network_env,
                self.tunnel_parameters
                    .tunnel_settings
                    .enable_credentials_mode,
                self.shutdown_token.child_token(),
            )
            .await
            .map_err(Box::new)?;

        let conn_data = connected_tunnel.connection_data();

        let packet_tunnel_settings = tunnel_provider::tunnel_settings::TunnelSettings {
            dns_servers: self
                .tunnel_parameters
                .tunnel_settings
                .dns
                .ip_addresses(&crate::DEFAULT_DNS_SERVERS)
                .to_vec(),
            interface_addresses: vec![
                IpNetwork::V4(Ipv4Network::from(conn_data.exit.private_ipv4)),
                IpNetwork::V6(Ipv6Network::from(conn_data.exit.private_ipv6)),
            ],
            remote_addresses: vec![conn_data.entry.endpoint.ip()],
            mtu: connected_tunnel.exit_mtu(),
        };

        let tun_device = TunnelDevice::new(packet_tunnel_settings).await?;
        let interface = tun_device.name().map_err(Error::GetTunDeviceName)?;
        let tunnel_metadata = TunnelMetadata {
            interface,
            ips: vec![
                IpAddr::V4(conn_data.exit.private_ipv4),
                IpAddr::V6(conn_data.exit.private_ipv6),
            ],
            ipv4_gateway: None,
            ipv6_gateway: None,
        };

        tracing::info!("Created tun device: {}", tunnel_metadata.interface);

        let tunnel_conn_data = TunnelConnectionData::Wireguard(WireguardConnectionData {
            entry: WireguardNode::from(conn_data.entry.clone()),
            exit: WireguardNode::from(conn_data.exit.clone()),
        });

        let dns_servers = self
            .tunnel_parameters
            .tunnel_settings
            .dns
            .ip_addresses(&crate::DEFAULT_DNS_SERVERS)
            .to_vec();

        let tunnel_handle = connected_tunnel
            .run(
                tun_device,
                dns_servers,
                #[cfg(target_os = "android")]
                self.tun_provider.clone(),
            )
            .await
            .map_err(Box::new)?;

        Ok(StartTunnelResult {
            tunnel_conn_data,
            tunnel_interface: TunnelInterface::One(tunnel_metadata),
            tunnel_handle: AnyTunnelHandle::from(tunnel_handle),
        })
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    async fn set_routes(&mut self, routing_config: RoutingConfig) -> Result<()> {
        self.route_handler
            .add_routes(routing_config)
            .await
            .map_err(Error::AddRoutes)?;

        Ok(())
    }
}

fn wait_delay(retry_attempt: u32) -> Duration {
    let multiplier = retry_attempt.saturating_mul(DELAY_MULTIPLIER);
    let delay = INITIAL_WAIT_DELAY.saturating_mul(multiplier);
    cmp::min(delay, MAX_WAIT_DELAY)
}

pub struct StartTunnelResult {
    tunnel_interface: TunnelInterface,
    tunnel_conn_data: TunnelConnectionData,
    tunnel_handle: AnyTunnelHandle,
}
