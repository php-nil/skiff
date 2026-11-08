//! Compose MVP 子集（§6.5）。
//!
//! 格式所有权：Compose 子集解析在本 crate 唯一实现，`skiff-docker` 以
//! `pub use skiff_compose as compose` 转出，`xtask` 直接依赖本 crate。
//! 拆出的理由：解析是纯 serde_yaml 演算，不该把 bollard/tokio 拖给校验工具。
//!
//! 支持：顶层 `services` / `volumes` / `networks`，可选 `version`（接受并忽略）。
//! 服务字段只允许 `image`、`command`、`entrypoint`、`environment`、`ports`、`restart`、
//! `depends_on`、`healthcheck`、`volumes`、`networks`、`labels`。
//! 其余字段（`build` / `extends` / `profiles` / `env_file` / `secrets` / `configs` 等）
//! 一律**显式报错**，不静默忽略；Compose 不调用 Engine `/build`，镜像只走 `fromImage` 拉取。

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use skiff_model::{is_posix_absolute, ContainerSpec, Healthcheck, Mount, PortMapping};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComposeError {
    Yaml(String),
    Unsupported(String),
    Invalid(String),
    /// 循环依赖
    Cycle(String),
}

impl std::fmt::Display for ComposeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComposeError::Yaml(m) => write!(f, "compose 解析失败: {m}"),
            ComposeError::Unsupported(m) => write!(f, "不支持的 compose 字段: {m}"),
            ComposeError::Invalid(m) => write!(f, "compose 内容非法: {m}"),
            ComposeError::Cycle(m) => write!(f, "服务依赖成环: {m}"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct ComposeFile {
    #[serde(default)]
    version: Option<serde_yaml::Value>,
    #[serde(default)]
    services: BTreeMap<String, Service>,
    #[serde(default)]
    volumes: BTreeMap<String, Option<serde_yaml::Value>>,
    #[serde(default)]
    networks: BTreeMap<String, Option<serde_yaml::Value>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Service {
    image: Option<String>,
    #[serde(default)]
    command: Option<serde_yaml::Value>,
    #[serde(default)]
    entrypoint: Option<serde_yaml::Value>,
    #[serde(default)]
    environment: Option<serde_yaml::Value>,
    #[serde(default)]
    ports: Vec<serde_yaml::Value>,
    #[serde(default)]
    restart: Option<String>,
    #[serde(default)]
    depends_on: Option<serde_yaml::Value>,
    #[serde(default)]
    healthcheck: Option<serde_yaml::Value>,
    #[serde(default)]
    volumes: Vec<String>,
    #[serde(default)]
    networks: Option<serde_yaml::Value>,
    #[serde(default)]
    labels: Option<serde_yaml::Value>,
}

/// 解析后的 compose 项目：可直接映射成容器创建调用。
#[derive(Debug, Clone)]
pub struct ComposeProject {
    pub name: String,
    pub services: Vec<PlannedService>,
    pub volumes: Vec<String>,
    pub networks: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PlannedService {
    pub service: String,
    pub container_name: String,
    pub spec: ContainerSpec,
    pub depends_on: Vec<String>,
}

/// 项目名归一化：小写、非字母数字转 `-`、去掉首尾 `-`，空值回落 `skiff-project`。
/// 公开是为了让 Web 层在 `down` / 起停时与 `up` 使用同一套名字规则。
pub fn normalize_project_name(raw: &str) -> String {
    // Docker 的 compose 项目名要求 ^[a-z0-9][a-z0-9_-]*$：
    // 允许的继续保留（'_' 归一到 '-'，避免与标签选择器混淆），分隔符统一成 '-'，
    // 其它字符（含 '/'）全部丢弃；尾部的 '-' 允许，开头不行。
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if c.is_whitespace() || c == '.' || c == '-' || c == '_' {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "skiff-project".to_string()
    } else {
        trimmed.chars().take(48).collect()
    }
}

pub fn parse(text: &str, project: &str) -> Result<ComposeProject, ComposeError> {
    let file: ComposeFile = serde_yaml::from_str(text).map_err(|e| {
        let msg = e.to_string();
        if msg.contains("unknown field") {
            ComposeError::Unsupported(msg)
        } else {
            ComposeError::Yaml(msg)
        }
    })?;
    if file.services.is_empty() {
        return Err(ComposeError::Invalid("services 为空".into()));
    }
    // version 只接受 2.x/3.x 字符串或数字，其它类型直接报错（避免把对象当版本静默接受）。
    if let Some(v) = &file.version {
        let ok = v.is_string() || v.is_number() || v.is_null();
        if !ok {
            return Err(ComposeError::Invalid("version 只接受字符串/数字".into()));
        }
    }

    let project_name = normalize_project_name(project);
    let declared_networks: BTreeSet<String> = file.networks.keys().cloned().collect();
    let declared_volumes: BTreeSet<String> = file.volumes.keys().cloned().collect();
    for (kind, names) in [
        ("networks", &declared_networks),
        ("volumes", &declared_volumes),
    ] {
        for name in names {
            validate_declared_name(kind, name)?;
        }
    }

    // 依赖闭环检测（只用于诊断，执行顺序由 depends_on 拓扑排序给出）。
    let mut deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, svc) in &file.services {
        let d = parse_depends_on(svc.depends_on.as_ref())?;
        for dep in &d {
            if !file.services.contains_key(dep) {
                return Err(ComposeError::Invalid(format!(
                    "服务 {name} 依赖了不存在的服务 {dep}"
                )));
            }
        }
        deps.insert(name.clone(), d);
    }
    topological_order(&deps)?;

    let mut services = Vec::new();
    let mut used_networks: BTreeSet<String> = BTreeSet::new();
    for name in topological_order(&deps)? {
        let svc = file
            .services
            .get(&name)
            .expect("拓扑顺序只包含已校验的服务");
        validate_declared_name("service", &name)?;
        let image = svc.image.clone().ok_or_else(|| {
            ComposeError::Unsupported(format!(
                "服务 {name} 使用 build 或缺少 image（不支持 build）"
            ))
        })?;
        if image.trim().is_empty() {
            return Err(ComposeError::Invalid(format!("服务 {name} 的 image 为空")));
        }

        // 保留标签由 Docker 执行层在策略校验通过后写入，解析器只保留用户标签。
        let labels: BTreeMap<String, String> = parse_map(svc.labels.as_ref(), "labels")?;
        let mut mounts = Vec::new();
        for v in &svc.volumes {
            mounts.push(parse_volume(v, &project_name)?);
        }

        let logical_networks = parse_networks(svc.networks.as_ref(), &declared_networks)?;
        let networks = if logical_networks.is_empty() {
            vec![format!("{project_name}_default")]
        } else {
            logical_networks
                .iter()
                .map(|n| format!("{project_name}_{n}"))
                .collect()
        };
        for n in &networks {
            used_networks.insert(n.clone());
        }

        let ports = svc
            .ports
            .iter()
            .map(parse_port)
            .collect::<Result<Vec<_>, _>>()?;

        let healthcheck = svc
            .healthcheck
            .as_ref()
            .map(parse_healthcheck)
            .transpose()?;

        let spec = ContainerSpec {
            image: image.clone(),
            name: Some(format!("{project_name}-{name}")),
            command: parse_command(svc.command.as_ref())?,
            entrypoint: parse_command(svc.entrypoint.as_ref())?,
            env: parse_env(svc.environment.as_ref())?,
            labels,
            ports,
            restart: svc
                .restart
                .clone()
                .or_else(|| Some("unless-stopped".into())),
            healthcheck,
            networks,
            network_mode: None,
            mounts,
            log_driver: None,
            log_max_size: None,
            memory_limit_mb: None,
            nano_cpus: None,
            pids_limit: None,
            cpu_shares: None,
            cpu_period: None,
            cpu_quota: None,
            memory_swap_mb: None,
        };

        services.push(PlannedService {
            service: name.clone(),
            container_name: format!("{project_name}-{name}"),
            depends_on: deps.get(&name).cloned().unwrap_or_default(),
            spec,
        });
    }

    let volumes = declared_volumes
        .into_iter()
        .map(|name| format!("{project_name}_{name}"))
        .collect();

    Ok(ComposeProject {
        name: project_name,
        services,
        volumes,
        networks: used_networks.into_iter().collect(),
    })
}

fn validate_declared_name(kind: &str, name: &str) -> Result<(), ComposeError> {
    // 项目名最长 48；`{project}_{name}` 要留在 Docker 对象名 128 字符以内。
    if name.is_empty() || name.len() > 64 {
        return Err(ComposeError::Invalid(format!(
            "{kind} 名称 {name:?} 为空或超过 64 字符"
        )));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '.'))
    {
        return Err(ComposeError::Invalid(format!(
            "{kind} 名称 {name:?} 只允许小写字母、数字、'_'、'-'、'.'"
        )));
    }
    Ok(())
}

fn parse_depends_on(v: Option<&serde_yaml::Value>) -> Result<Vec<String>, ComposeError> {
    let Some(v) = v else { return Ok(vec![]) };
    if let Some(seq) = v.as_sequence() {
        return seq
            .iter()
            .map(|x| {
                x.as_str()
                    .map(|s| s.to_string())
                    .ok_or_else(|| ComposeError::Invalid("depends_on 只支持服务名列表".into()))
            })
            .collect();
    }
    Err(ComposeError::Invalid(
        "depends_on 只支持列表形式（不支持 condition 等映射写法）".into(),
    ))
}

fn topological_order(deps: &BTreeMap<String, Vec<String>>) -> Result<Vec<String>, ComposeError> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        New,
        Active,
        Done,
    }
    fn visit(
        node: &str,
        deps: &BTreeMap<String, Vec<String>>,
        marks: &mut BTreeMap<String, Mark>,
        out: &mut Vec<String>,
        stack: &mut Vec<String>,
    ) -> Result<(), ComposeError> {
        match marks.get(node).copied().unwrap_or(Mark::New) {
            Mark::Done => return Ok(()),
            Mark::Active => {
                stack.push(node.to_string());
                return Err(ComposeError::Cycle(stack.join(" -> ")));
            }
            Mark::New => {}
        }
        marks.insert(node.to_string(), Mark::Active);
        stack.push(node.to_string());
        if let Some(ds) = deps.get(node) {
            for d in ds {
                visit(d, deps, marks, out, stack)?;
            }
        }
        stack.pop();
        marks.insert(node.to_string(), Mark::Done);
        out.push(node.to_string());
        Ok(())
    }
    let mut marks = BTreeMap::new();
    let mut out = Vec::new();
    let mut stack = Vec::new();
    for node in deps.keys() {
        visit(node, deps, &mut marks, &mut out, &mut stack)?;
    }
    Ok(out)
}

fn parse_map(
    v: Option<&serde_yaml::Value>,
    field: &str,
) -> Result<BTreeMap<String, String>, ComposeError> {
    let Some(v) = v else {
        return Ok(BTreeMap::new());
    };
    let map = v
        .as_mapping()
        .ok_or_else(|| ComposeError::Invalid(format!("{field} 只支持键值映射")))?;
    let mut out = BTreeMap::new();
    for (k, val) in map {
        let key = k
            .as_str()
            .ok_or_else(|| ComposeError::Invalid(format!("{field} 的键必须是字符串")))?;
        let value = match val {
            serde_yaml::Value::String(s) => s.clone(),
            serde_yaml::Value::Number(n) => n.to_string(),
            serde_yaml::Value::Bool(b) => b.to_string(),
            serde_yaml::Value::Null => String::new(),
            _ => return Err(ComposeError::Invalid(format!("{field}.{key} 只支持标量值"))),
        };
        out.insert(key.to_string(), value);
    }
    Ok(out)
}

fn parse_env(v: Option<&serde_yaml::Value>) -> Result<BTreeMap<String, String>, ComposeError> {
    let Some(v) = v else {
        return Ok(BTreeMap::new());
    };
    if let Some(seq) = v.as_sequence() {
        let mut out = BTreeMap::new();
        for item in seq {
            let s = item
                .as_str()
                .ok_or_else(|| ComposeError::Invalid("environment 列表项必须是字符串".into()))?;
            match s.split_once('=') {
                Some((k, val)) => {
                    out.insert(k.to_string(), val.to_string());
                }
                None => {
                    return Err(ComposeError::Invalid(format!(
                        "environment 不支持宿主环境透传（{s} 缺少 =值）"
                    )))
                }
            }
        }
        return Ok(out);
    }
    parse_map(Some(v), "environment")
}

fn parse_command(v: Option<&serde_yaml::Value>) -> Result<Option<Vec<String>>, ComposeError> {
    let Some(v) = v else { return Ok(None) };
    if let Some(seq) = v.as_sequence() {
        let mut out = Vec::new();
        for item in seq {
            out.push(
                item.as_str()
                    .map(|s| s.to_string())
                    .or_else(|| item.as_i64().map(|n| n.to_string()))
                    .ok_or_else(|| {
                        ComposeError::Invalid("command/entrypoint 列表项必须是标量".into())
                    })?,
            );
        }
        return Ok(Some(out));
    }
    if let Some(s) = v.as_str() {
        // Compose 的字符串形式经 shell 解析；这里显式落到 /bin/sh -c，避免自行分词歧义。
        return Ok(Some(vec!["/bin/sh".into(), "-c".into(), s.to_string()]));
    }
    Err(ComposeError::Invalid(
        "command/entrypoint 只支持字符串或列表".into(),
    ))
}

fn parse_port(v: &serde_yaml::Value) -> Result<PortMapping, ComposeError> {
    if let Some(s) = v.as_str() {
        let (host, container, proto) = split_port(s)?;
        return Ok(PortMapping {
            host,
            container,
            protocol: proto,
        });
    }
    if let Some(n) = v.as_i64() {
        // 只写容器端口：宿主端口交给 Engine 随机分配会让审计不可复现，直接拒绝。
        return Err(ComposeError::Unsupported(format!(
            "端口 {n} 缺少宿主端口（MVP 只支持 host:container 短语法）"
        )));
    }
    Err(ComposeError::Invalid("ports 只支持字符串短语法".into()))
}

fn split_port(s: &str) -> Result<(u16, u16, Option<String>), ComposeError> {
    let (body, proto) = match s.split_once('/') {
        Some((b, p)) => (b, Some(p.to_string())),
        None => (s, None),
    };
    let parts: Vec<&str> = body.split(':').collect();
    let (host, container) = match parts.as_slice() {
        [h, c] => (*h, *c),
        _ => {
            return Err(ComposeError::Unsupported(format!(
                "端口写法 {s} 不支持（只支持 host:container[/proto]）"
            )))
        }
    };
    let host: u16 = host
        .parse()
        .map_err(|_| ComposeError::Invalid(format!("宿主端口非法: {host}")))?;
    let container: u16 = container
        .parse()
        .map_err(|_| ComposeError::Invalid(format!("容器端口非法: {container}")))?;
    if host == 0 || container == 0 {
        return Err(ComposeError::Invalid("端口必须在 1–65535".into()));
    }
    match proto.as_deref() {
        None | Some("tcp") | Some("udp") => {}
        Some(other) => return Err(ComposeError::Unsupported(format!("端口协议 {other}"))),
    }
    Ok((host, container, proto))
}

fn parse_healthcheck(v: &serde_yaml::Value) -> Result<Healthcheck, ComposeError> {
    let map = v
        .as_mapping()
        .ok_or_else(|| ComposeError::Invalid("healthcheck 只支持映射".into()))?;
    let mut test: Option<Vec<String>> = None;
    let mut interval = None;
    let mut timeout = None;
    let mut retries = None;
    for (k, val) in map {
        let key = k.as_str().unwrap_or_default();
        match key {
            "test" => {
                test = Some(match val {
                    serde_yaml::Value::String(s) => vec!["CMD-SHELL".into(), s.clone()],
                    serde_yaml::Value::Sequence(seq) => seq
                        .iter()
                        .map(|x| {
                            x.as_str().map(|s| s.to_string()).ok_or_else(|| {
                                ComposeError::Invalid("healthcheck.test 项必须是字符串".into())
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    _ => {
                        return Err(ComposeError::Invalid(
                            "healthcheck.test 只支持字符串或列表".into(),
                        ))
                    }
                });
            }
            "interval" | "timeout" | "start_period" => {
                let secs = parse_duration_secs(val)?;
                if key == "interval" {
                    interval = Some(secs);
                } else if key == "timeout" {
                    timeout = Some(secs);
                }
            }
            "retries" => retries = val.as_i64(),
            other => {
                return Err(ComposeError::Unsupported(format!(
                    "healthcheck.{other}（MVP 只支持 test/interval/timeout/retries）"
                )))
            }
        }
    }
    Ok(Healthcheck {
        test: test.ok_or_else(|| ComposeError::Invalid("healthcheck 缺少 test".into()))?,
        interval_secs: interval,
        timeout_secs: timeout,
        retries,
    })
}

fn parse_duration_secs(v: &serde_yaml::Value) -> Result<i64, ComposeError> {
    if let Some(n) = v.as_i64() {
        return Ok(n);
    }
    let s = v
        .as_str()
        .ok_or_else(|| ComposeError::Invalid("时长必须是数字或 '30s' 形式".into()))?;
    let (num, unit) = s.split_at(s.len().saturating_sub(1));
    let n: i64 = num
        .parse()
        .map_err(|_| ComposeError::Invalid(format!("时长非法: {s}")))?;
    let mult = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        _ => return Err(ComposeError::Invalid(format!("时长单位只支持 s/m/h: {s}"))),
    };
    Ok(n * mult)
}

fn parse_volume(spec: &str, project: &str) -> Result<Mount, ComposeError> {
    let parts: Vec<&str> = spec.split(':').collect();
    if parts.len() < 2 {
        return Err(ComposeError::Invalid(format!(
            "volume 写法 {spec} 不支持（只支持 src:dst[:ro]）"
        )));
    }
    if parts.len() > 3 {
        return Err(ComposeError::Unsupported(format!(
            "volume 写法 {spec} 不支持（MVP 只支持 src:dst[:ro]）"
        )));
    }
    let source = parts[0];
    let target = parts[1];
    let read_only = parts.get(2).map(|m| *m == "ro").unwrap_or(false);
    if parts.get(2).map(|m| *m != "ro").unwrap_or(false) {
        return Err(ComposeError::Unsupported(format!(
            "volume 模式只支持 ro: {spec}"
        )));
    }
    if !is_posix_absolute(target) {
        return Err(ComposeError::Invalid(format!(
            "容器内路径必须是绝对路径: {spec}"
        )));
    }
    // 宿主绑定挂载的 source：POSIX 绝对路径 / `./x` / `~/x`；其余按 named volume。
    if is_posix_absolute(source) || source.starts_with('.') || source.contains('~') {
        return Ok(Mount::Bind {
            source: source.to_string(),
            target: target.to_string(),
            read_only,
        });
    }
    // named volume 一律加项目前缀（声明与隐式创建一致），避免跨项目碰撞。
    Ok(Mount::Volume {
        source: format!("{project}_{source}"),
        target: target.to_string(),
        read_only,
    })
}

fn parse_networks(
    v: Option<&serde_yaml::Value>,
    declared: &BTreeSet<String>,
) -> Result<Vec<String>, ComposeError> {
    let Some(v) = v else { return Ok(vec![]) };
    if let Some(seq) = v.as_sequence() {
        let mut out = Vec::new();
        for item in seq {
            let name = item
                .as_str()
                .ok_or_else(|| ComposeError::Invalid("networks 列表项必须是字符串".into()))?;
            if !declared.contains(name) && name != "default" {
                return Err(ComposeError::Invalid(format!(
                    "networks 引用了未声明的网络 {name}（MVP 不支持 external 网络）"
                )));
            }
            out.push(name.to_string());
        }
        return Ok(out);
    }
    // 映射写法常见于网络别名，MVP 不支持
    Err(ComposeError::Unsupported(
        "networks 只支持名称列表（不支持 aliases 等映射写法）".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
version: "3.8"
services:
  web:
    image: nginx:1.27-alpine
    ports:
      - "18081:80"
    environment:
      TZ: Asia/Shanghai
    volumes:
      - html:/usr/share/nginx/html
      - ./conf/nginx.conf:/etc/nginx/nginx.conf:ro
    restart: unless-stopped
  api:
    image: example/api:1.0
    depends_on:
      - web
    command: ["/app/api", "--port", "8080"]
volumes:
  html:
networks:
  default:
"#;

    #[test]
    fn parses_supported_subset() {
        let p = parse(SAMPLE, "My Project").unwrap();
        assert_eq!(p.name, "my-project");
        assert_eq!(p.services.len(), 2);
        let web = p.services.iter().find(|s| s.service == "web").unwrap();
        assert_eq!(web.container_name, "my-project-web");
        assert_eq!(web.spec.ports[0].host, 18081);
        // 保留标签只在执行层注入，解析器不写 io.skiff.*。
        assert!(!web.spec.labels.contains_key("io.skiff.project"));
        assert!(!web.spec.labels.contains_key("io.skiff.owner"));
        assert!(matches!(
            web.spec.mounts[0],
            Mount::Volume { ref source, .. } if source == "my-project_html"
        ));
        assert!(matches!(web.spec.mounts[1], Mount::Bind { .. }));
        assert!(web.spec.mounts[1].read_only());
        assert_eq!(web.spec.networks, vec!["my-project_default".to_string()]);
        assert_eq!(p.volumes, vec!["my-project_html".to_string()]);
        assert_eq!(p.networks, vec!["my-project_default".to_string()]);
        let api = p.services.iter().find(|s| s.service == "api").unwrap();
        assert_eq!(api.depends_on, vec!["web".to_string()]);
        // 拓扑顺序：web 先于 api。
        let names: Vec<_> = p.services.iter().map(|s| s.service.as_str()).collect();
        assert_eq!(names, vec!["web", "api"]);
    }

    #[test]
    fn declares_used_networks_and_user_labels() {
        let yaml = r#"
services:
  web:
    image: nginx
    networks: [front]
    labels:
      app: demo
networks:
  front:
  unused:
"#;
        let p = parse(yaml, "demo").unwrap();
        assert_eq!(p.networks, vec!["demo_front".to_string()]);
        assert_eq!(
            p.services[0].spec.labels.get("app").map(String::as_str),
            Some("demo")
        );
        assert_eq!(p.services[0].spec.networks, vec!["demo_front".to_string()]);
    }

    #[test]
    fn rejects_unsupported_fields_explicitly() {
        let yaml = "services:\n  web:\n    image: nginx\n    build: .\n";
        let e = parse(yaml, "p").unwrap_err();
        assert!(
            matches!(e, ComposeError::Unsupported(_)),
            "应为显式不支持错误: {e}"
        );
        assert!(e.to_string().contains("build"));

        let yaml = "services:\n  web:\n    image: nginx\n    secrets: [x]\n";
        assert!(matches!(
            parse(yaml, "p").unwrap_err(),
            ComposeError::Unsupported(_)
        ));

        let yaml = "services:\n  web:\n    image: nginx\n    privileged: true\n";
        assert!(matches!(
            parse(yaml, "p").unwrap_err(),
            ComposeError::Unsupported(_)
        ));
    }

    #[test]
    fn rejects_top_level_unknown_key() {
        let yaml = "services:\n  web:\n    image: nginx\nconfigs:\n  x: {}\n";
        assert!(matches!(
            parse(yaml, "p").unwrap_err(),
            ComposeError::Unsupported(_)
        ));
    }

    #[test]
    fn rejects_cycle_and_missing_dependency() {
        let yaml = r#"
services:
  a: { image: x, depends_on: [b] }
  b: { image: y, depends_on: [a] }
"#;
        assert!(matches!(
            parse(yaml, "p").unwrap_err(),
            ComposeError::Cycle(_)
        ));

        let yaml = r#"
services:
  a: { image: x, depends_on: [nope] }
"#;
        assert!(matches!(
            parse(yaml, "p").unwrap_err(),
            ComposeError::Invalid(_)
        ));
    }

    #[test]
    fn rejects_env_passthrough_and_odd_ports() {
        let yaml = "services:\n  a:\n    image: x\n    environment:\n      - FOO\n";
        assert!(parse(yaml, "p").is_err());

        let yaml = "services:\n  a:\n    image: x\n    ports: [\"80\"]\n";
        assert!(matches!(
            parse(yaml, "p").unwrap_err(),
            ComposeError::Unsupported(_)
        ));
    }

    #[test]
    fn project_name_normalization() {
        assert_eq!(normalize_project_name("My Project"), "my-project");
        assert_eq!(normalize_project_name("___"), "skiff-project");
        assert_eq!(normalize_project_name("a/b"), "ab");
    }
}
