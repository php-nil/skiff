//! bollard 封装：只用强类型方法，不提供任意 Engine HTTP 路径转发（§6.4）。
//!
//! 连接失败不阻塞面板启动：Docker 不可用时返回 `DockerError::Unavailable`，
//! Web 层据此渲染「Docker 不可用」而不是 500。
//!
//! bollard 0.21 把查询参数挪到了 `bollard::query_parameters`，请求体是
//! `bollard::models::*`（例如 `ContainerCreateBody`）。这里一律用 Builder 之外
//! 的显式结构体字面量，字段一目了然，也方便对照 §6.4.3 的参数白名单。

use std::collections::HashMap;

use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use bollard::models::{
    ContainerCreateBody, ContainerCreateResponse, ContainerInspectResponse, ContainerSummary,
    ContainerSummaryStateEnum, HealthConfig, HostConfig, Mount as DockerMount, MountType,
    NetworkCreateRequest, NetworkCreateResponse, NetworkingConfig, PortBinding, RestartPolicy,
    RestartPolicyNameEnum, Volume, VolumeCreateRequest,
};
use bollard::query_parameters as qp;
use bollard::Docker;
use futures_util::{Stream, StreamExt};
use serde::Serialize;

use crate::policy::{ContainerSpec, ObjectRef, Verb};

#[derive(Debug, Clone)]
pub enum DockerError {
    /// 未连接上 Engine（socket 不存在 / 权限不足 / 非 Linux 开发机）
    Unavailable(String),
    /// Engine 返回错误
    Api(String),
    /// 参数或形态不支持
    Unsupported(String),
}

impl std::fmt::Display for DockerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DockerError::Unavailable(m) => write!(f, "Docker 不可用: {m}"),
            DockerError::Api(m) => write!(f, "Docker API 错误: {m}"),
            DockerError::Unsupported(m) => write!(f, "不支持的 Docker 操作: {m}"),
        }
    }
}

impl std::error::Error for DockerError {}

impl From<bollard::errors::Error> for DockerError {
    fn from(e: bollard::errors::Error) -> Self {
        DockerError::Api(e.to_string())
    }
}

fn api_err(e: bollard::errors::Error) -> DockerError {
    DockerError::Api(e.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct ContainerRow {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
    pub labels: std::collections::BTreeMap<String, String>,
    pub project: Option<String>,
    pub owner: Option<String>,
    pub protected: bool,
}

impl ContainerRow {
    /// 短 id（Docker 惯例前 12 位）。展示与审计详情统一用它。
    pub fn short_id(&self) -> String {
        skiff_model::short_id(&self.id)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ImageRow {
    pub id: String,
    pub tags: Vec<String>,
    pub size: i64,
}

impl ImageRow {
    /// 短 id（Docker 惯例前 12 位）。
    pub fn short_id(&self) -> String {
        skiff_model::short_id(&self.id)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct VolumeRow {
    pub name: String,
    pub driver: String,
    pub mountpoint: String,
    /// 该卷是否带宿主绑定语义（`o=bind` / `type=none` / `device=<host-path>` 等）
    pub host_binding: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkRow {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub scope: String,
}

pub struct DockerService {
    docker: Option<Docker>,
    reason: String,
}

impl DockerService {
    /// 尝试连接；失败时保留原因，不 panic、不阻塞启动。
    pub fn connect(socket: &str) -> Self {
        match Self::try_connect(socket) {
            Ok(d) => Self {
                docker: Some(d),
                reason: String::new(),
            },
            Err(e) => Self {
                docker: None,
                reason: e.to_string(),
            },
        }
    }

    #[cfg(unix)]
    fn try_connect(socket: &str) -> Result<Docker, DockerError> {
        if !std::path::Path::new(socket).exists() {
            return Err(DockerError::Unavailable(format!("socket 不存在: {socket}")));
        }
        Docker::connect_with_unix(socket, 30, bollard::API_DEFAULT_VERSION)
            .map_err(|e| DockerError::Unavailable(e.to_string()))
    }

    /// Windows/macOS 开发机：MVP 只支持 Linux 主机上的 Unix socket。
    ///
    /// 显式报「不可用」而不是去试 named pipe —— 后者在 Windows 上会「连接成功」，
    /// 让面板误以为能管容器，随后每个请求都失败，比一开始就诚实降级更糟。
    #[cfg(not(unix))]
    fn try_connect(socket: &str) -> Result<Docker, DockerError> {
        Err(DockerError::Unavailable(format!(
            "非 Linux 形态：无法使用 {socket}（MVP 仅支持 Linux 主机上的 docker.sock）"
        )))
    }

    pub fn available(&self) -> bool {
        self.docker.is_some()
    }

    pub fn unavailable_reason(&self) -> &str {
        &self.reason
    }

    fn client(&self) -> Result<&Docker, DockerError> {
        self.docker
            .as_ref()
            .ok_or_else(|| DockerError::Unavailable(self.reason.clone()))
    }

    pub async fn ping(&self) -> Result<(), DockerError> {
        self.client()?.ping().await.map(|_| ()).map_err(api_err)
    }

    pub async fn version(&self) -> Result<serde_json::Value, DockerError> {
        let v = self.client()?.version().await.map_err(api_err)?;
        serde_json::to_value(v).map_err(|e| DockerError::Api(e.to_string()))
    }

    pub async fn info(&self) -> Result<serde_json::Value, DockerError> {
        let i = self.client()?.info().await.map_err(api_err)?;
        serde_json::to_value(i).map_err(|e| DockerError::Api(e.to_string()))
    }

    pub async fn list_containers(&self) -> Result<Vec<ContainerRow>, DockerError> {
        let items = self
            .client()?
            .list_containers(Some(qp::ListContainersOptions {
                all: true,
                ..Default::default()
            }))
            .await
            .map_err(api_err)?;
        Ok(items.into_iter().map(to_row).collect())
    }

    pub async fn inspect_container(
        &self,
        id: &str,
    ) -> Result<ContainerInspectResponse, DockerError> {
        self.client()?
            .inspect_container(id, None::<qp::InspectContainerOptions>)
            .await
            .map_err(api_err)
    }

    pub async fn start_container(&self, id: &str) -> Result<(), DockerError> {
        self.client()?
            .start_container(id, None::<qp::StartContainerOptions>)
            .await
            .map_err(api_err)
    }

    pub async fn stop_container(&self, id: &str) -> Result<(), DockerError> {
        self.client()?
            .stop_container(
                id,
                Some(qp::StopContainerOptions {
                    signal: None,
                    t: Some(10),
                }),
            )
            .await
            .map_err(api_err)
    }

    pub async fn restart_container(&self, id: &str) -> Result<(), DockerError> {
        self.client()?
            .restart_container(
                id,
                Some(qp::RestartContainerOptions {
                    signal: None,
                    t: Some(10),
                }),
            )
            .await
            .map_err(api_err)
    }

    /// 默认信号（SIGKILL）。自定义信号不在 MVP 白名单里，避免「用 SIGUSR 干别的」。
    pub async fn kill_container(&self, id: &str) -> Result<(), DockerError> {
        self.client()?
            .kill_container(id, Some(qp::KillContainerOptions::default()))
            .await
            .map_err(api_err)
    }

    /// 一次性取最近 `tail` 行日志（详情页首屏）。
    pub async fn logs(&self, id: &str, tail: usize) -> Result<String, DockerError> {
        let mut stream = self.client()?.logs(id, Some(logs_options(tail, false)));
        let mut out = String::new();
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(line) => out.push_str(&line.to_string()),
                Err(e) => return Err(api_err(e)),
            }
        }
        Ok(out)
    }

    /// 跟随日志（SSE）。断连由调用方 drop 流，Docker 侧连接随之关闭（§6.2）。
    pub fn stream_logs(
        &self,
        id: String,
        tail: usize,
    ) -> impl Stream<Item = Result<String, DockerError>> + Send + 'static {
        let client = self.docker.clone();
        async_stream::stream! {
            let Some(client) = client else {
                yield Err(DockerError::Unavailable("未连接 Docker".into()));
                return;
            };
            let mut s = client.logs(&id, Some(logs_options(tail, true)));
            while let Some(chunk) = s.next().await {
                match chunk {
                    Ok(line) => yield Ok(line.to_string()),
                    Err(e) => {
                        yield Err(api_err(e));
                        return;
                    }
                }
            }
        }
    }

    pub async fn list_images(&self) -> Result<Vec<ImageRow>, DockerError> {
        let items = self
            .client()?
            .list_images(Some(qp::ListImagesOptions {
                all: true,
                ..Default::default()
            }))
            .await
            .map_err(api_err)?;
        Ok(items
            .into_iter()
            .map(|i| ImageRow {
                id: i.id,
                tags: i.repo_tags,
                size: i.size,
            })
            .collect())
    }

    /// 只允许 `fromImage` 拉取，不走 `/build`（§6.5）。
    pub async fn pull_image(&self, image: &str) -> Result<(), DockerError> {
        let (name, tag) = match image.rsplit_once(':') {
            // 只有形如 repo:tag 才拆；带 registry 端口的 host:5000/img 交给 Docker 自己判
            Some((n, t)) if !t.contains('/') => (n.to_string(), t.to_string()),
            _ => (image.to_string(), "latest".to_string()),
        };
        let mut stream = self.client()?.create_image(
            Some(qp::CreateImageOptions {
                from_image: Some(name),
                tag: Some(tag),
                ..Default::default()
            }),
            None,
            None,
        );
        while let Some(chunk) = stream.next().await {
            if let Err(e) = chunk {
                return Err(api_err(e));
            }
        }
        Ok(())
    }

    pub async fn remove_image(&self, id: &str, force: bool) -> Result<(), DockerError> {
        self.client()?
            .remove_image(
                id,
                Some(qp::RemoveImageOptions {
                    force,
                    noprune: false,
                    platforms: None,
                }),
                None,
            )
            .await
            .map(|_| ())
            .map_err(api_err)
    }

    pub async fn list_volumes(&self) -> Result<Vec<VolumeRow>, DockerError> {
        let resp = self
            .client()?
            .list_volumes(None::<qp::ListVolumesOptions>)
            .await
            .map_err(api_err)?;
        Ok(resp
            .volumes
            .unwrap_or_default()
            .into_iter()
            .map(|v: Volume| {
                let host_binding = crate::policy::volume_has_host_binding(&v.driver, &v.options);
                VolumeRow {
                    name: v.name,
                    driver: v.driver,
                    mountpoint: v.mountpoint,
                    host_binding,
                }
            })
            .collect())
    }

    /// 只建默认 `local` 且 `driver_opts` 为空的卷（§6.4.3）。
    pub async fn create_volume(&self, name: &str) -> Result<(), DockerError> {
        self.client()?
            .create_volume(VolumeCreateRequest {
                name: Some(name.to_string()),
                driver: Some("local".to_string()),
                driver_opts: Some(HashMap::new()),
                labels: Some(HashMap::new()),
                ..Default::default()
            })
            .await
            .map(|_| ())
            .map_err(api_err)
    }

    pub async fn remove_volume(&self, name: &str) -> Result<(), DockerError> {
        self.client()?
            .remove_volume(name, None::<qp::RemoveVolumeOptions>)
            .await
            .map_err(api_err)
    }

    pub async fn list_networks(&self) -> Result<Vec<NetworkRow>, DockerError> {
        let items = self
            .client()?
            .list_networks(None::<qp::ListNetworksOptions>)
            .await
            .map_err(api_err)?;
        Ok(items
            .into_iter()
            .map(|n| NetworkRow {
                id: n.id.unwrap_or_default(),
                name: n.name.unwrap_or_default(),
                driver: n.driver.unwrap_or_default(),
                scope: n.scope.unwrap_or_default(),
            })
            .collect())
    }

    /// MVP 只允许默认 `bridge` 网络（§6.4.3）。
    pub async fn create_network(&self, name: &str) -> Result<NetworkCreateResponse, DockerError> {
        self.client()?
            .create_network(NetworkCreateRequest {
                name: name.to_string(),
                driver: Some("bridge".to_string()),
                ..Default::default()
            })
            .await
            .map_err(api_err)
    }

    pub async fn remove_network(&self, id: &str) -> Result<(), DockerError> {
        self.client()?.remove_network(id).await.map_err(api_err)
    }

    pub async fn create_container(&self, spec: &ContainerSpec) -> Result<String, DockerError> {
        let config = build_config(spec);
        let resp: ContainerCreateResponse = self
            .client()?
            .create_container(
                Some(qp::CreateContainerOptions {
                    name: spec.name.clone().filter(|n| !n.is_empty()),
                    platform: String::new(),
                }),
                config,
            )
            .await
            .map_err(api_err)?;
        Ok(resp.id)
    }

    pub async fn remove_container(&self, id: &str, force: bool) -> Result<(), DockerError> {
        self.client()?
            .remove_container(
                id,
                Some(qp::RemoveContainerOptions {
                    v: false,
                    force,
                    link: false,
                }),
            )
            .await
            .map_err(api_err)
    }

    pub async fn create_exec(
        &self,
        container: &str,
        cmd: Vec<String>,
    ) -> Result<String, DockerError> {
        let exec = self
            .client()?
            .create_exec(
                container,
                CreateExecOptions::<String> {
                    cmd: Some(cmd),
                    attach_stdout: Some(true),
                    attach_stderr: Some(true),
                    tty: Some(true),
                    ..Default::default()
                },
            )
            .await
            .map_err(api_err)?;
        Ok(exec.id)
    }

    pub async fn start_exec(&self, exec_id: &str) -> Result<StartExecResults, DockerError> {
        self.client()?
            .start_exec(
                exec_id,
                Some(StartExecOptions {
                    detach: false,
                    tty: true,
                    output_capacity: None,
                }),
            )
            .await
            .map_err(api_err)
    }

    /// 按白名单动词**单点分发**，返回已序列化的结果。
    ///
    /// 调用方（UI、JSON API、插件回调）只做三件事：策略判定、调这里、写审计。
    /// 之前 `Verb` → Docker 方法的匹配在 web 的两处和 kernel 一处各写了一遍，
    /// 第三处还有 `unreachable!()`——新增动词漏改就是 panic。现在只有这一份。
    ///
    /// 需要额外参数（镜像名、Compose 定义、exec 命令）的动词不在此处理，
    /// 返回 `Unsupported`，由调用方走各自的类型化接口；这是「不猜参数」的取舍，
    /// 免得通道里塞进一个 `args: Value` 又把参数白名单绕过去。
    pub async fn execute(
        &self,
        verb: Verb,
        target: Option<&ObjectRef>,
    ) -> Result<serde_json::Value, DockerError> {
        let need_target = || DockerError::Unsupported(format!("{} 需要指定对象", verb.as_str()));
        let id = || -> Result<String, DockerError> {
            let t = target.ok_or_else(need_target)?;
            if t.id.is_empty() {
                return Err(DockerError::Unsupported(format!(
                    "{} 需要对象 id（不能只用名字）",
                    verb.as_str()
                )));
            }
            Ok(t.id.clone())
        };
        let as_value = |v: serde_json::Value| Ok(v);

        match verb {
            Verb::ContainersList => {
                let rows = self.list_containers().await?;
                Ok(serde_json::to_value(rows).unwrap_or(serde_json::Value::Null))
            }
            Verb::ContainerInspect => {
                let v = self.inspect_container(&id()?).await?;
                Ok(serde_json::to_value(v).unwrap_or(serde_json::Value::Null))
            }
            Verb::ContainerLogs => as_value(serde_json::json!(self.logs(&id()?, 200).await?)),
            Verb::ImagesList => {
                let rows = self.list_images().await?;
                Ok(serde_json::to_value(rows).unwrap_or(serde_json::Value::Null))
            }
            Verb::NetworksList => {
                let rows = self.list_networks().await?;
                Ok(serde_json::to_value(rows).unwrap_or(serde_json::Value::Null))
            }
            Verb::VolumesList => {
                let rows = self.list_volumes().await?;
                Ok(serde_json::to_value(rows).unwrap_or(serde_json::Value::Null))
            }
            Verb::Info => self.info().await,
            Verb::Version => self.version().await,
            Verb::Start => {
                self.start_container(&id()?).await?;
                Ok(serde_json::Value::Null)
            }
            Verb::Stop => {
                self.stop_container(&id()?).await?;
                Ok(serde_json::Value::Null)
            }
            Verb::Restart => {
                self.restart_container(&id()?).await?;
                Ok(serde_json::Value::Null)
            }
            Verb::Kill => {
                self.kill_container(&id()?).await?;
                Ok(serde_json::Value::Null)
            }
            other => Err(DockerError::Unsupported(format!(
                "{} 需要专用参数，走类型化接口（不在通用分发内）",
                other.as_str()
            ))),
        }
    }

    pub async fn inspect_exec(&self, exec_id: &str) -> Result<serde_json::Value, DockerError> {
        let r = self
            .client()?
            .inspect_exec(exec_id)
            .await
            .map_err(api_err)?;
        serde_json::to_value(r).map_err(|e| DockerError::Api(e.to_string()))
    }
}

fn logs_options(tail: usize, follow: bool) -> qp::LogsOptions {
    qp::LogsOptions {
        follow,
        stdout: true,
        stderr: true,
        since: 0,
        until: 0,
        timestamps: true,
        tail: tail.to_string(),
    }
}

fn to_row(c: ContainerSummary) -> ContainerRow {
    let labels: std::collections::BTreeMap<String, String> =
        c.labels.unwrap_or_default().into_iter().collect();
    let name = c
        .names
        .unwrap_or_default()
        .first()
        .map(|n| n.trim_start_matches('/').to_string())
        .unwrap_or_default();
    let protected = labels
        .get(crate::policy::LABEL_PROTECTED)
        .map(|v| v == "true")
        .unwrap_or(false)
        || name == "skiff";
    ContainerRow {
        id: c.id.unwrap_or_default(),
        name,
        image: c.image.unwrap_or_default(),
        state: c
            .state
            .map(|s| state_str(s).to_string())
            .unwrap_or_default(),
        status: c.status.unwrap_or_default(),
        project: labels.get(crate::policy::LABEL_PROJECT).cloned(),
        owner: labels.get(crate::policy::LABEL_OWNER).cloned(),
        labels,
        protected,
    }
}

fn state_str(s: ContainerSummaryStateEnum) -> &'static str {
    match s {
        ContainerSummaryStateEnum::CREATED => "created",
        ContainerSummaryStateEnum::RUNNING => "running",
        ContainerSummaryStateEnum::PAUSED => "paused",
        ContainerSummaryStateEnum::RESTARTING => "restarting",
        ContainerSummaryStateEnum::EXITED => "exited",
        ContainerSummaryStateEnum::REMOVING => "removing",
        ContainerSummaryStateEnum::DEAD => "dead",
        _ => "",
    }
}

fn build_config(spec: &ContainerSpec) -> ContainerCreateBody {
    let mut port_bindings: bollard::models::PortMap = HashMap::new();
    for p in &spec.ports {
        let key = format!(
            "{}/{}",
            p.container,
            p.protocol.clone().unwrap_or_else(|| "tcp".into())
        );
        port_bindings
            .entry(key)
            .or_insert_with(|| Some(Vec::new()))
            .get_or_insert_with(Vec::new)
            .push(PortBinding {
                host_ip: None,
                host_port: Some(p.host.to_string()),
            });
    }

    let mounts: Vec<DockerMount> = spec
        .mounts
        .iter()
        .map(|m| match m {
            crate::policy::Mount::Volume {
                source,
                target,
                read_only,
            } => DockerMount {
                target: Some(target.clone()),
                source: Some(source.clone()),
                typ: Some(MountType::VOLUME),
                read_only: Some(*read_only),
                ..Default::default()
            },
            crate::policy::Mount::Bind {
                source,
                target,
                read_only,
            } => DockerMount {
                target: Some(target.clone()),
                source: Some(source.clone()),
                typ: Some(MountType::BIND),
                read_only: Some(*read_only),
                ..Default::default()
            },
        })
        .collect();

    let restart_policy = spec.restart.as_ref().map(|r| RestartPolicy {
        name: Some(match r.as_str() {
            "always" => RestartPolicyNameEnum::ALWAYS,
            "unless-stopped" => RestartPolicyNameEnum::UNLESS_STOPPED,
            "no" => RestartPolicyNameEnum::NO,
            _ => RestartPolicyNameEnum::ON_FAILURE,
        }),
        maximum_retry_count: r
            .strip_prefix("on-failure:")
            .and_then(|n| n.parse::<i64>().ok()),
    });

    let healthcheck = spec.healthcheck.as_ref().map(|h| HealthConfig {
        test: Some(h.test.clone()),
        interval: h.interval_secs.map(|s| s * 1_000_000_000),
        timeout: h.timeout_secs.map(|s| s * 1_000_000_000),
        retries: h.retries,
        start_period: None,
        start_interval: None,
    });

    let networking_config = if spec.networks.is_empty() {
        None
    } else {
        let mut endpoints = HashMap::new();
        for net in &spec.networks {
            endpoints.insert(net.clone(), bollard::models::EndpointSettings::default());
        }
        Some(NetworkingConfig {
            endpoints_config: Some(endpoints),
        })
    };

    ContainerCreateBody {
        image: Some(spec.image.clone()),
        cmd: spec.command.clone(),
        entrypoint: spec.entrypoint.clone(),
        env: Some(
            spec.env
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>(),
        ),
        labels: Some(
            spec.labels
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ),
        host_config: Some(HostConfig {
            port_bindings: Some(port_bindings),
            mounts: Some(mounts),
            restart_policy,
            memory: spec.memory_limit_mb.map(|m| m * 1024 * 1024),
            nano_cpus: spec.nano_cpus,
            pids_limit: spec.pids_limit,
            ..Default::default()
        }),
        healthcheck,
        networking_config,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{ContainerSpec, Mount, PortMapping};
    use std::collections::BTreeMap;

    fn spec() -> ContainerSpec {
        ContainerSpec {
            image: "nginx:1.27-alpine".into(),
            name: Some("web".into()),
            command: None,
            entrypoint: None,
            env: [("TZ".to_string(), "Asia/Shanghai".to_string())]
                .into_iter()
                .collect(),
            labels: [("app".to_string(), "web".to_string())]
                .into_iter()
                .collect(),
            ports: vec![PortMapping {
                host: 18081,
                container: 80,
                protocol: None,
            }],
            restart: Some("unless-stopped".into()),
            healthcheck: None,
            networks: vec![],
            mounts: vec![Mount::Volume {
                source: "html".into(),
                target: "/usr/share/nginx/html".into(),
                read_only: false,
            }],
            memory_limit_mb: Some(128),
            nano_cpus: None,
            pids_limit: Some(64),
        }
    }

    #[test]
    fn config_maps_only_whitelisted_fields() {
        let c = build_config(&spec());
        assert_eq!(c.image.as_deref(), Some("nginx:1.27-alpine"));
        assert_eq!(c.env.as_ref().unwrap()[0], "TZ=Asia/Shanghai");
        let hc = c.host_config.clone().unwrap();
        let pb = hc.port_bindings.unwrap();
        assert!(pb.contains_key("80/tcp"));
        assert_eq!(hc.memory, Some(128 * 1024 * 1024));
        assert_eq!(hc.pids_limit, Some(64));
        // 危险字段在类型层面不存在：privileged / cap_add 等无法被设置
        let json = serde_json::to_value(&c).unwrap();
        assert!(json.get("Privileged").is_none());
        assert!(json.get("privileged").is_none());
        assert!(json.get("CapAdd").is_none());
        assert!(json.get("cap_add").is_none());
    }

    #[test]
    fn unavailable_service_degrades_without_panic() {
        let svc = DockerService::connect("/nonexistent/docker.sock");
        assert!(!svc.available());
        assert!(!svc.unavailable_reason().is_empty());
    }

    #[test]
    fn label_row_marks_protected() {
        let c = ContainerSummary {
            id: Some("abc".into()),
            names: Some(vec!["/skiff".into()]),
            image: Some("skiff:0.1.0".into()),
            state: Some(ContainerSummaryStateEnum::RUNNING),
            status: Some("Up".into()),
            labels: Some(HashMap::new()),
            ..Default::default()
        };
        let row = to_row(c);
        assert!(row.protected);
        assert_eq!(row.name, "skiff");
        assert_eq!(row.state, "running");
    }

    #[test]
    fn env_map_is_sorted_for_reproducible_requests() {
        let mut s = spec();
        s.env = BTreeMap::from([
            ("B".to_string(), "2".to_string()),
            ("A".to_string(), "1".to_string()),
        ]);
        let c = build_config(&s);
        assert_eq!(c.env.unwrap(), vec!["A=1".to_string(), "B=2".to_string()]);
    }
}
