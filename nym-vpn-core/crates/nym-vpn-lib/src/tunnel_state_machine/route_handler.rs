// Copyright 2023 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

use std::{collections::HashSet, fmt, net::IpAddr};

use ipnetwork::IpNetwork;

use nym_common::trace_err_chain;
#[cfg(not(target_os = "linux"))]
use nym_routing::NetNode;
#[cfg(windows)]
pub use nym_routing::{Callback, CallbackHandle};
use nym_routing::{Node, RequiredRoute, RouteManagerHandle};

#[cfg(target_os = "linux")]
pub const TUNNEL_TABLE_ID: u32 = 0x14d;

#[cfg(target_os = "linux")]
pub const TUNNEL_FWMARK: u32 = 0x14d;

/// Size of IPv4 header in bytes
pub const IPV4_HEADER_SIZE: u16 = 20;

/// Size of IPv6 header in bytes
pub const IPV6_HEADER_SIZE: u16 = 40;

/// Size of wireguard header in bytes
pub const WIREGUARD_HEADER_SIZE: u16 = 40;

/// Size of ICMP header in bytes
pub const ICMP_HEADER_SIZE: u16 = 8;

/// Smallest allowed MTU for IPv4 in bytes
pub const MIN_IPV4_MTU: u16 = 576;

/// Smallest allowed MTU for IPv6 in bytes
pub const MIN_IPV6_MTU: u16 = 1280;

pub enum RoutingConfig {
    Mixnet {
        tun_name: String,
        #[cfg(not(target_os = "linux"))]
        entry_gateway_address: IpAddr,
    },
    Wireguard {
        entry_tun_name: String,
        exit_tun_name: String,
        #[cfg(not(target_os = "linux"))]
        entry_gateway_address: IpAddr,
        exit_gateway_address: IpAddr,
        entry_mtu: u16,
        exit_mtu: u16,
    },
    WireguardNetstack {
        exit_tun_name: String,
        #[cfg(not(target_os = "linux"))]
        entry_gateway_address: IpAddr,
    },
}

#[derive(Debug, Clone)]
pub struct RouteHandler {
    route_manager: RouteManagerHandle,
}

impl RouteHandler {
    pub async fn new() -> Result<Self> {
        let route_manager = RouteManagerHandle::spawn(
            #[cfg(target_os = "linux")]
            TUNNEL_TABLE_ID,
            #[cfg(target_os = "linux")]
            TUNNEL_FWMARK,
        )
        .await?;
        Ok(Self { route_manager })
    }

    pub async fn add_routes(&mut self, routing_config: RoutingConfig) -> Result<()> {
        let routes = Self::get_routes(routing_config);

        #[cfg(target_os = "linux")]
        self.route_manager.create_routing_rules().await?;

        self.route_manager.add_routes(routes).await?;

        Ok(())
    }

    pub async fn remove_routes(&mut self) {
        if let Err(e) = self.route_manager.clear_routes() {
            trace_err_chain!(e, "Failed to remove routes");
        }

        #[cfg(target_os = "linux")]
        if let Err(e) = self.route_manager.clear_routing_rules().await {
            trace_err_chain!(e, "Failed to remove routing rules");
        }
    }

    #[cfg(target_os = "macos")]
    pub async fn refresh_routes(&mut self) {
        if let Err(e) = self.route_manager.refresh_routes() {
            trace_err_chain!(e, "Failed to refresh routes");
        }
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    pub async fn get_mtu_for_route(&self, ip: IpAddr) -> Result<u16> {
        self.route_manager.get_mtu_for_route(ip).inspect_err(|e| {
            trace_err_chain!(e, "Failed to get mtu for route");
        })
    }

    #[cfg(windows)]
    pub async fn add_default_route_listener(
        &mut self,
        event_handler: Callback,
    ) -> Result<CallbackHandle> {
        self.route_manager
            .add_default_route_change_callback(event_handler)
            .await
            .map_err(Error::from)
    }

    pub async fn stop(self) {
        self.route_manager.stop().await;
    }

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub fn inner_handle(&self) -> nym_routing::RouteManagerHandle {
        self.route_manager.clone()
    }

    fn get_routes(routing_config: RoutingConfig) -> HashSet<RequiredRoute> {
        let mut routes = HashSet::new();

        match routing_config {
            RoutingConfig::Mixnet {
                tun_name,
                #[cfg(not(target_os = "linux"))]
                entry_gateway_address,
            } => {
                #[cfg(not(target_os = "linux"))]
                routes.insert(RequiredRoute::new(
                    IpNetwork::from(entry_gateway_address),
                    NetNode::DefaultNode,
                ));
                routes.extend(Self::get_default_routes(&tun_name));
            }
            RoutingConfig::Wireguard {
                entry_tun_name,
                exit_tun_name,
                #[cfg(not(target_os = "linux"))]
                entry_gateway_address,
                exit_gateway_address,
                entry_mtu,
                exit_mtu,
            } => {
                #[cfg(not(target_os = "linux"))]
                {
                    #[allow(unused_mut)]
                    let mut entry_via_default_gw_route = RequiredRoute::new(
                        IpNetwork::from(entry_gateway_address),
                        NetNode::DefaultNode,
                    );

                    #[cfg(target_os = "macos")]
                    {
                        entry_via_default_gw_route = entry_via_default_gw_route.mtu(entry_mtu);
                    }

                    routes.insert(entry_via_default_gw_route);
                }

                #[allow(unused_mut)]
                let mut exit_via_entry_route = RequiredRoute::new(
                    IpNetwork::from(exit_gateway_address),
                    Node::device(entry_tun_name.to_owned()),
                );

                #[cfg(any(target_os = "linux", target_os = "macos"))]
                {
                    exit_via_entry_route = exit_via_entry_route.mtu(exit_mtu);
                }

                routes.insert(exit_via_entry_route);

                for default_route in Self::get_default_routes(&exit_tun_name) {
                    routes.insert(default_route);
                }
            }
            RoutingConfig::WireguardNetstack {
                exit_tun_name,
                #[cfg(not(target_os = "linux"))]
                entry_gateway_address,
            } => {
                #[cfg(not(target_os = "linux"))]
                routes.insert(RequiredRoute::new(
                    IpNetwork::from(entry_gateway_address),
                    NetNode::DefaultNode,
                ));
                routes.extend(Self::get_default_routes(&exit_tun_name));
            }
        }

        routes
    }

    /// Returns 0.0.0.0/0 and ::0/0 routes via given interface name
    fn get_default_routes(iface_name: &str) -> Vec<RequiredRoute> {
        let ipv4_route = RequiredRoute::new(
            "0.0.0.0/0".parse().unwrap(),
            Node::device(iface_name.to_owned()),
        );

        let ipv6_route = RequiredRoute::new(
            "::0/0".parse().unwrap(),
            Node::device(iface_name.to_owned()),
        );

        #[allow(unused_mut)]
        let mut routes = vec![ipv4_route, ipv6_route];

        #[cfg(target_os = "linux")]
        {
            routes = routes
                .into_iter()
                .map(|route| route.use_main_table(false))
                .collect();
        }

        routes
    }
}

#[derive(Debug)]
pub struct Error {
    inner: nym_routing::Error,
}

unsafe impl Send for Error {}
unsafe impl Sync for Error {}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.inner)
    }
}

impl From<nym_routing::Error> for Error {
    fn from(value: nym_routing::Error) -> Self {
        Self { inner: value }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "routing error: {}", self.inner)
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
