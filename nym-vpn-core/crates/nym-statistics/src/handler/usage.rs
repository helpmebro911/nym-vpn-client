// Copyright 2025 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

use std::time::{Duration, Instant};
use time::Date;

use crate::{events::UsageEvent, storage::StatsStorage};

const SESSION_DURATION_BUCKETS_MIN: [i32; 5] = [1, 5, 10, 30, 60];

fn bucketize_session_duration(session_duration_secs: i32) -> i32 {
    let session_duration_min = session_duration_secs / 60;
    for upper_bound in SESSION_DURATION_BUCKETS_MIN {
        if session_duration_min < upper_bound {
            return upper_bound;
        }
    }
    (session_duration_min / SESSION_DURATION_BUCKETS_MIN[SESSION_DURATION_BUCKETS_MIN.len() - 1]
        + 1)
        * SESSION_DURATION_BUCKETS_MIN[SESSION_DURATION_BUCKETS_MIN.len() - 1] // SW Round to next multiple of last bound
}

#[derive(PartialEq, Eq, Debug)]
struct Session {
    two_hop: bool,
    exit_id: String,
    day: Date,
    error: Option<String>,
    connection_time: Option<Duration>,
    connecting_start: Option<Instant>,
    session_start: Option<Instant>,
    session_duration: Option<Duration>,
}

impl Session {
    fn new(two_hop: bool, connection_start: Instant) -> Session {
        Self {
            two_hop,
            exit_id: Default::default(),
            day: time::OffsetDateTime::now_utc().date(),
            connection_time: None,
            connecting_start: Some(connection_start),
            session_start: None,
            session_duration: None,
            error: None,
        }
    }

    fn set_connected(&mut self, session_start: Instant) {
        if let Some(connecting_time) = self.connecting_start.take() {
            self.connection_time = Some(session_start.duration_since(connecting_time));
            self.session_start = Some(session_start);
        }
    }

    fn set_finished(&mut self, session_end: Instant) {
        if let Some(session_start) = self.session_start.take() {
            self.session_duration = Some(session_end.duration_since(session_start))
        }
    }

    fn set_error(&mut self, session_end: Instant, error: String) {
        self.set_finished(session_end);
        self.error = Some(error);
    }
}

impl From<Session> for crate::storage::models::SessionReport {
    fn from(value: Session) -> Self {
        let connection_time = value.connection_time.unwrap_or_default();
        let session_duration = value.session_duration.unwrap_or_default();
        Self {
            day: value.day,
            connection_time_ms: connection_time.as_millis().try_into().unwrap_or(i32::MAX),
            session_duration_min: bucketize_session_duration(
                session_duration.as_secs().try_into().unwrap_or(i32::MAX),
            ),
            two_hop: value.two_hop,
            exit_id: value.exit_id,
            error: value.error,
        }
    }
}

pub(crate) struct UsageHandler {
    storage: StatsStorage,
    pub(crate) is_connected: bool,

    current_session: Option<Session>,
}

impl UsageHandler {
    pub(crate) fn new(storage: StatsStorage) -> Self {
        UsageHandler {
            storage,
            current_session: None,
            is_connected: false,
        }
    }

    // SW TBC, actual possible transitions
    pub(crate) async fn handle_event(&mut self, event: UsageEvent) {
        // Set "connected" only if the last event in "Connnected"
        self.is_connected = matches!(event, UsageEvent::Connected { .. });

        match event {
            // User pressed "Connect"
            UsageEvent::ConnectRequest {
                instant,
                enable_two_hop,
            } => {
                if let Some(mut current_session) = self.current_session.take() {
                    current_session.set_finished(instant);
                    self.store_report(current_session).await;
                }
                self.current_session = Some(Session::new(enable_two_hop, instant));
            }

            // State machine entered "Connected" state
            UsageEvent::Connected { instant, exit_id } => {
                if let Some(session) = &mut self.current_session {
                    session.set_connected(instant);
                    session.exit_id = exit_id;
                }
            }

            // User pressed "Disconnect"
            UsageEvent::DisconnectRequest(instant) => {
                if let Some(mut current_session) = self.current_session.take() {
                    current_session.set_finished(instant);
                    self.store_report(current_session).await;
                }
            }

            // State machine entered "Disconnecting" state
            UsageEvent::Disconnecting(_) => {}

            // State machine entered "Connecting" state
            UsageEvent::Connecting(_) => {}

            // State machine entered "Disconnected" state
            UsageEvent::Disconnected(instant) => {
                if let Some(mut current_session) = self.current_session.take() {
                    current_session.set_finished(instant);
                    self.store_report(current_session).await;
                }
            }

            // State machine entered "Error" state
            UsageEvent::Error { instant, error } => {
                if let Some(mut current_session) = self.current_session.take() {
                    current_session.set_error(instant, error);
                    self.store_report(current_session).await;
                }
            }
        }
    }

    async fn store_report(&self, session: Session) {
        if let Err(e) = self
            .storage
            .insert_pending_session_report(&session.into())
            .await
        {
            tracing::warn!("Failed to store session report : {e}")
        }
    }
}

#[cfg(test)]
mod tests {

    use std::time::{Duration, Instant};

    use crate::events::UsageEvent;
    use crate::handler::usage::{Session, UsageHandler, bucketize_session_duration};
    use crate::storage::models::{SessionReport, SessionReportWithId};

    #[tokio::test]
    async fn connect_request_handling_test() {
        let mock_storage = crate::storage::test::mock_database().await;

        let mut usage_handler = UsageHandler::new(mock_storage);

        let connect_request_instant = Instant::now();
        let connect_request_two_hop = true;

        let resulting_session = Session {
            two_hop: connect_request_two_hop,
            exit_id: Default::default(),
            day: time::OffsetDateTime::now_utc().date(),
            error: None,
            connecting_start: Some(connect_request_instant),
            connection_time: None,
            session_start: None,
            session_duration: None,
        };

        usage_handler
            .handle_event(UsageEvent::ConnectRequest {
                instant: connect_request_instant,
                enable_two_hop: connect_request_two_hop,
            })
            .await;

        assert_eq!(usage_handler.current_session, Some(resulting_session));
    }

    #[tokio::test]
    async fn successfull_connection_test() {
        let mock_storage = crate::storage::test::mock_database().await;
        let mut usage_handler = UsageHandler::new(mock_storage);

        let connect_request_instant = Instant::now();
        let connect_request_two_hop = false;

        let connection_duration = Duration::from_millis(1234);

        let connected_instant = connect_request_instant + connection_duration;
        let exit_id = "whatever".to_string();

        let resulting_session = Session {
            two_hop: connect_request_two_hop,
            exit_id: exit_id.clone(),
            day: time::OffsetDateTime::now_utc().date(),
            error: None,
            connecting_start: None,
            connection_time: Some(connection_duration),
            session_start: Some(connected_instant),
            session_duration: None,
        };

        let events = [
            UsageEvent::ConnectRequest {
                instant: connect_request_instant,
                enable_two_hop: connect_request_two_hop,
            },
            UsageEvent::Connecting(Instant::now()),
            UsageEvent::Connecting(Instant::now()),
            UsageEvent::Connecting(Instant::now()),
            UsageEvent::Connected {
                instant: connected_instant,
                exit_id,
            },
        ];

        for event in events {
            usage_handler.handle_event(event).await;
        }

        assert_eq!(usage_handler.current_session, Some(resulting_session));
        assert!(usage_handler.is_connected)
    }

    #[tokio::test]
    async fn successfull_session_test() {
        let mock_storage = crate::storage::test::mock_database().await;
        let mut usage_handler = UsageHandler::new(mock_storage);

        let connect_request_instant = Instant::now();
        let connect_request_two_hop = false;
        let exit_id = "whatever".to_string();

        let connection_duration = Duration::from_millis(1234);
        let connected_instant = connect_request_instant + connection_duration;

        let session_duration_secs = Duration::from_secs(123);
        let disconnect_request_instant = connected_instant + session_duration_secs;

        let expected_stored_session = SessionReportWithId {
            id: 1,
            report: SessionReport {
                day: time::OffsetDateTime::now_utc().date(),
                connection_time_ms: connection_duration.as_millis().try_into().unwrap(),
                session_duration_min: bucketize_session_duration(
                    session_duration_secs.as_secs().try_into().unwrap(),
                ),
                two_hop: connect_request_two_hop,
                exit_id: exit_id.clone(),
                error: None,
            },
        };

        let events = [
            UsageEvent::ConnectRequest {
                instant: connect_request_instant,
                enable_two_hop: connect_request_two_hop,
            },
            UsageEvent::Connecting(Instant::now()),
            UsageEvent::Connecting(Instant::now()),
            UsageEvent::Connecting(Instant::now()),
            UsageEvent::Connected {
                instant: connected_instant,
                exit_id,
            },
            UsageEvent::DisconnectRequest(disconnect_request_instant),
            UsageEvent::Disconnecting(Instant::now()),
            UsageEvent::Disconnected(Instant::now()),
        ];

        for event in events {
            usage_handler.handle_event(event).await;
        }

        let stored_sessions = usage_handler
            .storage
            .get_pending_session_report_with_id()
            .await
            .unwrap();
        let stored_session = stored_sessions.first().unwrap();
        assert_eq!(usage_handler.current_session, None);
        assert!(!usage_handler.is_connected);
        assert_eq!(expected_stored_session, *stored_session);
    }
}
