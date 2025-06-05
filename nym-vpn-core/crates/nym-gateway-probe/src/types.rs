use nym_connection_monitor::ConnectionStatusEvent;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeResult {
    pub node: String,
    pub used_entry: String,
    pub outcome: ProbeOutcome,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
#[allow(clippy::enum_variant_names)]
pub enum Entry {
    Tested(EntryTestResult),
    NotTested,
    EntryFailure,
}

impl From<EntryTestResult> for Entry {
    fn from(value: EntryTestResult) -> Self {
        Entry::Tested(value)
    }
}

impl Entry {
    pub fn fail_to_connect() -> Self {
        EntryTestResult {
            can_connect: false,
            can_route: false,
        }
        .into()
    }

    pub fn fail_to_route() -> Self {
        EntryTestResult {
            can_connect: true,
            can_route: false,
        }
        .into()
    }

    pub fn success() -> Self {
        EntryTestResult {
            can_connect: true,
            can_route: true,
        }
        .into()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryTestResult {
    pub can_connect: bool,
    pub can_route: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Exit {
    pub can_connect: bool,
    pub can_route_ip_v4: bool,
    pub can_route_ip_external_v4: bool,
    pub can_route_ip_v6: bool,
    pub can_route_ip_external_v6: bool,
}

impl Exit {
    pub fn fail_to_connect() -> Self {
        Self {
            can_connect: false,
            can_route_ip_v4: false,
            can_route_ip_external_v4: false,
            can_route_ip_v6: false,
            can_route_ip_external_v6: false,
        }
    }

    pub fn from_ping_replies(replies: &IpPingReplies) -> Self {
        Self {
            can_connect: true,
            can_route_ip_v4: replies.ipr_tun_ip_v4,
            can_route_ip_external_v4: replies.external_ip_v4,
            can_route_ip_v6: replies.ipr_tun_ip_v6,
            can_route_ip_external_v6: replies.external_ip_v6,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct IpPingReplies {
    pub ipr_tun_ip_v4: bool,
    pub ipr_tun_ip_v6: bool,
    pub external_ip_v4: bool,
    pub external_ip_v6: bool,
}

impl IpPingReplies {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_event(&mut self, event: &ConnectionStatusEvent) {
        match event {
            ConnectionStatusEvent::MixnetSelfPing => {}
            ConnectionStatusEvent::Icmpv4IprTunDevicePingReply => self.ipr_tun_ip_v4 = true,
            ConnectionStatusEvent::Icmpv6IprTunDevicePingReply => self.ipr_tun_ip_v6 = true,
            ConnectionStatusEvent::Icmpv4IprExternalPingReply => self.external_ip_v4 = true,
            ConnectionStatusEvent::Icmpv6IprExternalPingReply => self.external_ip_v6 = true,
        }
    }
}
