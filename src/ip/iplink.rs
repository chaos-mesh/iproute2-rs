use std::path::Path;

use anyhow::Result;
use enum_dispatch::enum_dispatch;
use futures::stream::{StreamExt, TryStreamExt};
use netlink_packet_route::rtnl::link::nlas::Nla;
use netlink_packet_route::{
    LinkMessage, NetlinkHeader, NetlinkMessage, NetlinkPayload, RtnlMessage, IFF_UP, NLM_F_ACK,
    NLM_F_CREATE, NLM_F_EXCL, NLM_F_REQUEST,
};
use nix::fcntl::OFlag;
use nix::sys::stat::Mode;
use rtnetlink::{new_connection, Handle, NETNS_PATH};

use crate::ip::bridge::Bridge;
use crate::ip::veth::Veth;

pub fn get_link_name(name: &str) -> Result<LinkMessage> {
    let (connection, handle, _) = new_connection()?;
    tokio::spawn(connection);

    futures::executor::block_on(async {
        let mut links = handle.link().get().match_name(name.parse()?).execute();
        if let Some(link) = links.try_next().await? {
            Ok(link)
        } else {
            Err(anyhow::anyhow!("no link named {}", name))
        }
    })
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct IPLink {
    pub action: Action,
    pub name: String,
    pub options: Vec<Opt>,
    pub link_type: Option<LinkTypeEnum>,
}

pub fn name(name: &str, message: &mut LinkMessage) {
    message.nlas.push(Nla::IfName(String::from(name)))
}

impl IPLink {
    pub async fn execute(&self, handle: &mut Handle) -> Result<()> {
        let mut message = LinkMessage::default();
        name(&self.name, &mut message);
        options(self.options.clone(), &mut message)?;

        self.link_type
            .as_ref()
            .map_or(Ok(()), |link_type| link_type.link_type(&mut message))?;

        let mut req = match self.action {
            Action::Delete => NetlinkMessage::from(RtnlMessage::DelLink(message)),
            Action::Add | Action::Set => NetlinkMessage::from(RtnlMessage::NewLink(message)),
        };
        self.action.action(&mut req.header);

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

#[cfg(test)]
mod test {
    use rtnetlink::new_connection;

    use crate::ip::iplink::{Action, IPLink, LinkTypeEnum};
    use crate::ip::veth::Veth;

    #[tokio::test]
    async fn test_veth() {
        let (connection, mut handle, _) = new_connection().unwrap();
        tokio::spawn(connection);

        IPLink {
            action: Action::Add,
            name: "v0".to_string(),
            options: vec![],
            link_type: Some(LinkTypeEnum::Veth(Veth {
                peer_name: "v1".to_string(),
                options: vec![],
            })),
        }
        .execute(&mut handle)
        .await
        .unwrap();

        IPLink {
            action: Action::Delete,
            name: "v0".to_string(),
            options: vec![],
            link_type: None,
        }
        .execute(&mut handle)
        .await
        .unwrap();
    }
}
