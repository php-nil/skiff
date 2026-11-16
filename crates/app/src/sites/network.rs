//! 站点专用容器网络与静态地址分配。
//!
//! 运行环境容器不再使用宿主网络、不向宿主发布端口；它们只接入本模块创建的
//! 专用 bridge，由内核静态分配地址。站点 vhost 只允许指向该地址，因此宿主上
//! 任何进程占用同号端口都与站点无关。
//!
//! 网段选择：
//! - 首选 `172.18.0.0/16`（与 1Panel 对齐）；
//! - 首选与宿主既有 Docker 网络或宿主网卡地址重叠时，按固定候选序列
//!   `172.19`、`172.20` … `172.31`（均为 `/16`）挑第一个不重叠的网段；
//! - 选中结果持久化（`settings.sites.network_cidr`），并把「首选被谁占用」一并
//!   记录（`settings.sites.network_fallback`），页面与日志都明确展示，不静默。
//!
//! 每个候选段沿用同一池划分（以 `X.Y.0.0/16` 为例）：
//! - Docker 动态池 `X.Y.0.0/24`（留给用户/其它容器自动分配）；
//! - 静态池 `X.Y.16.1`–`X.Y.31.254`，只由本模块分配。
//!
//! 静态地址持久化在 `site_runtimes.container_ip`：容器重建复用同一地址；若
//! Docker 报地址冲突，启动失败并明确报错，绝不把站点指到别的容器。
//!
//! **不改宿主 Docker 配置、不接管他人网络**：候选段全部被占用时直接报错，
//! 不随意挑段。

use serde::Serialize;
use skiff_store::Runtime;

use super::{SiteError, SiteSettings};
use crate::AppState;

/// 站点专用网络名。
pub const SITE_NETWORK: &str = "skiff-sites";
/// 首选网段（与 1Panel 对齐）。
pub const SITE_NETWORK_PREFERRED: &str = "172.18.0.0/16";
/// 回退候选序列（含首选，按优先级排列）。
pub const SITE_NETWORK_CANDIDATES: [&str; 14] = [
    "172.18.0.0/16",
    "172.19.0.0/16",
    "172.20.0.0/16",
    "172.21.0.0/16",
    "172.22.0.0/16",
    "172.23.0.0/16",
    "172.24.0.0/16",
    "172.25.0.0/16",
    "172.26.0.0/16",
    "172.27.0.0/16",
    "172.28.0.0/16",
    "172.29.0.0/16",
    "172.30.0.0/16",
    "172.31.0.0/16",
];
/// 实际使用的网段（空 = 尚未创建站点网络）。
const KEY_NETWORK_CIDR: &str = "sites.network_cidr";
/// 首选被占用的原因（空 = 使用首选或尚未判定）。
const KEY_NETWORK_FALLBACK: &str = "sites.network_fallback";

/// 静态池的三段号（第三段 16–31，第四段 1–254）。
const STATIC_POOL_START: (u8, u8) = (16, 1);
const STATIC_POOL_END: (u8, u8) = (31, 254);

/// 站点网络的实际选择结果：网段、池划分与首选被占用的原因。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SiteNetworkInfo {
    /// 实际网段；空串表示尚未创建。
    pub cidr: String,
    /// 首选网段（常量，便于页面直接展示）。
    pub preferred: String,
    /// 空串 = 使用首选；非空 = 首选被占用，内容含占用来源。
    pub fallback_reason: String,
    pub dynamic_pool: String,
    pub static_pool_start: String,
    pub static_pool_end: String,
}

impl SiteNetworkInfo {
    fn new(cidr: &str, fallback_reason: &str) -> Self {
        let (dynamic_pool, static_pool_start, static_pool_end) = pool_layout(cidr);
        Self {
            cidr: cidr.to_string(),
            preferred: SITE_NETWORK_PREFERRED.to_string(),
            fallback_reason: fallback_reason.to_string(),
            dynamic_pool,
            static_pool_start,
            static_pool_end,
        }
    }
}

/// 网段占用来源：Docker 网络或宿主网卡。
#[derive(Debug, Clone, PartialEq, Eq)]
struct NetworkOccupant {
    /// 展示名（如 `Docker 网络 bridge`、`宿主网卡 eth0`）。
    source: String,
    cidr: String,
}

/// IPv4 点分地址 → 四段；非 IPv4 返回 `None`。
fn ipv4_octets(ip: &str) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut parts = ip.trim().split('.');
    for slot in &mut out {
        *slot = parts.next()?.parse().ok()?;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(out)
}

/// `/16` 网段的前两段；非 `/16` 或非法输入返回 `None`。
fn cidr_base16(cidr: &str) -> Option<[u8; 2]> {
    let (addr, prefix) = cidr.trim().split_once('/')?;
    if prefix != "16" {
        return None;
    }
    let octets = ipv4_octets(addr)?;
    if octets[2] != 0 || octets[3] != 0 {
        return None;
    }
    Some([octets[0], octets[1]])
}

/// 池划分：`(动态池, 静态起始, 静态结束)`；网段不合法时返回空串。
fn pool_layout(cidr: &str) -> (String, String, String) {
    let Some([a, b]) = cidr_base16(cidr) else {
        return (String::new(), String::new(), String::new());
    };
    (
        format!("{a}.{b}.0.0/24"),
        format!("{a}.{b}.{}.{}", STATIC_POOL_START.0, STATIC_POOL_START.1),
        format!("{a}.{b}.{}.{}", STATIC_POOL_END.0, STATIC_POOL_END.1),
    )
}

/// 静态池里第一个未被占用的地址。
///
/// `used` 是所有运行环境已登记的地址（包括已停止的实例），保证地址不会在
/// 容器重建之间被重复分配。池耗尽或网段不合法时返回 `None`。
pub fn allocate_container_ip(cidr: &str, used: &[String]) -> Option<String> {
    let [a, b] = cidr_base16(cidr)?;
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
            let ip = format!("{a}.{b}.{third}.{fourth}");
            if !used.contains(&ip.as_str()) {
                return Some(ip);
            }
        }
    }
    None
}

/// 地址是否落在该网段的静态池内（用于识别旧数据/手工写坏的值）。
pub fn is_static_pool_ip(cidr: &str, ip: &str) -> bool {
    let Some([a, b]) = cidr_base16(cidr) else {
        return false;
    };
    let Some([pa, pb, third, fourth]) = ipv4_octets(ip) else {
        return false;
    };
    if (pa, pb) != (a, b) {
        return false;
    }
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

/// 与给定网段重叠的占用来源。
fn conflicts_for<'a>(cidr: &str, occupied: &'a [NetworkOccupant]) -> Vec<&'a NetworkOccupant> {
    occupied
        .iter()
        .filter(|o| cidr_overlaps(&o.cidr, cidr))
        .collect()
}

fn describe(occupants: &[&NetworkOccupant]) -> String {
    occupants
        .iter()
        .map(|o| format!("{}（{}）", o.source, o.cidr))
        .collect::<Vec<_>>()
        .join("、")
}

/// 从已有占用里挑第一个可用候选段（纯函数，便于单测）。
fn select_site_network(occupied: &[NetworkOccupant]) -> Result<SiteNetworkInfo, SiteError> {
    let preferred_conflicts = conflicts_for(SITE_NETWORK_PREFERRED, occupied);
    for cidr in SITE_NETWORK_CANDIDATES {
        if conflicts_for(cidr, occupied).is_empty() {
            let reason = if cidr == SITE_NETWORK_PREFERRED {
                String::new()
            } else {
                format!(
                    "首选 {SITE_NETWORK_PREFERRED} 被占用：{}；当前使用 {cidr}",
                    describe(&preferred_conflicts)
                )
            };
            return Ok(SiteNetworkInfo::new(cidr, &reason));
        }
    }
    Err(SiteError::Conflict(format!(
        "站点网络候选段全部与宿主既有 Docker 网络/网卡重叠（{}），无法创建 {SITE_NETWORK}；\
         请释放其中一个 /16 网段后重试（内核不会改动宿主网络，也不会接管他人网络）",
        SITE_NETWORK_CANDIDATES.join("、")
    )))
}

/// 解析 `ip -4 -o addr show` 输出里的接口地址。
///
/// 行形如 `2: eth0    inet 172.17.0.2/16 brd 172.17.255.255 scope global eth0`。
fn parse_ip_addr_output(output: &str) -> Vec<NetworkOccupant> {
    let mut out = Vec::new();
    for line in output.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let Some(pos) = tokens.iter().position(|t| *t == "inet") else {
            continue;
        };
        let Some(cidr) = tokens.get(pos + 1) else {
            continue;
        };
        if cidr_range(cidr).is_none() {
            continue;
        }
        // 形如 `2: eth0`：第二个 token 是接口名。
        let iface = tokens.get(1).copied().unwrap_or("unknown");
        out.push(NetworkOccupant {
            source: format!("宿主网卡 {iface}"),
            cidr: (*cidr).to_string(),
        });
    }
    out
}

async fn host_network_occupants() -> Vec<NetworkOccupant> {
    // 只读枚举宿主 IPv4 地址；`ip` 不可用时退化为仅检查 Docker 网络。
    //
    // 走统一执行通道（`skiff_host`）：原来在阻塞池里裸跑没有超时，`ip` 卡住就
    // 会一直占着一个阻塞线程。这里给它 8 秒上限，失败按「没有宿主地址」处理。
    let args: Vec<String> = ["-4", "-o", "addr", "show"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    crate::host::run_host_msg(
        skiff_host::HostSpec::direct("ip", &args)
            .prefix("skiff-net")
            .description("skiff 宿主地址探测")
            .timeout(8),
    )
    .await
    .map(|o| parse_ip_addr_output(&o.stdout))
    .unwrap_or_default()
}

impl AppState {
    /// 当前站点网络的选择结果（页面/接口只读，不触发 Docker）。
    pub fn site_network_info(&self) -> SiteNetworkInfo {
        let read = |key: &str| {
            self.db()
                .get_setting(key)
                .ok()
                .flatten()
                .unwrap_or_default()
        };
        let cidr = read(KEY_NETWORK_CIDR);
        if cidr.is_empty() {
            return SiteNetworkInfo {
                preferred: SITE_NETWORK_PREFERRED.to_string(),
                static_pool_end: String::new(),
                ..SiteNetworkInfo::default()
            };
        }
        SiteNetworkInfo::new(&cidr, &read(KEY_NETWORK_FALLBACK))
    }

    fn site_network_cidr(&self) -> String {
        self.db()
            .get_setting(KEY_NETWORK_CIDR)
            .ok()
            .flatten()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| SITE_NETWORK_PREFERRED.to_string())
    }

    /// 确保专用网络存在（幂等），返回实际使用的网段与回退原因。
    ///
    /// 首选不可用时按候选序列回退，选中结果落库并在日志里写明占用者；
    /// 候选全部不可用时显式报错，不随意挑段、不改宿主网络。
    pub async fn ensure_site_network(&self) -> Result<SiteNetworkInfo, SiteError> {
        let networks = self
            .docker()
            .list_networks()
            .await
            .map_err(|e| SiteError::Internal(format!("读取 Docker 网络失败：{e}")))?;

        let mut occupied: Vec<NetworkOccupant> = Vec::new();
        let mut existing: Vec<String> = Vec::new();
        for n in &networks {
            if n.name == SITE_NETWORK {
                existing = n.subnets.clone().unwrap_or_default();
                continue;
            }
            for subnet in n.subnets.iter().flatten() {
                occupied.push(NetworkOccupant {
                    source: format!("Docker 网络 {}", n.name),
                    cidr: subnet.clone(),
                });
            }
        }
        occupied.extend(host_network_occupants().await);

        if let Some(cidr) = existing.iter().find(|s| cidr_base16(s).is_some()) {
            // 已存在的网络以实际网段为准；与本库记录不一致时纠正并留日志。
            let stored = self.site_network_cidr();
            let preferred_conflicts = conflicts_for(SITE_NETWORK_PREFERRED, &occupied);
            let reason = if cidr == SITE_NETWORK_PREFERRED {
                String::new()
            } else if !preferred_conflicts.is_empty() {
                format!(
                    "首选 {SITE_NETWORK_PREFERRED} 被占用：{}；当前使用 {cidr}",
                    describe(&preferred_conflicts)
                )
            } else {
                format!("当前使用 {cidr}")
            };
            if stored != *cidr {
                tracing::warn!(
                    network = SITE_NETWORK,
                    cidr = %cidr,
                    stored = %stored,
                    "站点网络实际网段与库内记录不一致，按实际网段纠正"
                );
            }
            self.persist_site_network(cidr, &reason)?;
            return Ok(SiteNetworkInfo::new(cidr, &reason));
        }
        if !existing.is_empty() {
            return Err(SiteError::Conflict(format!(
                "已存在同名 Docker 网络 {SITE_NETWORK}，但网段无法识别（{}）；\
                 请先删除该网络再重试（内核不会自动改网段或接管该网络）",
                existing.join("、")
            )));
        }

        let info = select_site_network(&occupied)?;
        let (dynamic, start, end) = pool_layout(&info.cidr);
        self.docker()
            .create_network_with_ipam(SITE_NETWORK, Some(&info.cidr), Some(&dynamic))
            .await
            .map_err(|e| {
                SiteError::Internal(format!(
                    "创建站点网络 {SITE_NETWORK}（{}）失败：{e}",
                    info.cidr
                ))
            })?;
        self.persist_site_network(&info.cidr, &info.fallback_reason)?;
        if info.fallback_reason.is_empty() {
            tracing::info!(network = SITE_NETWORK, cidr = %info.cidr, "已创建站点专用网络");
        } else {
            tracing::warn!(
                network = SITE_NETWORK,
                cidr = %info.cidr,
                reason = %info.fallback_reason,
                "站点首选网段被占用，已回退到候选网段"
            );
        }
        tracing::info!(
            network = SITE_NETWORK,
            cidr = %info.cidr,
            dynamic_pool = %dynamic,
            static_pool = %format!("{start}–{end}"),
            "站点网络池划分"
        );
        Ok(info)
    }

    fn persist_site_network(&self, cidr: &str, fallback_reason: &str) -> Result<(), SiteError> {
        let db = self.db();
        db.set_setting(KEY_NETWORK_CIDR, cidr)
            .map_err(|e| SiteError::Internal(e.to_string()))?;
        db.set_setting(KEY_NETWORK_FALLBACK, fallback_reason)
            .map_err(|e| SiteError::Internal(e.to_string()))?;
        Ok(())
    }

    /// 取运行环境的静态地址：已有合法地址直接复用，否则在当前站点网段内分配并落库。
    ///
    /// 分配时把**所有**运行环境的登记地址视为已占用，避免两个实例拿到同一地址。
    pub async fn ensure_runtime_ip(&self, rt: &Runtime) -> Result<String, SiteError> {
        let cidr = self.site_network_cidr();
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
            .filter(|ip| is_static_pool_ip(&cidr, ip))
        {
            if !used.iter().any(|u| u == ip) {
                return Ok(ip.to_string());
            }
        }
        let ip = allocate_container_ip(&cidr, &used).ok_or_else(|| {
            let (_, start, end) = pool_layout(&cidr);
            SiteError::Internal(format!("站点静态地址池已耗尽（{cidr} 内 {start}–{end}）"))
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
    use super::{
        allocate_container_ip, cidr_overlaps, cidr_range, is_static_pool_ip, parse_ip_addr_output,
        pool_layout, select_site_network, NetworkOccupant, SITE_NETWORK_PREFERRED,
    };

    fn occ(source: &str, cidr: &str) -> NetworkOccupant {
        NetworkOccupant {
            source: source.to_string(),
            cidr: cidr.to_string(),
        }
    }

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
        assert!(cidr_overlaps("172.18.0.0/16", SITE_NETWORK_PREFERRED));
        assert!(cidr_overlaps("172.18.0.0/24", SITE_NETWORK_PREFERRED));
        assert!(cidr_overlaps("172.16.0.0/12", SITE_NETWORK_PREFERRED));
        assert!(cidr_overlaps("172.18.128.0/17", SITE_NETWORK_PREFERRED));
        assert!(!cidr_overlaps("172.19.0.0/16", SITE_NETWORK_PREFERRED));
        assert!(!cidr_overlaps("172.17.0.0/16", SITE_NETWORK_PREFERRED));
        assert!(!cidr_overlaps("10.0.0.0/8", SITE_NETWORK_PREFERRED));
        assert!(!cidr_overlaps("fd00::/64", SITE_NETWORK_PREFERRED));
    }

    #[test]
    fn preferred_is_free_keeps_alignment_segment() {
        let info = select_site_network(&[occ("宿主网卡 eth0", "172.17.0.1/16")]).unwrap();
        assert_eq!(info.cidr, SITE_NETWORK_PREFERRED);
        assert!(info.fallback_reason.is_empty());
        assert_eq!(info.dynamic_pool, "172.18.0.0/24");
        assert_eq!(info.static_pool_start, "172.18.16.1");
        assert_eq!(info.static_pool_end, "172.18.31.254");
    }

    #[test]
    fn occupied_preferred_falls_back_and_names_the_occupant() {
        let occupied = [
            occ("宿主网卡 eth0", "172.17.0.1/16"),
            occ("Docker 网络 bridge", "172.18.0.1/16"),
        ];
        let info = select_site_network(&occupied).unwrap();
        assert_eq!(info.cidr, "172.19.0.0/16");
        assert!(info.fallback_reason.contains("Docker 网络 bridge"));
        assert!(info.fallback_reason.contains("172.18.0.0/16"));
        // 池划分跟随实际网段。
        assert_eq!(info.dynamic_pool, "172.19.0.0/24");
        assert_eq!(info.static_pool_start, "172.19.16.1");
        assert_eq!(info.static_pool_end, "172.19.31.254");
    }

    #[test]
    fn host_interface_also_blocks_candidate_segment() {
        // 宿主网卡已占 172.19，回退应跳到 172.20。
        let occupied = [
            occ("Docker 网络 bridge", "172.18.0.1/16"),
            occ("宿主网卡 ens18", "172.19.0.2/16"),
        ];
        let info = select_site_network(&occupied).unwrap();
        assert_eq!(info.cidr, "172.20.0.0/16");
    }

    #[test]
    fn all_candidates_occupied_is_an_error() {
        let occupied: Vec<NetworkOccupant> = (18..=31)
            .map(|n| occ("Docker 网络 x", &format!("172.{n}.0.1/16")))
            .collect();
        let err = select_site_network(&occupied).unwrap_err();
        assert!(format!("{err}").contains("候选段全部"), "{err}");
    }

    #[test]
    fn allocation_follows_selected_segment() {
        let used = vec!["172.19.16.1".to_string(), "172.19.16.2".to_string()];
        assert_eq!(
            allocate_container_ip("172.19.0.0/16", &used).as_deref(),
            Some("172.19.16.3")
        );
        assert!(is_static_pool_ip("172.19.0.0/16", "172.19.16.3"));
        assert!(is_static_pool_ip("172.19.0.0/16", "172.19.31.254"));
        assert!(!is_static_pool_ip("172.19.0.0/16", "172.19.0.3"));
        assert!(!is_static_pool_ip("172.19.0.0/16", "172.18.16.3"));
    }

    #[test]
    fn pool_layout_only_accepts_16_bits() {
        assert_eq!(
            pool_layout("172.18.0.0/16"),
            (
                "172.18.0.0/24".to_string(),
                "172.18.16.1".to_string(),
                "172.18.31.254".to_string()
            )
        );
        assert_eq!(
            pool_layout("172.18.0.0/24"),
            (String::new(), String::new(), String::new())
        );
    }

    #[test]
    fn ip_addr_output_is_parsed_with_interface_names() {
        let output = "1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever\n\
                      2: eth0    inet 172.17.0.2/16 brd 172.17.255.255 scope global eth0\n\
                      3: docker0    inet 172.18.0.1/16 brd 172.18.255.255 scope global docker0\n";
        let got = parse_ip_addr_output(output);
        assert_eq!(got.len(), 3);
        assert_eq!(got[1].source, "宿主网卡 eth0");
        assert_eq!(got[1].cidr, "172.17.0.2/16");
        assert_eq!(got[2].cidr, "172.18.0.1/16");
    }
}
