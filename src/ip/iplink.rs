use anyhow::Result;
use enum_dispatch::enum_dispatch;
use futures::stream::{StreamExt, TryStreamExt};
use netlink_packet_route::{
    rtnl::link::nlas::{Info, Nla},
    LinkMessage, NetlinkHeader, NetlinkMessage, NetlinkPayload, RtnlMessage, IFF_UP, NLM_F_ACK,
    NLM_F_CREATE, NLM_F_EXCL, NLM_F_REQUEST,
};
use rtnetlink::{new_connection, try_nl, Handle, NETNS_PATH};
use serde::{Deserialize, Serialize};

use nix::fcntl::OFlag;
use nix::sys::stat::Mode;
use std::path::Path;
use crate::ip::veth::Veth;
use crate::ip::bridge::Bridge;

pub fn get_link_name(name: &str) -> Result<LinkMessage> {
    let (connection, handle, _) = new_connection()?;
    tokio::spawn(connection);

    futures::executor::block_on(async {
        let mut links = handle.link().get().set_name_filter(name.parse()?).execute();
        if let Some(link) = links.try_next().await? {
            Ok(link)
        } else {
            Err(anyhow::anyhow!("no link named {}", name))
        }
    })
}

#[derive(Debug, Eq, PartialEq, Clone)]
struct IPLink {
    pub action: Action,
    pub name: String,
    pub options: Vec<Opt>,
    pub link_type: LinkTypeEnum,
}

pub fn name(name: &str, message: &mut LinkMessage) {
    message.nlas.push(Nla::IfName(String::from(name)))
}

impl IPLink {
    pub async fn execute(&self, mut handle: Handle) -> Result<()> {
        let mut message = LinkMessage::default();
        let mut header = NetlinkHeader::default();
        self.action.action(&mut header);
        name(&self.name, &mut message);
        options(self.options.clone(), &mut message);
        self.link_type.link_type(&mut message);

        let mut req = match self.action {
            Action::Delete => NetlinkMessage::from(RtnlMessage::DelLink(message)),
            Action::Add | Action::Set => NetlinkMessage::from(RtnlMessage::NewLink(message)),
        };

        let mut response = handle.request(req)?;
        while let Some(message) = response.next().await {
            if let NetlinkPayload::Error(err) = message.payload {
                return Err(anyhow::Error::new(rtnetlink::Error::NetlinkError(err)));
            }
        }

        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum Action {
    Add,
    Delete,
    Set,
}

impl Action {
    pub fn action(&self, header: &mut NetlinkHeader) {
        match self {
            Action::Add => header.flags = NLM_F_REQUEST | NLM_F_ACK | NLM_F_EXCL | NLM_F_CREATE,
            _ => header.flags = NLM_F_REQUEST | NLM_F_ACK,
        }
    }
}

#[enum_dispatch]
pub trait LinkTypeTrait {
    fn link_type(&self, message: &mut LinkMessage) -> Result<()>;
}

#[enum_dispatch(LinkTypeTrait)]
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum LinkTypeEnum {
    Veth(Veth),
    Bridge(Bridge),
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum Opt {
    Up,
    Down,
    Master(String),
    NetNS(String),
}

impl Opt {
    pub fn opt(&self, message: &mut LinkMessage) -> Result<()> {
        match self {
            Opt::Up => {
                message.header.change_mask |= IFF_UP;
                message.header.flags |= IFF_UP;
            }
            Opt::Down => {
                message.header.change_mask |= IFF_UP;
                message.header.flags &= !IFF_UP;
            }
            Opt::Master(master_name) => {
                let link = get_link_name(master_name)?;
                message.nlas.push(Nla::Master(link.header.index));
            }
            Opt::NetNS(netns_name) => {
                let mut path_string = String::from(NETNS_PATH);
                path_string.push_str(netns_name.as_str());
                let path = Path::new(&path_string);
                // TODO : ERR HANDLING
                let fd = nix::fcntl::open(path, OFlag::O_RDONLY, Mode::empty())?;
                message.nlas.push(Nla::NetNsFd(fd));
            }
        }
        Ok(())
    }
}

pub fn options(opts: Vec<Opt>, message: &mut LinkMessage) -> Result<()> {
    for opt in opts {
        opt.opt(message)?;
    }
    Ok(())
}
