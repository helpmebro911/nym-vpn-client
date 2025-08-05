use crate::error::{BackendError, ErrorKey};
use nym_vpn_proto::proto::account_command_error::ErrorDetail;
use nym_vpn_proto::proto::{
    AccountCommandError, VpnApiError, VpnApiErrorResponse,
    vpn_api_error::ErrorDetail as VpnApiErrorDetail,
};
use tracing::error;

impl From<VpnApiError> for BackendError {
    fn from(error: VpnApiError) -> Self {
        let Some(detail) = error.error_detail else {
            error!("missing error detail in VpnApiError");
            return BackendError::internal("nym-vpn-api returned error", None);
        };
        match detail {
            VpnApiErrorDetail::Timeout(_) => BackendError::internal("nym-vpn-api timeout", None),
            VpnApiErrorDetail::StatusCode(code) => BackendError::internal_with_detail(
                "nym-vpn-api error",
                format!("nym-vpn-api returned: {code}"),
            ),
            VpnApiErrorDetail::Response(response) => BackendError::from(response),
        }
    }
}

impl From<VpnApiErrorResponse> for BackendError {
    fn from(error: VpnApiErrorResponse) -> Self {
        let mut detail = format!("VPN API response error: {}", error.message);
        if let Some(code) = error.message_id {
            detail.push_str(&format!(" (id: {code})"));
        }
        if let Some(id) = error.code_reference_id {
            detail.push_str(&format!(" (code: {id})"));
        }
        BackendError::internal_with_detail("VPN API response error", detail)
    }
}

impl From<AccountCommandError> for BackendError {
    fn from(error: AccountCommandError) -> Self {
        let Some(detail) = error.error_detail else {
            error!("missing error detail in AccountCommandError");
            return BackendError::internal("AC error", None);
        };
        match detail {
            ErrorDetail::Internal(e) => BackendError::internal_with_detail("AC internal error", e),
            ErrorDetail::StorageError(e) => BackendError::internal_with_detail(
                "AC storage error",
                format!("AC storage error: {e}"),
            ),
            ErrorDetail::VpnApi(e) => e.into(),
            ErrorDetail::UnexpectedResponse(e) => BackendError::internal_with_detail(
                "AC response error",
                format!("AC response error: {e}"),
            ),
            ErrorDetail::NoAccountStored(v) => BackendError::internal_with_detail(
                "AC no account stored",
                format!("AC no account stored: {v}"),
            ),
            ErrorDetail::NoDeviceStored(v) => BackendError::internal_with_detail(
                "AC no device stored",
                format!("AC no device stored: {v}"),
            ),
            ErrorDetail::ExistingAccount(v) => BackendError::internal_with_detail(
                "AC existing account",
                format!("AC existing account: {v}"),
            ),
            ErrorDetail::Offline(v) => {
                BackendError::internal_with_detail("AC offline", format!("AC offline: {v}"))
            }
            ErrorDetail::InvalidMnemonic(e) => BackendError::with_detail(
                "invalid mnemonic",
                ErrorKey::AccountInvalidMnemonic,
                format!("invalid mnemonic: {e}"),
            ),
        }
    }
}
