//! `com.skiff.sources`：软件源测速与选择。
//!
//! 三件事，按重要性排列：
//!
//! 1. **测速**：判据是「下得动多少」，不是「响应多快」。apk 源下 3 MB 的 ISO
//!    range，镜像源下 3 MB 的真实层 blob（先吃 401 挑战换匿名 token，再从
//!    manifest 解析首层 digest）。小文件测速会被首包延迟与边缘缓存主导——阿里源
//!    在 `APKINDEX` 上测出 4.2 MB/s、同刻真实下载 1.5 KB/s，按那个排序会把用户
//!    送到最慢的源。
//! 2. **可解释**：结果带采样字节数与耗时，UI 必须一起显示，否则「3 MB / 4 ms」
//!    这种缓存命中会被当成带宽。
//! 3. **用户决定**：数字只用于同一页横向比较，不落库、不自动改配置。apk 源写
//!    `<runtime>/cache/apk/.skiff-mirror`（安装器已读此文件），镜像源写
//!    `/etc/docker/daemon.json` 后 `systemctl reload docker`（SIGHUP，不动容器）。

use std::io::Read;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use skiff_plugin_common as common;
use skiff_sdk::{Plugin, RpcError};

const DAEMON_JSON: &str = "/etc/docker/daemon.json";
/// 安装器的源缓存：内容就是两行 `repositories`，24 小时内直接采用。
const APK_CACHE: &str = "/www/runtime/cache/apk/.skiff-mirror";
/// 首行标记 = 用户显式选择，安装器据此**跳过 24 小时过期重测**（见 prelude）。
const APK_FIXED_MARKER: &str = "# skiff-fixed";
const SYSTEMCTL: &str = "/usr/bin/systemctl";
const DOCKER: &str = "/usr/bin/docker";

const SAMPLE_BYTES: u64 = 3 * 1024 * 1024;
/// 兜底下限：低于这个量级连「读到了点什么」都算不上（空响应、被对端截断）。
///
/// 判「不是文件」的首选判据是 [`looks_like_html`]，不是大小——慢源 8 秒读到 53 KB
/// 同样有效。这里只挡真正的空壳。
const MIN_USABLE_BYTES: u64 = 16 * 1024;

/// 首字节像 HTML 就判「这不是文件」。镜像对不存在的仓库回 200 + HTML 错误页，
/// 或者回一个跳转提示页，字节数不大不小，只有看内容分得清。
fn looks_like_html(head: &[u8]) -> bool {
    head.iter()
        .find(|b| !b.is_ascii_whitespace())
        .map(|b| *b == b'<')
        .unwrap_or(false)
}
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

const PROBE_REPO: &str = "library/alpine";
const PROBE_TAG: &str = "latest";
const ACCEPT_MANIFEST: &str = "application/vnd.oci.image.index.v1+json, \
     application/vnd.docker.distribution.manifest.list.v2+json, \
     application/vnd.oci.image.manifest.v1+json, \
     application/vnd.docker.distribution.manifest.v2+json";

fn main() {
    // 调试口：`plugin --probe apk|registry <base>` 不经内核直接测一个源并打印 JSON。
    // 现场排查「这台机器为什么测出这个数」时用；内核拉起插件不带参数，不影响正常运行。
    if std::env::args().nth(1).as_deref() == Some("--probe") {
        let kind = std::env::args().nth(2).unwrap_or_default();
        let base = std::env::args().nth(3).unwrap_or_default();
        let out = if kind == "registry" {
            probe_registry(&base)
        } else {
            probe_apk(&base)
        };
        println!("{out}");
        return;
    }
    let plugin = match Plugin::from_env() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("启动失败: {e}");
            std::process::exit(1);
        }
    };
    let p = plugin.clone();
    if let Err(e) = plugin.serve(move |method, params| match method {
        "status" => Ok(status(&p)),
        "probe" => Ok(probe(&params)),
        "probe.all" => Ok(probe_all(&params)),
        "apply.apk" => apply_apk(&p, &params),
        "apply.registry" => apply_registry(&p, &params),
        other => Err(RpcError::method_not_found(other)),
    }) {
        eprintln!("服务异常退出: {e}");
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------- 候选与状态

/// 候选表是数据，不含任何速度数字：清单与实测必须分开，否则会变成「发布结论」。
fn apk_candidates() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        (
            "dl-cdn",
            "Alpine 官方 CDN",
            "https://dl-cdn.alpinelinux.org/alpine",
        ),
        (
            "tuna",
            "清华 TUNA",
            "https://mirrors.tuna.tsinghua.edu.cn/alpine",
        ),
        ("ustc", "中科大 USTC", "https://mirrors.ustc.edu.cn/alpine"),
        ("aliyun", "阿里云", "https://mirrors.aliyun.com/alpine"),
        (
            "tencent",
            "腾讯云",
            "https://mirrors.cloud.tencent.com/alpine",
        ),
    ]
}

fn registry_candidates() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("daocloud", "DaoCloud", "https://docker.m.daocloud.io"),
        ("1ms", "1ms.run", "https://docker.1ms.run"),
        ("xuanyuan", "轩辕镜像", "https://docker.xuanyuan.me"),
        ("dockerproxy", "dockerproxy", "https://dockerproxy.net"),
    ]
}

fn status(plugin: &Plugin) -> Value {
    let mut out = json!({
        "apk": apk_candidates().iter().map(|(id, name, base)| json!({
            "id": id, "name": name, "base": base
        })).collect::<Vec<_>>(),
        "registry": registry_candidates().iter().map(|(id, name, base)| json!({
            "id": id, "name": name, "base": base
        })).collect::<Vec<_>>(),
        "arch": std::env::consts::ARCH,
    });

    // 当前生效的 apk 源：读安装器缓存，读不到就是「跟随镜像默认源」。
    out["apk_current"] = match plugin.fs_host_read_text(APK_CACHE) {
        Ok(text) => json!(apk_base_of(&text)),
        Err(_) => Value::Null,
    };
    out["apk_fixed"] = json!(plugin
        .fs_host_read_text(APK_CACHE)
        .map(|t| t.contains(APK_FIXED_MARKER))
        .unwrap_or(false));

    // 当前生效的镜像源：读 daemon.json；读失败不回退到「空配置」，否则一保存
    // 就会把 data-root / log-driver 等既有键整文件冲掉。
    out["registry_current"] = match plugin.fs_host_read_text(DAEMON_JSON) {
        Ok(text) => json!(parse_mirrors_of(&text)),
        Err(_) => Value::Null,
    };
    out
}

fn parse_mirrors_of(text: &str) -> Vec<String> {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v.get("registry-mirrors").cloned())
        .and_then(|v| v.as_array().cloned())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------- 测速入口

fn probe(params: &Value) -> Value {
    let kind = common::param_str(params, "kind");
    let base = common::param_str(params, "base");
    if base.trim().is_empty() {
        return probe_error(&base, "缺少源地址");
    }
    if kind == "registry" {
        probe_registry(&base)
    } else {
        probe_apk(&base)
    }
}

/// 并行测所有候选：一个死源不该把整页拖满 8 秒。
fn probe_all(params: &Value) -> Value {
    let kind = common::param_str(params, "kind");
    let mut bases: Vec<String> = params
        .get("bases")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if bases.is_empty() {
        bases = if kind == "registry" {
            registry_candidates()
                .iter()
                .map(|c| c.2.to_string())
                .collect()
        } else {
            apk_candidates().iter().map(|c| c.2.to_string()).collect()
        };
    }
    let registry = kind == "registry";
    // 同时只测 2 个：4 路并行时实测有源挂在 `tls connection init failed:
    // Resource temporarily unavailable`，单独重测却正常——并发制造的假故障比慢更坏
    // （用户会以为源坏了）。2 路在测试机上稳定，4 个候选最坏 2 轮 × 8 秒。
    const PARALLEL: usize = 2;
    let mut results: Vec<Value> = Vec::with_capacity(bases.len());
    for chunk in bases.chunks(PARALLEL) {
        let part: Vec<Value> = std::thread::scope(|scope| {
            let handles: Vec<_> = chunk
                .iter()
                .map(|base| {
                    scope.spawn(move || {
                        if registry {
                            probe_registry(base)
                        } else {
                            probe_apk(base)
                        }
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join()
                        .unwrap_or_else(|_| probe_error("", "探测线程异常退出"))
                })
                .collect()
        });
        results.extend(part);
    }
    json!(results)
}

/// 一次采样。`kind` 说明测的是哪个文件，UI 必须显示——3 MB 的 ISO 与 0.5 MB 的
/// 包索引不能直接比大小，用户得知道这个数字是怎么来的。
struct Sample {
    kind: &'static str,
    bytes: u64,
    elapsed: Duration,
    via_token: bool,
}

fn probe_error(base: &str, message: &str) -> Value {
    json!({
        "base": base,
        "ok": false,
        "sample": Value::Null,
        "bytes": 0,
        "elapsed_ms": 0,
        "bytes_per_sec": 0,
        "via_token": false,
        "error": message,
    })
}

fn sample_done(base: &str, sample: &Sample) -> Value {
    let secs = sample.elapsed.as_secs_f64().max(0.001);
    json!({
        "base": base,
        "ok": true,
        "sample": sample.kind,
        "bytes": sample.bytes,
        "elapsed_ms": sample.elapsed.as_millis() as u64,
        "bytes_per_sec": (sample.bytes as f64 / secs) as u64,
        "via_token": sample.via_token,
        "error": Value::Null,
    })
}

// ---------------------------------------------------------------- apk 源

/// 展示用：从缓存文件里取出首条生效的 `repositories` 行（跳过 `#` 注释行）。
///
/// 固定态里这行是源根（`https://.../alpine`），自动态里是完整路径
/// （`https://.../alpine/v3.22/main`）。界面按前缀和候选地址匹配。
fn apk_base_of(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .unwrap_or_default()
        .to_string()
}

/// 固定态缓存体的唯一写法：标记行 + 源根。
///
/// **不写分支、不写 arch**：分支由容器按自己的 `/etc/alpine-release` 拼，
/// 写死分支会在镜像同步滞后时把用户钉在一个 404 路径上。
fn fixed_cache_body(base: &str) -> String {
    format!("{APK_FIXED_MARKER}\n{}\n", base.trim_end_matches('/'))
}

/// 取一个「足够大」的真实文件来测吞吐：`latest-stable/releases/<arch>/` 下的
/// virt ISO（3 MB range）。
///
/// **不能用 `/last-release` 找版本**：该文件在真实源上已经不存在（实测 dl-cdn /
/// 阿里 / 腾讯 / 中科大全部 404），据此实现会得到「所有源都不可用」。也不能把版本
/// 写死：镜像同步滞后时路径直接 404。因此版本从镜像自己的清单里读：
///
/// 1. `<base>/latest-stable/releases/<arch>/latest-releases.yaml` —— 官方与各大镜像
///    都提供的固定入口，内容随分支滚动；
/// 2. 只同步了某个分支的内网镜像没有 `latest-stable`：退化为扫 `<base>/` 的目录索引
///    取最高分支，再用该分支的 `latest-releases.yaml`。
///
/// 两处都失败才判不可用——但**「应用」不受此影响**（见 [`apply_apk`]）：写源不依赖
/// 版本发现，否则内网镜像会因为「测不出速」而连选都不能选。
fn iso_url(agent: &ureq::Agent, base: &str, arch: &str) -> Result<String, String> {
    let base = base.trim_end_matches('/');
    let mut dirs = vec![format!("{base}/latest-stable/releases/{arch}")];
    if let Some(branch) = latest_branch(agent, base) {
        dirs.push(format!("{base}/{branch}/releases/{arch}"));
    }
    let mut last_err = String::from("找不到可测速的 ISO");
    for dir in dirs {
        // **先看目录里真实存在哪个文件**：镜像的 `latest-releases.yaml` 会先于文件更新
        // （阿里实测：清单写 3.24.4，目录里只有 3.24.0），照清单拼地址就是 404 / 拒连。
        match iso_from_index(agent, &dir, arch) {
            Ok(url) => return Ok(url),
            Err(e) => last_err = e,
        }
        match virt_iso_in(agent, &dir, arch) {
            Ok(url) => return Ok(url),
            Err(e) => last_err = format!("{last_err}；清单：{e}"),
        }
    }
    Err(last_err)
}

/// 目录索引里版本最高的 `alpine-virt*-<arch>.iso`。
fn iso_from_index(agent: &ureq::Agent, dir: &str, arch: &str) -> Result<String, String> {
    let resp = agent
        .get(&format!("{dir}/"))
        .timeout(PROBE_TIMEOUT)
        .call()
        .map_err(|e| format!("目录不可读（{}）", transport_text(e)))?;
    let mut body = String::new();
    let _ = resp
        .into_reader()
        .take(256 * 1024)
        .read_to_string(&mut body);
    let suffix = format!("-{arch}.iso");
    let mut best: Option<(Vec<u32>, String)> = None;
    for quoted in body.split("href=\"").skip(1) {
        let name = match quoted.split('"').next() {
            Some(n) => n,
            None => continue,
        };
        if !name.starts_with("alpine-virt") || !name.ends_with(&suffix) {
            continue;
        }
        let ver: Vec<u32> = name
            .trim_start_matches("alpine-virt-")
            .trim_end_matches(&suffix)
            .split('.')
            .filter_map(|p| p.parse().ok())
            .collect();
        if ver.is_empty() {
            continue;
        }
        if best.as_ref().is_none_or(|(b, _)| ver > *b) {
            best = Some((ver, name.to_string()));
        }
    }
    best.map(|(_, name)| format!("{dir}/{name}"))
        .ok_or_else(|| format!("{dir} 目录里没有 alpine-virt ISO"))
}

/// 从 `latest-releases.yaml` 里取 virt ISO 的文件名。只认 `alpine-virt*.iso` 且
/// 结尾匹配本机架构，避免把 minirootfs 之类的小文件当成测速样本。
fn virt_iso_in(agent: &ureq::Agent, dir: &str, arch: &str) -> Result<String, String> {
    let url = format!("{dir}/latest-releases.yaml");
    let resp = agent
        .get(&url)
        .timeout(PROBE_TIMEOUT)
        .call()
        .map_err(|e| format!("清单不可读（{}）", transport_text(e)))?;
    let mut body = String::new();
    let _ = resp.into_reader().take(64 * 1024).read_to_string(&mut body);
    let suffix = format!("-{arch}.iso");
    for line in body.lines() {
        let line = line.trim();
        let Some(name) = line.strip_prefix("file:") else {
            continue;
        };
        let name = name.trim();
        if name.starts_with("alpine-virt") && name.ends_with(&suffix) {
            return Ok(format!("{dir}/{name}"));
        }
    }
    Err("清单里没有 alpine-virt ISO".to_string())
}

/// 目录索引里最高的 `vX.Y` 分支。只用于「镜像没同步 latest-stable」的兜底。
fn latest_branch(agent: &ureq::Agent, base: &str) -> Option<String> {
    let resp = agent
        .get(&format!("{base}/"))
        .timeout(PROBE_TIMEOUT)
        .call()
        .ok()?;
    let mut body = String::new();
    let _ = resp
        .into_reader()
        .take(256 * 1024)
        .read_to_string(&mut body);
    let mut best: Option<(u32, u32, String)> = None;
    for token in body.split("v") {
        let digits: String = token
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        let mut parts = digits.split('.');
        let (Some(major), Some(minor)) = (parts.next(), parts.next()) else {
            continue;
        };
        let (Ok(major), Ok(minor)) = (major.parse::<u32>(), minor.parse::<u32>()) else {
            continue;
        };
        let name = format!("v{major}.{minor}");
        if best
            .as_ref()
            .is_none_or(|(a, b, _)| (major, minor) > (*a, *b))
        {
            best = Some((major, minor, name));
        }
    }
    best.map(|(_, _, name)| name)
}

fn probe_apk(base: &str) -> Value {
    let base = base.trim().trim_end_matches('/').to_string();
    match apk_sample(&base) {
        Ok(sample) => sample_done(&base, &sample),
        Err(e) => probe_error(&base, &e),
    }
}

fn apk_sample(base: &str) -> Result<Sample, String> {
    let agent = agent();
    let arch = std::env::consts::ARCH;
    let iso_err = match iso_url(&agent, base, arch) {
        Ok(url) => match read_range(&agent, &url, None) {
            Ok((bytes, elapsed)) => {
                return Ok(Sample {
                    kind: "iso",
                    bytes,
                    elapsed,
                    via_token: false,
                })
            }
            Err(e) => e,
        },
        Err(e) => e,
    };
    // ISO 不是每个镜像都真能下：阿里的 ISO 会 302 到 `iso-osm.mirrors.aliyuncs.com`，
    // 那个主机在普通网络里不可达。此时退到 `APKINDEX.tar.gz`——apk 装包时**第一个**
    // 要拉的就是它，任何可用镜像都必然有。样本约 0.5 MB，界面上会标明「索引」。
    index_sample(&agent, base, arch)
        .map_err(|e| format!("ISO 不可用（{iso_err}）；包索引也不可用（{e}）"))
}

/// 分支的包索引：`<base>/vX.Y/main/<arch>/APKINDEX.tar.gz`。实测约 0.5 MB。
fn index_sample(agent: &ureq::Agent, base: &str, arch: &str) -> Result<Sample, String> {
    let mut urls = Vec::new();
    if let Some(branch) = latest_branch(agent, base) {
        urls.push(format!("{base}/{branch}/main/{arch}/APKINDEX.tar.gz"));
    }
    urls.push(format!("{base}/latest-stable/main/{arch}/APKINDEX.tar.gz"));
    let mut last = String::from("没有可读的包索引");
    for url in urls {
        match read_range(agent, &url, None) {
            Ok((bytes, elapsed)) => {
                return Ok(Sample {
                    kind: "apkindex",
                    bytes,
                    elapsed,
                    via_token: false,
                })
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// 「应用」只写源根（不写分支、不联网）：分支由容器用**自己的** `/etc/alpine-release`
/// 拼接，镜像同步滞后或内网只有一个分支时也不会拼错。详见 `installer_prelude()`。
fn apply_apk(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let base = common::param_str(params, "base")
        .trim()
        .trim_end_matches('/')
        .to_string();
    if !(base.starts_with("http://") || base.starts_with("https://")) {
        return Err(RpcError::invalid_params(
            "源地址必须以 http:// 或 https:// 开头",
        ));
    }
    if base.contains(char::is_whitespace) {
        return Err(RpcError::invalid_params("源地址不能含空白字符"));
    }
    let body = fixed_cache_body(&base);
    plugin
        .fs_host_write_text(APK_CACHE, &body)
        .map_err(common::rpc_err)?;
    Ok(json!({
        "ok": true,
        "written": APK_CACHE,
        "base": base,
        "note": "已固定为该源（不再被自动探测覆盖）；新建运行环境时生效，已存在的环境需重建",
    }))
}

// ---------------------------------------------------------------- 镜像源

fn probe_registry(base: &str) -> Value {
    let base = base.trim().trim_end_matches('/').to_string();
    match registry_sample(&base) {
        Ok(sample) => sample_done(&base, &sample),
        Err(e) => probe_error(&base, &e),
    }
}

fn registry_sample(base: &str) -> Result<Sample, String> {
    let agent = agent();
    let manifest_url = format!("{base}/v2/{PROBE_REPO}/manifests/{PROBE_TAG}");

    let mut token: Option<String> = None;
    let mut via_token = false;
    let manifest = match get_json(&agent, &manifest_url, None) {
        Ok(v) => v,
        Err(ProbeError::Auth(challenge)) => {
            let t = fetch_token(&agent, &challenge, PROBE_REPO)?;
            let v = get_json(&agent, &manifest_url, Some(&t)).map_err(|e| e.text())?;
            via_token = true;
            token = Some(t);
            v
        }
        Err(e) => return Err(e.text()),
    };

    // `alpine:latest` 通常是多架构 index，要再取一次子清单才是层列表。
    let digest = match first_layer(&manifest) {
        Some(d) => d,
        None => {
            let child = child_for_amd64(&manifest)
                .ok_or_else(|| "清单既无 layers，也没有 linux/amd64 子清单".to_string())?;
            let child_url = format!("{base}/v2/{PROBE_REPO}/manifests/{child}");
            let child_manifest =
                get_json(&agent, &child_url, token.as_deref()).map_err(|e| e.text())?;
            first_layer(&child_manifest).ok_or_else(|| "子清单无 layers".to_string())?
        }
    };
    let blob_url = format!("{base}/v2/{PROBE_REPO}/blobs/{digest}");
    let (bytes, elapsed) = read_range(&agent, &blob_url, token.as_deref())?;
    Ok(Sample {
        kind: "blob",
        bytes,
        elapsed,
        via_token,
    })
}

fn apply_registry(plugin: &Plugin, params: &Value) -> Result<Value, RpcError> {
    let raw = common::param_str(params, "mirrors");
    let mirrors = parse_mirrors(&raw).map_err(RpcError::invalid_params)?;

    // 读不到就中止：按空配置合并会整文件覆盖，把 data-root / log-driver 冲掉。
    let existing = plugin.fs_host_read_text(DAEMON_JSON).map_err(|_| {
        RpcError::internal(
            "读不到 /etc/docker/daemon.json。请先在「设置 → Docker」保存一次镜像源，或确认文件权限",
        )
    })?;
    let merged = merge_daemon_json(&existing, &mirrors).map_err(RpcError::invalid_params)?;
    if merged.trim() == existing.trim() {
        return Ok(json!({ "ok": true, "written": false, "note": "镜像源未变化" }));
    }

    plugin
        .fs_host_write_text(DAEMON_JSON, &merged)
        .map_err(common::rpc_err)?;

    // SIGHUP 热重载：不重启进程、不动运行中的容器。
    let reload = plugin.host_exec(SYSTEMCTL, &["reload", "docker"]);
    if let Err(e) = reload {
        // 写回原文，避免「文件改了但没生效」这种半截状态。
        let _ = plugin.fs_host_write_text(DAEMON_JSON, &existing);
        return Err(common::rpc_err(e));
    }

    match effective_mirrors(plugin) {
        Ok(actual) if same_mirrors(&actual, &mirrors) => Ok(json!({
            "ok": true,
            "written": true,
            "applied": "reload",
            "note": "已生效（热重载），运行中的容器未受影响",
        })),
        Ok(actual) => {
            let _ = plugin.fs_host_write_text(DAEMON_JSON, &existing);
            let _ = plugin.host_exec(SYSTEMCTL, &["reload", "docker"]);
            Err(RpcError::internal(format!(
                "重载后 Docker 未切换到新镜像源（当前 {} 条），已还原",
                actual.len()
            )))
        }
        Err(e) => {
            let _ = plugin.fs_host_write_text(DAEMON_JSON, &existing);
            let _ = plugin.host_exec(SYSTEMCTL, &["reload", "docker"]);
            Err(RpcError::internal(format!(
                "无法确认是否生效（{e}），已还原为原配置"
            )))
        }
    }
}

fn effective_mirrors(plugin: &Plugin) -> Result<Vec<String>, String> {
    let out = plugin
        .host_exec(
            DOCKER,
            &["info", "--format", "{{json .RegistryConfig.Mirrors}}"],
        )
        .map_err(|e| e.to_string())?;
    if out.get("success").and_then(Value::as_bool) != Some(true) {
        return Err(format!(
            "docker info 失败：{}",
            out.get("stderr").and_then(Value::as_str).unwrap_or("")
        ));
    }
    let text = out.get("stdout").and_then(Value::as_str).unwrap_or("");
    let v: Value = serde_json::from_str(text.trim()).map_err(|e| format!("解析失败：{e}"))?;
    Ok(v.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default())
}

fn parse_mirrors(raw: &str) -> Result<Vec<String>, String> {
    let list: Vec<String> = raw
        .split(|c: char| c == ',' || c == '，' || c.is_whitespace())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| v.trim_end_matches('/').to_string())
        .collect();
    for m in &list {
        if !(m.starts_with("http://") || m.starts_with("https://")) {
            return Err(format!("镜像源必须以 http:// 或 https:// 开头：{m}"));
        }
    }
    Ok(list)
}

/// 合并只替换 `registry-mirrors`，其它键原样保留。
fn merge_daemon_json(existing: &str, mirrors: &[String]) -> Result<String, String> {
    let mut root: Value = if existing.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(existing).map_err(|e| format!("daemon.json 不是合法 JSON：{e}"))?
    };
    let obj = root
        .as_object_mut()
        .ok_or_else(|| "daemon.json 顶层必须是对象".to_string())?;
    if mirrors.is_empty() {
        obj.remove("registry-mirrors");
    } else {
        obj.insert(
            "registry-mirrors".to_string(),
            Value::Array(mirrors.iter().map(|m| Value::String(m.clone())).collect()),
        );
    }
    serde_json::to_string_pretty(&root).map_err(|e| format!("序列化失败：{e}"))
}

fn same_mirrors(actual: &[String], want: &[String]) -> bool {
    fn norm(s: &str) -> String {
        s.trim().trim_end_matches('/').to_ascii_lowercase()
    }
    actual.len() == want.len() && actual.iter().zip(want).all(|(a, b)| norm(a) == norm(b))
}

// ---------------------------------------------------------------- HTTP

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(PROBE_TIMEOUT)
        .timeout_connect(Duration::from_secs(4))
        .redirects(3)
        .build()
}

fn transport_text(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, _) => format!("HTTP {code}"),
        ureq::Error::Transport(t) => t.to_string(),
    }
}

/// 读满 [`SAMPLE_BYTES`] 或读尽；中途超时按已读字节结算——慢源也要给出数字。
fn read_range(
    agent: &ureq::Agent,
    url: &str,
    token: Option<&str>,
) -> Result<(u64, Duration), String> {
    let mut req = agent
        .get(url)
        .set("Range", &format!("bytes=0-{}", SAMPLE_BYTES - 1))
        .timeout(PROBE_TIMEOUT);
    if let Some(t) = token {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    let resp = match req.call() {
        Ok(r) => r,
        Err(ureq::Error::Status(code, _)) => return Err(format!("下载请求 HTTP {code}")),
        Err(ureq::Error::Transport(e)) => return Err(e.to_string()),
    };
    let started = Instant::now();
    let mut reader = resp.into_reader().take(SAMPLE_BYTES);
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    let mut head = Vec::new();
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if head.len() < 512 {
                    head.extend_from_slice(&buf[..n.min(512 - head.len())]);
                }
                total += n as u64;
            }
            Err(_) => break,
        }
    }
    let elapsed = started.elapsed();
    // 判「不是文件」要看**内容**，不能只看大小：慢源 8 秒只读到 53 KB 是有效观测
    // （约 6.6 KB/s），按大小判会把它说成故障；而镜像回 200 + HTML 错误页只有几 KB，
    // 按大小判又可能漏。所以直接看首字节是不是 HTML。
    if looks_like_html(&head) {
        return Err(format!(
            "该地址返回 HTML 而不是文件（{total} 字节），可能未代理该仓库"
        ));
    }
    if total < MIN_USABLE_BYTES {
        return Err(format!(
            "只读到 {total} 字节（需 ≥{MIN_USABLE_BYTES}），判为不可用"
        ));
    }
    Ok((total, elapsed))
}

enum ProbeError {
    Auth(String),
    Other(String),
}

impl ProbeError {
    fn text(self) -> String {
        match self {
            ProbeError::Auth(ch) => format!("需要认证但未能取得 token：{ch}"),
            ProbeError::Other(e) => e,
        }
    }
}

fn get_json(agent: &ureq::Agent, url: &str, token: Option<&str>) -> Result<Value, ProbeError> {
    let mut req = agent.get(url).set("Accept", ACCEPT_MANIFEST);
    if let Some(t) = token {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    let resp = match req.call() {
        Ok(r) => r,
        Err(ureq::Error::Status(401, resp)) => {
            return Err(ProbeError::Auth(
                resp.header("www-authenticate").unwrap_or("").to_string(),
            ))
        }
        Err(ureq::Error::Status(code, _)) => return Err(ProbeError::Other(format!("HTTP {code}"))),
        Err(ureq::Error::Transport(e)) => return Err(ProbeError::Other(e.to_string())),
    };
    let mut body = Vec::new();
    // manifest 正常几 KB；上限防 mirror 回 HTML 错误页把内存吃满。
    let _ = resp.into_reader().take(512 * 1024).read_to_end(&mut body);
    serde_json::from_slice(&body)
        .map_err(|e| ProbeError::Other(format!("响应不是 JSON（{e}），可能未代理该仓库")))
}

fn fetch_token(agent: &ureq::Agent, challenge: &str, repo: &str) -> Result<String, String> {
    let realm = challenge_field(challenge, "realm")
        .ok_or_else(|| format!("认证挑战缺少 realm：{challenge}"))?;
    let mut url = format!("{realm}?scope=repository:{repo}:pull");
    if let Some(service) = challenge_field(challenge, "service") {
        url = format!("{realm}?service={service}&scope=repository:{repo}:pull");
    }
    let resp = agent
        .get(&url)
        .timeout(PROBE_TIMEOUT)
        .call()
        .map_err(transport_text)?;
    let mut body = Vec::new();
    let _ = resp.into_reader().take(64 * 1024).read_to_end(&mut body);
    let v: Value =
        serde_json::from_slice(&body).map_err(|e| format!("token 响应不是 JSON：{e}"))?;
    v["token"]
        .as_str()
        .or_else(|| v["access_token"].as_str())
        .map(str::to_string)
        .ok_or_else(|| "token 响应里没有 token 字段".to_string())
}

fn challenge_field(challenge: &str, key: &str) -> Option<String> {
    let lower = challenge.to_ascii_lowercase();
    let at = lower.find(&format!("{key}="))? + key.len() + 1;
    let rest = challenge.get(at..)?.trim_start();
    if let Some(stripped) = rest.strip_prefix('"') {
        return stripped.find('"').map(|end| stripped[..end].to_string());
    }
    rest.split([',', ' '])
        .next()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn first_layer(manifest: &Value) -> Option<String> {
    manifest["layers"]
        .as_array()?
        .first()?
        .get("digest")?
        .as_str()
        .map(str::to_string)
}

fn child_for_amd64(index: &Value) -> Option<String> {
    index["manifests"]
        .as_array()?
        .iter()
        .find(|m| {
            m["platform"]["os"].as_str() == Some("linux")
                && m["platform"]["architecture"].as_str() == Some("amd64")
        })?
        .get("digest")?
        .as_str()
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_pages_are_not_samples() {
        assert!(looks_like_html(b"  \n<!DOCTYPE html>"));
        assert!(looks_like_html(b"<html><body>404</body></html>"));
        // gzip（APKINDEX 与镜像层）、ISO 9660 都不是 HTML
        assert!(!looks_like_html(&[0x1f, 0x8b, 0x08, 0x00]));
        assert!(!looks_like_html(b"\x00\x00CD001"));
        assert!(!looks_like_html(b""));
    }

    #[test]
    fn fixed_cache_body_has_marker_and_bare_base() {
        let body = fixed_cache_body("https://mirror.example/alpine/");
        assert_eq!(
            body, "# skiff-fixed\nhttps://mirror.example/alpine\n",
            "固定态只写源根：分支由容器自己拼"
        );
        // 写 arch 会被 apk 再拼一次，变成 .../main/x86_64/x86_64 直接全盘失败。
        assert!(!body.contains("x86_64"));
        assert!(!body.contains("/v3."));
    }

    #[test]
    fn fixed_marker_is_not_mistaken_for_a_source() {
        let body = fixed_cache_body("https://mirror.example/alpine");
        // 首行是注释标记，读「当前源」时必须跳过它，否则界面会显示 `# skiff-fixed`。
        assert_eq!(apk_base_of(&body), "https://mirror.example/alpine");
        assert!(body.contains(APK_FIXED_MARKER));
        assert_eq!(apk_base_of(""), "");
    }

    #[test]
    fn merge_keeps_other_keys() {
        let existing = r#"{"log-driver":"local","registry-mirrors":["https://old.example"]}"#;
        let out = merge_daemon_json(existing, &["https://new.example".into()]).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["log-driver"], "local");
        assert_eq!(v["registry-mirrors"][0], "https://new.example");
    }

    #[test]
    fn invalid_daemon_json_is_rejected_not_overwritten() {
        assert!(merge_daemon_json("{oops", &["https://a.example".into()]).is_err());
    }

    #[test]
    fn mirror_comparison_ignores_trailing_slash_and_case() {
        let want = vec!["https://A.example".to_string()];
        assert!(same_mirrors(&["https://a.example/".to_string()], &want));
        assert!(!same_mirrors(&[], &want));
    }

    #[test]
    fn challenge_and_layer_parsing() {
        let ch = r#"Bearer realm="https://auth.docker.io/token",service="registry.docker.io""#;
        assert_eq!(
            challenge_field(ch, "realm").as_deref(),
            Some("https://auth.docker.io/token")
        );
        let index = json!({ "manifests": [
            { "digest": "sha256:arm", "platform": { "os": "linux", "architecture": "arm64" } },
            { "digest": "sha256:amd", "platform": { "os": "linux", "architecture": "amd64" } }
        ]});
        assert_eq!(child_for_amd64(&index).as_deref(), Some("sha256:amd"));
    }
}
