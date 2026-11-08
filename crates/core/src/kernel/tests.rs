//! kernel.sock 的回归用例：起服务、绑失败降级、fs 能力面授权边界。

use super::*;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use skiff_app::WebConfig;
use skiff_docker::{DockerService, Form, Policy, Tier};
use skiff_plugin::{Supervisor, SupervisorConfig};
use skiff_store::{Db, Grant, PluginRecord};
use tower::ServiceExt;

use skiff_plugin::capability::{self as cap, PluginId};

fn test_state(data_dir: std::path::PathBuf) -> Arc<AppState> {
    let db = Arc::new(Db::open_in_memory().unwrap());
    let docker = Arc::new(DockerService::connect("/nonexistent/docker.sock"));
    let sup = Arc::new(Supervisor::new(
        SupervisorConfig::new(data_dir.clone(), data_dir.join("run")),
        db.clone(),
    ));
    let cfg = WebConfig {
        web_root: data_dir.join("www"),
        trash_retention_days: 30,
        config_path: data_dir.join("config.toml"),
        listen: "127.0.0.1:18080".parse().unwrap(),
        data_dir,
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
        allow_unsigned: false,
        allow_reduced_sandbox: true,
        policy: Policy::new(Tier::Basic, Form::Native, vec![]).unwrap(),
        session: skiff_identity::SessionLimits::default(),
        login_concurrency: 1,
        ext_exec_timeout: std::time::Duration::from_secs(15 * 60),
        ext_batch_timeout: std::time::Duration::from_secs(30 * 60),
    };
    AppState::new(db, docker, sup, cfg, "csrf-test-secret".into(), "test")
}

fn healthz_req() -> Request<Body> {
    Request::builder()
        .uri("/healthz")
        .body(Body::empty())
        .unwrap()
}

/// 回归：`kernel::spawn` 必须立即返回、真后台跑 accept 循环，且不拖死 HTTP。
///
/// 历史缺陷：unix 版曾是常驻 accept 循环，被 `main` `.await` 后 HTTP serve
/// 永远排不到（Windows stub 立即返回，掩盖了整个问题，最后是冒烟抓到的）。
/// 这里用独立线程 + 超时把「阻塞调用方」变成 2s 内的红灯，而不是挂死测试。
#[tokio::test]
async fn spawn_does_not_block_and_http_serves_alongside() {
    let tmp = tempfile::tempdir().unwrap();
    let run_dir = tmp.path().join("run");
    let state = test_state(tmp.path().to_path_buf());

    let handle = tokio::runtime::Handle::current();
    let (tx, rx) = std::sync::mpsc::channel();
    {
        let st = state.clone();
        let rd = run_dir.clone();
        std::thread::spawn(move || {
            let _guard = handle.enter();
            spawn(st, rd);
            tx.send(()).ok();
        });
    }
    rx.recv_timeout(Duration::from_secs(2))
        .expect("kernel::spawn 必须立即返回，不得阻塞调用方");

    // HTTP 侧必须同时可用（oneshot 不占端口；真 TCP 探测由冒烟 job 负责）
    let resp = skiff_web::router(state)
        .oneshot(healthz_req())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    #[cfg(unix)]
    {
        let sock = run_dir.join("kernel.sock");
        let mut ok = false;
        for _ in 0..20 {
            if tokio::net::UnixStream::connect(&sock).await.is_ok() {
                ok = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(ok, "kernel.sock 2s 内应可连接: {}", sock.display());
    }
}

/// kernel.sock 绑不上（路径被占）时：不得 panic，HTTP 侧照常服务。
#[tokio::test]
async fn kernel_bind_failure_degrades_but_http_stays_up() {
    let tmp = tempfile::tempdir().unwrap();
    let run_dir = tmp.path().join("run");
    std::fs::create_dir_all(&run_dir).unwrap();
    // 用目录占住 socket 路径：remove_file 删不掉目录，bind 必失败
    std::fs::create_dir_all(run_dir.join("kernel.sock")).unwrap();

    let state = test_state(tmp.path().to_path_buf());
    spawn(state.clone(), run_dir);

    let resp = skiff_web::router(state)
        .oneshot(healthz_req())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "kernel 起不来不能拖死 HTTP");
}

// ---- fs 能力面（宿主路径分支）--------------------------------------------
//
// kernel.sock 只在 unix 编得出来（`server` 是 `#[cfg(unix)]`），这些用例也只在
// unix 上跑；Windows 上插件体系整体是 stub（见 `kernel::spawn` 的 not(unix)）。

fn seed_plugin(state: &AppState, id: &str, perms: &[&str], objects: &[&str]) {
    let mut grants = Vec::new();
    for p in perms {
        grants.push(Grant::perm((*p).to_string()));
    }
    for o in objects {
        grants.push(Grant::object((*o).to_string()));
    }
    state
        .db()
        .upsert_plugin(&PluginRecord {
            id: id.into(),
            name: "Demo".into(),
            version: "0.1.0".into(),
            api: "1.0".into(),
            key_id: "0".repeat(64),
            manifest: "{}".into(),
            enabled: true,
            status: "installed".into(),
            status_detail: String::new(),
            active_version: "0.1.0".into(),
            installed_at: 0,
            grants,
        })
        .unwrap();
}

fn fs_req(id: &str, body: serde_json::Value) -> Request<Body> {
    let mut req = Request::builder()
        .method("POST")
        .uri("/")
        .body(Body::from(body.to_string()))
        .unwrap();
    req.extensions_mut().insert(PluginId(id.to_string()));
    req
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

/// 宿主路径必须**两把钥匙**都在：`fs:read` 权限 + 安装时确认的 `path:` 授权根。
#[tokio::test]
async fn host_fs_requires_perm_and_path_grant() {
    let tmp = tempfile::tempdir().unwrap();
    // 数据根与授权根互不包含：授权根落在数据根内时会被 FS-08 保护前缀拦下，
    // 那是另一条用例的语义，这里要验的是「授权根内放行」。
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("skiff.db"), b"db").unwrap();
    let db = data.join("skiff.db").display().to_string();
    let root = tmp.path().join("grant");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("site.conf"), b"server {}").unwrap();
    let abs = root.join("site.conf").display().to_string();

    let state = test_state(data);
    let id = "com.example.demo";

    // 未安装 → 无权限
    let r = cap::fs_read(&*state, fs_req(id, json!({ "path": abs }))).await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);

    // 只有 fs:read，没有 path: 授权 → 仍是 403
    seed_plugin(&state, id, &["fs:read"], &[]);
    let r = cap::fs_read(&*state, fs_req(id, json!({ "path": abs }))).await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);

    // 补上 path: 授权 → 放行
    seed_plugin(
        &state,
        id,
        &["fs:read"],
        &[&format!("path:{}", root.display())],
    );
    let r = cap::fs_read(&*state, fs_req(id, json!({ "path": abs }))).await;
    assert_eq!(r.status(), StatusCode::OK);
    let v = json_body(r).await;
    assert_eq!(v["data"], "server {}");
    assert_eq!(v["size"], 9, "`server {{}}` 共 9 字节");
    assert_eq!(v["eof"], true);

    // 把授权根扩到 `tmp`（覆盖数据根）后，数据根里的 `skiff.db` 仍必须 403：
    // 受保护前缀优先于授权根，读口与写口一致。
    seed_plugin(
        &state,
        id,
        &["fs:read"],
        &[
            &format!("path:{}", root.display()),
            &format!("path:{}", tmp.path().display()),
        ],
    );
    let r = cap::fs_read(&*state, fs_req(id, json!({ "path": db }))).await;
    assert_eq!(
        r.status(),
        StatusCode::FORBIDDEN,
        "数据根的 skiff.db 不可读"
    );
}

/// 写入被限制在授权根内；授权根自身不许删，根内文件可删可建目录。
#[tokio::test]
async fn host_fs_write_is_confined_and_roots_are_protected() {
    let tmp = tempfile::tempdir().unwrap();
    // 同上：授权根必须与数据根分离，否则普通写入会先撞上 FS-08 保护。
    let root = tmp.path().join("grant");
    std::fs::create_dir_all(&root).unwrap();
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let state = test_state(data.clone());
    let id = "com.example.demo";
    seed_plugin(
        &state,
        id,
        &["fs:read", "fs:write", "fs:list", "fs:mkdir", "fs:remove"],
        &[&format!("path:{}", root.display())],
    );

    let target = root.join("new.conf").display().to_string();
    let r = cap::fs_write(&*state, fs_req(id, json!({ "path": target, "data": "x" }))).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert!(root.join("new.conf").exists());

    // 根外写入必须被拒（且不落盘）
    let outside = tmp.path().join("evil.conf");
    let r = cap::fs_write(
        &*state,
        fs_req(
            id,
            json!({ "path": outside.display().to_string(), "data": "x" }),
        ),
    )
    .await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    assert!(!outside.exists());

    // 建目录
    let sub = root.join("sub");
    let r = cap::fs_mkdir(
        &*state,
        fs_req(id, json!({ "path": sub.display().to_string() })),
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);

    // 删授权根本身 → 拒；删根内文件 → 放行
    let r = cap::fs_remove(
        &*state,
        fs_req(id, json!({ "path": root.display().to_string() })),
    )
    .await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    assert!(root.exists(), "授权根不能被插件删掉");

    let r = cap::fs_remove(&*state, fs_req(id, json!({ "path": target }))).await;
    assert_eq!(r.status(), StatusCode::OK);
    assert!(!root.join("new.conf").exists());

    // 授权根扩到 `tmp`（覆盖数据根）后，写数据根仍必须 403 且不落盘。
    seed_plugin(
        &state,
        id,
        &["fs:read", "fs:write", "fs:list", "fs:mkdir", "fs:remove"],
        &[
            &format!("path:{}", root.display()),
            &format!("path:{}", tmp.path().display()),
        ],
    );
    let pwn = data.join("pwn.txt");
    let r = cap::fs_write(
        &*state,
        fs_req(
            id,
            json!({ "path": pwn.display().to_string(), "data": "x" }),
        ),
    )
    .await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN, "数据根不可写");
    assert!(!pwn.exists(), "被拒的写入不得落盘");
}

/// 相对路径仍走插件 `data/`（原有语义不回退），绝对路径没有授权时必须拒。
#[tokio::test]
async fn data_dir_roundtrip_still_uses_relative_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let state = test_state(tmp.path().to_path_buf());
    let id = "com.example.demo";
    seed_plugin(&state, id, &["fs:read", "fs:write"], &[]);

    let r = cap::fs_write(
        &*state,
        fs_req(id, json!({ "path": "notes.json", "data": "{\"a\":1}" })),
    )
    .await;
    assert_eq!(r.status(), StatusCode::OK);

    let r = cap::fs_read(&*state, fs_req(id, json!({ "path": "notes.json" }))).await;
    assert_eq!(r.status(), StatusCode::OK);
    let v = json_body(r).await;
    assert_eq!(v["data"], "{\"a\":1}");

    // 同一插件对绝对路径没有授权 → 拒
    let r = cap::fs_read(&*state, fs_req(id, json!({ "path": "/etc/passwd" }))).await;
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
}
