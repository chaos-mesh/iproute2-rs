use anyhow::Result;
use netlink_packet_route::rtnl::link::nlas::{Info, InfoData, InfoKind, Nla, VethInfo};
use netlink_packet_route::LinkMessage;

use super::iplink::{name, options, LinkTypeTrait, Opt};

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Veth {
    pub peer_name: String,
    pub options: Vec<Opt>,
}

impl LinkTypeTrait for Veth {
    fn link_type(&self, message: &mut LinkMessage) -> Result<()> {
        let mut link_info_nlas = vec![Info::Kind(InfoKind::Veth)];
        let mut peer_message = LinkMessage::default();
        name(&self.peer_name, &mut peer_message);
        options(self.options.clone(), message)?;
        link_info_nlas.push(Info::Data(InfoData::Veth(VethInfo::Peer(peer_message))));
        message.nlas.push(Nla::Info(link_info_nlas));
        Ok(())
    }
}
