//! 资源读模型：Docker 网络行的装配（SSR 表格与 `/ui/v1` 共用）。
//!
//! 这里只做「原始行 → 展示值」的归一（未知必须显式显示「未知」，不能显示 0
//! 或留空）；模板与 DTO 各自从这份模型取值，不再互相引用。

/// 网络行：`None` 一律显式落成「未知」，`Some(空)` 落成「无子网/无网关」。
#[derive(Debug, Clone)]
pub struct NetworkRow {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub scope: String,
    pub ipam_driver: String,
    pub subnet: String,
    pub gateway: String,
    pub internal: String,
    pub attachable: String,
    pub container_count: String,
    pub containers_known: bool,
    pub containers: Vec<NetworkContainer>,
    pub builtin: bool,
    pub can_remove: bool,
}

#[derive(Debug, Clone)]
pub struct NetworkContainer {
    pub name: String,
    pub ipv4: String,
    pub ipv6: String,
    pub mac: String,
}

pub fn network_row(n: skiff_docker::NetworkRow) -> NetworkRow {
    let builtin = n.builtin();
    let can_remove = n.can_remove();
    // 未知（Docker 未返回）必须显式显示「未知」，不能显示 0 或留空。
    let ipam_driver = match n.ipam_driver.as_deref() {
        Some(d) if !d.is_empty() => d.to_string(),
        _ => "未知".to_string(),
    };
    let subnet = match n.subnets.as_ref() {
        Some(v) if v.is_empty() => "—（无子网）".to_string(),
        Some(v) => v.join("，"),
        None => "未知".to_string(),
    };
    let gateway = match n.gateways.as_ref() {
        Some(v) if v.is_empty() => "—（无网关）".to_string(),
        Some(v) => v.join("，"),
        None => "未知".to_string(),
    };
    let internal = match n.internal {
        Some(true) => "内部",
        Some(false) => "外部",
        None => "未知",
    }
    .to_string();
    let attachable = match n.attachable {
        Some(true) => "可附加",
        Some(false) => "不可附加",
        None => "未知",
    }
    .to_string();
    let containers_known = n.containers.is_some();
    let container_count = match n.containers.as_ref() {
        Some(v) => format!("{} 个", v.len()),
        None => "未知".to_string(),
    };
    let containers = n
        .containers
        .unwrap_or_default()
        .into_iter()
        .map(|c| NetworkContainer {
            name: c.container,
            ipv4: c.ipv4.unwrap_or_else(|| "—".into()),
            ipv6: c.ipv6.unwrap_or_else(|| "—".into()),
            mac: c.mac.unwrap_or_else(|| "—".into()),
        })
        .collect();
    NetworkRow {
        id: n.id,
        name: n.name,
        driver: n.driver,
        scope: n.scope,
        ipam_driver,
        subnet,
        gateway,
        internal,
        attachable,
        container_count,
        containers_known,
        containers,
        builtin,
        can_remove,
    }
}
