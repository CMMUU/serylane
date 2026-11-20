//! Owns the macOS utun interface and only the routes installed for this lease.
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::{Command, Stdio};

const KERNEL_CONTROL: libc::c_int = 2;
const CONTROL_INFO: libc::c_ulong = 0xc0644e03;
const UTUN_IFNAME: libc::c_int = 2;
pub(super) const IPV4_ADDRESS: &str = "198.18.0.1";
pub(super) const IPV6_ADDRESS: &str = "fdfe:dcba:9876::1";

// Use subranges like the bundled core's macOS auto-route implementation, so
// the physical default route and more specific routes stay intact.
const IPV4_ROUTES: &[&str] = &[
    "1.0.0.0/8",
    "2.0.0.0/7",
    "4.0.0.0/6",
    "8.0.0.0/5",
    "16.0.0.0/4",
    "32.0.0.0/3",
    "64.0.0.0/2",
    "128.0.0.0/1",
];
const IPV6_ROUTES: &[&str] = &[
    "100::/8", "200::/7", "400::/6", "800::/5", "1000::/4", "2000::/3", "4000::/2", "8000::/1",
];

pub(super) struct Device {
    fd: OwnedFd,
    interface: String,
    ipv6: bool,
    routes: Vec<(&'static str, &'static str)>,
}

impl Device {
    pub(super) fn open(ipv6: bool) -> Result<Self, String> {
        let (fd, interface) =
            open_interface().map_err(|error| format!("创建 TUN 接口失败：{error}"))?;
        adjust_interface(&[
            &interface,
            "inet",
            IPV4_ADDRESS,
            IPV4_ADDRESS,
            "netmask",
            "255.255.255.252",
            "mtu",
            "9000",
            "up",
        ])?;
        if ipv6 {
            adjust_interface(&[
                &interface,
                "inet6",
                IPV6_ADDRESS,
                "prefixlen",
                "126",
                "alias",
            ])?;
        }
        Ok(Self {
            fd,
            interface,
            ipv6,
            routes: Vec::new(),
        })
    }

    pub(super) fn descriptor(&self) -> io::Result<OwnedFd> {
        self.fd.try_clone()
    }

    pub(super) fn active(&self) -> bool {
        self.routes.len() == IPV4_ROUTES.len() + if self.ipv6 { IPV6_ROUTES.len() } else { 0 }
    }

    pub(super) fn set_active(&mut self, enabled: bool) -> Result<(), String> {
        if !enabled {
            return self.remove_routes();
        }
        if self.active() {
            return Ok(());
        }
        // Retry orphaned routes only after clearing the preceding attempt.
        self.remove_routes()?;
        let requested = IPV4_ROUTES
            .iter()
            .map(|route| ("-inet", *route))
            .chain(
                IPV6_ROUTES
                    .iter()
                    .filter(|_| self.ipv6)
                    .map(|route| ("-inet6", *route)),
            )
            .collect::<Vec<_>>();
        for route in requested {
            if let Err(error) = self.change_route("add", route) {
                return match self.remove_routes() {
                    Ok(()) => Err(error),
                    Err(rollback) => Err(format!("{error}（清理：{rollback}）")),
                };
            }
            self.routes.push(route);
        }
        flush_dns();
        Ok(())
    }

    fn change_route(&self, action: &str, route: (&str, &str)) -> Result<(), String> {
        if action == "delete" {
            let result = Command::new("/sbin/route")
                .args(["-n", "get", route.0, "-net", route.1])
                .env("LC_ALL", "C")
                .stdin(Stdio::null())
                .output()
                .map_err(|error| format!("读取 TUN 路由失败：{error}"))?;
            let output = String::from_utf8_lossy(&result.stdout);
            let owner = output
                .lines()
                .find_map(|line| line.trim().strip_prefix("interface: ").map(str::trim));
            if result.status.success() && owner != Some(self.interface.as_str()) {
                // The route is gone or another service has already replaced it.
                return Ok(());
            }
            if !result.status.success() {
                let error = String::from_utf8_lossy(&result.stderr);
                if error.contains("not in table") {
                    return Ok(());
                }
                return Err(format!("读取 TUN 路由失败：{}", error.trim()));
            }
        }
        command(
            "/sbin/route",
            &[
                "-n",
                action,
                route.0,
                "-net",
                route.1,
                "-interface",
                &self.interface,
            ],
        )
    }

    fn remove_routes(&mut self) -> Result<(), String> {
        let mut error = None;
        let mut remaining = Vec::new();
        let installed = std::mem::take(&mut self.routes);
        let changed = !installed.is_empty();
        for route in installed.into_iter().rev() {
            if let Err(failure) = self.change_route("delete", route) {
                remaining.push(route);
                error = Some(failure);
            }
        }
        self.routes = remaining;
        if changed {
            flush_dns();
        }
        error.map_or(Ok(()), Err)
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        let _ = self.remove_routes();
    }
}

fn adjust_interface(arguments: &[&str]) -> Result<(), String> {
    command("/sbin/ifconfig", arguments)
}

fn command(program: &str, arguments: &[&str]) -> Result<(), String> {
    let output = Command::new(program)
        .args(arguments)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("{program}：{error}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "{program}：{}",
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn flush_dns() {
    let _ = command("/usr/bin/killall", &["-HUP", "mDNSResponder"]);
}

fn open_interface() -> io::Result<(OwnedFd, String)> {
    let fd = unsafe { libc::socket(libc::AF_SYSTEM, libc::SOCK_DGRAM, KERNEL_CONTROL) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let raw = fd.as_raw_fd();
    let mut info: libc::ctl_info = unsafe { std::mem::zeroed() };
    let service = c"com.apple.net.utun_control".to_bytes_with_nul();
    for (dest, value) in info.ctl_name.iter_mut().zip(service) {
        *dest = *value as libc::c_char;
    }
    let mut name = [0_u8; libc::IFNAMSIZ];
    let mut length = name.len() as libc::socklen_t;
    unsafe {
        if libc::fcntl(raw, libc::F_SETFD, libc::FD_CLOEXEC) < 0
            || libc::ioctl(raw, CONTROL_INFO, &mut info) < 0
        {
            return Err(io::Error::last_os_error());
        }
        let address = libc::sockaddr_ctl {
            sc_len: std::mem::size_of::<libc::sockaddr_ctl>() as u8,
            sc_family: libc::AF_SYSTEM as u8,
            ss_sysaddr: KERNEL_CONTROL as u16,
            sc_id: info.ctl_id,
            sc_unit: 0,
            sc_reserved: [0; 5],
        };
        if libc::connect(
            raw,
            (&address as *const libc::sockaddr_ctl).cast(),
            address.sc_len.into(),
        ) < 0
            || libc::getsockopt(
                raw,
                KERNEL_CONTROL,
                UTUN_IFNAME,
                name.as_mut_ptr().cast(),
                &mut length,
            ) < 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    let name =
        std::str::from_utf8(&name[..name.iter().position(|&b| b == 0).unwrap_or(name.len())])
            .map_err(io::Error::other)?;
    if !name
        .strip_prefix("utun")
        .is_some_and(|n| n.parse::<u32>().is_ok())
    {
        return Err(io::Error::other("系统没有返回有效的 utun 接口"));
    }
    Ok((fd, name.to_string()))
}
