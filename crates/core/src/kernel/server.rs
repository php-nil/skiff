//! kernel.sock 的监听：socket 建立、属组/权限、accept 循环（仅 unix）。

use super::{routes, PeerUid};

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::http::Request;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use skiff_app::AppState;
use tokio::net::UnixListener;
use tower::ServiceExt;

pub(super) async fn run(state: Arc<AppState>, run_dir: std::path::PathBuf) -> std::io::Result<()> {
    std::fs::create_dir_all(&run_dir)?;
    let sock = run_dir.join("kernel.sock");
    let _ = std::fs::remove_file(&sock);
    let listener = UnixListener::bind(&sock)?;
    // 插件进程的 gid 是它自己的独立 gid，只能靠**附加组** skiff-plugins 拿到
    // 这个 socket 的组权限。所以这里必须把属组改成运行组，否则插件连上来就是
    // EACCES——所有能力调用全灭，且报错发生在插件侧，很难定位。
    match skiff_plugin::install::shared_gid() {
        Some(gid) => {
            if let Ok(c) = std::ffi::CString::new(sock.to_string_lossy().as_bytes()) {
                // SAFETY: c 是合法 C 字符串；uid 传 -1 表示「不改属主」。
                let rc = unsafe { libc::chown(c.as_ptr(), u32::MAX, gid) };
                if rc != 0 {
                    tracing::warn!(
                        error = %std::io::Error::last_os_error(),
                        "kernel.sock 改属组失败，插件可能连不上"
                    );
                }
            }
            let _ = std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o660));
        }
        None => {
            // 手跑（没走 install.sh，没有运行组）：退化为 0666。
            // 这不是提权缺口——每个连接都要过 SO_PEERCRED(uid↔插件) + Bearer token。
            tracing::warn!(
                "运行组 skiff-plugins 不存在：kernel.sock 退化为 0666（鉴权仍靠 peer uid + token）"
            );
            let _ = std::fs::set_permissions(&sock, std::fs::Permissions::from_mode(0o666));
        }
    }

    let router = routes::build(state);

    tracing::info!(socket = %sock.display(), "kernel.sock 监听中");
    loop {
        let (stream, _addr) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "kernel.sock accept 失败");
                continue;
            }
        };
        let uid = match stream.peer_cred() {
            Ok(cred) => cred.uid(),
            Err(e) => {
                tracing::warn!(error = %e, "取 SO_PEERCRED 失败，拒绝连接");
                continue;
            }
        };
        let router = router.clone();
        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let svc = service_fn(move |req: Request<Incoming>| {
                let router = router.clone();
                async move {
                    let (parts, body) = req.into_parts();
                    let bytes = body
                        .collect()
                        .await
                        .map(|c| c.to_bytes())
                        .unwrap_or_else(|_| Bytes::new());
                    let mut req = Request::from_parts(parts, Body::from(bytes));
                    req.extensions_mut().insert(PeerUid(uid));
                    router.oneshot(req).await
                }
            });
            if let Err(e) = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, svc)
                .await
            {
                tracing::debug!(error = %e, "kernel.sock 连接结束");
            }
        });
    }
}
