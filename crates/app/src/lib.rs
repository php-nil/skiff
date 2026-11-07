//! 应用上下文（装配层）：`AppState` 与它的配置子集 `WebConfig`。
//!
//! 这里**不放路由、模板和中间件**：内核侧（`skiff::kernel` 的插件 UDS 服务）也要
//! 拿到 db / docker / policy，如果上下文住在 `web`，内核就被迫反向依赖表现层，
//! 既编译更重，也没法脱离 axum 单测。层序是 `app` → 下层，`web` → `app`。

pub mod admin_fs;
pub mod auth;
pub mod docker_daemon;
pub mod nginx;
pub mod runtime_catalog;
pub mod sites;
pub mod tasks;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use skiff_docker::{DockerService, Form, Policy, Tier};
use skiff_identity::{SessionLimits, SessionTokens};
use skiff_plugin::Supervisor;
use skiff_store::{Db, Role, User};
use tokio::sync::{Mutex, Semaphore};

use skiff_model::{Cidr, Permission, RoleExt, Scope, ScopeSet};

/// Web 层需要的配置子集（由 `skiff` 主 crate 从 env > config.toml > 默认 构造）。
#[derive(Debug, Clone)]
pub struct WebConfig {
    pub data_dir: PathBuf,
    /// 站点/nginx/日志/证书/PHP socket 的宿主根（默认 `/www`）。
    pub web_root: PathBuf,
    pub entry_prefix: String,
    /// loopback 明文开发模式（非 `__Host-` cookie、关闭 Secure、UI 标红）
    pub dev_http: bool,
    pub trusted_proxies: Vec<Cidr>,
    /// 面板预期 Origin（WS 校验、TLS 重定向用）
    pub external_url: Option<String>,
    pub tier: Tier,
    pub form: Form,
    pub allow_unsigned: bool,
    pub allow_reduced_sandbox: bool,
    pub policy: Policy,
    pub session: SessionLimits,
    /// 登录并发上限（argon2 单次 ~70MB，默认 2）
    pub login_concurrency: usize,
}

/// 装配层持有的三根服务句柄。
///
/// 这三个字段**私有**：上层（`web` / `core`）只应通过应用服务方法（`auth::login`
/// 等）与只读访问器取用，而不是随手 `state.db.audit(...)` 把存储层的细节漏进
/// 表现层与内核服务。`cfg` 是纯数据（不可变配置），保持公开。
pub struct AppState {
    db: Arc<Db>,
    docker: Arc<DockerService>,
    supervisor: Arc<Supervisor>,
    pub cfg: WebConfig,
    pub started_at: Instant,
    pub version: String,
    /// CSRF 派生密钥：持久化在 settings，重启后已发出的页面仍有效
    pub csrf_secret: String,
    login_limiter: Mutex<HashMap<String, LoginAttempt>>,
    login_semaphore: Semaphore,
    /// exec 单连票据（§6.2）：短 TTL、一次性、绑定会话与容器
    exec_tickets: Mutex<HashMap<String, ExecTicket>>,
    /// 正在作为 interop 目标执行的插件 id。
    ///
    /// 跨插件调用是**单层**的：A 调 B 期间，B 不能再发起 interop（否则 A→B→A
    /// 会无限递归）。这里用「进入/离开」标记，而不是调用栈深度计数——目标插件
    /// 并发处理两个请求时，任一请求尝试再调用都应当被拒。
    interop_busy: Mutex<HashSet<String>>,
    /// 「网站 → 运行环境」后台安装进度（内存态：重启后清空即可，安装动作幂等）。
    site_env: Mutex<sites::InstallProgress>,
    /// 按 `(kind,name)` 的运行环境安装互斥；第二个并发安装直接返回「正在安装」。
    runtime_installs: sites::InstallLocks,
    /// 运行环境异步安装/扩展任务（按 runtime id）；列表与抽屉轮询读取。
    runtime_jobs: Mutex<sites::RuntimeJobs>,
    /// 镜像版本元数据缓存：image_id → (读到的版本/None, 负缓存起点)。
    ///
    /// image_id 不可变：命中即 0 次 Docker API。inspect 失败做 60 秒负缓存，
    /// 避免每页重复打；容器重建换 image_id 自然失效。
    image_versions: Mutex<HashMap<String, (Option<String>, std::time::Instant)>>,
    /// 内核定时任务服务。`OnceLock` 而不是构造参数：`AppState::new` 被大量
    /// 测试直接使用，任务服务需要真实库与 Supervisor，由 `bootstrap` 显式初始化。
    tasks: std::sync::OnceLock<Arc<tasks::TaskService>>,
    /// 管理员侧文件服务（`skiff-fs` 的唯一接线点）。
    admin_fs: admin_fs::AdminFs,
}

#[derive(Debug, Clone)]
pub struct LoginAttempt {
    pub failures: u32,
    pub first_failure: Instant,
    pub blocked_until: Option<Instant>,
}

pub const LOGIN_WINDOW: Duration = Duration::from_secs(300);
pub const LOGIN_MAX_FAILURES: u32 = 5;
pub const LOGIN_BLOCK: Duration = Duration::from_secs(300);
/// 限流表条目上限。登录失败可能来自任意源 IP（含可信代理下 `X-Forwarded-For` 的取值），
/// 条目此前只增不删 —— 一张永不回收的表本身就是内存 DoS 面。超限时先回收过期条目，
/// 仍超限再按最早失败时间淘汰最旧的。
pub const LOGIN_TRACK_MAX: usize = 4096;

fn new_attempt(now: Instant) -> LoginAttempt {
    LoginAttempt {
        failures: 0,
        first_failure: now,
        blocked_until: None,
    }
}

/// 条目是否还值得保留：封禁未结束，或仍在统计窗口内。
fn attempt_live(e: &LoginAttempt, now: Instant) -> bool {
    if let Some(until) = e.blocked_until {
        return now < until;
    }
    now.saturating_duration_since(e.first_failure) <= LOGIN_WINDOW
}

fn prune_attempts(map: &mut HashMap<String, LoginAttempt>, now: Instant) {
    map.retain(|_, e| attempt_live(e, now));
    // 留 1/4 余量再清一次，避免「每来一个新 IP 就全表扫一遍」。
    let target = LOGIN_TRACK_MAX * 3 / 4;
    if map.len() <= target {
        return;
    }
    // 仍超限：最旧的先出局。只在越界时发生，O(n log n) 不在热路径上。
    let mut by_age: Vec<(Instant, String)> = map
        .iter()
        .map(|(k, e)| (e.first_failure, k.clone()))
        .collect();
    by_age.sort();
    for (_, key) in by_age.into_iter().take(map.len() - target) {
        map.remove(&key);
    }
}

/// 限流判定：同 IP 5 次/5 分钟，超限封禁 5 分钟（§6.3）。`now` 显式传入以便单测。
fn login_gate(map: &mut HashMap<String, LoginAttempt>, ip: &str, now: Instant) -> Result<(), u64> {
    let e = map
        .entry(ip.to_string())
        .or_insert_with(|| new_attempt(now));
    if let Some(until) = e.blocked_until {
        if now < until {
            return Err(until.saturating_duration_since(now).as_secs().max(1));
        }
        *e = new_attempt(now);
    }
    if now.saturating_duration_since(e.first_failure) > LOGIN_WINDOW && e.failures > 0 {
        e.failures = 0;
        e.first_failure = now;
    }
    Ok(())
}

fn note_failure(map: &mut HashMap<String, LoginAttempt>, ip: &str, now: Instant) {
    if !map.contains_key(ip) && map.len() >= LOGIN_TRACK_MAX {
        prune_attempts(map, now);
    }
    let e = map
        .entry(ip.to_string())
        .or_insert_with(|| new_attempt(now));
    e.failures += 1;
    if e.failures >= LOGIN_MAX_FAILURES {
        e.blocked_until = Some(now + LOGIN_BLOCK);
    }
}

// ---------------------------------------------------------------------------
// 容器终端单连票据（§6.2）
//
// WS 握手只靠 Cookie 时，一张被其它页面骗开的连接与「本人点击」不可区分。票据把
// 「谁、对哪个容器、什么权限」在 HTTP 侧先固化成一次性凭据，WS 侧只认票据：
// - 30 秒 TTL：来得及升级，来不及被复制到别处；
// - 一次性：消费即删，重放无效；
// - 绑定 session_id + 容器 id：跨会话或换容器都不认。
// 内存里只存票据的 sha256，明文只在响应体里出现一次（与会话/Token 同一原则）。
// ---------------------------------------------------------------------------
pub const EXEC_TICKET_TTL: Duration = Duration::from_secs(30);
/// 票据表上限：管理员误连/重复点击不会撑爆内存；越界先回收过期再按最早到期淘汰。
pub const EXEC_TICKET_MAX: usize = 512;

#[derive(Debug, Clone)]
pub struct ExecTicket {
    pub session_id: String,
    pub container_id: String,
    pub expires_at: Instant,
}

fn ticket_live(t: &ExecTicket, now: Instant) -> bool {
    now < t.expires_at
}

fn prune_tickets(map: &mut HashMap<String, ExecTicket>, now: Instant) {
    map.retain(|_, t| ticket_live(t, now));
    let target = EXEC_TICKET_MAX * 3 / 4;
    if map.len() <= target {
        return;
    }
    let mut by_expiry: Vec<(Instant, String)> =
        map.iter().map(|(k, t)| (t.expires_at, k.clone())).collect();
    by_expiry.sort();
    for (_, key) in by_expiry.into_iter().take(map.len() - target) {
        map.remove(&key);
    }
}

/// 核销票据。**无论校验是否通过都先删除**：一张被错误会话出示的票据应视为已泄露，
/// 留着只会让同一张票被反复试探。
fn consume_ticket(
    map: &mut HashMap<String, ExecTicket>,
    ticket: &str,
    session_id: &str,
    container_id: &str,
    now: Instant,
) -> Result<(), &'static str> {
    let key = skiff_identity::sha256_hex(ticket);
    let Some(t) = map.get(&key).cloned() else {
        return Err("票据无效或已使用");
    };
    map.remove(&key);
    if !ticket_live(&t, now) {
        return Err("票据已过期");
    }
    if t.session_id != session_id {
        return Err("票据与当前会话不匹配");
    }
    if t.container_id != container_id {
        return Err("票据与容器不匹配");
    }
    Ok(())
}

impl AppState {
    pub fn new(
        db: Arc<Db>,
        docker: Arc<DockerService>,
        supervisor: Arc<Supervisor>,
        cfg: WebConfig,
        csrf_secret: String,
        version: impl Into<String>,
    ) -> Arc<Self> {
        let permits = cfg.login_concurrency.max(1);
        let admin_fs = admin_fs::AdminFs::new(&cfg, Arc::clone(&db));
        Arc::new(Self {
            db,
            docker,
            supervisor,
            cfg,
            started_at: Instant::now(),
            version: version.into(),
            csrf_secret,
            login_limiter: Mutex::new(HashMap::new()),
            login_semaphore: Semaphore::new(permits),
            exec_tickets: Mutex::new(HashMap::new()),
            interop_busy: Mutex::new(HashSet::new()),
            site_env: Mutex::new(sites::InstallProgress::default()),
            runtime_installs: sites::InstallLocks::default(),
            runtime_jobs: Mutex::new(sites::RuntimeJobs::default()),
            image_versions: Mutex::new(HashMap::new()),
            tasks: std::sync::OnceLock::new(),
            admin_fs,
        })
    }

    /// 初始化内核任务服务：装载库内任务、种内置任务、起 tick 前调用一次。
    pub fn init_tasks(&self) -> Result<(), String> {
        let svc = tasks::TaskService::open(
            Arc::clone(&self.db),
            Arc::clone(&self.supervisor),
            self.cfg.web_root.clone(),
            self.cfg.data_dir.clone(),
        )?;
        self.tasks
            .set(svc)
            .map_err(|_| "任务服务已初始化".to_string())
    }

    /// 任务服务句柄。未初始化时返回 `None`（Web 写操作应回 503）。
    pub fn tasks(&self) -> Option<&Arc<tasks::TaskService>> {
        self.tasks.get()
    }

    /// 管理员侧文件服务（只读访问）。Web 文件管理页是它的唯一调用方。
    pub fn admin_fs(&self) -> &admin_fs::AdminFs {
        &self.admin_fs
    }

    /// 存储层句柄（只读访问）。上层要写审计/查插件记录时用这个入口，
    /// 而不是直接摸存储句柄——字段私有是刻意的：跨层直接调用存储 API
    /// 会让「谁负责这条业务规则」变得没有答案。
    pub fn db(&self) -> &Arc<Db> {
        &self.db
    }

    /// Docker 服务句柄（只读访问）。
    pub fn docker(&self) -> &Arc<DockerService> {
        &self.docker
    }

    /// 插件进程监管器（只读访问）。
    pub fn supervisor(&self) -> &Arc<Supervisor> {
        &self.supervisor
    }

    /// 标记插件进入「作为 interop 目标执行」状态；返回 `false` 表示已在执行中
    /// （即出现了环或并发重入，调用方应拒绝）。
    pub async fn enter_interop(&self, id: &str) -> bool {
        self.interop_busy.lock().await.insert(id.to_string())
    }

    /// 离开 interop 目标状态。无论调用成功、失败还是超时都必须调用。
    pub async fn leave_interop(&self, id: &str) {
        self.interop_busy.lock().await.remove(id);
    }

    /// 插件当前是否正作为执行目标（interop 或任务执行中）。
    /// 调度能力面据此拒绝「插件来源的任务在执行时回调调度器」。
    pub async fn is_interop_busy(&self, id: &str) -> bool {
        self.interop_busy.lock().await.contains(id)
    }

    /// 签发 exec 单连票据：明文只返回这一次，库里存 sha256。
    pub async fn issue_exec_ticket(&self, session_id: &str, container_id: &str) -> (String, u64) {
        let token = skiff_identity::new_token();
        let key = skiff_identity::sha256_hex(&token);
        let now = Instant::now();
        let mut map = self.exec_tickets.lock().await;
        if map.len() >= EXEC_TICKET_MAX {
            prune_tickets(&mut map, now);
        }
        map.insert(
            key,
            ExecTicket {
                session_id: session_id.to_string(),
                container_id: container_id.to_string(),
                expires_at: now + EXEC_TICKET_TTL,
            },
        );
        (token, EXEC_TICKET_TTL.as_secs())
    }

    /// 核销 exec 单连票据（一次性）。
    pub async fn consume_exec_ticket(
        &self,
        ticket: &str,
        session_id: &str,
        container_id: &str,
    ) -> Result<(), &'static str> {
        let mut map = self.exec_tickets.lock().await;
        consume_ticket(&mut map, ticket, session_id, container_id, Instant::now())
    }

    pub fn dev_http(&self) -> bool {
        self.cfg.dev_http
    }

    pub fn cookie_name(&self) -> &'static str {
        skiff_identity::session_cookie_name(self.cfg.dev_http)
    }

    /// 登录限流：同 IP 5 次/5 分钟，超限封禁 5 分钟（§6.3）。
    pub async fn check_login_allowed(&self, ip: &str) -> Result<(), u64> {
        let mut map = self.login_limiter.lock().await;
        login_gate(&mut map, ip, Instant::now())
    }

    pub async fn record_login_failure(&self, ip: &str) {
        let mut map = self.login_limiter.lock().await;
        note_failure(&mut map, ip, Instant::now());
    }

    pub async fn reset_login_failures(&self, ip: &str) {
        self.login_limiter.lock().await.remove(ip);
    }

    /// CSRF 令牌由会话令牌派生：无需额外存储，改会话即失效。
    pub fn csrf_token(&self, session_token: &str) -> String {
        skiff_identity::sha256_hex(format!("{}:{}", self.csrf_secret, session_token))
    }

    pub fn new_session_tokens(&self) -> SessionTokens {
        SessionTokens::new()
    }

    pub fn docker_unavailable_reason(&self) -> &str {
        self.docker.unavailable_reason()
    }
}

/// `AppState` 实现插件能力面的宿主上下文。
///
/// 能力面（`skiff-plugin::capability`）住在插件 crate，但插件 crate 不能反向依赖
/// `skiff-app`（否则成环）。这里实现 trait，把「数据根在哪」「授了哪些权限」
/// 「审计往哪写」三件事交出去，能力面因此不碰数据库与状态装配。
impl skiff_plugin::capability::CapabilityHost for AppState {
    fn data_dir(&self) -> &std::path::Path {
        &self.cfg.data_dir
    }

    fn plugin_grants(&self, plugin_id: &str) -> Vec<skiff_store::Grant> {
        self.db
            .plugin(plugin_id)
            .ok()
            .flatten()
            .map(|p| p.grants)
            .unwrap_or_default()
    }

    fn audit(
        &self,
        actor: &str,
        channel: &str,
        action: &str,
        target: &str,
        result: &str,
        detail: &str,
    ) {
        // 审计写失败不阻断能力调用：调用结果已由 handler 决定，审计只是流水。
        let _ = self
            .db
            .audit(actor, channel, action, target, result, detail);
    }
}

/// 已认证请求的上下文。
#[derive(Debug, Clone)]
pub struct CurrentUser {
    pub user: User,
    pub session_id: String,
    /// 明文会话令牌（仅用于派生 CSRF；不落库、不进日志）
    pub session_token: String,
    /// API Token 的 scope 集（§6.1）。`None` = 会话认证，不受 scope 收窄；
    /// `Some` = Bearer API Token，权限 = 角色权限 ∩ scope。
    pub scopes: Option<ScopeSet>,
}

impl CurrentUser {
    pub fn role(&self) -> Role {
        self.user.role
    }
    pub fn is_admin(&self) -> bool {
        self.user.role == Role::Admin
    }
    /// 角色→权限判定：查 `skiff_model::Permission` 这张唯一的矩阵。
    ///
    /// 判定层（`skiff_policy::Policy::check`）、UI 层（按钮显隐）、路由层
    /// （`AdminUser` 提取器）都走这一个入口，避免同一规则写在三处。
    pub fn can(&self, perm: Permission) -> bool {
        self.role().can(perm)
    }
    pub fn must_change_password(&self) -> bool {
        self.user.must_change_password
    }
    /// 是否具备某个 API scope。会话认证恒为 true（角色限制另由 `policy.check` 负责）。
    pub fn allows_scope(&self, scope: Scope) -> bool {
        match &self.scopes {
            None => true,
            Some(set) => set.allows(scope),
        }
    }
}

/// 客户端真实 IP / scheme（可信代理命中后才采信 `X-Forwarded-*`）。
#[derive(Debug, Clone)]
pub struct ClientInfo {
    pub ip: String,
    pub scheme: String,
    pub forwarded_ignored: bool,
}

impl Default for ClientInfo {
    fn default() -> Self {
        Self {
            ip: "unknown".into(),
            scheme: "http".into(),
            forwarded_ignored: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> HashMap<String, LoginAttempt> {
        HashMap::new()
    }

    #[test]
    fn blocks_after_max_failures_and_reports_wait() {
        let mut m = map();
        let ip = "203.0.113.7";
        let t0 = Instant::now();
        for _ in 0..LOGIN_MAX_FAILURES {
            note_failure(&mut m, ip, t0);
        }
        let wait = login_gate(&mut m, ip, t0).unwrap_err();
        assert!(wait > 0, "封禁后应返回剩余秒数");

        // 封禁结束后放行，并清零计数
        let t1 = t0 + LOGIN_BLOCK + Duration::from_secs(1);
        assert!(login_gate(&mut m, ip, t1).is_ok());
        assert_eq!(m[ip].failures, 0);
    }

    #[test]
    fn failure_counter_resets_after_window() {
        let mut m = map();
        let ip = "203.0.113.8";
        let t0 = Instant::now();
        note_failure(&mut m, ip, t0);
        let later = t0 + LOGIN_WINDOW + Duration::from_secs(1);
        assert!(login_gate(&mut m, ip, later).is_ok());
        assert_eq!(m[ip].failures, 0);
    }

    /// 表必须有界：此前每个新 IP 都留一条永不回收的记录。
    #[test]
    fn tracking_table_stays_bounded() {
        let mut m = map();
        let t0 = Instant::now();
        for i in 0..(LOGIN_TRACK_MAX * 2) {
            note_failure(&mut m, &format!("198.51.100.{i}"), t0);
        }
        assert!(
            m.len() <= LOGIN_TRACK_MAX,
            "限流表越界: {} > {}",
            m.len(),
            LOGIN_TRACK_MAX
        );
    }

    /// 过期条目应被回收，而不是把仍在封禁期的 IP 挤掉。
    #[test]
    fn expired_entries_are_reclaimed_before_eviction() {
        let mut m = map();
        let t0 = Instant::now();
        for i in 0..LOGIN_TRACK_MAX {
            note_failure(&mut m, &format!("10.0.0.{i}"), t0);
        }
        // 让 10.0.0.0 在「窗口已过」之后再次失败，进入封禁状态
        let blocked = "10.0.0.0";
        let t_blocked = t0 + LOGIN_WINDOW + Duration::from_secs(1);
        for _ in 0..LOGIN_MAX_FAILURES {
            note_failure(&mut m, blocked, t_blocked);
        }
        assert!(m[blocked].blocked_until.is_some());

        // 封禁期内来一个新 IP：其余条目早已过期，应被回收，封禁中的必须留下
        let t_late = t_blocked + Duration::from_secs(1);
        note_failure(&mut m, "192.0.2.99", t_late);
        assert!(m.contains_key(blocked), "封禁中的条目不应被淘汰");
        assert!(m.contains_key("192.0.2.99"));
        assert!(m.len() <= 2, "过期条目未被回收，仍有 {} 条", m.len());
    }

    fn ticket_map() -> HashMap<String, ExecTicket> {
        HashMap::new()
    }

    fn put(map: &mut HashMap<String, ExecTicket>, ticket: &str, exp: Instant) {
        map.insert(
            skiff_identity::sha256_hex(ticket),
            ExecTicket {
                session_id: "sess-1".into(),
                container_id: "cid-1".into(),
                expires_at: exp,
            },
        );
    }

    #[test]
    fn ticket_is_single_use() {
        let t0 = Instant::now();
        let mut m = ticket_map();
        put(&mut m, "tkt", t0 + EXEC_TICKET_TTL);
        assert!(consume_ticket(&mut m, "tkt", "sess-1", "cid-1", t0).is_ok());
        assert_eq!(
            consume_ticket(&mut m, "tkt", "sess-1", "cid-1", t0),
            Err("票据无效或已使用"),
            "同一张票据第二次必须失败"
        );
        assert!(m.is_empty(), "核销后不应留下条目");
    }

    #[test]
    fn expired_ticket_is_rejected() {
        let t0 = Instant::now();
        let mut m = ticket_map();
        put(&mut m, "tkt", t0 + EXEC_TICKET_TTL);
        let late = t0 + EXEC_TICKET_TTL + Duration::from_secs(1);
        assert_eq!(
            consume_ticket(&mut m, "tkt", "sess-1", "cid-1", late),
            Err("票据已过期")
        );
    }

    /// 票据绑定会话与容器：换一个会话或换一个容器都必须拒绝。
    #[test]
    fn ticket_is_bound_to_session_and_container() {
        let t0 = Instant::now();
        let mut m = ticket_map();
        put(&mut m, "tkt-a", t0 + EXEC_TICKET_TTL);
        assert_eq!(
            consume_ticket(&mut m, "tkt-a", "sess-2", "cid-1", t0),
            Err("票据与当前会话不匹配")
        );
        // 出示失败后票据作废，合法会话也不能再用
        assert!(consume_ticket(&mut m, "tkt-a", "sess-1", "cid-1", t0).is_err());

        put(&mut m, "tkt-b", t0 + EXEC_TICKET_TTL);
        assert_eq!(
            consume_ticket(&mut m, "tkt-b", "sess-1", "cid-2", t0),
            Err("票据与容器不匹配")
        );
    }

    #[test]
    fn unknown_ticket_is_rejected() {
        let mut m = ticket_map();
        assert_eq!(
            consume_ticket(&mut m, "nope", "sess-1", "cid-1", Instant::now()),
            Err("票据无效或已使用")
        );
    }

    /// 票据表必须有界（与登录限流表同样的理由）。
    #[test]
    fn ticket_table_stays_bounded() {
        let t0 = Instant::now();
        let mut m = ticket_map();
        for i in 0..(EXEC_TICKET_MAX * 2) {
            if m.len() >= EXEC_TICKET_MAX {
                prune_tickets(&mut m, t0);
            }
            put(&mut m, &format!("tkt-{i}"), t0 + EXEC_TICKET_TTL);
        }
        assert!(
            m.len() <= EXEC_TICKET_MAX,
            "票据表越界: {} > {}",
            m.len(),
            EXEC_TICKET_MAX
        );
    }
}
