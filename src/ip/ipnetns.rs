use anyhow::{anyhow, Result};
use nix::fcntl::{open, OFlag};
use nix::sched::CloneFlags;
use nix::sys::stat::Mode;
use rtnetlink::NETNS_PATH;
use std::path::Path;
use std::thread::JoinHandle;

// Fatal : Never add device or do something that change files related with network
// in filesystem after set_net_ns.
pub fn set_net_ns(ns_name: String) -> Result<()> {
    let mut open_flags = OFlag::empty();
    open_flags = OFlag::empty();
    open_flags.insert(OFlag::O_RDONLY);
    open_flags.insert(OFlag::O_CLOEXEC);

    let fd = match open(
        Path::new(&format!("{}{}", NETNS_PATH, &ns_name)),
        open_flags,
        Mode::empty(),
    ) {
        Ok(raw_fd) => raw_fd,
        Err(e) => {
            return Err(anyhow!(
                "Cannot open network namespace \"{}\": {}\n",
                ns_name,
                e.to_string()
            ))
        }
    };

    let mut setns_flags = CloneFlags::empty();
    setns_flags.insert(CloneFlags::CLONE_NEWNET);
    if let Err(e) = nix::sched::setns(fd, setns_flags) {
        return Err(anyhow!(
            "setting the network namespace {} failed: {}",
            ns_name,
            e.to_string()
        ));
    };
    Ok(())
}

// Fatal : Never add device or do something that change files related with network
// in filesystem in thread_netns_exec.
pub fn thread_net_ns_exec<F, T>(ns_name:String, f: F) -> JoinHandle<Result<T>>
    where
        F: FnOnce() -> T,
        F: Send + 'static,
        T: Send + 'static,
{
    std::thread::spawn(|| {
        set_net_ns(ns_name)?;
        Ok(f())
    })
}

#[cfg(test)]
mod test {
    use std::ffi::OsString;
    use super::set_net_ns;
    use futures::stream::TryStreamExt;
    use netlink_packet_route::LinkMessage;
    use rtnetlink::{new_connection, Error, Handle, NetworkNamespace};
    use std::path::Path;
    use tokio;
    use uuid::Uuid;



    async fn get_links(handle: Handle) -> Result<Vec<LinkMessage>, Error> {
        let mut links = handle.link().get().execute();
        let mut msgs: Vec<LinkMessage> = vec![];
        while let Some(msg) = links.try_next().await? {
            msgs.push(msg);
        }
        Ok(msgs)
    }

    #[test]
    fn test_set_net_ns() {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let ns_name = Uuid::new_v4().to_string();
                NetworkNamespace::add(ns_name.clone()).await.unwrap();
                let ns_name_in = ns_name.clone();
                let (msgs_in, devices_in) = std::thread::spawn(move || {
                    tokio::runtime::Builder::new_multi_thread()
                        .enable_all()
                        .build()
                        .unwrap()
                        .block_on(async {
                            set_net_ns(ns_name_in).unwrap();
                            let (connection, handle, _) = new_connection().unwrap();
                            tokio::spawn(connection);

                            let devices = std::fs::read_dir(Path::new("/sys/class/net/")).unwrap();
                            (
                                futures::executor::block_on(async {
                                    get_links(handle).await.unwrap()
                                }),
                                devices,
                            )
                        })
                })
                .join()
                .unwrap();

                let (connection, handle, _) = new_connection().unwrap();
                tokio::spawn(connection);
                let msgs_out =
                    futures::executor::block_on(async { get_links(handle).await.unwrap() });
                let devices_out = std::fs::read_dir(Path::new("/sys/class/net/")).unwrap();
                NetworkNamespace::del(ns_name.clone()).await.unwrap();
                assert_ne!(msgs_out, msgs_in);
                assert_eq!(msgs_in.len(), 1);

                let dev_names_in = devices_in
                    .filter_map(|entry| {
                        entry.ok().map(|device| device.file_name())
                    })
                    .collect::<Vec<OsString>>();
                let dev_names_out = devices_out
                    .filter_map(|entry| {
                        entry.ok().map(|device| device.file_name())
                    })
                    .collect::<Vec<OsString>>();
                assert_eq!(dev_names_in, dev_names_out);
            });
    }
}
