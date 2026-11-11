//! 命令行入口：`gen` 生成静态产物、`publish` 生成+签名+严格自检、
//! `check` 做发布前校验、`sync` 从主站同步到镜像目录、`serve` 只读提供产物目录、
//! `api` 发布面提交/审核（默认关闭、独立进程）、`release` 把已审提交落成产物。

use std::collections::{BTreeMap, BTreeSet};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use skiff_server::api;
use skiff_server::build::{build, write};
use skiff_server::check;
use skiff_server::release;
use skiff_server::serve;
use skiff_server::sign::{Signer, ENV_PUBLIC_KEY};
use skiff_server::sync::{self, DirSource, HttpSource};

const HELP: &str = "\
Skiff-server：商店清单校验、静态产物生成、签名发布、镜像同步、静态服务与发布面 API

用法:
  skiff-server gen     [--catalog <dir>] [--schema <dir>] [--out <dir>] [--unsigned]
  skiff-server publish [--catalog <dir>] [--schema <dir>] [--out <dir>] [--prev <dir>]
  skiff-server check   [--catalog <dir>] [--schema <dir>] [--dist <dir>] [--prev <dir>]
                       [--require-signature] [--public-key <hex>]
  skiff-server sync    --from <url|dir> --to <dir> [--channel stable] [--public-key <hex>]
  skiff-server serve   --dist <dir> [--listen 127.0.0.1:8443]
                       [--tls-cert <pem> --tls-key <pem> | --http]
                       [--access-log [<path>]]
  skiff-server api     --state <dir> --authors <json> --reviewers <json>
                       [--catalog <dir>] [--channel stable]
                       [--listen 127.0.0.1:9443] [--allow-remote]
  skiff-server release --state <dir> --catalog <dir> [--schema <dir>] [--channel stable]
                       [--staging-out <dir> | --dist <dir> --swap]
                       [--seq <n>] [--prev <dir>] [--submission <sid>] [--public-key <hex>]

选项:
  --catalog <dir>       源数据目录，默认 deploy/skiff-server/catalog
  --schema <dir>        JSON Schema 目录，默认 deploy/skiff-server/schema
  --out <dir>           gen/publish 的输出目录，默认 target/skiff-server/dist（生成前会清空）
  --unsigned            仅本地演练：产出未签名占位；发布请用 publish
  --dist <dir>          校验已生成的产物目录；缺省则先在内存生成再校验
  --prev <dir>          上一版产物目录，用于拒绝 seq 回退 / release 的 seq 基准
  --require-signature   占位/空签名直接判失败（发布门禁用）
  --public-key <hex>    固定 Ed25519 公钥；缺省读环境变量 SKIFF_SERVER_PUBLIC_KEY
  --from <url|dir>      同步源：https:// 基地址或本地产物目录
  --to <dir>            同步目标镜像目录
  --channel <name>      渠道，默认 stable
  --listen <addr:port>  serve 默认 127.0.0.1:8443；api 默认 127.0.0.1:9443
  --tls-cert <pem>      serve 服务器证书链（默认 https，必须与 --tls-key 成对）
  --tls-key <pem>       serve 服务器私钥（PEM）
  --http                serve 明文模式，仅测试用；与证书参数互斥
  --access-log [<path>] serve 访问日志，默认关闭；给路径则追加写该文件，缺省取值或 `-`
                        写 stderr（systemd 下即 journal）；每请求一行，不影响协议语义
  --state <dir>         api/release 的提交状态目录（必须 0700）
  --authors <json>      作者公钥白名单（必须 0600）
  --reviewers <json>    审核公钥白名单（必须 0600）
  --allow-remote        api 允许监听非回环地址（必须前置 TLS 反代，不推荐）
  --staging-out <dir>   release 只产出 staging，不碰线上目录（推荐给离线发布机/CI）
  --dist <dir>          release 线上产物目录（必须显式 --swap 才换入）
  --swap                release 显式换入 --dist（同机部署属测试姿态）
  --seq <n>             release 由调用方指定 seq；缺省从 --prev/--dist 只读读取 +1
  --submission <sid>    release 只处理一个提交

环境变量:
  SKIFF_SIGNING_KEY     发布私钥种子（64 位十六进制），仅从环境/CI secret 读取
  SKIFF_SERVER_PUBLIC_KEY  客户端固定公钥（64 位十六进制）

退出码: 0 通过，1 失败。
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("错误: {error:#}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("gen") => gen(&args[1..]),
        Some("publish") => publish(&args[1..]),
        Some("check") => check_cmd(&args[1..]),
        Some("sync") => sync_cmd(&args[1..]),
        Some("serve") => serve_cmd(&args[1..]),
        Some("api") => api_cmd(&args[1..]),
        Some("release") => release_cmd(&args[1..]),
        Some("help" | "-h" | "--help") | None => {
            print!("{HELP}");
            Ok(())
        }
        Some(other) => bail!("未知子命令: {other}\n\n{HELP}"),
    }
}

fn gen(args: &[String]) -> Result<()> {
    let (options, flags) = parse(args, &["--unsigned"])?;
    let catalog = path_option(&options, "--catalog", "deploy/skiff-server/catalog");
    let schema = path_option(&options, "--schema", "deploy/skiff-server/schema");
    let out = path_option(&options, "--out", "target/skiff-server/dist");

    let signer = if flags.contains("--unsigned") {
        eprintln!("警告：--unsigned 仅用于本地演练，产物不得发布");
        Signer::Unsigned
    } else {
        let signer = Signer::from_env()?;
        enforce_public_key(&options, &signer)?;
        signer
    };

    let artifact = build(&catalog, &schema, &signer)?;
    write(&artifact, &out)?;
    println!("生成 {} 个文件 → {}", artifact.files.len(), out.display());
    if signer.is_unsigned() {
        println!("注意：当前为未签名占位产物，客户端必须拒绝；发布请用 publish");
    }
    Ok(())
}

/// 发布路径：生成 → 签名 → 严格自检（验签/摘要/seq 不回退）。
fn publish(args: &[String]) -> Result<()> {
    let (options, _) = parse(args, &[])?;
    let catalog = path_option(&options, "--catalog", "deploy/skiff-server/catalog");
    let schema = path_option(&options, "--schema", "deploy/skiff-server/schema");
    let out = path_option(&options, "--out", "target/skiff-server/dist");
    let prev = options.get("--prev").map(Path::new);

    let signer = Signer::from_env()?;
    let public_key = public_key(&options)?.ok_or_else(|| {
        anyhow::anyhow!(
            "缺少固定公钥：请设置环境变量 {ENV_PUBLIC_KEY} 或传 --public-key；\
             发布必须与客户端信任的公钥一致"
        )
    })?;
    let artifact = release::publish(&catalog, &schema, &out, prev, &signer, &public_key)?;
    println!(
        "✓ 发布产物已就绪：{} 个文件 → {}（签名公钥 {}）",
        artifact.files.len(),
        out.display(),
        signer.public_key_hex().unwrap_or_default()
    );
    println!("下一步：按 docs/SKIFF_SERVER.md §14 上传到目录托管；镜像用 `sync` 同步。");
    Ok(())
}

fn check_cmd(args: &[String]) -> Result<()> {
    let (options, flags) = parse(args, &["--require-signature"])?;
    let catalog = path_option(&options, "--catalog", "deploy/skiff-server/catalog");
    let schema = path_option(&options, "--schema", "deploy/skiff-server/schema");
    let dist = options.get("--dist").map(Path::new);
    let prev = options.get("--prev").map(Path::new);
    let key = public_key(&options)?;
    if key.is_some() && dist.is_none() {
        bail!("--public-key 必须配合 --dist 使用（内存生成的产物没有可验证的签名）");
    }
    check::run_with_public_key(
        &catalog,
        &schema,
        dist,
        prev,
        flags.contains("--require-signature"),
        key.as_deref(),
    )
}

fn sync_cmd(args: &[String]) -> Result<()> {
    let (options, _) = parse(args, &[])?;
    let from = options
        .get("--from")
        .ok_or_else(|| anyhow::anyhow!("缺少 --from（https:// 基地址或本地目录）"))?;
    let to = options
        .get("--to")
        .ok_or_else(|| anyhow::anyhow!("缺少 --to（镜像目录）"))?;
    let channel = options
        .get("--channel")
        .cloned()
        .unwrap_or_else(|| "stable".to_string());
    let key =
        public_key(&options)?.ok_or_else(|| anyhow::anyhow!("缺少固定公钥，无法校验主站清单"))?;

    let source: Box<dyn sync::Source> =
        if from.starts_with("https://") || from.starts_with("http://") {
            Box::new(HttpSource::new(from))
        } else {
            Box::new(DirSource::new(from))
        };
    let report = sync::run(source.as_ref(), &channel, Path::new(to), &key)?;
    println!(
        "✓ 同步完成：渠道 {channel}，blob 复用 {} / 新增 {} / 共 {}（{} 字节），清单{}",
        report.blobs_reused,
        report.blobs_fetched,
        report.blobs_total,
        report.bytes_fetched,
        if report.manifest_written {
            "已更新"
        } else {
            "未变化"
        }
    );
    Ok(())
}

fn serve_cmd(args: &[String]) -> Result<()> {
    let args = normalize_access_log(args);
    let (options, flags) = parse(&args, &["--http"])?;
    let dist = options
        .get("--dist")
        .ok_or_else(|| anyhow::anyhow!("缺少 --dist（产物目录，需含 channels/ 与 blobs/）"))?;
    let listen = options
        .get("--listen")
        .cloned()
        .unwrap_or_else(|| "127.0.0.1:8443".to_string());
    let addr: std::net::SocketAddr = listen
        .parse()
        .with_context(|| format!("--listen 不是合法地址: {listen}"))?;
    let cert = options.get("--tls-cert");
    let key = options.get("--tls-key");
    let plaintext = flags.contains("--http");

    if plaintext && (cert.is_some() || key.is_some()) {
        bail!("--http 与 --tls-cert/--tls-key 互斥：要明文就别给证书");
    }
    let tls = if plaintext {
        eprintln!("警告：--http 仅测试用，无 TLS 与证书校验，不要用于任何对外地址");
        None
    } else {
        let (Some(cert), Some(key)) = (cert, key) else {
            bail!(
                "默认 https：请同时提供 --tls-cert 与 --tls-key；确需明文测试请显式加 --http\n\n{HELP}"
            );
        };
        Some(serve::tls_config(Path::new(cert), Path::new(key))?)
    };

    // 访问日志默认关闭：不指定时整条日志路径不存在（不建文件、不写 stderr）。
    let (access_log, access_desc) = match options.get("--access-log") {
        None => (None, None),
        Some(value) if value == "-" => (
            Some(serve::AccessLog::stderr()),
            Some("stderr（systemd 下即 journal）".to_string()),
        ),
        Some(value) => (
            Some(serve::AccessLog::file(Path::new(value))?),
            Some(value.clone()),
        ),
    };

    let listener = TcpListener::bind(addr).with_context(|| format!("监听 {addr} 失败"))?;
    let scheme = if tls.is_some() { "https" } else { "http" };
    println!("skiff-server serve：{scheme}://{addr}（dist={dist}，Ctrl-C 停止）");
    if let Some(desc) = access_desc {
        println!("访问日志：{desc}（每请求一行；写失败不影响响应）");
    }
    if tls.is_some() {
        println!(
            "自签证书部署时，客户端需把签发该证书的 CA 配到 [catalog].ca_bundle（仍强制 https）"
        );
    }
    serve::serve(listener, Path::new(dist), tls, access_log)
}

/// `--access-log` 的取值可省略：裸写等价于 `--access-log -`（写 stderr/journal）。
fn normalize_access_log(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    for (index, arg) in args.iter().enumerate() {
        if arg == "--access-log" {
            let has_value = args
                .get(index + 1)
                .is_some_and(|next| !next.starts_with("--"));
            out.push(if has_value {
                arg.clone()
            } else {
                "--access-log=-".to_string()
            });
        } else {
            out.push(arg.clone());
        }
    }
    out
}

/// 发布面 API：默认关闭、独立进程、默认只绑回环；TLS 由前置反代终止。
fn api_cmd(args: &[String]) -> Result<()> {
    let (options, flags) = parse(args, &["--allow-remote"])?;
    let state = required_path(&options, "--state")?;
    let authors = required_path(&options, "--authors")?;
    let reviewers = required_path(&options, "--reviewers")?;
    let catalog = path_option(&options, "--catalog", "deploy/skiff-server/catalog");
    let channel = options
        .get("--channel")
        .cloned()
        .unwrap_or_else(|| "stable".to_string());
    let listen = options
        .get("--listen")
        .cloned()
        .unwrap_or_else(|| "127.0.0.1:9443".to_string());
    let addr: std::net::SocketAddr = listen
        .parse()
        .with_context(|| format!("--listen 不是合法地址: {listen}"))?;

    let server = api::ApiServer::prepare(
        state,
        authors,
        reviewers,
        catalog,
        channel,
        flags.contains("--allow-remote"),
    )?;
    server.check_listen(&addr)?;

    let listener = TcpListener::bind(addr).with_context(|| format!("监听 {addr} 失败"))?;
    if !addr.ip().is_loopback() {
        eprintln!(
            "警告：api 监听非回环地址 {addr}，包体与元数据将以明文传输；\
             生产必须由 TLS 反代终止并转发到 127.0.0.1"
        );
    }
    println!("skiff-server api：http://{addr}（默认关闭、独立进程；客户端不访问本服务）");
    println!(
        "提交状态：{}（0700）；白名单每请求重读，改文件即生效",
        server.state.display()
    );
    api::serve(listener, server)
}

/// 发布 job：把 approved 提交落成源数据并复用 `publish` 生成签名产物。
fn release_cmd(args: &[String]) -> Result<()> {
    let (options, flags) = parse(args, &["--swap"])?;
    let seq = match options.get("--seq") {
        Some(value) => Some(
            value
                .parse::<u64>()
                .with_context(|| format!("--seq 不是合法数字: {value}"))?,
        ),
        None => None,
    };
    let job = api::job::JobOptions {
        state: required_path(&options, "--state")?,
        catalog: path_option(&options, "--catalog", "deploy/skiff-server/catalog"),
        schema: path_option(&options, "--schema", "deploy/skiff-server/schema"),
        channel: options
            .get("--channel")
            .cloned()
            .unwrap_or_else(|| "stable".to_string()),
        staging_out: options.get("--staging-out").map(PathBuf::from),
        dist: options.get("--dist").map(PathBuf::from),
        swap: flags.contains("--swap"),
        seq,
        prev: options.get("--prev").map(PathBuf::from),
        submission: options.get("--submission").cloned(),
        public_key: public_key(&options)?,
    };
    let report = api::job::run(&job)?;
    println!(
        "✓ 发布完成：seq={}，提交 {} 个、撤包 {} 条 → {}{}",
        report.seq,
        report.released.len(),
        report.takedowns.len(),
        report.output.display(),
        if report.swapped {
            "（已换入线上目录）"
        } else {
            "（仅 staging，需由发布机/CI 搬运）"
        }
    );
    Ok(())
}

fn required_path(options: &BTreeMap<String, String>, name: &str) -> Result<PathBuf> {
    options
        .get(name)
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("缺少 {name}"))
}

fn public_key(options: &BTreeMap<String, String>) -> Result<Option<String>> {
    if let Some(value) = options.get("--public-key") {
        return Ok(Some(value.clone()));
    }
    Ok(std::env::var(ENV_PUBLIC_KEY)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty()))
}

fn enforce_public_key(options: &BTreeMap<String, String>, signer: &Signer) -> Result<()> {
    let Some(expected) = public_key(options)? else {
        return Ok(());
    };
    let actual = signer
        .public_key_hex()
        .ok_or_else(|| anyhow::anyhow!("当前签名器没有公钥"))?;
    if actual != expected {
        bail!("签名公钥 {actual} 与 {ENV_PUBLIC_KEY}={expected} 不一致：客户端会拒绝该清单");
    }
    Ok(())
}

fn path_option(options: &BTreeMap<String, String>, name: &str, default: &str) -> PathBuf {
    options
        .get(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default))
}

fn parse(args: &[String], flags: &[&str]) -> Result<(BTreeMap<String, String>, BTreeSet<String>)> {
    let mut options = BTreeMap::new();
    let mut seen_flags = BTreeSet::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        // 旗标与选项统一用带 `--` 的完整形式表示，比较也按完整形式，避免与调用方口径错位。
        if flags.contains(&arg.as_str()) {
            seen_flags.insert(arg.clone());
            index += 1;
            continue;
        }
        let Some(name) = arg.strip_prefix("--") else {
            bail!("无法识别的参数: {arg}");
        };
        if let Some((key, value)) = name.split_once('=') {
            options.insert(format!("--{key}"), value.to_string());
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .ok_or_else(|| anyhow::anyhow!("参数 --{name} 缺少取值"))?;
        options.insert(format!("--{name}"), value.clone());
        index += 2;
    }
    Ok((options, seen_flags))
}
