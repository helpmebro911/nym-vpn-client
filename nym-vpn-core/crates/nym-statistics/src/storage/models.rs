use nym_statistics_common::report::vpn_client::VpnSessionReport;
use time::Date;

#[derive(sqlx::FromRow, PartialEq, Eq, Debug)]
pub(crate) struct SessionReport {
    pub day: Date,
    pub connection_time_ms: i32,
    pub session_duration_min: i32,
    pub two_hop: bool,
    pub exit_id: String,
    pub error: Option<String>,
}

#[derive(sqlx::FromRow, PartialEq, Eq, Debug)]
pub(crate) struct SessionReportWithId {
    pub id: i32,
    #[sqlx(flatten)]
    pub report: SessionReport,
}

impl From<SessionReport> for VpnSessionReport {
    fn from(value: SessionReport) -> Self {
        Self {
            day: value.day,
            connection_time_ms: value.connection_time_ms,
            session_duration_min: value.session_duration_min,
            two_hop: value.two_hop,
            exit_id: value.exit_id,
            error: value.error,
            ..Default::default()
        }
    }
}

impl From<SessionReportWithId> for VpnSessionReport {
    fn from(value: SessionReportWithId) -> Self {
        value.report.into()
    }
}
