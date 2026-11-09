//! 登录用例（应用服务层）：UI 表单登录与 JSON API 登录共用同一份流程。
//!
//! 此前 `crates/web` 的 `handlers/auth.rs::login_submit` 与 `handlers/api.rs::login`
//! 各自实现了一遍几乎相同的流程：限流 → 并发闸 → 查用户 → 验口令 → 验 TOTP
//! → 备份码 → 建会话 → 写审计。两份实现此后各自漂移——UI 侧限流时写审计、
//! API 侧不写；错误文案也不同（API 侧永远只说「用户名或口令不正确」）。
//! 判定逻辑只此一份，调用方只负责「怎么把结果渲染出去」。

use skiff_store::Role;

use crate::AppState;

/// 登录凭据（UI 表单与 JSON API 共用同一形状）。
#[derive(Debug, Clone)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    /// TOTP 动态码或备份码；未启用 TOTP 时留空。
    pub otp: String,
}

/// 登录失败的三种语义。调用方据此选状态码与文案，不再各写一套。
#[derive(Debug, Clone)]
pub enum LoginError {
    /// 被限流：还需等待的秒数。
    RateLimited(u64),
    /// 凭据不符。`message` 是可直接展示给用户的文案。
    Invalid(String),
    /// 内部故障（数据库等）。
    Internal(String),
}

/// 登录成功：调用方据此下发会话 Cookie 或 JSON。
#[derive(Debug, Clone)]
pub struct LoginSuccess {
    pub username: String,
    pub role: Role,
    pub session_id: String,
    /// 明文会话令牌（只在这里出现一次）。
    pub session_token: String,
    /// 由会话令牌派生的 CSRF 令牌。
    pub csrf_token: String,
    /// 绝对过期时间（unix 秒）。
    pub expires_at: i64,
    /// 首启账号必须改密：调用方据此跳转到改密页。
    pub must_change_password: bool,
}

/// 当前 unix 时间（秒）。系统时钟早于 epoch 时按 0 处理。
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

impl AppState {
    /// 登录主流程。
    ///
    /// `channel` 是审计里的目标域（UI 传 `session`，API 传 `api`）；`user_agent`
    /// 是写进会话记录的客户端标识（UI 传 Host 头，API 传固定值）。
    pub async fn login(
        &self,
        req: &LoginRequest,
        ip: &str,
        channel: &str,
        user_agent: &str,
    ) -> Result<LoginSuccess, LoginError> {
        // 限流先于任何口令计算：被封禁的 IP 不该再消耗 argon2 的 CPU 与内存。
        if let Err(wait) = self.check_login_allowed(ip).await {
            if let Err(e) = self.db.audit(
                &req.username,
                ip,
                "auth.login",
                channel,
                "blocked",
                &format!("限流：还需等待 {wait}s"),
            ) {
                tracing::error!(error = %e, "限流登录的审计写入失败");
            }
            return Err(LoginError::RateLimited(wait));
        }

        // 并发闸：argon2 单次验口令约 70MB 内存，不限并发等于放任被拖垮。
        let _permit = self
            .login_semaphore
            .acquire()
            .await
            .map_err(|_| LoginError::Internal("登录并发控制不可用".into()))?;

        let user = self
            .db
            .user_by_name(&req.username)
            .map_err(|e| LoginError::Internal(e.to_string()))?;

        let mut ok = false;
        if let Some(u) = &user {
            // argon2 是 CPU + 内存密集的同步计算（约 70MB/次），必须在阻塞线程池里跑：
            // 直接在 async worker 上算会把整个面板（含探针）拖住。
            let password = req.password.clone();
            let stored = u.password_hash.clone();
            let verified = tokio::task::spawn_blocking(move || {
                skiff_identity::verify_password(&password, &stored)
            })
            .await
            .map_err(|e| LoginError::Internal(format!("口令校验任务失败：{e}")))?;
            if verified {
                if u.totp_enabled {
                    let secret = u.totp_secret.clone().unwrap_or_default();
                    let now = unix_now().max(0) as u64;
                    ok = skiff_identity::verify_totp(&secret, &req.otp, now, 1);
                    if !ok && !req.otp.is_empty() {
                        // 动态码不匹配时再试一次备份码；备份码一次性消费。
                        let hash = skiff_identity::totp::hash_backup_code(&req.otp);
                        ok = self
                            .db
                            .consume_backup_code(u.id, &hash)
                            .map_err(|e| LoginError::Internal(e.to_string()))?;
                    }
                } else {
                    ok = true;
                }
            }
        }

        if !ok {
            self.record_login_failure(ip).await;
            if let Err(e) = self
                .db
                .audit(&req.username, ip, "auth.login", channel, "fail", "")
            {
                tracing::error!(error = %e, "登录失败审计写入失败");
            }
            return Err(LoginError::Invalid(
                if user.map(|u| u.totp_enabled).unwrap_or(false) {
                    "用户名、口令或动态码不正确"
                } else {
                    "用户名或口令不正确"
                }
                .to_string(),
            ));
        }

        // 逻辑上不可达：只有命中用户且口令校验通过才会把 `ok` 置真。这里不 panic——
        // 登录是最热的请求路径，宁可按内部错误拒绝，也不让一个断言把进程带走。
        let Some(user) = user else {
            return Err(LoginError::Internal(
                "登录状态不一致：口令已通过但用户记录丢失".into(),
            ));
        };
        self.reset_login_failures(ip).await;
        let tokens = self.new_session_tokens();
        let now = unix_now();
        self.db
            .create_session(
                &tokens.session_id,
                user.id,
                &tokens.session_token_hash,
                ip,
                user_agent,
                now + self.cfg.session.idle_secs,
                now + self.cfg.session.abs_secs,
            )
            .map_err(|e| LoginError::Internal(e.to_string()))?;
        if let Err(e) = self
            .db
            .audit(&user.username, ip, "auth.login", channel, "ok", "")
        {
            tracing::error!(error = %e, "登录成功审计写入失败");
        }

        let csrf_token = self.csrf_token(&tokens.session_token);
        Ok(LoginSuccess {
            username: user.username,
            role: user.role,
            session_id: tokens.session_id,
            session_token: tokens.session_token,
            csrf_token,
            expires_at: now + self.cfg.session.abs_secs,
            must_change_password: user.must_change_password,
        })
    }

    /// 注销用例：撤销当前会话并留审计。handler 不再直接写 `sessions` 表。
    pub fn logout(&self, username: &str, session_id: &str, ip: &str) -> Result<(), String> {
        self.db
            .revoke_session(session_id)
            .map_err(|e| e.to_string())?;
        if let Err(e) = self
            .db
            .audit(username, ip, "auth.logout", "session", "ok", "")
        {
            tracing::error!(error = %e, "注销审计写入失败");
        }
        Ok(())
    }

    /// 改密写路径：换口令 → 撤销该用户全部会话 → 立刻重开一个会话（避免把自己踢下线）。
    ///
    /// 口令校验与哈希仍在调用方（要 `spawn_blocking`），这里只负责落库与会话轮换。
    pub fn rotate_password_session(
        &self,
        user_id: i64,
        username: &str,
        password_hash: &str,
        ip: &str,
        user_agent: &str,
    ) -> Result<skiff_identity::SessionTokens, String> {
        self.db
            .set_password(user_id, password_hash)
            .map_err(|e| e.to_string())?;
        self.db
            .revoke_all_sessions(user_id)
            .map_err(|e| e.to_string())?;
        let tokens = self.new_session_tokens();
        let now = unix_now();
        self.db
            .create_session(
                &tokens.session_id,
                user_id,
                &tokens.session_token_hash,
                ip,
                user_agent,
                now + self.cfg.session.idle_secs,
                now + self.cfg.session.abs_secs,
            )
            .map_err(|e| e.to_string())?;
        if let Err(e) = self
            .db
            .audit(username, ip, "auth.password.change", "user", "ok", "")
        {
            tracing::error!(error = %e, "改密审计写入失败");
        }
        Ok(tokens)
    }

    /// TOTP 密钥写入（生成时 `enabled=false`，验证动态码后置真）。
    pub fn save_totp_secret(
        &self,
        user_id: i64,
        secret: &str,
        enabled: bool,
    ) -> Result<(), String> {
        self.db
            .set_totp(user_id, Some(secret), enabled)
            .map_err(|e| e.to_string())
    }

    /// 启用 TOTP：写备份码 + 置启用位 + 审计，一次调用完成，避免 handler 分三步写库。
    pub fn enable_totp(
        &self,
        user_id: i64,
        username: &str,
        secret: &str,
        backup_hashes: &[String],
    ) -> Result<(), String> {
        self.db
            .replace_backup_codes(user_id, backup_hashes)
            .map_err(|e| e.to_string())?;
        self.db
            .set_totp(user_id, Some(secret), true)
            .map_err(|e| e.to_string())?;
        if let Err(e) = self
            .db
            .audit(username, "local", "auth.totp.enable", "user", "ok", "")
        {
            tracing::error!(error = %e, "TOTP 启用审计写入失败");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WebConfig;
    use skiff_docker::{DockerService, Form, Policy, Tier};
    use skiff_identity::SessionLimits;
    use skiff_model::Role;
    use skiff_plugin::{Supervisor, SupervisorConfig};
    use std::sync::Arc;

    /// 造一个带管理员账号的应用上下文。数据目录落在系统临时目录，避免污染源码树。
    fn state_with_user(username: &str, password: &str) -> (Arc<AppState>, i64) {
        let db = Arc::new(skiff_store::Db::open_in_memory().unwrap());
        let hash = skiff_identity::hash_password(password).unwrap();
        let id = db.create_user(username, &hash, Role::Admin, false).unwrap();
        let tmp = std::env::temp_dir().join("skiff-app-test");
        let docker = Arc::new(DockerService::connect("/nonexistent/docker.sock"));
        let sup = Arc::new(Supervisor::new(
            SupervisorConfig::new(tmp.join("data"), tmp.join("run")),
            db.clone(),
        ));
        let cfg = WebConfig {
            data_dir: tmp.join("data"),
            web_root: tmp.join("www"),
            trash_retention_days: 30,
            config_path: tmp.join("config.toml"),
            listen: "127.0.0.1:18080".parse().unwrap(),
            entry_prefix: "p".into(),
            entry_prefix_previous: None,
            tls_enabled: false,
            tls_cert: None,
            tls_key: None,
            allow_insecure_http: true,
            dev_http: true,
            trusted_proxies: vec![],
            external_url: None,
            tier: Tier::Basic,
            form: Form::Native,
            allow_unsigned: true,
            allow_reduced_sandbox: true,
            policy: Policy::new(Tier::Basic, Form::Native, vec![]).unwrap(),
            session: SessionLimits::default(),
            login_concurrency: 1,
            ext_exec_timeout: std::time::Duration::from_secs(15 * 60),
            ext_batch_timeout: std::time::Duration::from_secs(30 * 60),
        };
        (
            AppState::new(db, docker, sup, cfg, "csrf-test".into(), "test"),
            id,
        )
    }

    fn req(username: &str, password: &str, otp: &str) -> LoginRequest {
        LoginRequest {
            username: username.into(),
            password: password.into(),
            otp: otp.into(),
        }
    }

    #[tokio::test]
    async fn login_creates_a_usable_session() {
        let (state, _) = state_with_user("admin", "s3cret-pass");
        let ok = state
            .login(
                &req("admin", "s3cret-pass", ""),
                "203.0.113.1",
                "session",
                "host",
            )
            .await
            .unwrap();
        assert_eq!(ok.username, "admin");
        assert!(!ok.session_token.is_empty());
        assert!(!ok.csrf_token.is_empty());

        // 会话确实落库，且令牌是哈希入库（明文查不到）。
        let hash = skiff_identity::sha256_hex(&ok.session_token);
        let session = state
            .db()
            .verify_session(&hash, state.cfg.session.idle_secs)
            .unwrap();
        assert!(session.is_some(), "登录后应有一条有效会话");
    }

    #[tokio::test]
    async fn wrong_password_is_rejected_and_audited() {
        let (state, _) = state_with_user("admin", "s3cret-pass");
        let err = state
            .login(&req("admin", "wrong", ""), "203.0.113.2", "session", "host")
            .await
            .unwrap_err();
        match err {
            LoginError::Invalid(msg) => assert_eq!(msg, "用户名或口令不正确"),
            other => panic!("应当是凭据错误，实际: {other:?}"),
        }
        let entries = state.db().recent_audit(10).unwrap();
        assert!(
            entries
                .iter()
                .any(|e| e.action == "auth.login" && e.result == "fail"),
            "失败登录应写审计，实际: {entries:?}"
        );
    }

    /// UI 与 API 共用同一份流程：限流判定只此一处，API 侧同样受罚。
    #[tokio::test]
    async fn repeated_failures_rate_limit_the_caller() {
        let (state, _) = state_with_user("admin", "s3cret-pass");
        for _ in 0..crate::LOGIN_MAX_FAILURES {
            let err = state
                .login(&req("admin", "wrong", ""), "203.0.113.3", "api", "api")
                .await
                .unwrap_err();
            assert!(matches!(err, LoginError::Invalid(_)));
        }
        let err = state
            .login(
                &req("admin", "s3cret-pass", ""),
                "203.0.113.3",
                "api",
                "api",
            )
            .await
            .unwrap_err();
        match err {
            LoginError::RateLimited(wait) => assert!(wait > 0),
            other => panic!("应当被限流，实际: {other:?}"),
        }
        let entries = state.db().recent_audit(10).unwrap();
        assert!(
            entries
                .iter()
                .any(|e| e.action == "auth.login" && e.result == "blocked"),
            "限流应写审计，实际: {entries:?}"
        );
    }

    #[tokio::test]
    async fn totp_is_required_when_enabled() {
        let (state, id) = state_with_user("admin", "s3cret-pass");
        let secret = skiff_identity::generate_totp_secret();
        state.db().set_totp(id, Some(&secret), true).unwrap();

        // 口令正确但没有动态码：仍应失败，且文案要提示动态码。
        let err = state
            .login(
                &req("admin", "s3cret-pass", ""),
                "203.0.113.4",
                "session",
                "host",
            )
            .await
            .unwrap_err();
        match err {
            LoginError::Invalid(msg) => assert!(msg.contains("动态码"), "实际: {msg}"),
            other => panic!("应当是凭据错误，实际: {other:?}"),
        }

        // 带上正确动态码即可通过。
        let code = skiff_identity::totp::totp_now(&secret, unix_now().max(0) as u64).unwrap();
        state
            .login(
                &req("admin", "s3cret-pass", &code),
                "203.0.113.4",
                "session",
                "host",
            )
            .await
            .unwrap();
    }

    /// 备份码是一次性的：第一次消费掉，第二次必须失败。
    #[tokio::test]
    async fn backup_code_is_single_use() {
        let (state, id) = state_with_user("admin", "s3cret-pass");
        let secret = skiff_identity::generate_totp_secret();
        state.db().set_totp(id, Some(&secret), true).unwrap();
        let (plain, hashes) = skiff_identity::generate_backup_codes(3);
        state.db().replace_backup_codes(id, &hashes).unwrap();

        state
            .login(
                &req("admin", "s3cret-pass", &plain[0]),
                "203.0.113.5",
                "session",
                "host",
            )
            .await
            .unwrap();

        let err = state
            .login(
                &req("admin", "s3cret-pass", &plain[0]),
                "203.0.113.5",
                "session",
                "host",
            )
            .await
            .unwrap_err();
        assert!(matches!(err, LoginError::Invalid(_)), "备份码不得复用");
    }
}
