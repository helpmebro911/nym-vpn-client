// Copyright 2025 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

use nym_statistics_common::{
    generate_vpn_client_stats_id, report::vpn_client::VpnClientStatsReport,
};
use static_information::StaticInformationHandler;
use usage::UsageHandler;

use crate::{
    api_client::StatisticsControllerApiClient,
    config::StatisticsControllerConfig,
    error::Error,
    events::{ControllerEvent, StatisticsEvent},
    storage::StatsStorage,
};

mod static_information;
mod usage;

pub(crate) struct StatisticsHandler {
    storage: StatsStorage,
    stats_api_client: StatisticsControllerApiClient,
    config: StatisticsControllerConfig,

    static_information_handler: StaticInformationHandler,
    usage_handler: UsageHandler,
    //SW TODO investigate using trait like Andrew did in Nym-nodes
}

impl StatisticsHandler {
    pub fn new(
        storage: StatsStorage,
        stats_api_client: StatisticsControllerApiClient,
        config: StatisticsControllerConfig,
    ) -> Self {
        StatisticsHandler {
            storage: storage.clone(),
            stats_api_client,
            config,
            static_information_handler: StaticInformationHandler::new(),
            usage_handler: UsageHandler::new(storage),
        }
    }

    pub(crate) async fn close(&self) {
        self.storage.close().await
    }

    pub async fn handle_event(&mut self, event: StatisticsEvent) {
        match event {
            StatisticsEvent::Usage(e) => self.usage_handler.handle_event(e).await,
            StatisticsEvent::Controller(e) => self.handle_controller_event(e).await,
        }
    }

    async fn handle_controller_event(&mut self, event: ControllerEvent) {
        match event {
            ControllerEvent::RemoveSeed => self
                .storage
                .remove_seed()
                .await
                .unwrap_or_else(|e| tracing::error!("Failed to remove stats seed : {e}")),
            ControllerEvent::ResetSeed => self
                .storage
                .reset_seed()
                .await
                .unwrap_or_else(|e| tracing::error!("Failed to reset stats seed : {e}")),
            ControllerEvent::SendReport => self.send_reports().await,
        };
    }

    // Initial sending strategy, send after a random amount of time, if we're still connected
    pub async fn send_reports(&mut self) {
        if self.usage_handler.is_connected {
            tracing::debug!("StatisticsHandler: Sending reports");

            // Basic report
            self.send_basic_report()
                .await
                .unwrap_or_else(|e| tracing::error!("Failed to handle basic report sending : {e}"));

            // Sessions report
            self.send_session_reports().await.unwrap_or_else(|e| {
                tracing::error!("Failed to handle session reports sending : {e}")
            });
            tracing::debug!("Stats report handling done");
        } else {
            tracing::debug!("Not connected, not sending anything")
        }
    }

    pub async fn send_basic_report(&mut self) -> Result<(), Error> {
        // Use seed override or storage one
        let seed = self
            .config
            .stats_id_seed
            .clone()
            .unwrap_or(self.storage.maybe_init_and_load_seed().await?);
        let identifier = generate_vpn_client_stats_id(seed);

        let report =
            VpnClientStatsReport::new(identifier, self.static_information_handler.get_report());
        self.stats_api_client.post_basic_report(report).await
    }

    pub async fn send_session_reports(&mut self) -> Result<(), Error> {
        for report_with_id in self.storage.get_pending_session_report_with_id().await? {
            if let Err(e) = self
                .stats_api_client
                .post_session_report(report_with_id.report.into())
                .await
            {
                tracing::error!(
                    "Failed to send session report with id {0} : {e}",
                    report_with_id.id
                )
            } else {
                self.storage
                    .delete_pending_session_report(report_with_id.id)
                    .await.unwrap_or_else(|e| tracing::error!("Failed to delete session report with id {0}, it might be re-sent in the future : {e}", report_with_id.id))
            }
        }
        Ok(())
    }
}
