use anyhow::Result;
use futures::{StreamExt, TryStreamExt};
use netlink_packet_route::constants::*;
use netlink_packet_route::{NetlinkMessage, NetlinkPayload, RouteMessage, RtnlMessage};
use rtnetlink::{Handle, IpVersion};

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct IPRoute {
    pub action: Action,
    pub msg: RouteMessage,
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum Action {
    Add,
    Del,
}

impl IPRoute {
    pub async fn execute(&self, handle: &mut Handle) -> Result<()> {
        let mut req = match self.action {
            Action::Del => NetlinkMessage::from(RtnlMessage::DelRoute(self.msg.clone())),
            Action::Add => NetlinkMessage::from(RtnlMessage::NewRoute(self.msg.clone())),
        };

        if self.action == Action::Add {
            req.header.flags = NLM_F_REQUEST | NLM_F_ACK | NLM_F_EXCL | NLM_F_CREATE
        } else {
            req.header.flags = NLM_F_REQUEST | NLM_F_ACK
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

pub async fn get_routes(handle: &Handle, ip_version: IpVersion) -> Result<Vec<RouteMessage>> {
    let routes_exec = handle.route().get(ip_version).execute();
    let routes: Vec<RouteMessage> = routes_exec.try_collect().await?;
    Ok(routes)
}

pub async fn del_routes(handle: Handle, route_msg: RouteMessage) -> Result<()> {
    handle.route().del(route_msg).execute().await?;
    Ok(())
}

#[cfg(test)]
mod test {
    use netlink_packet_route::RouteMessage;
    use rtnetlink::{new_connection, IpVersion};

    use crate::ip::iproute::{get_routes, Action, IPRoute};

    #[tokio::test]
    async fn test_dump_addresses() {
        let (connection, mut handle, _) = new_connection().unwrap();
        tokio::spawn(connection);

        let routes = get_routes(&handle, IpVersion::V4).await.unwrap();
        let mut routes: Vec<RouteMessage> = routes
            .into_iter()
            .filter(|route| route.header.table != 255)
            .collect();

        for route in &routes {
            handle.route().del(route.clone()).execute().await.unwrap();
        }

        routes.reverse();

        for route in routes {
            IPRoute {
                action: Action::Add,
                msg: route.clone(),
            }
            .execute(&mut handle)
            .await
            .unwrap();
        }
    }
}
