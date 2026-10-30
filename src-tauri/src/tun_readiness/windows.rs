use super::{failure, AppResult, TunStage};
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    os::windows::io::AsRawSocket,
};
use tokio::net::UdpSocket;
use windows_sys::Win32::{
    NetworkManagement::{
        IpHelper::{
            ConvertInterfaceAliasToLuid, GetBestRoute2, GetIfEntry2, IF_TYPE_PROP_VIRTUAL,
            MIB_IF_ROW2, MIB_IPFORWARD_ROW2,
        },
        Ndis::{IfOperStatusUp, NET_LUID_LH},
    },
    Networking::WinSock::{
        setsockopt, AF_INET, AF_INET6, IN6_ADDR, IN6_ADDR_0, IN_ADDR, IN_ADDR_0, IPPROTO_IP,
        IP_UNICAST_IF, SOCKADDR_IN, SOCKADDR_IN6, SOCKADDR_INET, SOCKET_ERROR,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Interface {
    pub index: u32,
    luid: u64,
}

#[cfg(test)]
fn device_exists(device: &str) -> bool {
    let name: Vec<u16> = device.encode_utf16().chain(std::iter::once(0)).collect();
    let mut luid = NET_LUID_LH::default();
    // SAFETY: read-only lookup with terminated name and valid output buffer.
    unsafe { ConvertInterfaceAliasToLuid(name.as_ptr(), &mut luid) == 0 }
}

pub(super) fn interface(device: &str) -> AppResult<Interface> {
    if device != "Serylane" {
        return Err(failure(
            TunStage::Interface,
            "TUN 设备名称与应用管理的配置不一致",
            false,
        ));
    }
    let name: Vec<u16> = device.encode_utf16().chain(std::iter::once(0)).collect();
    let mut luid = NET_LUID_LH::default();
    // SAFETY: the name is NUL-terminated and luid is a live output buffer.
    let code = unsafe { ConvertInterfaceAliasToLuid(name.as_ptr(), &mut luid) };
    if code != 0 {
        return Err(failure(
            TunStage::Interface,
            format!("TUN 网卡尚未出现（Windows 状态 {code}）"),
            true,
        ));
    }
    let mut row = MIB_IF_ROW2 {
        InterfaceLuid: luid,
        ..Default::default()
    };
    // SAFETY: the structure is initialized with a valid LUID for this read.
    let code = unsafe { GetIfEntry2(&mut row) };
    if code != 0 {
        return Err(failure(
            TunStage::Interface,
            format!("TUN 网卡状态暂不可读（Windows 状态 {code}）"),
            true,
        ));
    }
    if row.Type != IF_TYPE_PROP_VIRTUAL
        || row.OperStatus != IfOperStatusUp
        || row.InterfaceIndex == 0
    {
        return Err(failure(
            TunStage::Interface,
            "名为 Serylane 的虚拟网卡尚未进入运行状态",
            true,
        ));
    }
    // SAFETY: NET_LUID_LH::Value is the documented numeric view of the union.
    Ok(Interface {
        index: row.InterfaceIndex,
        luid: unsafe { row.InterfaceLuid.Value },
    })
}

fn socket_address(ip: IpAddr) -> SOCKADDR_INET {
    match ip {
        IpAddr::V4(ip) => SOCKADDR_INET {
            Ipv4: SOCKADDR_IN {
                sin_family: AF_INET,
                sin_addr: IN_ADDR {
                    S_un: IN_ADDR_0 {
                        S_addr: u32::from_ne_bytes(ip.octets()),
                    },
                },
                ..Default::default()
            },
        },
        IpAddr::V6(ip) => SOCKADDR_INET {
            Ipv6: SOCKADDR_IN6 {
                sin6_family: AF_INET6,
                sin6_addr: IN6_ADDR {
                    u: IN6_ADDR_0 { Byte: ip.octets() },
                },
                ..Default::default()
            },
        },
    }
}

fn matches_route(expected: Interface, index: u32, luid: u64, loopback: bool) -> bool {
    index == expected.index && luid == expected.luid && !loopback
}

fn route(interface: Interface, destination: IpAddr) -> AppResult<()> {
    let destination = socket_address(destination);
    let mut best = MIB_IPFORWARD_ROW2::default();
    let mut source = SOCKADDR_INET::default();
    // Leave interface and source unconstrained: asking for the best route ON
    // the desired adapter would hide another VPN's more-preferred route.
    // SAFETY: destination has a matching family and all outputs are writable.
    let code = unsafe {
        GetBestRoute2(
            std::ptr::null(),
            0,
            std::ptr::null(),
            &destination,
            0,
            &mut best,
            &mut source,
        )
    };
    if code != 0 {
        return Err(failure(
            TunStage::Route,
            format!("TUN 目标路由尚未就绪（Windows 状态 {code}）"),
            true,
        ));
    }
    // SAFETY: the API returned a complete MIB_IPFORWARD_ROW2 including LUID.
    let luid = unsafe { best.InterfaceLuid.Value };
    if !matches_route(interface, best.InterfaceIndex, luid, best.Loopback) {
        return Err(failure(
            TunStage::Route,
            "系统仍将验收流量送往其他网卡；TUN 路由尚未接管或与其他 VPN 冲突",
            true,
        ));
    }
    Ok(())
}

pub(super) fn routes(interface: Interface, require_ipv6: bool) -> AppResult<()> {
    // Samples span both IPv4 /1 halves. This is selected-route evidence for
    // these targets, not a promise that every application/protocol is captured.
    route(interface, Ipv4Addr::new(1, 1, 1, 1).into())?;
    route(interface, Ipv4Addr::new(208, 67, 222, 222).into())?;
    if require_ipv6 {
        route(
            interface,
            Ipv6Addr::new(0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111).into(),
        )?;
        route(
            interface,
            Ipv6Addr::new(0x2001, 0x4860, 0x4860, 0, 0, 0, 0, 0x8888).into(),
        )?;
    }
    Ok(())
}

pub(super) fn dns_socket(index: u32) -> AppResult<UdpSocket> {
    let socket = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|_| failure(TunStage::Dns, "无法创建 TUN DNS 检测套接字", true))?;
    // IP_UNICAST_IF accepts the interface index in NETWORK byte order. Only
    // this temporary socket is pinned; no global adapter setting is modified.
    let index = index.to_be();
    // SAFETY: socket and the 4-byte option buffer remain live for this call.
    let result = unsafe {
        setsockopt(
            socket.as_raw_socket() as _,
            IPPROTO_IP,
            IP_UNICAST_IF,
            (&index as *const u32).cast(),
            std::mem::size_of::<u32>() as i32,
        )
    };
    if result == SOCKET_ERROR {
        return Err(failure(
            TunStage::Dns,
            "无法将 DNS 检测限定到 TUN 网卡；未发送检测请求",
            true,
        ));
    }
    socket
        .set_nonblocking(true)
        .map_err(|_| failure(TunStage::Dns, "DNS 检测套接字初始化失败", true))?;
    UdpSocket::from_std(socket).map_err(|_| failure(TunStage::Dns, "DNS 检测调度初始化失败", true))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn different_luid_reused_index_or_loopback_is_not_accepted() {
        let expected = Interface {
            index: 42,
            luid: 99,
        };
        assert!(matches_route(expected, 42, 99, false));
        assert!(!matches_route(expected, 43, 99, false));
        assert!(!matches_route(expected, 42, 100, false));
        assert!(!matches_route(expected, 42, 99, true));
    }

    #[test]
    fn route_addresses_preserve_network_byte_order() {
        let v4 = socket_address(Ipv4Addr::new(1, 2, 3, 4).into());
        let v6 = socket_address(Ipv6Addr::new(0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111).into());
        // SAFETY: each constructor initialized the tested union variant.
        unsafe {
            assert_eq!(v4.Ipv4.sin_family, AF_INET);
            assert_eq!(v4.Ipv4.sin_addr.S_un.S_addr.to_ne_bytes(), [1, 2, 3, 4]);
            assert_eq!(v6.Ipv6.sin6_family, AF_INET6);
            assert_eq!(&v6.Ipv6.sin6_addr.u.Byte[..2], &[0x26, 0x06]);
        }
    }

    /// Explicitly opts in to changing ONLY an ephemeral CI runner's routes.
    /// Never run as part of the local/default suite or on the user's machine.
    #[tokio::test]
    #[ignore = "requires isolated Windows GitHub runner and SERYLANE_NATIVE_TUN_TEST=1"]
    async fn native_windows_tun_dns_route_lifecycle() {
        use std::{
            os::windows::process::CommandExt,
            process::{Child, Command, Stdio},
            time::Duration,
        };
        assert_eq!(
            std::env::var("SERYLANE_NATIVE_TUN_TEST").as_deref(),
            Ok("1")
        );
        assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
        assert_eq!(std::env::var("RUNNER_OS").as_deref(), Ok("Windows"));
        assert!(
            !device_exists("Serylane"),
            "refuse to touch a pre-existing adapter"
        );
        let system = sysinfo::System::new_all();
        assert!(
            !system.processes().values().any(|process| {
                matches!(
                    process
                        .name()
                        .to_string_lossy()
                        .to_ascii_lowercase()
                        .as_str(),
                    "mihomo.exe" | "serylane.exe"
                )
            }),
            "refuse to run beside an existing application or core"
        );
        let binary = std::env::var_os("MIHOMO_TEST_BINARY")
            .expect("MIHOMO_TEST_BINARY must be the pinned prepared core");
        assert!(std::path::Path::new(&binary).is_absolute());
        let directory = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let secret = uuid::Uuid::new_v4().to_string();
        let config = format!("mode: direct\nipv6: false\nlog-level: warning\nexternal-controller: 127.0.0.1:{port}\nsecret: {secret}\nallow-lan: false\ndns:\n  enable: true\n  listen: ''\n  enhanced-mode: fake-ip\n  fake-ip-range: 198.18.0.1/16\n  nameserver: [1.1.1.1]\n  default-nameserver: [1.1.1.1]\ntun:\n  enable: true\n  device: Serylane\n  stack: mixed\n  auto-route: true\n  auto-detect-interface: true\n  dns-hijack: [any:53, 'tcp://any:53']\nproxies: []\nproxy-groups: []\nrules: []\n");
        let config_path = directory.path().join("test.yaml");
        std::fs::write(&config_path, config).unwrap();
        let log_path = directory.path().join("core.log");
        let log = std::fs::File::create(&log_path).unwrap();
        struct Core(Child);
        impl Drop for Core {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        drop(listener);
        let core = Core(
            Command::new(binary)
                .arg("-d")
                .arg(directory.path())
                .arg("-f")
                .arg(config_path)
                .creation_flags(0x08000000)
                .stdin(Stdio::null())
                .stdout(log.try_clone().unwrap())
                .stderr(log)
                .spawn()
                .expect("start isolated pinned core"),
        );
        let deadline = tokio::time::Instant::now() + super::super::LOCAL_BUDGET;
        let client =
            crate::mihomo_api::MihomoApiClient::from_endpoint(port, secret.clone()).unwrap();
        let result = async {
            // Exercise the same control-plane and OS acceptance path as the app.
            client.wait_tun_config("Serylane", deadline).await?;
            super::super::wait_ready_until("Serylane", false, deadline, || false, |_| {}).await
        }
        .await;
        if let Err(error) = result {
            panic!(
                "native TUN acceptance failed: {error}; isolated core log: {}",
                std::fs::read_to_string(&log_path)
                    .unwrap_or_default()
                    .replace(&secret, "[redacted]")
            );
        }
        super::super::verify_current_routes("Serylane", false).unwrap();
        drop(core);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while super::super::verify_current_routes("Serylane", false).is_ok() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "TUN route remained accepted after core termination"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
