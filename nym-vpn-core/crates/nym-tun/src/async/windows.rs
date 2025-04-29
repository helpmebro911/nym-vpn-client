// Copyleft 2017-2024 meh. <meh@schizofreni.co> | http://meh.schizofreni.co
// Copyright 2025 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

use std::{
    io::{self, Error},
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
    thread::JoinHandle,
};

use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::mpsc,
};
use tokio_util::codec::Framed;

use super::IntoFramedError;
use crate::platform::Device;

pub struct AsyncDevice {
    inner: Device,
    session: WinSession,
}

/// Returns a shared reference to the underlying Device object
impl AsRef<Device> for AsyncDevice {
    fn as_ref(&self) -> &Device {
        &self.inner
    }
}

/// Returns a mutable reference to the underlying Device object
impl AsMut<Device> for AsyncDevice {
    fn as_mut(&mut self) -> &mut Device {
        &mut self.inner
    }
}

impl AsyncDevice {
    /// Create a new `AsyncDevice` wrapping around a `Device`.
    pub fn new(device: Device) -> Result<AsyncDevice, super::Error> {
        let session = device.start_session()?;
        let session = WinSession::new(Arc::new(session));

        Ok(AsyncDevice {
            inner: device,
            session,
        })
    }

    /// Consumes this AsyncDevice and return a Framed object (unified Stream and Sink interface)
    pub fn into_framed(self) -> Result<Framed<Self, TunPacketCodec>, IntoFramedError> {
        let device = self.as_ref();
        let pi = device.has_packet_information();
        let mtu = device.mtu().map_err(IntoFramedError::Mtu)?;
        let codec = TunPacketCodec::new(pi, mtu);
        // guarantee to avoid the mtu of wintun may far away larger than the default provided capacity of ReadBuf of Framed
        Ok(Framed::with_capacity(self, codec, mtu as usize))
    }
}

impl AsyncRead for AsyncDevice {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.session).poll_read(cx, buf)
    }
}

impl AsyncWrite for AsyncDevice {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
        Pin::new(&mut self.session).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Pin::new(&mut self.session).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Pin::new(&mut self.session).poll_shutdown(cx)
    }
}

struct WinSession {
    session: Arc<wintun::Session>,
    receiver: mpsc::UnboundedReceiver<Vec<u8>>,
    task: Option<JoinHandle<()>>,
}

impl WinSession {
    fn new(session: Arc<wintun::Session>) -> WinSession {
        let session_reader = session.clone();
        let (receiver_tx, receiver_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let task = std::thread::spawn(move || loop {
            match session_reader.receive_blocking() {
                Ok(packet) => {
                    if let Err(err) = receiver_tx.send(packet.bytes().to_vec()) {
                        tracing::error!("{}", err);
                        break;
                    }
                }
                Err(err) => {
                    tracing::info!("{}", err);
                    break;
                }
            }
        });

        WinSession {
            session,
            receiver: receiver_rx,
            task: Some(task),
        }
    }
}

impl Drop for WinSession {
    fn drop(&mut self) {
        match self.session.shutdown() {
            Ok(_) => {
                tracing::debug!("Shutdown wintun session");
            }
            Err(e) => {
                tracing::error!("Failed to shutdown wintun session: {}", e);
            }
        }
        if let Some(task) = self.task.take() {
            task.join()
                .expect("failed to join on WinSession task handle");
        }
    }
}

impl AsyncRead for WinSession {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match ready!(self.receiver.poll_recv(cx)) {
            Some(bytes) => {
                buf.put_slice(&bytes);
                Poll::Ready(Ok(()))
            }
            None => Poll::Ready(Ok(())),
        }
    }
}

impl AsyncWrite for WinSession {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
        let mut write_pack = self.session.allocate_send_packet(buf.len() as u16)?;
        write_pack.bytes_mut().copy_from_slice(buf.as_ref());
        self.session.send_packet(write_pack);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        Poll::Ready(Ok(()))
    }
}
