//! SDK ↔ 内核契约测试：起一个假 kernel（std UDS server），跑通 SDK 全部能力方法。
//!
//! 目的不是测内核实现（真内核要 DB/Docker/root），而是**把路由表钉死**：
//! 任何一端改路径、改鉴权头、改帧格式，这里都会红。
//! 之前的真实缺陷正是「SDK 调了一条内核不存在的路由」，静态阅读没发现，只有
//! 端到端跑一遍才会暴露。
//!
//! 仅在 unix 下运行（UDS 是平台能力）；Windows 开发机上这条测试会被跳过。

#![cfg(unix)]

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{json, Value};
use skiff_sdk::Plugin;
use skiff_wire::routes;

/// 假内核：记录收到的 (path, authorization)，并按路径返回固定响应。
struct FakeKernel {
    seen: Arc<Mutex<Vec<(String, String)>>>,
}

impl FakeKernel {
    fn start(socket: std::path::PathBuf) -> FakeKernel {
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).expect("bind fake kernel");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_for_thread = seen.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let seen = seen_for_thread.clone();
                thread::spawn(move || {
                    let _ = handle(stream, &seen);
                });
            }
        });
        FakeKernel { seen }
    }

    fn paths(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|(p, _)| p.clone())
            .collect()
    }

    fn auths(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|(_, a)| a.clone())
            .collect()
    }
}

fn handle(mut stream: UnixStream, seen: &Arc<Mutex<Vec<(String, String)>>>) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let (head, end) = loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(v) = skiff_wire::parse_request_head(&buf)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?
        {
            break v;
        }
    };
    let mut body = buf[end..].to_vec();
    while body.len() < head.content_length {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    seen.lock().unwrap().push((
        head.path.clone(),
        head.authorization.clone().unwrap_or_default(),
    ));

    let (status, payload): (u16, Value) = match head.path.as_str() {
        routes::CAP_CONFIG => (200, json!({ "site_name": "示例站", "token": "s3cret" })),
        routes::CAP_IDENTITY_SELF => (
            200,
            json!({ "id": "demo", "version": "1.0.0", "api": "1.0", "granted": ["fs:read"] }),
        ),
        routes::CAP_FS_READ => (200, json!({ "data": "hello" })),
        routes::CAP_FS_WRITE => (200, json!({ "ok": true })),
        routes::CAP_FS_LIST => (200, json!({ "entries": ["a.txt"] })),
        routes::CAP_DOCKER_CALL => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            (200, json!({ "result": v }))
        }
        // 内核把目标插件的 RPC `result` 放在信封的 `result` 键里；SDK 负责剥壳。
        routes::CAP_INTEROP_CALL => {
            let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            (
                200,
                json!({
                    "plugin": v["plugin"],
                    "method": v["method"],
                    "result": { "ok": true, "ticket": v["params"]["name"] }
                }),
            )
        }
        routes::CAP_INTEROP_CATALOG => (
            200,
            json!({
                "permission": "interop:call",
                "interfaces": [{ "id": "com.skiff.cron", "name": "计划任务", "methods": [] }]
            }),
        ),
        // 内核当前对 net.apply 一律 501（nftables 收口属 M4 门禁）
        routes::CAP_NET_APPLY => (501, json!({ "error": "not implemented" })),
        other => (404, json!({ "error": format!("no route {other}") })),
    };

    let raw = serde_json::to_vec(&payload).unwrap_or_default();
    stream.write_all(skiff_wire::build_response_head(status, raw.len()).as_bytes())?;
    stream.write_all(&raw)?;
    stream.flush()
}

fn plugin(socket: &std::path::Path) -> Plugin {
    Plugin {
        id: "demo".into(),
        api: "1.0".into(),
        socket: std::env::temp_dir().join("skiff-test-plugin.sock"),
        data_dir: std::env::temp_dir().join("skiff-test-data"),
        cache_dir: std::env::temp_dir().join("skiff-test-cache"),
        run_dir: std::env::temp_dir().join("skiff-test-run"),
        kernel_socket: socket.to_path_buf(),
        granted: vec!["fs:read".into(), "fs:write".into(), "fs:list".into()],
        token: "t0ken".into(),
    }
}

#[test]
fn sdk_capabilities_match_kernel_routes() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("kernel.sock");
    let kernel = FakeKernel::start(sock.clone());
    let p = plugin(&sock);

    let cfg = p
        .config()
        .expect("config 必须返回 200（这正是历史上的 404 点）");
    assert_eq!(cfg.get("site_name").map(|s| s.as_str()), Some("示例站"));
    assert_eq!(
        cfg.get("token").map(|s| s.as_str()),
        Some("s3cret"),
        "secret 项对插件自身可读"
    );

    let id = p.identity().unwrap();
    assert_eq!(id["id"], "demo");

    assert_eq!(p.fs_read("a.txt").unwrap()["data"], "hello");
    assert!(p.fs_write("a.txt", "hi").unwrap()["ok"].as_bool().unwrap());
    assert_eq!(p.fs_list("").unwrap()["entries"][0], "a.txt");

    let r = p.docker_call("containers.list", json!({})).unwrap();
    assert_eq!(r["result"]["verb"], "containers.list");

    // 插件间调用：SDK 直接拿到目标 RPC 的 result（不是内核信封）
    let r = p
        .interop_call(
            "com.skiff.cron",
            "jobs.create",
            json!({ "name": "站点备份" }),
        )
        .unwrap();
    assert_eq!(r["ok"], true);
    assert_eq!(r["ticket"], "站点备份");
    let cat = p.interop_catalog().unwrap();
    assert_eq!(cat["interfaces"][0]["id"], "com.skiff.cron");

    // net 未实现：必须是可读的错误，而不是「响应不是 JSON」
    let err = p.net_apply(json!({})).unwrap_err().to_string();
    assert!(err.contains("501"), "期望 501，实际: {err}");

    // 每个请求都得带上进程级 Bearer token，路由也得是 wire 里声明的那些
    let paths = kernel.paths();
    assert_eq!(
        paths,
        vec![
            routes::CAP_CONFIG.to_string(),
            routes::CAP_IDENTITY_SELF.to_string(),
            routes::CAP_FS_READ.to_string(),
            routes::CAP_FS_WRITE.to_string(),
            routes::CAP_FS_LIST.to_string(),
            routes::CAP_DOCKER_CALL.to_string(),
            routes::CAP_INTEROP_CALL.to_string(),
            routes::CAP_INTEROP_CATALOG.to_string(),
            routes::CAP_NET_APPLY.to_string(),
        ]
    );
    for a in kernel.auths() {
        assert_eq!(a, "Bearer t0ken");
    }
}

#[test]
fn unreachable_kernel_is_an_error_not_a_panic() {
    let p = plugin(std::path::Path::new("/nonexistent/kernel.sock"));
    assert!(p.config().is_err());
}

/// `serve` 是插件侧的服务端：用内核同样的客户端去看它。
#[test]
fn plugin_serve_answers_healthz_and_rpc() {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("plugin.sock");
    let mut p = plugin(&sock);
    // serve 绑的是 Plugin::socket（插件自身 socket），不是 kernel_socket；
    // helper 的参数语义是 kernel_socket，这里显式对齐，否则轮询和 connect 都在等一个永远不会出现的文件。
    p.socket = sock.clone();
    let (tx, rx) = mpsc::channel::<()>();
    let p2 = p.clone();
    thread::spawn(move || {
        let _ = p2.serve(|method, params| match method {
            "echo" => Ok(params),
            other => Err(skiff_sdk::RpcError::method_not_found(other)),
        });
        let _ = tx.send(());
    });

    // 等 socket 就绪（插件侧 serve 是阻塞循环，只能轮询文件出现）
    for _ in 0..200 {
        if sock.exists() {
            break;
        }
        thread::sleep(std::time::Duration::from_millis(10));
    }

    let health = skiff_wire::build_request("GET", routes::PLUGIN_HEALTHZ, 0, None);
    let mut s = UnixStream::connect(&sock).unwrap();
    s.write_all(health.as_bytes()).unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    let resp = skiff_wire::parse_response(&buf).unwrap().unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.json().unwrap()["ok"], true);

    let body = br#"{"jsonrpc":"2.0","id":7,"method":"echo","params":{"x":1}}"#;
    let head = skiff_wire::build_request("POST", routes::PLUGIN_RPC, body.len(), None);
    let mut s = UnixStream::connect(&sock).unwrap();
    s.write_all(head.as_bytes()).unwrap();
    s.write_all(body).unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).unwrap();
    let resp = skiff_wire::parse_response(&buf).unwrap().unwrap();
    let v = resp.json().unwrap();
    assert_eq!(v["id"], 7);
    assert_eq!(v["result"]["x"], 1);

    drop(rx);
}
