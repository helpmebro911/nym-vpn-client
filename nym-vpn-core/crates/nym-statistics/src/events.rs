// Copyright 2025 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

use std::time::Instant;

use nym_vpn_lib_types::TunnelState;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

/// Channel receiving generic stats events to be used by a statistics aggregator.
pub type StatisticsReceiver = tokio::sync::mpsc::UnboundedReceiver<StatisticsEvent>;

/// Channel allowing generic statistics events to be reported to a stats event aggregator
#[derive(Clone)]
pub struct StatisticsSender {
    stats_tx: Option<UnboundedSender<StatisticsEvent>>,
    cancel_token: CancellationToken,
}

impl StatisticsSender {
    /// Create a new statistics Sender
    pub fn new(
        stats_tx: Option<UnboundedSender<StatisticsEvent>>,
        cancel_token: CancellationToken,
    ) -> Self {
        StatisticsSender {
            stats_tx,
            cancel_token,
        }
    }

    /// Report a statistics event using the sender.
    pub fn report(&self, event: StatisticsEvent) {
        if let Some(tx) = &self.stats_tx {
            if let Err(err) = tx.send(event) {
                if !self.cancel_token.is_cancelled() {
                    tracing::error!("Failed to send stats event: {err}");
                }
            }
        }
    }
}

/// Statistics events
#[derive(Debug, Clone)]
pub enum StatisticsEvent {
    Usage(UsageEvent),
    Controller(ControllerEvent),
}

impl StatisticsEvent {
    pub fn new_connect_request(enable_two_hop: bool) -> Self {
        Self::Usage(UsageEvent::ConnectRequest {
            instant: Instant::now(),
            enable_two_hop,
        })
    }
    pub fn new_disconnect_request() -> Self {
        Self::Usage(UsageEvent::DisconnectRequest(Instant::now()))
    }
    pub fn new_connecting() -> Self {
        Self::Usage(UsageEvent::Connecting(Instant::now()))
    }

    pub fn new_connected(exit_id: String) -> Self {
        Self::Usage(UsageEvent::Connected {
            instant: Instant::now(),
            exit_id,
        })
    }

    pub fn new_disconnecting() -> Self {
        Self::Usage(UsageEvent::Disconnecting(Instant::now()))
    }

    pub fn new_disconnected() -> Self {
        Self::Usage(UsageEvent::Disconnected(Instant::now()))
    }

    pub fn new_error(error: impl ToString) -> Self {
        Self::Usage(UsageEvent::Error {
            instant: Instant::now(),
            error: error.to_string(),
        })
    }

    pub fn new_from_state(state: TunnelState) -> Option<Self> {
        match state {
            TunnelState::Disconnected => Some(Self::new_disconnected()),
            TunnelState::Connecting { .. } => Some(Self::new_connecting()),
            TunnelState::Connected { connection_data } => {
                Some(Self::new_connected(connection_data.exit_gateway.id))
            }
            TunnelState::Disconnecting { .. } => Some(Self::new_disconnecting()),
            TunnelState::Error(client_error_reason) => Some(Self::new_error(client_error_reason)),
            TunnelState::Offline { .. } => None,
        }
    }

    pub fn reset_seed() -> Self {
        Self::Controller(ControllerEvent::ResetSeed)
    }

    pub fn remove_seed() -> Self {
        Self::Controller(ControllerEvent::RemoveSeed)
    }

    pub fn send_report() -> Self {
        Self::Controller(ControllerEvent::SendReport)
    }
}

#[derive(Debug, Clone)]
pub enum UsageEvent {
    // User action
    ConnectRequest {
        instant: Instant,
        enable_two_hop: bool,
    },
    DisconnectRequest(Instant),

    // State machine state events
    Connecting(Instant),
    Connected {
        instant: Instant,
        exit_id: String,
    },
    Disconnected(Instant),
    Disconnecting(Instant),
    Error {
        instant: Instant,
        error: String,
    },
}

// Not a stat event per se, but used to instruct the controller to do stuff
#[derive(Debug, Clone)]
pub enum ControllerEvent {
    ResetSeed,
    RemoveSeed,
    SendReport,
}
