use serde::{Deserialize, Serialize};
use netlink_packet_route::{LinkMessage,
                           rtnl::link::nlas::{Nla, Info , InfoKind, InfoData, VethInfo}};
use netlink_packet_route::rtnl::nlas::link::InfoBridge;

use anyhow::Result;
use super::iplink::LinkTypeTrait;
use netlink_packet_route::rtnl::nlas::DefaultNla;

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Bridge {
    pub info : Vec<InfoBridge>
}

impl LinkTypeTrait for Bridge {
    fn link_type(&self,  message: &mut LinkMessage) -> Result<()> {
        let mut link_info_nlas = vec![Info::Kind(InfoKind::Bridge)];
        link_info_nlas.push(Info::Data(InfoData::Bridge(self.info.clone())));
        message.nlas.push(Nla::Info(link_info_nlas));
        Ok(())
    }
}