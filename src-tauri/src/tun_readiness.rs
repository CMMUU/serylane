//! Read-only local TUN acceptance. A listening controller is not evidence that
//! Windows routes or the intercepted DNS path are usable. Never changes routes,
//! adapters, DNS settings or the user's proxy configuration.
#![cfg(any(windows, test))]

use crate::error::{AppError, AppResult, TunStage};
use std::{future::Future, net::Ipv4Addr, time::Duration};
use tokio::{net::UdpSocket, time::Instant};

#[cfg(windows)]
mod windows;

#[cfg(test)]
const LOCAL_BUDGET: Duration = Duration::from_secs(12);
#[cfg(windows)]
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const DNS_BUDGET: Duration = Duration::from_secs(2);
const DNS_NAME: &str = "example.com";
#[cfg(windows)]
const DNS_TARGET: &str = "1.1.1.1:53";

/// Recheck after external health verification without issuing another DNS
/// request. The caller must also check its running core identity/cancellation.
#[cfg(windows)]
pub fn verify_current_routes(device: &str, require_ipv6: bool) -> AppResult<()> {
    windows::routes(windows::interface(device)?, require_ipv6)
}

fn failure(stage: TunStage, detail: impl Into<String>, retryable: bool) -> AppError {
    AppError::TunStartup {
        stage,
        detail: detail.into(),
        retryable,
    }
}

fn cancelled_error() -> AppError {
    failure(
        TunStage::Cancelled,
        "TUN 就绪检查已取消；运行实例或退出状态已变化",
        false,
    )
}

/// The caller shares this deadline with controller configuration readback so
/// control-plane initialization cannot silently double the local wait budget.
/// This only proves the local Windows path. The caller must separately check
/// external network health, and retain the same runtime identity throughout.
#[cfg(windows)]
pub async fn wait_ready_until(
    device: &str,
    require_ipv6: bool,
    deadline: Instant,
    cancelled: impl Fn() -> bool,
    progress: impl Fn(TunStage),
) -> AppResult<()> {
    let mut last_error = failure(TunStage::Interface, "等待 TUN 网卡就绪", true);
    let mut current_stage = TunStage::Interface;
    progress(current_stage);
    while Instant::now() < deadline {
        if cancelled() {
            return Err(cancelled_error());
        }
        // Native reads are isolated from the async executor. They do not invoke
        // PowerShell or any process, nor modify the inspected network state.
        let name = device.to_string();
        let snapshot = tokio::task::spawn_blocking(move || windows::interface(&name));
        let observed = bounded(snapshot, deadline, TunStage::Interface, &cancelled).await?;
        match observed.map_err(|_| failure(TunStage::Interface, "无法读取 TUN 网卡状态", true))?
        {
            Err(error) => last_error = error,
            Ok(interface) => {
                if current_stage == TunStage::Interface {
                    current_stage = TunStage::Route;
                    progress(current_stage);
                }
                let routes =
                    tokio::task::spawn_blocking(move || windows::routes(interface, require_ipv6));
                match bounded(routes, deadline, TunStage::Route, &cancelled)
                    .await?
                    .map_err(|_| failure(TunStage::Route, "无法读取 TUN 路由状态", true))?
                {
                    Err(error) => last_error = error,
                    Ok(()) => {
                        if current_stage != TunStage::Dns {
                            current_stage = TunStage::Dns;
                            progress(current_stage);
                        }
                        let dns_deadline = deadline.min(Instant::now() + DNS_BUDGET);
                        let socket = windows::dns_socket(interface.index)?;
                        match bounded(
                            dns_query(&socket, DNS_TARGET),
                            dns_deadline,
                            TunStage::Dns,
                            &cancelled,
                        )
                        .await
                        {
                            Ok(Ok(())) => {
                                // Do not accept a DNS reply if another VPN or a
                                // concurrent operation changed routes meanwhile.
                                let name = device.to_string();
                                let confirmed = tokio::task::spawn_blocking(move || {
                                    let current = windows::interface(&name)?;
                                    if current != interface {
                                        return Err(failure(
                                            TunStage::Interface,
                                            "TUN 网卡在验收期间发生变化",
                                            true,
                                        ));
                                    }
                                    windows::routes(current, require_ipv6)
                                });
                                match bounded(confirmed, deadline, TunStage::Route, &cancelled)
                                    .await?
                                    .map_err(|_| {
                                        failure(TunStage::Route, "TUN 路由复核未完成", true)
                                    })? {
                                    Ok(()) if !cancelled() => return Ok(()),
                                    Ok(()) => return Err(cancelled_error()),
                                    Err(error) => last_error = error,
                                }
                            }
                            Ok(Err(error)) => last_error = error,
                            Err(AppError::TunStartup {
                                stage: TunStage::Cancelled,
                                ..
                            }) => return Err(cancelled_error()),
                            Err(_) => {
                                last_error = failure(
                                    TunStage::Dns,
                                    "TUN 内置 DNS 查询未在时限内返回；请检查 DNS 配置和劫持路径",
                                    true,
                                )
                            }
                        }
                    }
                }
            }
        }
        if !last_error.retryable() {
            return Err(last_error);
        }
        if let AppError::TunStartup { stage, .. } = &last_error {
            if current_stage != *stage {
                current_stage = *stage;
                progress(current_stage);
            }
        }
        bounded(
            tokio::time::sleep(POLL_INTERVAL),
            deadline,
            TunStage::Interface,
            &cancelled,
        )
        .await
        .ok();
    }
    if cancelled() {
        return Err(cancelled_error());
    }
    Err(last_error)
}

/// Poll cancellation even while the operation is waiting on a network response.
async fn bounded<T>(
    future: impl Future<Output = T>,
    deadline: Instant,
    stage: TunStage,
    cancelled: &impl Fn() -> bool,
) -> AppResult<T> {
    tokio::pin!(future);
    loop {
        if cancelled() {
            return Err(cancelled_error());
        }
        if Instant::now() >= deadline {
            return Err(failure(stage, "TUN 本地就绪检查超过总时限", true));
        }
        tokio::select! {
            result = &mut future => return Ok(result),
            _ = tokio::time::sleep_until(deadline.min(Instant::now() + Duration::from_millis(100))) => {}
        }
    }
}

fn dns_question(id: u16) -> Vec<u8> {
    let mut bytes = vec![(id >> 8) as u8, id as u8, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    for label in DNS_NAME.split('.') {
        bytes.push(label.len() as u8);
        bytes.extend_from_slice(label.as_bytes());
    }
    bytes.extend_from_slice(&[0, 0, 1, 0, 1]); // A, IN
    bytes
}

async fn dns_query(socket: &UdpSocket, target: &str) -> AppResult<()> {
    let id = rand::random::<u16>();
    let request = dns_question(id);
    socket
        .connect(target)
        .await
        .map_err(|_| failure(TunStage::Dns, "TUN DNS 检测套接字连接失败", true))?;
    socket
        .send(&request)
        .await
        .map_err(|_| failure(TunStage::Dns, "TUN DNS 检测请求未发送", true))?;
    // A bounded buffer and connected UDP socket reject other senders; validate
    // response ID and the complete question, not just the existence of bytes.
    let mut response = [0u8; 1500];
    let length = socket
        .recv(&mut response)
        .await
        .map_err(|_| failure(TunStage::Dns, "TUN DNS 响应读取失败", true))?;
    validate_dns_response(&response[..length], id)
        .map_err(|detail| failure(TunStage::Dns, detail, true))
}

fn read_u16(bytes: &[u8], cursor: &mut usize) -> Option<u16> {
    let pair = bytes.get(*cursor..(*cursor).checked_add(2)?)?;
    *cursor += 2;
    Some(u16::from_be_bytes([pair[0], pair[1]]))
}

fn dns_name(bytes: &[u8], cursor: &mut usize) -> Option<String> {
    let mut offset = *cursor;
    let mut jumped = false;
    let mut labels = vec![];
    let mut total = 0;
    // A compression pointer cycle or an overlong name must never spin.
    for _ in 0..128 {
        let length = *bytes.get(offset)?;
        if length == 0 {
            if !jumped {
                *cursor = offset + 1;
            }
            return Some(labels.join("."));
        }
        if length & 0xc0 == 0xc0 {
            let next = *bytes.get(offset + 1)?;
            if !jumped {
                *cursor = offset + 2;
            }
            offset = (((length & 0x3f) as usize) << 8) | next as usize;
            jumped = true;
            continue;
        }
        if length > 63 {
            return None;
        }
        let label = bytes.get(offset + 1..offset + 1 + length as usize)?;
        total += label.len() + 1;
        if total > 254 || !label.is_ascii() {
            return None;
        }
        labels.push(String::from_utf8_lossy(label).to_ascii_lowercase());
        offset += 1 + length as usize;
        if !jumped {
            *cursor = offset;
        }
    }
    None
}

fn validate_dns_response(bytes: &[u8], id: u16) -> Result<(), &'static str> {
    let malformed = "TUN DNS 返回无效响应；未确认本地 DNS 就绪";
    let mut cursor = 0;
    if read_u16(bytes, &mut cursor) != Some(id) {
        return Err(malformed);
    }
    let flags = read_u16(bytes, &mut cursor).ok_or(malformed)?;
    if flags & 0x8000 == 0 || flags & 0x7800 != 0 || flags & 0x0200 != 0 {
        return Err(malformed);
    }
    if flags & 0xf != 0 {
        return Err(if flags & 0xf == 2 {
            "TUN DNS 返回 SERVFAIL；请检查内置 DNS 是否启用及其上游配置"
        } else {
            "TUN DNS 查询被拒绝或未解析成功"
        });
    }
    if read_u16(bytes, &mut cursor) != Some(1) {
        return Err(malformed);
    }
    let answers = read_u16(bytes, &mut cursor).ok_or(malformed)?;
    if answers == 0 || answers > 64 || bytes.len() < 12 {
        return Err(malformed);
    }
    cursor = 12;
    if dns_name(bytes, &mut cursor).as_deref() != Some(DNS_NAME)
        || read_u16(bytes, &mut cursor) != Some(1)
        || read_u16(bytes, &mut cursor) != Some(1)
    {
        return Err(malformed);
    }
    let mut addresses = std::collections::BTreeSet::new();
    let mut aliases = std::collections::BTreeMap::new();
    for _ in 0..answers {
        let owner = dns_name(bytes, &mut cursor).ok_or(malformed)?;
        let kind = read_u16(bytes, &mut cursor).ok_or(malformed)?;
        let class = read_u16(bytes, &mut cursor).ok_or(malformed)?;
        cursor = cursor.checked_add(4).ok_or(malformed)?; // TTL
        let length = read_u16(bytes, &mut cursor).ok_or(malformed)? as usize;
        let data = bytes
            .get(cursor..cursor.checked_add(length).ok_or(malformed)?)
            .ok_or(malformed)?;
        if kind == 1 && class == 1 && length == 4 {
            let ip = Ipv4Addr::new(data[0], data[1], data[2], data[3]);
            if !ip.is_unspecified() && !ip.is_loopback() && !ip.is_multicast() && !ip.is_broadcast()
            {
                addresses.insert(owner.clone());
            }
        } else if kind == 5 && class == 1 {
            let mut name_cursor = cursor;
            let target = dns_name(bytes, &mut name_cursor).ok_or(malformed)?;
            if name_cursor != cursor + length {
                return Err(malformed);
            }
            aliases.insert(owner, target);
        }
        cursor += length;
    }
    let mut owner = DNS_NAME;
    for _ in 0..=answers {
        if addresses.contains(owner) {
            return Ok(());
        }
        match aliases.get(owner) {
            Some(target) => owner = target,
            None => break,
        }
    }
    Err("TUN DNS 尚未返回有效 A 记录")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(id: u16) -> Vec<u8> {
        let mut reply = dns_question(id);
        reply[2] = 0x81;
        reply[3] = 0x80;
        reply[7] = 1;
        reply.extend_from_slice(&[0xc0, 12, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 198, 18, 0, 10]);
        reply
    }

    #[test]
    fn requires_matching_question_id_success_and_a_record() {
        let valid = reply(1234);
        assert!(validate_dns_response(&valid, 1234).is_ok());
        assert!(validate_dns_response(&valid, 4321).is_err());
        let mut fail = valid.clone();
        fail[3] = 0x82;
        assert!(validate_dns_response(&fail, 1234)
            .unwrap_err()
            .contains("SERVFAIL"));
        let mut truncated = valid.clone();
        truncated[2] |= 2;
        assert!(validate_dns_response(&truncated, 1234).is_err());
        let mut wrong_name = valid.clone();
        wrong_name[13] = b'x';
        assert!(validate_dns_response(&wrong_name, 1234).is_err());
        for length in 0..valid.len() {
            assert!(validate_dns_response(&valid[..length], 1234).is_err());
        }
        let mut cycle = valid.clone();
        cycle[12] = 0xc0;
        cycle[13] = 12;
        assert!(validate_dns_response(&cycle, 1234).is_err());
    }

    #[test]
    fn follows_a_declared_cname_but_rejects_unrelated_answers() {
        let mut response = dns_question(7);
        response[2] = 0x81;
        response[3] = 0x80;
        response[7] = 2;
        // example.com CNAME www.example.com, then an A answer for that alias.
        response.extend_from_slice(&[0xc0, 12, 0, 5, 0, 1, 0, 0, 0, 60, 0, 6]);
        let alias_offset = response.len();
        response.extend_from_slice(&[3, b'w', b'w', b'w', 0xc0, 12]);
        response.extend_from_slice(&[
            0xc0,
            alias_offset as u8,
            0,
            1,
            0,
            1,
            0,
            0,
            0,
            60,
            0,
            4,
            198,
            18,
            0,
            9,
        ]);
        assert!(validate_dns_response(&response, 7).is_ok());
        let mut unrelated = dns_question(7);
        unrelated[2] = 0x81;
        unrelated[3] = 0x80;
        unrelated[7] = 1;
        unrelated.extend_from_slice(&[
            3, b'w', b'w', b'w', 0xc0, 12, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 198, 18, 0, 9,
        ]);
        assert!(validate_dns_response(&unrelated, 7).is_err());
    }

    #[tokio::test]
    async fn real_udp_fixture_distinguishes_dns_success_from_servfail() {
        for successful in [true, false] {
            let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let target = server.local_addr().unwrap().to_string();
            let task = tokio::spawn(async move {
                let mut bytes = [0; 512];
                let (size, peer) = server.recv_from(&mut bytes).await.unwrap();
                assert_eq!(size, dns_question(0).len());
                let mut response = reply(u16::from_be_bytes([bytes[0], bytes[1]]));
                if !successful {
                    response[3] = 0x82;
                }
                server.send_to(&response, peer).await.unwrap();
            });
            let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let result = bounded(
                dns_query(&client, &target),
                Instant::now() + DNS_BUDGET,
                TunStage::Dns,
                &|| false,
            )
            .await
            .unwrap();
            assert_eq!(result.is_ok(), successful);
            task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn bounded_wait_cancels_and_times_out_without_waiting_for_io() {
        let error = bounded(
            std::future::pending::<()>(),
            Instant::now() + LOCAL_BUDGET,
            TunStage::Dns,
            &|| true,
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            AppError::TunStartup {
                stage: TunStage::Cancelled,
                ..
            }
        ));
        let start = Instant::now();
        let timed_out = bounded(
            std::future::pending::<()>(),
            start + Duration::from_millis(20),
            TunStage::Route,
            &|| false,
        )
        .await
        .unwrap_err();
        assert!(matches!(
            timed_out,
            AppError::TunStartup {
                stage: TunStage::Route,
                ..
            }
        ));
        assert!(start.elapsed() < Duration::from_secs(1));
    }
}
