#[cfg(target_os = "linux")]
use nix::sys::socket::{SetSockOpt, sockopt::Mark};

#[cfg(target_os = "android")]
use crate::tunnel_provider::android::AndroidTunProvider;
#[cfg(target_os = "linux")]
use crate::tunnel_state_machine::route_handler::TUNNEL_FWMARK;

#[cfg(target_os = "android")]
pub fn get_socket_bypass_fn(tun_provider: AndroidTunProvider) -> Arc<dyn Fn(RawFd) + Send + Sync> {
    Arc::new(move |fd: RawFd| {
        tracing::debug!("Bypass websocket");
        tun_provider.bypass(fd);
    })
}

#[cfg(target_os = "linux")]
pub fn get_socket_bypass_fn() -> Arc<dyn Fn(RawFd) + Send + Sync> {
    Arc::new(move |fd: RawFd| {
        tracing::debug!("Bypass websocket");
        let borrowed_fd = unsafe { &BorrowedFd::borrow_raw(fd) };
        if let Err(err) = Mark.set(borrowed_fd, &TUNNEL_FWMARK) {
            tracing::error!("Could not set fwmark for websocket fd: {err}");
        }
    })
}
