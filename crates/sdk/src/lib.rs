//! Skiff 插件 SDK（第三方侧，核心③）：读取运行环境、提供 `/healthz` + `/rpc`、
//! 回调内核能力面。窄接口，只承诺 manifest `api` 主版本内的兼容（§4.4）。
//!
//! 插件作者只需要：
//! ```ignore
//! fn main() {
//!     let plugin = skiff_sdk::Plugin::from_env().expect("环境变量缺失");
//!     plugin.serve(|method, params| match method {
//!         "tick" => Ok(serde_json::json!({ "ok": true })),
//!         "status" => Ok(serde_json::json!({ "granted": plugin.granted() })),
//!         _ => Err(skiff_sdk::RpcError::method_not_found(method)),
//!     }).expect("服务启动失败");
//! }
//! ```

use std::collections::BTreeMap;
// 下面 `uds_http` / `read_request` / `write_response` 只在 unix 编译，
// 它们是标准库同步 I/O 的 trait 方法——漏了这行在 Windows 上完全看不出来。
#[cfg(unix)]
use std::io::{Read, Write};

use std::path::PathBuf;

use serde_json::{json, Value};

#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    #[error("缺少环境变量 {0}（必须由 Supervisor 拉起，不能手工启动）")]
    MissingEnv(&'static str),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("超时: {0}")]
    Timeout(String),
    #[error("协议错误: {0}")]
    Protocol(String),
    #[error("平台不支持: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, SdkError>;

/// 宿主路径单次读写上限。插件↔内核走 `skiff_wire::MAX_FRAME`（1MiB）单帧，
/// 这里取 256KiB：base64 膨胀 4/3 后仍在帧内，且留足 JSON 头部余量。
pub const HOST_IO_MAX: usize = 256 * 1024;

/// `fs_host_read_bytes` / `fs_host_write_bytes` 的整文件上限（常规用例够用，
/// 再大的文件请自己按页读写，别让插件把整文件塞进内存）。
pub const HOST_WHOLE_MAX: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    pub fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("method not found: {method}"),
        }
    }
    pub fn invalid_params(msg: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: msg.into(),
        }
    }
    pub fn internal(msg: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: msg.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Plugin {
    pub id: String,
    pub api: String,
    pub socket: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub run_dir: PathBuf,
    pub kernel_socket: PathBuf,
    pub granted: Vec<String>,
    /// 一次性身份令牌（经 stdin 继承交付，不落环境变量）
    pub token: String,
}

impl Plugin {
    /// 从 Supervisor 注入的环境变量构造。token 从 stdin 读一行。
    pub fn from_env() -> Result<Plugin> {
        let env = |k: &'static str| -> Result<String> {
            std::env::var(k).map_err(|_| SdkError::MissingEnv(k))
        };
        let id = env("SKIFF_PLUGIN_ID")?;
        let socket = PathBuf::from(env("SKIFF_PLUGIN_SOCKET")?);
        let data_dir = PathBuf::from(env("SKIFF_PLUGIN_DATA_DIR")?);
        let cache_dir = PathBuf::from(env("SKIFF_PLUGIN_CACHE_DIR")?);
        let run_dir = PathBuf::from(env("SKIFF_PLUGIN_RUN_DIR")?);
        let kernel_socket = PathBuf::from(env("SKIFF_KERNEL_SOCKET")?);
        let api = std::env::var("SKIFF_PLUGIN_API").unwrap_or_else(|_| "1.0".into());
        let granted = std::env::var("SKIFF_GRANTED_PERMS")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let mut token = String::new();
        if std::io::stdin().read_line(&mut token).is_ok() {
            token = token.trim().to_string();
        }

        Ok(Plugin {
            id,
            api,
            socket,
            data_dir,
            cache_dir,
            run_dir,
            kernel_socket,
            granted,
            token,
        })
    }

    pub fn has_perm(&self, perm: &str) -> bool {
        self.granted.iter().any(|g| g == perm)
    }

    pub fn granted(&self) -> &[String] {
        &self.granted
    }

    /// 自身身份与已授予权限（内核 `capability/identity/self`）。
    pub fn identity(&self) -> Result<Value> {
        self.call_kernel(skiff_wire::routes::CAP_IDENTITY_SELF, json!({}))
    }

    /// 读取内核代管的配置项（`config_schema`），`secret` 项同样经此返回（不落插件目录）。
    pub fn config(&self) -> Result<BTreeMap<String, String>> {
        let v = self.call_kernel(skiff_wire::routes::CAP_CONFIG, json!({}))?;
        let mut out = BTreeMap::new();
        if let Some(obj) = v.as_object() {
            for (k, val) in obj {
                if let Some(s) = val.as_str() {
                    out.insert(k.clone(), s.to_string());
                }
            }
        }
        Ok(out)
    }

    /// 读自身 `data/` 下的文件（相对路径）。
    pub fn fs_read(&self, path: &str) -> Result<Value> {
        self.call_kernel(skiff_wire::routes::CAP_FS_READ, json!({ "path": path }))
    }

    /// 写自身 `data/` 下的文件（相对路径）。
    pub fn fs_write(&self, path: &str, data: &str) -> Result<Value> {
        self.call_kernel(
            skiff_wire::routes::CAP_FS_WRITE,
            json!({ "path": path, "data": data }),
        )
    }

    /// 列自身 `data/` 目录（相对路径）。
    pub fn fs_list(&self, path: &str) -> Result<Value> {
        self.call_kernel(skiff_wire::routes::CAP_FS_LIST, json!({ "path": path }))
    }

    /// 宿主路径读取的一页（base64），`path` 必须是安装时确认过的 `path:` 根内绝对路径。
    ///
    /// 返回 `(b64, offset, size, eof)`。单页上限 [`HOST_IO_MAX`]——插件↔内核是单帧
    /// 1MiB 的 UDS 协议，大文件必须分页，SDK 不替调用方猜「多大算大」。
    pub fn fs_host_read_b64(
        &self,
        path: &str,
        offset: u64,
        max_bytes: usize,
    ) -> Result<(String, u64, u64, bool)> {
        let v = self.call_kernel(
            skiff_wire::routes::CAP_FS_READ,
            json!({
                "path": path,
                "offset": offset,
                "max_bytes": max_bytes.min(HOST_IO_MAX),
                "encoding": "base64",
            }),
        )?;
        let b64 = v
            .get("b64")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let off = v.get("offset").and_then(|x| x.as_u64()).unwrap_or(0);
        let size = v.get("size").and_then(|x| x.as_u64()).unwrap_or(0);
        let eof = v.get("eof").and_then(|x| x.as_bool()).unwrap_or(true);
        Ok((b64, off, size, eof))
    }

    /// 宿主路径写一页（base64）。`offset = None` 表示从 0 覆盖写；`append = true`
    /// 表示追加（分块上传的最后一块之外都用追加）。
    pub fn fs_host_write_b64(
        &self,
        path: &str,
        b64: &str,
        offset: Option<u64>,
        append: bool,
    ) -> Result<Value> {
        let mut body = json!({ "path": path, "b64": b64 });
        if let Some(o) = offset {
            body["offset"] = json!(o);
        }
        if append {
            body["append"] = json!(true);
        }
        self.call_kernel(skiff_wire::routes::CAP_FS_WRITE, body)
    }

    /// 宿主路径读文本（整文件，超过 [`HOST_IO_MAX`] 时报错而不是静默截断）。
    pub fn fs_host_read_text(&self, path: &str) -> Result<String> {
        let v = self.call_kernel(
            skiff_wire::routes::CAP_FS_READ,
            json!({ "path": path, "max_bytes": HOST_IO_MAX }),
        )?;
        if !v.get("eof").and_then(|x| x.as_bool()).unwrap_or(true) {
            return Err(SdkError::Protocol(format!(
                "文件超过 {} 字节，请改用 fs_host_read_b64 分页读取",
                HOST_IO_MAX
            )));
        }
        Ok(v.get("data")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string())
    }

    /// 宿主路径写文本（整文件覆盖）。
    pub fn fs_host_write_text(&self, path: &str, text: &str) -> Result<Value> {
        self.call_kernel(
            skiff_wire::routes::CAP_FS_WRITE,
            json!({ "path": path, "data": text }),
        )
    }

    /// 列宿主目录（`items` 含 name/dir/size/mtime/mode）。
    pub fn fs_host_list(&self, path: &str) -> Result<Value> {
        self.call_kernel(skiff_wire::routes::CAP_FS_LIST, json!({ "path": path }))
    }

    pub fn fs_host_mkdir(&self, path: &str) -> Result<Value> {
        self.call_kernel(skiff_wire::routes::CAP_FS_MKDIR, json!({ "path": path }))
    }

    pub fn fs_host_remove(&self, path: &str) -> Result<Value> {
        self.call_kernel(skiff_wire::routes::CAP_FS_REMOVE, json!({ "path": path }))
    }

    pub fn fs_host_rename(&self, from: &str, to: &str) -> Result<Value> {
        self.call_kernel(
            skiff_wire::routes::CAP_FS_RENAME,
            json!({ "path": from, "to": to }),
        )
    }

    /// 已授予的对象（`container:<id|name>` / `project:<p>` / `path:<abs>`）。
    ///
    /// 来自内核 `identity/self`，不是本地猜测——对象授权只在内核库里，env 里只有权限串。
    pub fn objects(&self) -> Result<Vec<String>> {
        let v = self.identity()?;
        Ok(v.get("objects")
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// 已授予的 `path:` 授权根（原始字符串，未 canonicalize）。
    pub fn path_roots(&self) -> Result<Vec<String>> {
        Ok(self
            .objects()?
            .into_iter()
            .filter_map(|o| o.strip_prefix("path:").map(str::to_string))
            .collect())
    }

    /// 已授予的 `bin:` 可执行体（原始字符串，未 canonicalize）。
    ///
    /// 插件前置快筛用：真正能不能跑由内核 `execgrant` canonicalize 后判定。
    pub fn bin_grants(&self) -> Result<Vec<String>> {
        Ok(self
            .objects()?
            .into_iter()
            .filter_map(|o| o.strip_prefix("bin:").map(str::to_string))
            .collect())
    }

    /// 整文件读字节（内部按 [`HOST_IO_MAX`] 分页，超过 [`HOST_WHOLE_MAX`] 报错）。
    pub fn fs_host_read_bytes(&self, path: &str) -> Result<Vec<u8>> {
        use base64::Engine as _;
        let engine = base64::engine::general_purpose::STANDARD;
        let mut out = Vec::new();
        let mut offset = 0u64;
        loop {
            let (b64, _, _, eof) = self.fs_host_read_b64(path, offset, HOST_IO_MAX)?;
            let page = engine
                .decode(b64.as_bytes())
                .map_err(|e| SdkError::Protocol(format!("base64 解码失败: {e}")))?;
            offset += page.len() as u64;
            out.extend_from_slice(&page);
            if out.len() > HOST_WHOLE_MAX {
                return Err(SdkError::Protocol(format!(
                    "文件超过 {HOST_WHOLE_MAX} 字节，请改用分页接口"
                )));
            }
            if eof {
                return Ok(out);
            }
        }
    }

    /// 整文件写字节（内部按 [`HOST_IO_MAX`] 分页，第一页覆盖写、其余追加）。
    pub fn fs_host_write_bytes(&self, path: &str, data: &[u8]) -> Result<Value> {
        use base64::Engine as _;
        let engine = base64::engine::general_purpose::STANDARD;
        if data.len() > HOST_WHOLE_MAX {
            return Err(SdkError::Protocol(format!(
                "写入超过 {HOST_WHOLE_MAX} 字节，请改用分页接口"
            )));
        }
        // 空文件也必须显式写一次，否则「新建空文件」会变成「什么都不做」。
        if data.is_empty() {
            return self.fs_host_write_b64(path, "", None, false);
        }
        let mut last = Value::Null;
        for (i, chunk) in data.chunks(HOST_IO_MAX).enumerate() {
            let b64 = engine.encode(chunk);
            last = self.fs_host_write_b64(path, &b64, None, i > 0)?;
        }
        Ok(last)
    }

    /// Docker 白名单动词（三层策略在内核侧判定，SDK 不做本地放行）。
    pub fn docker_call(&self, verb: &str, target: Value) -> Result<Value> {
        self.call_kernel(
            skiff_wire::routes::CAP_DOCKER_CALL,
            json!({ "verb": verb, "target": target }),
        )
    }

    /// 出网收口申请。**内核当前一律 501**（nftables 收口属 M4 门禁）。
    pub fn net_apply(&self, spec: Value) -> Result<Value> {
        self.call_kernel(skiff_wire::routes::CAP_NET_APPLY, spec)
    }

    /// 调用另一个插件声明过的接口（manifest `[interop].consumes` + `interop:call` + `plugin:` 授权）。
    ///
    /// 内核在 `params` 里注入 `_interop.caller`（调用方插件 id），目标插件据此做来源
    /// 展示/幂等键；调用方传的同名字段会被内核覆盖。
    ///
    /// 返回目标插件 RPC 的 `result`（不是 `{"result": …}` 信封）。只有**单层**转发：
    /// 目标插件在自己的 handler 里再次发起 interop 会被内核以「环」拒绝。
    pub fn interop_call(&self, plugin: &str, method: &str, params: Value) -> Result<Value> {
        let v = self.call_kernel(
            skiff_wire::routes::CAP_INTEROP_CALL,
            json!({ "plugin": plugin, "method": method, "params": params }),
        )?;
        Ok(v.get("result").cloned().unwrap_or(v))
    }

    /// 列出已启用插件对外暴露的接口（`{plugins:[{id,name,methods:[…]}]}`）。
    ///
    /// 需要 `interop:call` 权限；不返回目标插件的授权/配置等敏感信息。
    pub fn interop_catalog(&self) -> Result<Value> {
        self.call_kernel(skiff_wire::routes::CAP_INTEROP_CATALOG, json!({}))
    }

    /// 内核调度能力面（manifest 需声明 `scheduler` 能力与对应 verbs）。
    ///
    /// 常用方法：`task.list` / `task.get` / `task.history` / `task.register` /
    /// `task.update` / `task.delete` / `task.run`。返回内核原样 JSON（任务视图已带
    /// `next_run`/`state` 等展示字段，插件 UI 不需要自己算表达式）。
    pub fn scheduler_call(&self, method: &str, params: Value) -> Result<Value> {
        self.call_kernel(
            skiff_wire::routes::CAP_SCHEDULER_CALL,
            json!({ "method": method, "params": params }),
        )
    }

    /// 宿主机 FTP 能力面（manifest 需声明 `ftp` 能力与对应 verbs）。
    ///
    /// 方法：`status` / `install` / `uninstall` / `sites.list` / `accounts.list` /
    /// `account.create` / `account.password` / `account.delete` / `service.restart`。
    /// 返回内核原样 JSON；仅 Native 形态可用，受限形态安装期就不会授予 `ftp`。
    pub fn ftp_call(&self, method: &str, params: Value) -> Result<Value> {
        self.call_kernel(
            skiff_wire::routes::CAP_FTP_CALL,
            json!({ "method": method, "params": params }),
        )
    }

    /// 宿主机防火墙/fail2ban 能力面（manifest 需声明 `firewall` 能力与对应 verbs）。
    ///
    /// 方法：`status` / `rules.list` / `rule.add` / `rule.remove` / `port.allow` /
    /// `fail2ban.status` / `fail2ban.install` / `fail2ban.start` / `fail2ban.stop` /
    /// `fail2ban.uninstall` / `fail2ban.jail.ensure` /
    /// `fail2ban.jail.remove` / `fail2ban.banned` / `fail2ban.ban` / `fail2ban.unban`。
    /// 仅 Native 形态可用；受限形态安装期就不会授予 `firewall`。
    pub fn firewall_call(&self, method: &str, params: Value) -> Result<Value> {
        self.call_kernel(
            skiff_wire::routes::CAP_FIREWALL_CALL,
            json!({ "method": method, "params": params }),
        )
    }

    /// 宿主机命令执行（manifest 需 `requires` 含 `exec`、`permissions` 里写
    /// `exec:run`、`resource_hints` 里声明对应的 `bin:<绝对路径>`）。
    ///
    /// 跑了未授权的二进制会被内核 403，所以插件应当先用 [`Self::bin_grants`]
    /// 判断本机有没有这条授权，再决定 UI 显不显示这个功能。
    /// `argv` 按数组原样传给宿主，**不经 shell**；密码/私钥用
    /// [`Self::host_exec_with_stdin`]，不要放进 argv——argv 全文会进审计。
    pub fn host_exec(&self, binary: &str, argv: &[&str]) -> Result<Value> {
        self.host_exec_full(binary, argv, None, None)
    }

    /// 带 stdin 的执行：`stdin` 用于投递敏感内容（只记字节数，不进审计、不落库）。
    pub fn host_exec_with_stdin(
        &self,
        binary: &str,
        argv: &[&str],
        stdin: &str,
        timeout_sec: u64,
    ) -> Result<Value> {
        self.host_exec_full(binary, argv, Some(stdin), Some(timeout_sec))
    }

    /// 执行并返回 `{success, code, stdout, stderr, truncated, job_id}`。
    ///
    /// 非零退出**不是**错误：`success = false` 但 `stdout`/`stderr` 有内容，
    /// 编排（装完配、配完重启）要靠 stderr 判断下一步。只有被拒（403）、
    /// 同插件已有命令在跑（409）与通道/超时失败才返回 `Err`。
    ///
    /// 内核侧是**提交作业**语义（长任务要能看过程、能取消），这里封装成
    /// 「提交 + 等到结束」：给需要「一步拿到结果」的编排用。要边跑边看进度，
    /// 用 [`Self::host_exec_submit`] 拿 `job_id` 后轮询 [`Self::host_exec_status`]。
    pub fn host_exec_full(
        &self,
        binary: &str,
        argv: &[&str],
        stdin: Option<&str>,
        timeout_sec: Option<u64>,
    ) -> Result<Value> {
        // 纯异步提交（`wait_sec = 0`）：把等待交给下面的长轮询。
        // 旧写法让 `exec/run` 这一次请求最多挂 120s，既占住插件线程，也逼近
        // 内核 RPC 上限；长作业本就应该「提交 + 长轮询」。
        let timeout = timeout_sec.unwrap_or(600);
        let mut submitted = self.host_exec_submit(binary, argv, stdin, Some((timeout, 0)))?;
        let job_id = submitted
            .get("job_id")
            .and_then(Value::as_str)
            .ok_or_else(|| SdkError::Protocol("exec 响应缺少 job_id".into()))?
            .to_string();

        let budget = std::time::Duration::from_secs(timeout + 15);
        let started = std::time::Instant::now();
        // 长轮询：每次最多让内核挂起 5s，状态变化立即返回，比固定 200ms 轮询少两个数量级的请求。
        const POLL_WAIT_MS: u64 = 5_000;
        while submitted.get("active").and_then(Value::as_bool) == Some(true) {
            let elapsed = started.elapsed();
            if elapsed > budget {
                return Err(SdkError::Protocol(format!(
                    "命令执行超过 {budget:?} 未结束，作业 {job_id} 可能仍在运行（可用 host_exec_status 复查或 host_exec_cancel 取消）"
                )));
            }
            let remaining_ms = (budget - elapsed).as_millis() as u64;
            submitted =
                self.host_exec_status_wait(&job_id, 0, 0, remaining_ms.min(POLL_WAIT_MS))?;
        }
        Ok(finalize_exec_result(submitted, &job_id))
    }

    /// 只提交、不等结果：返回 `{job_id, status, active}` 等提交快照。
    ///
    /// `timeout_sec` 与 `wait_sec` 分开给：`timeout_sec` 是命令本身的运行上限，
    /// `wait_sec` 是「这次请求愿意等多久」（内核上限 120s，0 = 纯异步）。
    pub fn host_exec_submit(
        &self,
        binary: &str,
        argv: &[&str],
        stdin: Option<&str>,
        limits: Option<(u64, u64)>,
    ) -> Result<Value> {
        let mut body = json!({ "binary": binary, "argv": argv });
        if let Some(s) = stdin {
            body["stdin"] = json!(s);
        }
        if let Some((timeout, wait)) = limits {
            body["timeout_sec"] = json!(timeout);
            body["wait_sec"] = json!(wait);
        }
        self.call_kernel(skiff_wire::routes::CAP_EXEC_RUN, body)
    }

    /// 查询状态 + 结果 + 增量日志（`after` 为已读到的最大 `seq`，`limit=0` 不带日志）。
    pub fn host_exec_status(&self, job_id: &str, after: i64, limit: i64) -> Result<Value> {
        self.host_exec_status_wait(job_id, after, limit, 0)
    }

    /// 长轮询版本：`wait_ms > 0` 时内核会挂起到「作业进入终态」或超时（内核上限 30s）
    /// 再返回，响应形状不变。轮询进度时用它替代固定间隔的短轮询。
    pub fn host_exec_status_wait(
        &self,
        job_id: &str,
        after: i64,
        limit: i64,
        wait_ms: u64,
    ) -> Result<Value> {
        let mut body = json!({ "job_id": job_id, "after": after, "limit": limit });
        if wait_ms > 0 {
            body["wait_ms"] = json!(wait_ms);
        }
        self.call_kernel(skiff_wire::routes::CAP_EXEC_STATUS, body)
    }

    /// 取消：返回 `{outcome: cancelled|requested|terminal}`。
    pub fn host_exec_cancel(&self, job_id: &str) -> Result<Value> {
        self.call_kernel(
            skiff_wire::routes::CAP_EXEC_CANCEL,
            json!({ "job_id": job_id }),
        )
    }

    /// 轻量执行：短命令 / 只读探测，一次 HTTP 返回 `{success, code, stdout, stderr,
    /// truncated}`。
    ///
    /// 授权判定与 [`Self::host_exec`] 完全一致（`exec:run` + `bin:` 三道闸），
    /// 差别只在**不提交作业、不写任务中心、不抢同插件唯一活跃槽位**，因此适合
    /// 并发探测（`nft --version` / `dpkg-query` / `systemctl is-active`）。
    /// `timeout_sec` 缺省 10s、内核上限 30s（超出夹取）——跑更久的命令应该走
    /// [`Self::host_exec`]，那样才有过程日志与取消。
    ///
    /// 非零退出**不是** `Err`：`success = false` 且 `stderr` 有内容；被拒（403）、
    /// 二进制不可用（409）、并发满（429）、形态不支持（503）、超时（504）才是 `Err`。
    pub fn host_exec_light(
        &self,
        binary: &str,
        argv: &[&str],
        timeout_sec: Option<u64>,
    ) -> Result<Value> {
        let mut body = json!({ "binary": binary, "argv": argv });
        if let Some(t) = timeout_sec {
            body["timeout_sec"] = json!(t);
        }
        self.call_kernel(skiff_wire::routes::CAP_EXEC_RUN_LIGHT, body)
    }

    /// 回调内核能力面（`/api/v1/capability/*`），带进程级 Bearer token。
    pub fn call_kernel(&self, path: &str, body: Value) -> Result<Value> {
        let payload = serde_json::to_vec(&body).map_err(|e| SdkError::Protocol(e.to_string()))?;
        let resp = kernel_http(&self.kernel_socket, path, &payload, &self.token)?;
        if resp.status != 200 {
            return Err(SdkError::Protocol(format!(
                "内核返回 HTTP {}: {}",
                resp.status,
                String::from_utf8_lossy(&resp.body)
            )));
        }
        resp.json().map_err(|e| SdkError::Protocol(e.to_string()))
    }

    /// 启动插件 HTTP 服务（阻塞）。只暴露 `/healthz` 与 `/rpc`。
    pub fn serve<F>(&self, handler: F) -> Result<()>
    where
        F: Fn(&str, Value) -> std::result::Result<Value, RpcError> + Send + Sync + 'static,
    {
        serve_inner(self, std::sync::Arc::new(handler))
    }
}

fn serve_inner<F>(plugin: &Plugin, handler: std::sync::Arc<F>) -> Result<()>
where
    F: Fn(&str, Value) -> std::result::Result<Value, RpcError> + Send + Sync + 'static,
{
    #[cfg(unix)]
    {
        use std::os::unix::net::UnixListener;

        // 运行目录由 Supervisor 预建并归属插件 UID；socket 0600 避免同机其它插件探活
        if let Some(parent) = plugin.socket.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let _ = std::fs::remove_file(&plugin.socket);
        let listener = UnixListener::bind(&plugin.socket)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ =
                std::fs::set_permissions(&plugin.socket, std::fs::Permissions::from_mode(0o600));
        }

        for stream in listener.incoming() {
            let mut stream = match stream {
                Ok(s) => s,
                Err(_) => continue,
            };
            let plugin = plugin.clone();
            let handler = handler.clone();
            std::thread::spawn(move || {
                let _ = handle_conn(&mut stream, &plugin, handler.as_ref());
            });
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (plugin, handler);
        Err(SdkError::Unsupported(
            "插件只支持在 Linux/unix 宿主运行".into(),
        ))
    }
}

#[cfg(unix)]
fn handle_conn<F>(
    stream: &mut std::os::unix::net::UnixStream,
    plugin: &Plugin,
    handler: &F,
) -> Result<()>
where
    F: Fn(&str, Value) -> std::result::Result<Value, RpcError>,
{
    let (method, path, body, protocol) = read_request(stream)?;
    // 协议版本先判：内核与插件不同步时，明确拒绝比让插件按旧语义处理要安全。
    if let Err(e) = skiff_wire::check_protocol(protocol) {
        return write_response(
            stream,
            426,
            &json!({"error": {"code": -32000, "message": e.to_string()}}),
        );
    }
    let (status, payload) = match (method.as_str(), path.as_str()) {
        ("GET", skiff_wire::routes::PLUGIN_HEALTHZ) => {
            (200, json!({ "ok": true, "plugin": plugin.id }))
        }
        ("POST", skiff_wire::routes::PLUGIN_RPC) => {
            let req: Value = match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(_) => {
                    return write_response(
                        stream,
                        400,
                        &json!({"error": {"code": -32700, "message": "parse error"}}),
                    );
                }
            };
            let id = req.get("id").cloned().unwrap_or(Value::Null);
            let m = req
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let params = req.get("params").cloned().unwrap_or(Value::Null);
            match handler(&m, params) {
                Ok(result) => (200, json!({ "jsonrpc": "2.0", "id": id, "result": result })),
                Err(e) => (
                    200,
                    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": e.code, "message": e.message } }),
                ),
            }
        }
        _ => (404, json!({ "error": "not found" })),
    };
    write_response(stream, status, &payload)
}

#[cfg(unix)]
fn read_request(
    stream: &mut std::os::unix::net::UnixStream,
) -> Result<(String, String, Vec<u8>, Option<u32>)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    // 头部解析与上限交给 skiff-wire，和内核侧同一份实现
    let (head, header_end) = loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Err(SdkError::Protocol("连接提前关闭".into()));
        }
        buf.extend_from_slice(&chunk[..n]);
        match skiff_wire::parse_request_head(&buf) {
            Ok(Some(v)) => break v,
            Ok(None) => continue,
            Err(e) => return Err(SdkError::Protocol(e.to_string())),
        }
    };
    let mut body = buf[header_end..].to_vec();
    while body.len() < head.content_length {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(head.content_length);
    Ok((head.method, head.path, body, head.protocol))
}

#[cfg(unix)]
fn write_response(
    stream: &mut std::os::unix::net::UnixStream,
    status: u16,
    body: &Value,
) -> Result<()> {
    let payload = serde_json::to_vec(body).unwrap_or_default();
    let head = skiff_wire::build_response_head(status, payload.len());
    stream.write_all(head.as_bytes())?;
    stream.write_all(&payload)?;
    stream.flush()?;
    Ok(())
}

/// 最小 UDS HTTP 客户端（回调内核用）。帧编解码来自 `skiff-wire`。
///
/// **连接复用**：每线程对同一个 socket 只保留一条连接（`Connection: keep-alive`），
/// 失败即丢弃；只有幂等只读路由才在失败后重连重试一次（`exec/run` 这类写操作
/// 重发可能重复提交，不重试）。
///
/// **超时**：读/写都有上限，内核卡死时不会再让插件线程永久挂起。
#[cfg(unix)]
pub fn uds_http(
    socket: &std::path::Path,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    token: Option<&str>,
) -> Result<skiff_wire::HttpResponse> {
    uds_http_timeout(
        socket,
        method,
        path,
        body,
        token,
        KERNEL_READ_TIMEOUT,
        false,
    )
}

/// 读超时：`exec/status` 长轮询最长挂起 30s，这里多留 10s 余量。
#[cfg(unix)]
const KERNEL_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(40);
/// `exec/run` 的前台等待上限是 120s（内核 `WAIT_MAX_SEC`），读超时必须比它长，
/// 否则插件显式 `wait_sec = 120` 的提交会被客户端自己掐断。
#[cfg(unix)]
const KERNEL_EXEC_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(135);
/// 写超时：单帧上限 1MiB，本地 UDS 正常远低于 1s。
#[cfg(unix)]
const KERNEL_WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// 按路由选读超时：长等待只发生在 `exec/run`，其余保持 40s。
#[cfg(unix)]
fn read_timeout_for(path: &str) -> std::time::Duration {
    if path == skiff_wire::routes::CAP_EXEC_RUN {
        KERNEL_EXEC_READ_TIMEOUT
    } else {
        KERNEL_READ_TIMEOUT
    }
}

#[cfg(unix)]
struct KernelConn {
    stream: std::os::unix::net::UnixStream,
    /// 已读但尚未组成完整响应的字节（连接复用时保留，避免丢帧前缀）。
    buf: Vec<u8>,
}

#[cfg(unix)]
thread_local! {
    /// 每线程到 kernel.sock 的复用连接。插件 RPC 处理线程通常连续回调内核。
    static KERNEL_CONNS: std::cell::RefCell<
        std::collections::HashMap<PathBuf, KernelConn>,
    > = std::cell::RefCell::new(std::collections::HashMap::new());
}

#[cfg(unix)]
fn uds_http_timeout(
    socket: &std::path::Path,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    token: Option<&str>,
    read_timeout: std::time::Duration,
    fresh: bool,
) -> Result<skiff_wire::HttpResponse> {
    if fresh {
        drop_kernel_conn(socket);
    }
    let mut conn = match take_kernel_conn(socket) {
        Some(c) => c,
        None => connect_kernel(socket, read_timeout)?,
    };
    match exchange(&mut conn, method, path, body, token, read_timeout) {
        Ok(resp) => {
            put_kernel_conn(socket, conn);
            Ok(resp)
        }
        Err(e) => {
            // 连接级失败：旧连接已不可信，直接丢弃。
            if !fresh && is_idempotent(path) {
                uds_http_timeout(socket, method, path, body, token, read_timeout, true)
            } else {
                Err(e)
            }
        }
    }
}

#[cfg(unix)]
fn connect_kernel(
    socket: &std::path::Path,
    read_timeout: std::time::Duration,
) -> Result<KernelConn> {
    let stream = std::os::unix::net::UnixStream::connect(socket)?;
    // 设置失败（不支持的平台）不致命：退化为阻塞读写。
    let _ = stream.set_read_timeout(Some(read_timeout));
    let _ = stream.set_write_timeout(Some(KERNEL_WRITE_TIMEOUT));
    Ok(KernelConn {
        stream,
        buf: Vec::new(),
    })
}

#[cfg(unix)]
fn take_kernel_conn(socket: &std::path::Path) -> Option<KernelConn> {
    KERNEL_CONNS.with(|pool| pool.borrow_mut().remove(socket))
}

#[cfg(unix)]
fn put_kernel_conn(socket: &std::path::Path, conn: KernelConn) {
    KERNEL_CONNS.with(|pool| {
        pool.borrow_mut().insert(socket.to_path_buf(), conn);
    });
}

#[cfg(unix)]
fn drop_kernel_conn(socket: &std::path::Path) {
    KERNEL_CONNS.with(|pool| {
        pool.borrow_mut().remove(socket);
    });
}

/// 幂等只读路由：连接层失败后可以安全地重连重发。
#[cfg(unix)]
fn is_idempotent(path: &str) -> bool {
    use skiff_wire::routes as r;
    matches!(
        path,
        r::CAP_IDENTITY_SELF
            | r::CAP_CONFIG
            | r::CAP_FS_READ
            | r::CAP_FS_LIST
            | r::CAP_EXEC_STATUS
            | r::CAP_INTEROP_CATALOG
    )
}

#[cfg(unix)]
fn exchange(
    conn: &mut KernelConn,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    token: Option<&str>,
    read_timeout: std::time::Duration,
) -> Result<skiff_wire::HttpResponse> {
    let payload = body.unwrap_or(&[]);
    if payload.len() > skiff_wire::MAX_FRAME {
        return Err(SdkError::Protocol("请求体超过单帧上限".into()));
    }
    // 连接是复用的，超时随路由变化（`exec/run` 更长），每次调用重设。
    conn.stream
        .set_read_timeout(Some(read_timeout))
        .map_err(map_io_error)?;
    let head = skiff_wire::build_request_keep_alive(method, path, payload.len(), token);
    conn.stream
        .write_all(head.as_bytes())
        .map_err(map_io_error)?;
    conn.stream.write_all(payload).map_err(map_io_error)?;
    conn.stream.flush().map_err(map_io_error)?;

    let mut chunk = [0u8; 8192];
    loop {
        if let Some((resp, used)) = skiff_wire::parse_response_at(&conn.buf)
            .map_err(|e| SdkError::Protocol(e.to_string()))?
        {
            conn.buf.drain(..used);
            return Ok(resp);
        }
        let n = conn.stream.read(&mut chunk).map_err(map_io_error)?;
        if n == 0 {
            return Err(SdkError::Protocol(
                "内核关闭连接（响应不完整，结果未知）".into(),
            ));
        }
        conn.buf.extend_from_slice(&chunk[..n]);
        if conn.buf.len() > skiff_wire::MAX_FRAME * 2 {
            return Err(SdkError::Protocol("响应超过帧上限".into()));
        }
    }
}

/// 读写超时统一映射成 `Timeout`：文案必须写明「结果未知」，调用方不能当成「没执行」。
#[cfg(unix)]
fn map_io_error(e: std::io::Error) -> SdkError {
    match e.kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => SdkError::Timeout(
            "读写 kernel.sock 超时，结果未知（可能仍在处理，或连接已卡死）".into(),
        ),
        _ => SdkError::Io(e),
    }
}

#[cfg(not(unix))]
pub fn uds_http(
    _socket: &std::path::Path,
    _method: &str,
    _path: &str,
    _body: Option<&[u8]>,
    _token: Option<&str>,
) -> Result<skiff_wire::HttpResponse> {
    Err(SdkError::Unsupported(
        "插件只支持在 Linux/unix 宿主运行".into(),
    ))
}

/// SDK 内部回调内核的入口：按路由选读超时（`exec/run` 用更长的等待）。
#[cfg(unix)]
fn kernel_http(
    socket: &std::path::Path,
    path: &str,
    payload: &[u8],
    token: &str,
) -> Result<skiff_wire::HttpResponse> {
    uds_http_timeout(
        socket,
        "POST",
        path,
        Some(payload),
        Some(token),
        read_timeout_for(path),
        false,
    )
}

#[cfg(not(unix))]
fn kernel_http(
    _socket: &std::path::Path,
    _path: &str,
    _payload: &[u8],
    _token: &str,
) -> Result<skiff_wire::HttpResponse> {
    Err(SdkError::Unsupported(
        "插件只支持在 Linux/unix 宿主运行".into(),
    ))
}

/// 把 exec 作业快照转成「一次执行的结果」，与旧版同步响应同形。
///
/// 保持 `{success, code, stdout, stderr, truncated}` 这个形状是有意的：编排代码
/// （装完配、配完重启）只关心「成没成、退出码、报了什么错」，不该因为内核把执行
/// 记录化就被迫改写。`job_id` / `status` / `error_detail` 是新增的可选字段。
fn finalize_exec_result(snapshot: Value, job_id: &str) -> Value {
    let status = snapshot
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("failed");
    let mut out = snapshot
        .get("result")
        .cloned()
        .filter(|v| v.is_object() && v.as_object().is_some_and(|o| !o.is_empty()))
        .unwrap_or_else(|| json!({}));
    if out.get("success").is_none() {
        out["success"] = json!(status == "succeeded");
    }
    out["job_id"] = json!(job_id);
    out["status"] = json!(status);
    if let Some(v) = snapshot.get("exit_code") {
        out["exit_code"] = v.clone();
    }
    if let Some(v) = snapshot.get("error_detail").filter(|v| !v.is_null()) {
        out["error_detail"] = v.clone();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 内核卡死时不得永久占用插件线程：读超时必须返回 `Timeout`（结果未知）。
    #[cfg(unix)]
    #[test]
    fn read_timeout_surfaces_as_timeout() {
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("kernel.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        std::thread::spawn(move || {
            if let Ok((_stream, _)) = listener.accept() {
                // 收下连接但永不响应，模拟内核侧卡死
                std::thread::sleep(std::time::Duration::from_secs(5));
            }
        });

        let err = uds_http_timeout(
            &sock,
            "POST",
            skiff_wire::routes::CAP_CONFIG,
            Some(b"{}"),
            None,
            std::time::Duration::from_millis(100),
            false,
        )
        .expect_err("无响应的内核必须触发读超时");
        assert!(matches!(err, SdkError::Timeout(_)), "{err:?}");
    }

    /// `exec/run` 的内核前台等待上限是 120s，客户端读超时必须比它长；
    /// 普通路由仍保持 40s，避免内核卡死时长期占用插件线程。
    #[cfg(unix)]
    #[test]
    fn exec_run_gets_a_longer_read_timeout() {
        use skiff_wire::routes as r;
        assert_eq!(read_timeout_for(r::CAP_CONFIG), KERNEL_READ_TIMEOUT);
        assert!(read_timeout_for(r::CAP_EXEC_RUN) > std::time::Duration::from_secs(120));
    }

    /// 连接复用：同一线程的两次调用应落在同一条 UDS 连接上（服务端只 accept 一次）。
    #[cfg(unix)]
    #[test]
    fn reuses_one_connection_per_thread() {
        use std::io::{Read, Write};
        use std::os::unix::net::{UnixListener, UnixStream};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("kernel.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let accepts = Arc::new(AtomicUsize::new(0));
        let acc = Arc::clone(&accepts);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                acc.fetch_add(1, Ordering::SeqCst);
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let parsed = match skiff_wire::parse_request_head(&buf) {
                        Ok(Some(v)) => v,
                        Ok(None) => {
                            let n = stream.read(&mut chunk).unwrap_or(0);
                            if n == 0 {
                                break;
                            }
                            buf.extend_from_slice(&chunk[..n]);
                            continue;
                        }
                        Err(_) => break,
                    };
                    let (head, end) = parsed;
                    if buf.len() < end + head.content_length {
                        let n = stream.read(&mut chunk).unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        continue;
                    }
                    buf.drain(..end + head.content_length);
                    let body = br#"{"ok":1}"#;
                    let head = skiff_wire::build_response_head(200, body.len());
                    if stream.write_all(head.as_bytes()).is_err()
                        || stream.write_all(body).is_err()
                        || stream.flush().is_err()
                    {
                        break;
                    }
                }
            }
        });

        let first = uds_http(
            &sock,
            "POST",
            skiff_wire::routes::CAP_CONFIG,
            Some(b"{}"),
            None,
        )
        .unwrap();
        let second = uds_http(
            &sock,
            "POST",
            skiff_wire::routes::CAP_CONFIG,
            Some(b"{}"),
            None,
        )
        .unwrap();
        assert_eq!(first.json().unwrap()["ok"], 1);
        assert_eq!(second.json().unwrap()["ok"], 1);
        assert_eq!(
            accepts.load(Ordering::SeqCst),
            1,
            "两次调用应复用同一条连接"
        );
    }

    /// 轻量通道：SDK 走 `exec/run_light`，body 只有 binary/argv/timeout_sec，
    /// 响应原样返回（非零退出不是 Err）。
    #[cfg(unix)]
    #[test]
    fn host_exec_light_posts_to_light_route() {
        use std::io::{Read, Write};
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("kernel.sock");
        let listener = UnixListener::bind(&sock).unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                match skiff_wire::parse_request_head(&buf) {
                    Ok(Some((head, end))) => {
                        while buf.len() < end + head.content_length {
                            let n = stream.read(&mut chunk).unwrap_or(0);
                            if n == 0 {
                                return;
                            }
                            buf.extend_from_slice(&chunk[..n]);
                        }
                        let body = String::from_utf8_lossy(&buf[end..end + head.content_length])
                            .to_string();
                        tx.send((head.path.clone(), body)).ok();
                        let payload = br#"{"success":false,"code":3,"stdout":"","stderr":"x","truncated":{"stdout":false,"stderr":false}}"#;
                        let h = skiff_wire::build_response_head(200, payload.len());
                        let _ = stream.write_all(h.as_bytes());
                        let _ = stream.write_all(payload);
                        let _ = stream.flush();
                        return;
                    }
                    _ => {
                        let n = stream.read(&mut chunk).unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                    }
                }
            }
        });

        let p = Plugin {
            id: "com.example.demo".into(),
            api: "1.0".into(),
            socket: PathBuf::from("/tmp/x"),
            data_dir: PathBuf::from("/tmp/d"),
            cache_dir: PathBuf::from("/tmp/c"),
            run_dir: PathBuf::from("/tmp/r"),
            kernel_socket: sock,
            granted: vec!["exec:run".into()],
            token: "t".into(),
        };
        let out = p
            .host_exec_light("/usr/sbin/nft", &["--version"], Some(5))
            .unwrap();
        let (path, body) = rx.recv().unwrap();
        assert_eq!(path, skiff_wire::routes::CAP_EXEC_RUN_LIGHT);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["binary"], "/usr/sbin/nft");
        assert_eq!(v["argv"][0], "--version");
        assert_eq!(v["timeout_sec"], 5);
        // 非零退出不是 Err：结果按原样交给调用方判断。
        assert_eq!(out["success"], false);
        assert_eq!(out["code"], 3);
    }

    #[test]
    fn rpc_error_codes_follow_jsonrpc() {
        assert_eq!(RpcError::method_not_found("x").code, -32601);
        assert_eq!(RpcError::invalid_params("x").code, -32602);
        assert_eq!(RpcError::internal("x").code, -32603);
    }

    #[test]
    fn granted_permission_lookup() {
        let p = Plugin {
            id: "com.example.demo".into(),
            api: "1.0".into(),
            socket: PathBuf::from("/tmp/x"),
            data_dir: PathBuf::from("/tmp/d"),
            cache_dir: PathBuf::from("/tmp/c"),
            run_dir: PathBuf::from("/tmp/r"),
            kernel_socket: PathBuf::from("/tmp/k"),
            granted: vec!["docker:containers.list".into()],
            token: "t".into(),
        };
        assert!(p.has_perm("docker:containers.list"));
        assert!(!p.has_perm("docker:containers.exec"));
    }
}
