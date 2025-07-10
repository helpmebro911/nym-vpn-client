// Copyright 2023 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

use std::net::{IpAddr, SocketAddr};
use std::collections::HashMap;

use nym_common::trace_err_chain;
use nym_http_api_client::HickoryDnsResolver;

use crate::{Config, Error, error::Result, gateway_client::ResolvedConfig};

async fn try_resolve_hostname(hostname: &str) -> Result<Vec<IpAddr>> {
    tracing::debug!("Trying to resolve hostname: {hostname}");
    let resolver = HickoryDnsResolver::default();
    let addrs = resolver.resolve_str(hostname).await.map_err(|err| {
        trace_err_chain!(err, "Failed to resolve gateway hostname");
        Error::FailedToDnsResolveGateway {
            hostname: hostname.to_string(),
            source: err,
        }
    })?;
    tracing::debug!("Resolved to: {addrs:?}");

    let ips = addrs.iter().collect::<Vec<_>>();
    if ips.is_empty() {
        return Err(Error::ResolvedHostnameButNoIp(hostname.to_string()));
    }

    Ok(ips)
}

async fn url_to_socket_addr(unresolved_url: &url::Url) -> Result<Vec<SocketAddr>> {
    let port = unresolved_url
        .port_or_known_default()
        .ok_or(Error::UrlError {
            url: unresolved_url.clone(),
            reason: "missing port".to_string(),
        })?;
    let hostname = unresolved_url.host_str().ok_or(Error::UrlError {
        url: unresolved_url.clone(),
        reason: "missing hostname".to_string(),
    })?;

    Ok(try_resolve_hostname(hostname)
        .await?
        .into_iter()
        .map(|ip| SocketAddr::new(ip, port))
        .collect())
}


async fn url_with_fronts_to_socket_addrs(
    unresolved_url: &nym_http_api_client::Url,
) -> Result<HashMap<String, Vec<SocketAddr>>> {

    let socket_addrs = url_to_socket_addr(unresolved_url.as_ref()).await?;
    let mut result = HashMap::new();
    result.insert(unresolved_url.host_str().unwrap().to_string(), socket_addrs);

    if let Some(fronts) = unresolved_url.fronts() {
        for front in fronts {
            let front_socket_addrs = url_to_socket_addr(&front).await?;
            result.insert(front.host_str().unwrap().to_string(), front_socket_addrs);
        }
    }

    Ok(result)
}

async fn urls_with_fronts_to_socket_addrs(
    unresolved_urls: &[nym_http_api_client::Url],
) -> Result<HashMap<String, Vec<SocketAddr>>> {

    let mut result = HashMap::new();

    for unresolved_url in unresolved_urls {
        let m = url_with_fronts_to_socket_addrs(unresolved_url).await?;
        result.extend(m);
    }

    Ok(result)
}

pub async fn resolve_config(config: &Config) -> Result<ResolvedConfig> {

    let nyxd_socket_addrs = url_with_fronts_to_socket_addrs(config.nyxd_url()).await?;
    let api_socket_addrs = urls_with_fronts_to_socket_addrs(&config.api_urls).await?;
    let nym_vpn_api_socket_addrs = match config.nym_vpn_api_urls {
        Some(ref urls) => {
            Some(urls_with_fronts_to_socket_addrs(urls).await?)
        }
        None => None,
    };

    Ok(ResolvedConfig {
        nyxd_socket_addrs,
        api_socket_addrs,
        nym_vpn_api_socket_addrs,
    })
}
