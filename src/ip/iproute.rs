use rtnetlink::{Handle, IpVersion, RouteAddRequest};
use anyhow::Result;
use futures::{TryStreamExt, StreamExt};
use netlink_packet_route::{constants::*, RouteMessage, NetlinkMessage, RtnlMessage, NetlinkPayload};

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct IPRoute {
    pub action: Action,
    pub msg: RouteMessage,
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum Action {
    Add,
    Del
}

impl IPRoute {
    pub async fn execute(&self, handle: &mut Handle) -> Result<()> {
        let mut req = match self.action {
            Action::Del => NetlinkMessage::from(RtnlMessage::DelRoute(self.msg.clone())),
            Action::Add  => NetlinkMessage::from(RtnlMessage::NewRoute(self.msg.clone())),
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

async fn get_routes(handle: &Handle, ip_version: IpVersion) -> Result<Vec<RouteMessage>> {
    let routes_exec = handle.route().get(ip_version).execute();
    let routes: Vec<RouteMessage> = routes_exec.try_collect().await?;
    Ok(routes)
}

async fn del_routes(handle: Handle, route_msg: RouteMessage) -> Result<()> {
    handle.route().del(route_msg).execute().await?;
    return Ok(())
}


#[cfg(test)]
mod test {
    use netlink_packet_route::RouteMessage;
    use rtnetlink::{IpVersion, new_connection};
    use crate::ip::iproute::{Action, del_routes, get_routes, IPRoute};

    #[tokio::test]
    async fn test_dump_addresses() {
        let (connection, mut handle, _) = new_connection().unwrap();
        tokio::spawn(connection);

        let routes = get_routes(&handle,IpVersion::V4).await.unwrap();
        let routes: Vec<RouteMessage> = routes.
            into_iter().filter(|route| route.header.table != 255).collect();

        for route in &routes {
            dbg!(route);
        }

        handle.route().del(routes[0].clone()).execute().await.unwrap();

        let iproute = IPRoute{
            action: Action::Add,
            msg: routes[0].clone(),
        };

        iproute.execute(&mut handle).await;

        let routes = get_routes(&handle,IpVersion::V4).await.unwrap();
        let routes: Vec<RouteMessage> = routes.
            into_iter().filter(|route| route.header.table != 255).collect();

        for route in routes {
            dbg!(route);
        }
    }
}