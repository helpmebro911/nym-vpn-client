// Copyright 2025 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

use nym_statistics_common::report::vpn_client::{VpnClientStatsReport, VpnSessionReport};

use crate::config::StatisticsControllerConfig;
use crate::error::Error;

#[derive(Clone, Debug)]
pub(crate) struct StatisticsControllerApiClient {
    inner: nym_statistics_api_client::StatisticsApiClient,
}

impl StatisticsControllerApiClient {
    pub fn new(config: &StatisticsControllerConfig) -> Result<Option<Self>, Error> {
        if let Some(url) = &config.stats_collector_url {
            let inner_api_client = nym_statistics_api_client::StatisticsApiClient::new(
                url.clone(),
                config.user_agent.clone(),
            )?;
            Ok(Some(StatisticsControllerApiClient::from(inner_api_client)))
        } else {
            Ok(None)
        }
    }
    pub async fn post_basic_report(&self, report: VpnClientStatsReport) -> Result<(), Error> {
        self.inner.post_basic_report(report).await?;
        Ok(())
    }

    pub async fn post_session_report(&self, report: VpnSessionReport) -> Result<(), Error> {
        self.inner.post_session_report(report).await?;
        Ok(())
    }
}

impl From<nym_statistics_api_client::StatisticsApiClient> for StatisticsControllerApiClient {
    fn from(statistics_api_client: nym_statistics_api_client::StatisticsApiClient) -> Self {
        Self {
            inner: statistics_api_client,
        }
    }
}
