// Copyleft 2017-2024 meh. <meh@schizofreni.co> | http://meh.schizofreni.co
// Copyright 2025 - Nym Technologies SA <contact@nymtech.net>
// SPDX-License-Identifier: GPL-3.0-only

use bytes::Bytes;
use std::io;
use tokio_util::{
    bytes::{BufMut, BytesMut},
    codec::{Decoder, Encoder},
};

// Packet information length
const PIL: usize = 4;

/// A packet protocol IP version
#[derive(Debug, Clone, Copy, Default)]
enum PacketProtocol {
    #[default]
    IPv4,
    IPv6,
    #[allow(dead_code)]
    Other(u8),
}

impl PacketProtocol {
    /// Build the packet information header comprising of 2 u16 fields: flags and protocol.
    /// flags is always 0
    /// Note: the protocol in the packet information header is platform dependent.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn gen_pi_field(&self) -> io::Result<[u8; PIL]> {
        match self {
            PacketProtocol::IPv4 => Ok((nix::libc::ETH_P_IP as u32).to_be_bytes()),
            PacketProtocol::IPv6 => Ok((nix::libc::ETH_P_IPV6 as u32).to_be_bytes()),
            PacketProtocol::Other(_) => Err(io::Error::new(
                io::ErrorKind::Other,
                "neither an IPv4 or IPv6 packet",
            )),
        }
    }

    #[cfg(any(target_os = "macos", target_os = "ios"))]
    fn gen_pi_field(&self) -> io::Result<[u8; PIL]> {
        match self {
            PacketProtocol::IPv4 => Ok((nix::libc::PF_INET as u32).to_be_bytes()),
            PacketProtocol::IPv6 => Ok((nix::libc::PF_INET6 as u32).to_be_bytes()),
            PacketProtocol::Other(_) => Err(io::Error::new(
                io::ErrorKind::Other,
                "neither an IPv4 or IPv6 packet",
            )),
        }
    }

    #[cfg(target_os = "windows")]
    fn gen_pi_field(&self) -> [u8; PIL] {
        unreachable!()
    }
}

/// A Tun Packet to be sent or received on the TUN interface.
#[derive(Debug)]
pub struct TunPacket(PacketProtocol, Bytes);

/// Infer the protocol based on the first nibble in the packet buffer.
fn infer_proto(buf: &[u8]) -> io::Result<PacketProtocol> {
    use std::io::{Error, ErrorKind::InvalidData};
    if buf.is_empty() {
        return Err(Error::new(InvalidData, "Zero-length data"));
    }
    Ok(match buf[0] >> 4 {
        4 => PacketProtocol::IPv4,
        6 => PacketProtocol::IPv6,
        p => PacketProtocol::Other(p),
    })
}

impl TunPacket {
    /// Create a new `TunPacket` based on a byte slice.
    pub fn new(bytes: Vec<u8>) -> std::io::Result<TunPacket> {
        let proto = infer_proto(&bytes)?;
        Ok(TunPacket(proto, Bytes::from(bytes)))
    }

    pub fn into_bytes(self) -> Bytes {
        self.1
    }
}

impl AsRef<[u8]> for TunPacket {
    fn as_ref(&self) -> &[u8] {
        &self.1
    }
}

impl From<TunPacket> for Bytes {
    fn from(packet: TunPacket) -> Self {
        packet.1
    }
}

/// A TUN packet Encoder/Decoder.
#[derive(Debug, Default)]
pub struct TunPacketCodec {
    pi: bool,
    mtu: usize,
}

impl TunPacketCodec {
    /// Create a new `TunPacketCodec` specifying whether the underlying
    ///  tunnel Device has enabled the packet information header.
    pub fn new(pi: bool, mtu: u16) -> TunPacketCodec {
        TunPacketCodec {
            pi,
            mtu: mtu as usize,
        }
    }
}

impl Decoder for TunPacketCodec {
    type Item = TunPacket;
    type Error = std::io::Error;

    fn decode(&mut self, buf: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if buf.is_empty() {
            return Ok(None);
        }

        let mut pkt = buf.split_to(buf.len());

        // if the packet information is enabled we have to ignore the first 4 bytes
        if self.pi {
            let _ = pkt.split_to(PIL);
            // reserve enough space for the next packet
            buf.reserve(self.mtu + PIL);
        } else {
            buf.reserve(self.mtu);
        }

        let proto = infer_proto(pkt.as_ref())?;
        Ok(Some(TunPacket(proto, pkt.freeze())))
    }
}

impl Encoder<TunPacket> for TunPacketCodec {
    type Error = io::Error;

    fn encode(&mut self, item: TunPacket, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let TunPacket(proto, bytes) = item;
        if self.pi {
            dst.reserve(bytes.len() + PIL);
            dst.put_slice(&proto.gen_pi_field()?);
        } else {
            dst.reserve(bytes.len());
        }
        dst.put(bytes);

        Ok(())
    }
}
