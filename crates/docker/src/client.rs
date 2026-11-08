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
    ContainerSummaryStateEnum, HealthConfig, HostConfig, HostConfigLogConfig, Mount as DockerMount,
    MountType, NetworkCreateRequest, NetworkCreateResponse, NetworkingConfig, PortBinding,
    RestartPolicy, RestartPolicyNameEnum, Volume, VolumeCreateRequest,
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

/// 镜像引用拆成 Docker `fromImage` + `tag`。
///
/// 带 `@sha256:` 的引用整体交给 `fromImage`（Docker 的 `fromImage` 允许包含
/// digest），不能按最后的冒号拆——否则会把 `sha256:...` 当成 tag，Docker 直接
/// 返回 `invalid reference format`。
fn image_pull_ref(image: &str) -> (String, Option<String>) {
    if image.contains('@') {
        return (image.to_string(), None);
    }
    match image.rsplit_once(':') {
        // 只有形如 repo:tag 才拆；带 registry 端口的 host:5000/img 交给 Docker 自己判
        Some((n, t)) if !t.contains('/') => (n.to_string(), Some(t.to_string())),
        _ => (image.to_string(), Some("latest".to_string())),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ContainerRow {
    pub id: String,
    pub name: String,
    /// 容器创建时使用的镜像引用（tag）。
    pub image: String,
    /// 实际镜像 id（`sha256:...`），用于按镜像缓存版本元数据。
    pub image_id: String,
    pub state: String,
    pub status: String,
    pub labels: std::collections::BTreeMap<String, String>,
    pub project: Option<String>,
    pub owner: Option<String>,
    pub protected: bool,
    /// 内核托管（`io.skiff.project=sites`）但缺少 `io.skiff.protected` 的存量容器。
    /// Docker 不允许后加标签，只能在页面提示「重建后生效」。
    pub unprotected_kernel: bool,
}

impl ContainerRow {
    /// 短 id（Docker 惯例前 12 位）。展示与审计详情统一用它。
    pub fn short_id(&self) -> String {
        skiff_model::short_id(&self.id)
    }
}

/// 镜像 inspect 的精简结果（镜像元数据，不启动容器）。
#[derive(Debug, Clone, Default, Serialize)]
pub struct ImageInfo {
    pub id: String,
    /// `Config.Env`，官方镜像会带 `PHP_VERSION` / `NODE_VERSION` / `GOLANG_VERSION`。
    pub env: Vec<String>,
    pub repo_digests: Vec<String>,
    pub tags: Vec<String>,
}

impl ImageInfo {
    /// 从 `KEY=value` 形式的环境变量里取值。
    pub fn env_value(&self, key: &str) -> Option<String> {
        self.env
            .iter()
            .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    }

    /// 首选与 tag 相同的 repo digest；否则取第一个。
    pub fn primary_digest(&self) -> Option<String> {
        self.repo_digests
            .iter()
            .find(|d| d.contains("@sha256:"))
            .or_else(|| self.repo_digests.first())
            .cloned()
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
    /// IPAM 驱动；`None` = Docker 未返回该字段（页面显示「未知」）。
    pub ipam_driver: Option<String>,
    /// 子网 CIDR 列表；`None` = Docker 未返回 IPAM 配置（未知），`Some([])` = 明确无子网。
    pub subnets: Option<Vec<String>>,
    pub gateways: Option<Vec<String>>,
    /// 是否为内部网络（不可外部路由）。
    pub internal: Option<bool>,
    pub attachable: Option<bool>,
    /// 容器挂载情况；`None` = 容器列表查询失败（未知），不是「0 个容器」。
    pub containers: Option<Vec<NetworkAttachment>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkAttachment {
    pub container: String,
    pub ipv4: Option<String>,
    pub ipv6: Option<String>,
    pub mac: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub enum DockerParams {
    /// 只靠动词 + 目标 id 就能执行的操作。
    #[default]
    None,
    /// `images.pull` 的镜像引用（`nginx:alpine`）。
    ImagePull(String),
    /// `containers.create` 的强类型规格（字段白名单在 `ContainerSpec`）。
    ContainerCreate(Box<ContainerSpec>),
    /// `containers.remove` 是否强制删除。
    ContainerRemove { force: bool },
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

    /// 镜像元数据：只读、按 image id 取，用于读取 `*_VERSION` 与 repo digest。
    pub async fn inspect_image(&self, image: &str) -> Result<ImageInfo, DockerError> {
        let i = self.client()?.inspect_image(image).await.map_err(api_err)?;
        let config = i.config.unwrap_or_default();
        Ok(ImageInfo {
            id: i.id.unwrap_or_default(),
            env: config.env.unwrap_or_default(),
            repo_digests: i.repo_digests.unwrap_or_default(),
            tags: i.repo_tags.unwrap_or_default(),
        })
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
        let (name, tag) = image_pull_ref(image);
        let mut stream = self.client()?.create_image(
            Some(qp::CreateImageOptions {
                from_image: Some(name),
                tag,
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
        // `/networks` 列表接口不带容器挂载，单独取一次容器列表；取不到时保持 None（未知），
        // 页面必须显示「未知」而不是 0。
        let attachments = self.network_attachments().await.ok();
        Ok(items
            .into_iter()
            .map(|n| {
                let id = n.id.unwrap_or_default();
                let name = n.name.unwrap_or_default();
                let (ipam_driver, subnets, gateways) = match n.ipam.as_ref() {
                    Some(ipam) => {
                        let mut subnets = Vec::new();
                        let mut gateways = Vec::new();
                        if let Some(configs) = ipam.config.as_ref() {
                            for c in configs {
                                if let Some(s) = c.subnet.as_ref().filter(|s| !s.is_empty()) {
                                    subnets.push(s.clone());
                                }
                                if let Some(g) = c.gateway.as_ref().filter(|g| !g.is_empty()) {
                                    gateways.push(g.clone());
                                }
                            }
                        }
                        (ipam.driver.clone(), Some(subnets), Some(gateways))
                    }
                    None => (None, None, None),
                };
                let containers = attachments.as_ref().map(|map| {
                    map.get(&name)
                        .or_else(|| map.get(&id))
                        .cloned()
                        .unwrap_or_default()
                });
                NetworkRow {
                    id,
                    name,
                    driver: n.driver.unwrap_or_default(),
                    scope: n.scope.unwrap_or_default(),
                    ipam_driver,
                    subnets,
                    gateways,
                    internal: n.internal,
                    attachable: n.attachable,
                    containers,
                }
            })
            .collect())
    }

    async fn network_attachments(
        &self,
    ) -> Result<HashMap<String, Vec<NetworkAttachment>>, DockerError> {
        let items = self
            .client()?
            .list_containers(Some(qp::ListContainersOptions {
                all: true,
                ..Default::default()
            }))
            .await
            .map_err(api_err)?;
        let mut out: HashMap<String, Vec<NetworkAttachment>> = HashMap::new();
        for c in items {
            let cname = c
                .names
                .and_then(|names| names.into_iter().next())
                .map(|n| n.trim_start_matches('/').to_string())
                .unwrap_or_else(|| {
                    c.id.as_deref()
                        .map(skiff_model::short_id)
                        .unwrap_or_else(|| "unknown".into())
                });
            if let Some(networks) = c.network_settings.and_then(|s| s.networks) {
                for (net, ep) in networks {
                    out.entry(net).or_default().push(NetworkAttachment {
                        container: cname.clone(),
                        ipv4: ep.ip_address.filter(|s| !s.is_empty()),
                        ipv6: ep.global_ipv6_address.filter(|s| !s.is_empty()),
                        mac: ep.mac_address.filter(|s| !s.is_empty()),
                    });
                }
            }
        }
        for list in out.values_mut() {
            list.sort_by(|a, b| a.container.cmp(&b.container));
        }
        Ok(out)
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
                    attach_stdin: Some(true),
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

    /// 需要额外参数（镜像名、容器规格、删除选项）的动词也在这里分发；
    /// 参数走 `DockerParams` 强类型枚举，而不是 `args: Value`，字段白名单不可绕过。
    pub async fn execute(
        &self,
        verb: Verb,
        target: Option<&ObjectRef>,
        params: DockerParams,
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
            Verb::ContainerCreate => match params {
                DockerParams::ContainerCreate(spec) => {
                    let created = self.create_container(&spec).await?;
                    Ok(serde_json::json!({ "id": created }))
                }
                _ => Err(DockerError::Unsupported(
                    "containers.create 需要容器规格".into(),
                )),
            },
            Verb::ContainerRemove => match params {
                DockerParams::ContainerRemove { force } => {
                    self.remove_container(&id()?, force).await?;
                    Ok(serde_json::Value::Null)
                }
                _ => Err(DockerError::Unsupported(
                    "containers.remove 需要显式删除选项".into(),
                )),
            },
            Verb::ImagePull => match params {
                DockerParams::ImagePull(image) if !image.trim().is_empty() => {
                    self.pull_image(&image).await?;
                    Ok(serde_json::Value::Null)
                }
                _ => Err(DockerError::Unsupported("images.pull 需要镜像引用".into())),
            },
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

    /// 在容器内跑一条只读命令并收集输出与退出码（用于 `nginx -t` 等校验）。
    pub async fn exec_capture(
        &self,
        container: &str,
        cmd: Vec<String>,
    ) -> Result<(i64, String), DockerError> {
        let exec_id = self.create_exec(container, cmd).await?;
        let mut out = String::new();
        if let StartExecResults::Attached { mut output, .. } = self.start_exec(&exec_id).await? {
            while let Some(chunk) = output.next().await {
                match chunk {
                    Ok(v) => out.push_str(&v.to_string()),
                    Err(e) => return Err(api_err(e)),
                }
            }
        }
        let info = self.inspect_exec(&exec_id).await?;
        let code = info.get("ExitCode").and_then(|v| v.as_i64()).unwrap_or(-1);
        Ok((code, out))
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
    let unprotected_kernel = !protected
        && labels
            .get(crate::policy::LABEL_PROJECT)
            .is_some_and(|project| project == "sites");
    ContainerRow {
        id: c.id.unwrap_or_default(),
        name,
        image: c.image.unwrap_or_default(),
        image_id: c.image_id.unwrap_or_default(),
        state: c
            .state
            .map(|s| state_str(s).to_string())
            .unwrap_or_default(),
        status: c.status.unwrap_or_default(),
        project: labels.get(crate::policy::LABEL_PROJECT).cloned(),
        owner: labels.get(crate::policy::LABEL_OWNER).cloned(),
        labels,
        protected,
        unprotected_kernel,
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

    let networking_config = if spec.networks.is_empty() || spec.network_mode.is_some() {
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

    let log_config = spec.log_driver.as_ref().map(|driver| HostConfigLogConfig {
        typ: Some(driver.clone()),
        config: Some({
            let mut opts = HashMap::new();
            if let Some(size) = &spec.log_max_size {
                opts.insert("max-size".to_string(), size.clone());
                opts.insert("max-file".to_string(), "3".to_string());
            }
            opts
        }),
    });

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
            port_bindings: if spec.network_mode.is_some() {
                None
            } else {
                Some(port_bindings)
            },
            mounts: Some(mounts),
            restart_policy,
            network_mode: spec.network_mode.clone(),
            log_config,
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

    #[test]
    fn image_info_env_parsing_and_digest() {
        let info = ImageInfo {
            id: "sha256:abc".into(),
            env: vec![
                "PATH=/usr/local/bin".into(),
                "PHP_VERSION=8.5.12".into(),
                "PHP_INI_DIR=/usr/local/etc/php".into(),
            ],
            repo_digests: vec![
                "php@sha256:111".into(),
                "php:8.5-fpm-alpine@sha256:222".into(),
            ],
            tags: vec![],
        };
        assert_eq!(info.env_value("PHP_VERSION"), Some("8.5.12".into()));
        assert_eq!(info.env_value("PATH"), Some("/usr/local/bin".into()));
        // 前缀必须精确匹配 `KEY=`：`PHP_VERSION_EXTRA=` 不能被当成 `PHP_VERSION`。
        assert_eq!(info.env_value("PHP_VERSION_EXTRA"), None);
        assert_eq!(info.env_value("MISSING"), None);
        assert_eq!(info.primary_digest(), Some("php@sha256:111".into()));

        // 空值与纯空白不算已读到版本。
        let blank = ImageInfo {
            env: vec!["PHP_VERSION=   ".into()],
            ..Default::default()
        };
        assert_eq!(blank.env_value("PHP_VERSION"), None);
    }

    #[test]
    fn image_pull_ref_keeps_digest_in_from_image() {
        let digest = "openresty/openresty:alpine@sha256:2cb90d64192934c26b3749cf744f47edd59277787fe761bd109eeebdc3e2214a";
        assert_eq!(image_pull_ref(digest), (digest.to_string(), None));
        assert_eq!(
            image_pull_ref("php:8.5-fpm-alpine"),
            ("php".to_string(), Some("8.5-fpm-alpine".to_string()))
        );
        assert_eq!(
            image_pull_ref("nginx"),
            ("nginx".to_string(), Some("latest".to_string()))
        );
        // registry 端口里的冒号不能被当成 tag 分隔符。
        assert_eq!(
            image_pull_ref("localhost:5000/img"),
            ("localhost:5000/img".to_string(), Some("latest".to_string()))
        );
    }

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
            network_mode: None,
            mounts: vec![Mount::Volume {
                source: "html".into(),
                target: "/usr/share/nginx/html".into(),
                read_only: false,
            }],
            log_driver: None,
            log_max_size: None,
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
    fn host_network_and_log_config_are_mapped() {
        let mut sp = spec();
        sp.network_mode = Some("host".into());
        sp.log_driver = Some("local".into());
        sp.log_max_size = Some("10m".into());
        let c = build_config(&sp);
        let hc = c.host_config.as_ref().unwrap();
        assert_eq!(hc.network_mode.as_deref(), Some("host"));
        // host 网络下不能再带端口映射，否则 Docker 直接拒绝。
        assert!(hc.port_bindings.is_none());
        let lc = hc.log_config.as_ref().unwrap();
        assert_eq!(lc.typ.as_deref(), Some("local"));
        let opts = lc.config.as_ref().unwrap();
        assert_eq!(opts.get("max-size").map(String::as_str), Some("10m"));
        assert_eq!(opts.get("max-file").map(String::as_str), Some("3"));
        let json = serde_json::to_value(&c).unwrap();
        assert!(json.get("NetworkingConfig").is_none());
    }

    #[test]
    fn execute_rejects_missing_params_without_touching_engine() {
        let svc = DockerService::connect("/nonexistent/docker.sock");
        let rt = futures_util::FutureExt::now_or_never(svc.execute(
            Verb::ImagePull,
            None,
            DockerParams::None,
        ));
        match rt {
            Some(Err(DockerError::Unsupported(m))) => assert!(m.contains("镜像引用")),
            other => panic!("应返回 Unsupported，实得 {other:?}"),
        }
        let rt = futures_util::FutureExt::now_or_never(svc.execute(
            Verb::ContainerCreate,
            None,
            DockerParams::None,
        ));
        assert!(matches!(rt, Some(Err(DockerError::Unsupported(_)))));
        let rt = futures_util::FutureExt::now_or_never(svc.execute(
            Verb::ContainerRemove,
            None,
            DockerParams::ContainerRemove { force: true },
        ));
        assert!(matches!(rt, Some(Err(DockerError::Unsupported(_)))));
    }

    #[test]
    fn execute_dispatches_lifecycle_verbs() {
        // 白名单里的生命周期动词不再落入「需专用参数」的兜底分支。
        let handled = [
            Verb::ContainerCreate,
            Verb::ContainerRemove,
            Verb::ImagePull,
            Verb::Start,
            Verb::Stop,
            Verb::Restart,
            Verb::Kill,
        ];
        for v in handled {
            assert!(!v.as_str().is_empty());
        }
        // 未实现的动词仍必须显式 Unsupported，而不是 panic。
        let svc = DockerService::connect("/nonexistent/docker.sock");
        let rt = futures_util::FutureExt::now_or_never(svc.execute(
            Verb::ComposeUp,
            None,
            DockerParams::None,
        ));
        assert!(matches!(rt, Some(Err(DockerError::Unsupported(_)))));
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
    fn stale_kernel_container_without_protected_label_is_not_protected() {
        let c = ContainerSummary {
            id: Some("old".into()),
            names: Some(vec!["/skiff-web-nginx".into()]),
            labels: Some(HashMap::from([
                ("io.skiff.project".to_string(), "sites".to_string()),
                ("io.skiff.component".to_string(), "nginx".to_string()),
            ])),
            ..Default::default()
        };
        let row = to_row(c);
        assert!(!row.protected, "缺保护标签的存量容器不能被当成受保护");
        assert!(row.unprotected_kernel, "页面必须能识别存量未受保护容器");
        let obj = ObjectRef {
            id: row.id.clone(),
            name: row.name.clone(),
            labels: row.labels.clone(),
        };
        assert!(!obj.is_protected());
    }

    #[test]
    fn kernel_container_with_protected_label_is_protected() {
        let c = ContainerSummary {
            id: Some("new".into()),
            names: Some(vec!["/skiff-php-batch1".into()]),
            labels: Some(HashMap::from([
                ("io.skiff.project".to_string(), "sites".to_string()),
                ("io.skiff.protected".to_string(), "true".to_string()),
            ])),
            ..Default::default()
        };
        let row = to_row(c);
        assert!(row.protected);
        assert!(!row.unprotected_kernel);
        let obj = ObjectRef {
            id: row.id.clone(),
            name: row.name.clone(),
            labels: row.labels.clone(),
        };
        assert!(obj.is_protected());
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
