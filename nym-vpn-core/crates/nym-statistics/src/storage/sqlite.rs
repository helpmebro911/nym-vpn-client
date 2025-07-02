// Copyright 2025 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

use crate::storage::models::{SessionReport, SessionReportWithId};

use super::error::StatsStorageError;
use sqlx_pool_guard::SqlitePoolGuard;

#[derive(Clone, Debug)]
pub struct SqliteStatsStorageManager {
    connection_pool: SqlitePoolGuard,
}

impl SqliteStatsStorageManager {
    pub fn new(connection_pool: SqlitePoolGuard) -> Self {
        Self { connection_pool }
    }

    pub(crate) async fn close(&self) {
        self.connection_pool.close().await
    }

    pub async fn load_seed(&self) -> Result<Option<String>, StatsStorageError> {
        Ok(sqlx::query!("SELECT seed FROM seed")
            .fetch_optional(&*self.connection_pool)
            .await?
            .map(|r| r.seed))
    }

    pub async fn set_seed(&self, seed: String) -> Result<(), StatsStorageError> {
        self.remove_seed().await?;
        sqlx::query!("INSERT INTO seed VALUES (?)", seed)
            .execute(&*self.connection_pool)
            .await?;
        Ok(())
    }

    pub async fn remove_seed(&self) -> Result<(), StatsStorageError> {
        sqlx::query!("DELETE FROM seed")
            .execute(&*self.connection_pool)
            .await?;
        Ok(())
    }

    pub async fn insert_pending_session_report(
        &self,
        report: &SessionReport,
    ) -> Result<(), StatsStorageError> {
        sqlx::query!(
            r#"INSERT INTO pending_session_report (day, 
            connection_time_ms, 
            session_duration_min, 
            two_hop, 
            exit_id, 
            error) VALUES (?, ?, ?, ?, ?, ?)"#,
            report.day,
            report.connection_time_ms,
            report.session_duration_min,
            report.two_hop,
            report.exit_id,
            report.error
        )
        .execute(&*self.connection_pool)
        .await?;
        Ok(())
    }

    pub async fn get_pending_session_report_with_id(
        &self,
    ) -> Result<Vec<SessionReportWithId>, StatsStorageError> {
        Ok(sqlx::query_as(
            r#"SELECT 
            id, 
            day, 
            connection_time_ms, 
            session_duration_min, 
            two_hop, 
            exit_id, 
            error FROM pending_session_report
            LIMIT 5"#,
        )
        .fetch_all(&*self.connection_pool)
        .await?)
    }

    pub async fn delete_pending_session_report(&self, id: i32) -> Result<(), StatsStorageError> {
        sqlx::query!(r#"DELETE FROM pending_session_report WHERE id = ?"#, id)
            .execute(&*self.connection_pool)
            .await?;
        Ok(())
    }
}
