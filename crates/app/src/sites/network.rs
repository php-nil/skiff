//! 站点专用容器网络与静态地址分配。
//!
//! 运行环境容器不再使用宿主网络、不向宿主发布端口；它们只接入本模块创建的
//! 专用 bridge，由内核静态分配地址。站点 vhost 只允许指向该地址，因此宿主上
//! 任何进程占用同号端口都与站点无关。
//!
//! 网段划分（与 1Panel 对齐，落在 `172.18.0.0/16`）：
//! - 子网 `172.18.0.0/16`，网关 `172.18.0.1`；
//! - Docker 动态池 `172.18.0.0/24`（留给用户/其它容器自动分配）；
//! - 静态池 `172.18.16.0/20`（`172.18.16.1`–`172.18.31.254`），只由本模块分配。
//!
//! 静态地址持久化在 `site_runtimes.container_ip`：容器重建复用同一地址；若
//! Docker 报地址冲突，启动失败并明确报错，绝不把站点指到别的容器。
//!
//! **网段冲突不静默处理**：宿主上已存在占用该网段的其它 Docker 网络，或同名
//! 网络的网段不是 `172.18.0.0/16` 时，直接报错并指出冲突来源，等待人工处理。

use skiff_store::Runtime;

use super::{SiteError, SiteSettings};
use crate::AppState;

/// 站点专用网络名。
pub const SITE_NETWORK: &str = "skiff-sites";
/// 专用网络子网。
pub const SITE_NETWORK_SUBNET: &str = "172.18.0.0/16";
/// Docker 动态分配段：与静态池隔离，避免自动分配抢占站点地址。
pub const SITE_NETWORK_DYNAMIC_RANGE: &str = "172.18.0.0/24";
/// 静态池起始/结束（含）。
const STATIC_POOL_START: (u8, u8) = (16, 1);
const STATIC_POOL_END: (u8, u8) = (31, 254);
const STATIC_PREFIX: &str = "172.18.";
/// 静态池的完整区间文案（错误提示用，避免多处手写网段）。
const STATIC_POOL_LABEL: &str = "172.18.16.1–172.18.31.254";

/// 静态池里第一个未被占用的地址。
///
/// `used` 是所有运行环境已登记的地址（包括已停止的实例），保证地址不会在
/// 容器重建之间被重复分配。池耗尽返回 `None`。
pub fn allocate_container_ip(used: &[String]) -> Option<String> {
    let used: Vec<&str> = used.iter().map(String::as_str).collect();
    for third in STATIC_POOL_START.0..=STATIC_POOL_END.0 {
        let first = if third == STATIC_POOL_START.0 {
            STATIC_POOL_START.1
        } else {
            1
        };
        let last = if third == STATIC_POOL_END.0 {
            STATIC_POOL_END.1
        } else {
            254
        };
        for fourth in first..=last {
            let ip = format!("{STATIC_PREFIX}{third}.{fourth}");
            if !used.contains(&ip.as_str()) {
                return Some(ip);
            }
        }
    }
    None
}

/// 地址是否落在静态池内（用于识别旧数据/手工写坏的值）。
pub fn is_static_pool_ip(ip: &str) -> bool {
    let Some(rest) = ip.strip_prefix(STATIC_PREFIX) else {
        return false;
    };
    let mut parts = rest.split('.');
    let (Some(third), Some(fourth), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let (Ok(third), Ok(fourth)) = (third.parse::<u8>(), fourth.parse::<u8>()) else {
        return false;
    };
    if !(STATIC_POOL_START.0..=STATIC_POOL_END.0).contains(&third) || !(1..=254).contains(&fourth) {
        return false;
    }
    if third == STATIC_POOL_START.0 && fourth < STATIC_POOL_START.1 {
        return false;
    }
    if third == STATIC_POOL_END.0 && fourth > STATIC_POOL_END.1 {
        return false;
    }
    true
}

/// IPv4 CIDR → 整数区间 `[起始, 结束]`；非 IPv4 或非法 CIDR 返回 `None`。
fn cidr_range(cidr: &str) -> Option<(u64, u64)> {
    let (addr, prefix) = cidr.trim().split_once('/')?;
    let prefix: u8 = prefix.parse().ok()?;
    if prefix > 32 {
        return None;
    }
    let mut octets = addr.split('.');
    let mut value = 0u32;
    for _ in 0..4 {
        let octet: u8 = octets.next()?.parse().ok()?;
        value = (value << 8) | u32::from(octet);
    }
    if octets.next().is_some() {
        return None;
    }
    let size = 1u64 << (32 - u32::from(prefix));
    let start = u64::from(value) & !(size - 1);
    Some((start, start + size - 1))
}

/// 两个 CIDR 是否有交集。无法解析（含 IPv6）按不重叠处理：本站点网络只用 IPv4。
fn cidr_overlaps(a: &str, b: &str) -> bool {
    match (cidr_range(a), cidr_range(b)) {
        (Some((a0, a1)), Some((b0, b1))) => a0 <= b1 && b0 <= a1,
        _ => false,
    }
}

impl AppState {
    /// 确保专用网络存在（幂等）。Docker 不可用时返回错误，由调用方决定是否阻塞。
    ///
    /// 网段冲突必须显式失败：同名网络网段不符、或其它网络已占用 `172.18.0.0/16`
    /// 时都直接报错并给出冲突来源，不自动换网段、不接管别人的网络。
    pub async fn ensure_site_network(&self) -> Result<(), SiteError> {
        let networks = self
            .docker()
            .list_networks()
            .await
            .map_err(|e| SiteError::Internal(format!("读取 Docker 网络失败：{e}")))?;
        let mut existing_subnets: Option<Vec<String>> = None;
        let mut conflicts: Vec<String> = Vec::new();
        for n in &networks {
            if n.name == SITE_NETWORK {
                existing_subnets = Some(n.subnets.clone().unwrap_or_default());
                continue;
            }
            for subnet in n.subnets.iter().flatten() {
                if cidr_overlaps(subnet, SITE_NETWORK_SUBNET) {
                    conflicts.push(format!("{}（{subnet}）", n.name));
                }
            }
        }
        if let Some(subnets) = existing_subnets {
            if subnets.iter().any(|s| s == SITE_NETWORK_SUBNET) {
                return Ok(());
            }
            let actual = if subnets.is_empty() {
                "未知（Docker 未返回 IPAM 配置）".to_string()
            } else {
                subnets.join("、")
            };
            return Err(SiteError::Conflict(format!(
                "已存在同名 Docker 网络 {SITE_NETWORK}，但网段是 {actual}，期望 {SITE_NETWORK_SUBNET}；\
                 请先删除该网络再重试（内核不会自动改网段或接管该网络）"
            )));
        }
        if !conflicts.is_empty() {
            return Err(SiteError::Conflict(format!(
                "宿主已存在占用 {SITE_NETWORK_SUBNET} 的 Docker 网络：{}；无法创建 {SITE_NETWORK}。\
                 请先删除或调整冲突网络（内核不会自动更换网段，也不会接管该网络）",
                conflicts.join("、")
            )));
        }
        self.docker()
            .create_network_with_ipam(
                SITE_NETWORK,
                Some(SITE_NETWORK_SUBNET),
                Some(SITE_NETWORK_DYNAMIC_RANGE),
            )
            .await
            .map_err(|e| {
                SiteError::Internal(format!(
                    "创建站点网络 {SITE_NETWORK}（{SITE_NETWORK_SUBNET}）失败：{e}"
                ))
            })?;
        Ok(())
    }

    /// 取运行环境的静态地址：已有合法地址直接复用，否则分配并落库。
    ///
    /// 分配时把**所有**运行环境的登记地址视为已占用，避免两个实例拿到同一地址。
    pub async fn ensure_runtime_ip(&self, rt: &Runtime) -> Result<String, SiteError> {
        let runtimes = self
            .db()
            .list_runtimes()
            .map_err(|e| SiteError::Internal(e.to_string()))?;
        let used: Vec<String> = runtimes
            .iter()
            .filter(|r| r.id != rt.id)
            .filter_map(|r| r.container_ip.clone())
            .collect();
        if let Some(ip) = rt
            .container_ip
            .as_deref()
            .filter(|ip| is_static_pool_ip(ip))
        {
            if !used.iter().any(|u| u == ip) {
                return Ok(ip.to_string());
            }
        }
        let ip = allocate_container_ip(&used).ok_or_else(|| {
            SiteError::Internal(format!("站点静态地址池已耗尽（{STATIC_POOL_LABEL}）"))
        })?;
        self.db()
            .set_runtime_container_ip(&rt.id, &ip)
            .map_err(|e| SiteError::Internal(e.to_string()))?;
        Ok(ip)
    }

    /// 站点 vhost 使用的容器内可达地址。PHP 走 socket，不需要地址。
    pub(crate) fn runtime_site_address(
        &self,
        s: &SiteSettings,
        rt: &Runtime,
    ) -> Result<String, SiteError> {
        if rt.kind == skiff_store::KIND_PHP {
            return Ok(s.runtime_socket(&rt.kind, &rt.name).display().to_string());
        }
        rt.container_ip
            .clone()
            .filter(|ip| !ip.is_empty())
            .ok_or_else(|| {
                SiteError::Unavailable(format!(
                    "运行环境 {} 尚未分配容器地址，请重建运行环境后再启用站点",
                    rt.name
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{cidr_overlaps, cidr_range, SITE_NETWORK_SUBNET};

    #[test]
    fn cidr_range_masks_host_bits() {
        assert_eq!(
            cidr_range("172.18.0.0/16"),
            Some((0xAC12_0000, 0xAC12_FFFF))
        );
        assert_eq!(
            cidr_range("172.18.16.1/16"),
            Some((0xAC12_0000, 0xAC12_FFFF))
        );
        assert_eq!(cidr_range("0.0.0.0/0"), Some((0, u64::from(u32::MAX))));
        assert_eq!(cidr_range("not-a-cidr"), None);
        assert_eq!(cidr_range("172.18.0.0/33"), None);
    }

    #[test]
    fn cidr_overlap_detects_same_and_nested_networks() {
        // 同名网段与包含关系都算冲突。
        assert!(cidr_overlaps("172.18.0.0/16", SITE_NETWORK_SUBNET));
        assert!(cidr_overlaps("172.18.0.0/24", SITE_NETWORK_SUBNET));
        assert!(cidr_overlaps("172.16.0.0/12", SITE_NETWORK_SUBNET));
        assert!(cidr_overlaps("172.18.128.0/17", SITE_NETWORK_SUBNET));
        // 相邻但不重叠的网段不算冲突。
        assert!(!cidr_overlaps("172.19.0.0/16", SITE_NETWORK_SUBNET));
        assert!(!cidr_overlaps("172.17.0.0/16", SITE_NETWORK_SUBNET));
        assert!(!cidr_overlaps("10.0.0.0/8", SITE_NETWORK_SUBNET));
        // IPv6 不参与本站点网段的冲突判定。
        assert!(!cidr_overlaps("fd00::/64", SITE_NETWORK_SUBNET));
    }
}
