use std::fs::read_dir;
use std::path::Path;
use std::process::exit;
use std::thread::JoinHandle;

use anyhow::{anyhow, Result};
use nix::fcntl::{open, OFlag};
use nix::mount::{mount, umount2, MntFlags, MsFlags};
use nix::sched::CloneFlags;
use nix::sys::stat::Mode;
use nix::sys::statvfs::{statvfs, FsFlags};
use nix::unistd::{close, fork, ForkResult};
use rtnetlink::NetworkNamespace;

pub const NETNS_RUN_DIR: &str = "/var/run/netns/";

/// Fatal : Never add device or do something that change files related with network
/// in filesystem after set_net_ns.
pub fn set_net_ns(ns_name: String) -> Result<()> {
    let mut open_flags = OFlag::empty();
    open_flags.insert(OFlag::O_RDONLY);
    open_flags.insert(OFlag::O_CLOEXEC);

    let fd = match open(
        Path::new(&format!("{}{}", NETNS_RUN_DIR, &ns_name)),
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

    if let Err(e) = nix::sched::setns(fd, CloneFlags::CLONE_NEWNET) {
        close(fd)?;
        return Err(anyhow!(
            "setting the network namespace {} failed: {}",
            ns_name,
            e.to_string()
        ));
    };
    close(fd)?;
    Ok(())
}

/// just setns & exec f()
/// Fatal : Never add device or do something that change files related with network
/// in filesystem in thread_netns_exec.
pub fn thread_net_ns_exec<F, T>(ns_name: String, f: F) -> JoinHandle<Result<T>>
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

fn bind_etc(ns_name: String) {
    if ns_name.len() > 255 {
        return;
    }

    let dirs = match read_dir(format!("/etc/netns/{}", ns_name)) {
        Ok(dir) => dir,
        Err(_) => return,
    };
    dirs.filter_map(|entry| entry.ok()).for_each(|entry| {
        let netns_name = format!(
            "/etc/netns/{}/{}",
            ns_name,
            entry.file_name().to_string_lossy()
        );
        let etc_name = format!("/etc/{}", entry.file_name().to_string_lossy());
        if let Err(e) = mount::<_, _, _, str>(
            Some(netns_name.as_str()),
            etc_name.as_str(),
            Some("none"),
            MsFlags::MS_BIND,
            None,
        ) {
            println!(
                "Bind {} -> {} failed: {}\n",
                netns_name,
                etc_name,
                e
            )
        }
    });
}

fn netns_switch(ns_name: String) -> Result<()> {
    set_net_ns(ns_name.clone())?;
    // unshare to the new network namespace
    if let Err(e) = nix::sched::unshare(CloneFlags::CLONE_NEWNS) {
        return Err(anyhow!("unshare failed: {}", e.to_string()));
    }
    let mut mount_flags = MsFlags::empty();
    mount_flags.insert(MsFlags::MS_SLAVE);
    mount_flags.insert(MsFlags::MS_REC);
    if let Err(e) = mount::<_, _, _, str>(Some(""), "/", Some("none"), mount_flags, None) {
        return Err(anyhow!(
            "\"mount --make-rslave /\" failed: {}\n",
            e.to_string()
        ));
    }

    let mut mount_flags = MsFlags::empty();
    if umount2("/sys", MntFlags::MNT_DETACH).is_err() {
        if let Ok(stat) = statvfs("/sys") {
            if stat.flags().contains(FsFlags::ST_RDONLY) {
                mount_flags.insert(MsFlags::MS_RDONLY);
            }
        }
    }

    if let Err(e) = mount::<_, _, _, str>(
        Some(ns_name.as_str()),
        "/sys",
        Some("sysfs"),
        mount_flags,
        None,
    ) {
        return Err(anyhow!("mount of /sys failed: {}\n", e.to_string()));
    }

    /* Setup bind mounts for config files in /etc */
    bind_etc(ns_name);

    Ok(())
}

/// It seems using both tokio & fork will bring a lot of error.
/// ip netns exec name f()
pub fn ip_net_ns_exec<F, T>(ns_name: String, f: F) -> Result<()>
where
    F: FnOnce() -> Result<T>,
    F: Send + 'static,
    T: Send + 'static,
{
    match unsafe { fork() } {
        Ok(ForkResult::Parent { child, .. }) => Ok(NetworkNamespace::parent_process(child)?),
        Ok(ForkResult::Child) => {
            if netns_switch(ns_name).is_err() {
                exit(1);
            }
            if f().is_err() {
                exit(1);
            }
            exit(0)
        }
        Err(_) => Err(anyhow!("Fork failed")),
    }
}

/// ip netns add name
pub fn ip_net_ns_add(ns_name: String) -> Result<()> {
    match unsafe { fork() } {
        Ok(ForkResult::Parent { child, .. }) => Ok(NetworkNamespace::parent_process(child)?),
        Ok(ForkResult::Child) => {
            let netns_path = match NetworkNamespace::child_process(ns_name) {
                Ok(netns_path) => netns_path,
                Err(_) => exit(1),
            };
            match NetworkNamespace::unshare_processing(netns_path) {
                Ok(_) => exit(0),
                _ => exit(1),
            };
        }
        Err(_) => Err(anyhow!("Fork failed")),
    }
}

/// just ip netns del name
pub fn ip_net_ns_del(ns_name: String) -> Result<()> {
    let netns_path = format!("{}{}", NETNS_RUN_DIR, ns_name);

    if let Err(e) = nix::mount::umount2(
        netns_path.as_str(),
        nix::mount::MntFlags::MNT_DETACH,
    ) {
        println!(
            "Cannot umount namespace file \" {} \": {}",
            netns_path,
            e
        );
    }

    if let Err(e) = nix::unistd::unlink(netns_path.as_str()) {
        return Err(anyhow!(
            "Cannot remove namespace file \"{}\": {}\n",
            netns_path,
            e.to_string()
        ));
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use std::ffi::OsString;
    use std::path::Path;

    use futures::stream::TryStreamExt;
    use netlink_packet_route::LinkMessage;
    use rtnetlink::{new_connection, Error, Handle};
    use serial_test::serial;
    use tokio;

    use crate::ip::ipnetns::{ip_net_ns_add, ip_net_ns_del, ip_net_ns_exec, set_net_ns};

    async fn get_links(handle: Handle) -> Result<Vec<LinkMessage>, Error> {
        let mut links = handle.link().get().execute();
        let mut msgs: Vec<LinkMessage> = vec![];
        while let Some(msg) = links.try_next().await? {
            msgs.push(msg);
        }
        Ok(msgs)
    }

    #[test]
    #[serial]
    fn test_set_net_ns() {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let ns_name = "vnetns1".to_string();
                let ns_name_in = ns_name.clone();
                let (connection, handle, _) = new_connection().unwrap();
                tokio::spawn(connection);
                ip_net_ns_add(ns_name.clone()).unwrap();
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

                let msgs_out =
                    futures::executor::block_on(async { get_links(handle).await.unwrap() });
                let devices_out = std::fs::read_dir(Path::new("/sys/class/net/")).unwrap();
                ip_net_ns_del(ns_name).unwrap();
                assert_ne!(msgs_out, msgs_in);
                assert_eq!(msgs_in.len(), 1);

                let dev_names_in = devices_in
                    .filter_map(|entry| entry.ok().map(|device| device.file_name()))
                    .collect::<Vec<OsString>>();
                let dev_names_out = devices_out
                    .filter_map(|entry| entry.ok().map(|device| device.file_name()))
                    .collect::<Vec<OsString>>();
                assert_eq!(dev_names_in, dev_names_out);
            });
    }

    #[test]
    #[serial]
    fn test_ip_net_ns_exec() {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                std::thread::spawn(|| {
                    let ns_name = "vnetns0".to_string();
                    ip_net_ns_add(ns_name.clone()).unwrap();
                    ip_net_ns_exec(ns_name.clone(), || {
                        tokio::runtime::Builder::new_multi_thread()
                            .enable_all()
                            .build()
                            .unwrap()
                            .block_on(async {
                                let (connection, handle, _) = new_connection()?;
                                tokio::spawn(connection);
                                let msgs = get_links(handle).await?;
                                assert_eq!(msgs.len(), 1);
                                Ok(())
                            })
                    })
                    .unwrap();
                    ip_net_ns_del(ns_name).unwrap();
                })
                .join()
                .unwrap();
            });
    }
}
