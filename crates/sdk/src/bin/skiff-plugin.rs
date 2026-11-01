//! `skiff-plugin` CLI：脚手架 / 密钥 / 构建 / 打包 / 自检（§4.4）。
//!
//! 与内核共用 `skiff-pkgfmt`，因此哈希清单排序、签名输入编码不会出现「打包器与
//! 校验器各写一遍」的偏差。
//!
//! 注：发布物（tar.gz）的 detached 签名链路（`sign-file` / `verify-file`）已于
//! 2026-11-01 移除（§11 决策 12）——发布物只用 SHA-256 摘要。插件包 `.wbpkg`
//! 的签名/验签不在此列，仍是产品安全内核（§4.3）。

use std::path::{Path, PathBuf};
use std::process::Command;

use skiff_pkgfmt::package::{pack_dir, verify_file, VerifyOptions, DEFAULT_LIMITS};
use skiff_pkgfmt::{Manifest, SigningKey};

const HELP: &str = "\
skiff-plugin — Skiff 插件工具链（与内核共用同一份包格式实现）

用法:
  skiff-plugin keygen [--out skiff-signing.key]
  skiff-plugin new <dir> [--id com.example.demo] [--name 展示名] [--sdk-path <sdk 目录>]
  skiff-plugin build [--dir <插件目录>] [--debug]
  skiff-plugin pack [--dir <插件目录>] [--key <私钥文件>] [--out <输出.wbpkg>]
  skiff-plugin verify <pkg> [--pubkey <hex>] [--allow-unsigned]

说明:
  - 私钥只放在打包机器上，0600；发布物只有 .wbpkg + 公钥指纹。
  - SDK 未发布到 crates.io：`new` 默认找 cwd 下的 `crates/sdk`，或用 --sdk-path /
    SKIFF_SDK_PATH 指定；都没有时写成版本依赖（会解析失败，需手工改 Cargo.toml）。
  - `pack` 会校验 manifest.key_id 与私钥派生公钥的指纹一致。
  - `verify` 是装前自查，校验逻辑与内核安装时完全一致。
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        println!("{HELP}");
        return;
    }
    // 任何位置出现 -h/--help 都只打帮助、**不做实事**。
    // 曾经 `keygen --help` 会真的生成一把私钥并写进当前目录——比报错更糟。
    if args[1..].iter().any(|a| a == "-h" || a == "--help") {
        println!("{HELP}");
        return;
    }
    let result = match args[0].as_str() {
        "-h" | "--help" | "help" => {
            println!("{HELP}");
            Ok(())
        }
        "keygen" => keygen(&args[1..]),
        "new" => new(&args[1..]),
        "build" => build(&args[1..]),
        "pack" => pack(&args[1..]),
        "verify" => verify(&args[1..]),
        other => Err(format!("未知子命令: {other}\n\n{HELP}")),
    };
    if let Err(e) = result {
        eprintln!("错误: {e}");
        std::process::exit(1);
    }
}

fn opt(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn positional(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if a.starts_with("--") {
            skip = true;
            continue;
        }
        out.push(a.clone());
    }
    out
}

fn keygen(args: &[String]) -> Result<(), String> {
    // 这个子命令会**写出私钥**，不能像只读命令那样对多余参数睁一只眼。
    if let Some(bad) = args
        .iter()
        .find(|a| a.as_str() != "--out" && a.starts_with('-'))
    {
        return Err(format!("keygen 不认识的选项: {bad}（仅支持 --out <文件>）"));
    }
    if let Some(extra) = positional(args).first() {
        return Err(format!("keygen 不接受位置参数: {extra}"));
    }
    let out = opt(args, "--out").unwrap_or_else(|| "skiff-signing.key".into());
    let path = PathBuf::from(&out);
    if path.exists() {
        return Err(format!("{out} 已存在，拒绝覆盖私钥"));
    }
    let key = SigningKey::generate();
    std::fs::write(&path, format!("{}\n", key.seed_hex()))
        .map_err(|e| format!("写入私钥失败: {e}"))?;
    restrict(&path);
    println!("私钥已写入: {out}（0600，务必离线保存）");
    println!("公钥(hex)   : {}", key.public_hex());
    println!("指纹(key_id): {}", key.key_id());
    println!("把指纹与公钥一起公布，用户导入时逐位核对。");
    Ok(())
}

#[cfg(unix)]
fn restrict(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict(_path: &Path) {}

fn new(args: &[String]) -> Result<(), String> {
    let dir = positional(args)
        .into_iter()
        .next()
        .ok_or("new 需要目标目录名")?;
    let id =
        opt(args, "--id").unwrap_or_else(|| format!("com.example.{}", sanitize(&dir_tail(&dir))));
    let name = opt(args, "--name").unwrap_or_else(|| dir.clone());
    let root = PathBuf::from(&dir);
    if root.exists() {
        return Err(format!("目录已存在: {dir}"));
    }
    // SDK 还没发包到 crates.io：默认依赖会解析失败。允许显式指定本地 SDK 路径，
    // 否则把 cwd 里能找到的 `crates/sdk` 当作 SDK（在仓库里跑时开箱即用）。
    let sdk_path = opt(args, "--sdk-path")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("SKIFF_SDK_PATH").map(PathBuf::from))
        .or_else(|| {
            let cand = std::env::current_dir().ok()?.join("crates/sdk");
            if cand.join("Cargo.toml").is_file() {
                Some(cand)
            } else {
                None
            }
        });

    std::fs::create_dir_all(root.join("src")).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(root.join("ui")).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(root.join("migrations")).map_err(|e| e.to_string())?;

    let key = SigningKey::generate();
    let key_path = root.join("skiff-signing.key");
    std::fs::write(&key_path, format!("{}\n", key.seed_hex())).map_err(|e| e.to_string())?;
    restrict(&key_path);

    let manifest = format!(
        r#"id = "{id}"
name = "{name}"
version = "0.1.0"
key_id = "{}"
api = "1.0"
run = "on-demand"
requires = ["docker"]
idle_timeout_sec = 300
max_restarts = 3
network_egress = []

[[permissions]]
capability = "docker"
verbs = ["containers.list", "containers.inspect", "containers.logs"]

[[ui]]
label = "{name}"
path = "index.html"

[config_schema.api_base]
type = "string"
label = "上游地址"
"#,
        key.key_id()
    );
    std::fs::write(root.join("manifest.toml"), manifest).map_err(|e| e.to_string())?;
    std::fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{pkg}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[bin]]\nname = \"plugin\"\npath = \"src/main.rs\"\n\n[dependencies]\nskiff-sdk = {dep}\nserde_json = \"1\"\n",
            pkg = crate_name(&dir),
            dep = match &sdk_path {
                Some(p) => format!("{{ path = \"{}\" }}", p.display()),
                None => "\"0.1\"".to_string(),
            }
        ),
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(
        root.join("src/main.rs"),
        r#"fn main() {
    let plugin = match skiff_sdk::Plugin::from_env() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("启动失败: {e}");
            std::process::exit(1);
        }
    };
    let granted = plugin.granted().to_vec();
    plugin
        .serve(move |method, params| match method {
            "ping" => Ok(serde_json::json!({ "pong": true, "granted": granted })),
            "tick" => Ok(serde_json::json!({ "ok": true, "params": params })),
            other => Err(skiff_sdk::RpcError::method_not_found(other)),
        })
        .expect("服务异常退出");
}
"#,
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(
        root.join("ui/index.html"),
        "<!doctype html><meta charset=\"utf-8\"><title>Plugin</title>\n<h1>Hello from plugin</h1>\n",
    )
    .map_err(|e| e.to_string())?;
    let _ = std::fs::write(root.join("migrations/.gitkeep"), "");

    println!("已生成插件脚手架: {dir}");
    println!("  manifest.key_id = {}", key.key_id());
    println!("  公钥(hex)       = {}", key.public_hex());
    println!("  （装包前把公钥导入面板「设置 → 信任公钥」）");
    println!("下一步: cd {dir} && skiff-plugin build && skiff-plugin pack");
    Ok(())
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .to_lowercase()
}

/// 目录参数 → 末段名字。`skiff-plugin new /opt/plugins/demo` 的默认 id 与 crate 名
/// 都只能取 `demo`：拿整条路径去 sanitize 会得到 `-opt-plugins-demo`。
fn dir_tail(dir: &str) -> String {
    std::path::Path::new(dir)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(dir)
        .to_string()
}

/// 末段名字 → 合法 crate 名。
///
/// cargo 要求首字符是字母或 `_`（`-` / 数字都不行），所以 `new ../demo`、`new .demo`
/// 这类输入必须削掉首部的 `-` 并兜底加前缀，否则下一步 `cargo build` 直接报
/// `invalid character '-' in package name`。
fn crate_name(dir: &str) -> String {
    let s = sanitize(&dir_tail(dir)).replace('.', "-");
    let s = s.trim_matches('-');
    if s.is_empty() {
        "plugin".to_string()
    } else if !s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        format!("plugin-{s}")
    } else {
        s.to_string()
    }
}

fn build(args: &[String]) -> Result<(), String> {
    let dir = PathBuf::from(opt(args, "--dir").unwrap_or_else(|| ".".into()));
    if !dir.join("Cargo.toml").exists() {
        return Err(format!("{} 下没有 Cargo.toml", dir.display()));
    }
    let debug = args.iter().any(|a| a == "--debug");
    let mut cmd = Command::new("cargo");
    cmd.arg("build").current_dir(&dir);
    if !debug {
        cmd.arg("--release");
    }
    let status = cmd.status().map_err(|e| format!("无法执行 cargo: {e}"))?;
    if !status.success() {
        return Err("cargo build 失败".into());
    }
    let profile = if debug { "debug" } else { "release" };
    let exe = dir.join("target").join(profile).join(if cfg!(windows) {
        "plugin.exe"
    } else {
        "plugin"
    });
    if !exe.exists() {
        return Err(format!("未找到构建产物: {}", exe.display()));
    }
    let arch = skiff_pkgfmt::host_arch();
    let dest = dir.join("bin").join(&arch).join("plugin");
    std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::copy(&exe, &dest).map_err(|e| format!("复制产物失败: {e}"))?;
    println!(
        "已产出 bin/{arch}/plugin（{} 字节）",
        std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0)
    );
    println!("提示：多架构是发布者责任；交叉编译请先自行指定 --target，再 pack。");
    Ok(())
}

fn pack(args: &[String]) -> Result<(), String> {
    let dir = PathBuf::from(opt(args, "--dir").unwrap_or_else(|| ".".into()));
    let key_path = PathBuf::from(
        opt(args, "--key")
            .or_else(|| std::env::var("SKIFF_SIGNING_KEY").ok())
            .unwrap_or_else(|| dir.join("skiff-signing.key").display().to_string()),
    );
    let seed = std::fs::read_to_string(&key_path)
        .map_err(|e| format!("读取私钥失败 {}: {e}", key_path.display()))?;
    let manifest_text = std::fs::read_to_string(dir.join("manifest.toml"))
        .map_err(|e| format!("读取 manifest 失败: {e}"))?;
    let manifest = Manifest::parse(&manifest_text).map_err(|e| e.to_string())?;
    let arch = skiff_pkgfmt::host_arch();
    if !dir.join("bin").join(&arch).join("plugin").exists() {
        return Err(format!(
            "缺少 bin/{arch}/plugin，请先执行 skiff-plugin build"
        ));
    }
    let out = opt(args, "--out").map(PathBuf::from).unwrap_or_else(|| {
        dir.join(format!(
            "{}-{}.wbpkg",
            sanitize(&manifest.id),
            manifest.version
        ))
    });

    pack_dir(&dir, &out, seed.trim()).map_err(|e| e.to_string())?;
    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    println!("已打包: {}（{} 字节）", out.display(), size);
    println!("指纹: {}", manifest.key_id);
    // 装包前要先把公钥导入面板「设置 → 信任公钥」，这里直接给出来，
    // 省得再回头找 keygen 的输出（new 生成的私钥当时并没有打印过公钥）。
    if let Ok(k) = SigningKey::from_seed_hex(seed.trim()) {
        println!("公钥(hex): {}", k.public_hex());
    }
    Ok(())
}

fn verify(args: &[String]) -> Result<(), String> {
    let pkg = positional(args)
        .into_iter()
        .find(|a| a.ends_with(".wbpkg"))
        .ok_or("verify 需要 .wbpkg 路径")?;
    let pubkey = opt(args, "--pubkey");
    let allow_unsigned = args.iter().any(|a| a == "--allow-unsigned");
    let meta = verify_file(
        Path::new(&pkg),
        &VerifyOptions {
            trusted_public_key: pubkey,
            allow_unsigned,
            limits: DEFAULT_LIMITS,
            arch: skiff_pkgfmt::host_arch(),
        },
    )
    .map_err(|e| e.to_string())?;

    println!("包: {pkg}");
    println!("插件: {} ({})", meta.manifest.name, meta.manifest.id);
    println!(
        "版本: {}  API: {}",
        meta.manifest.version, meta.manifest.api
    );
    println!("指纹: {}", meta.key_id);
    println!(
        "签名: {}",
        if meta.signature_verified {
            "已验签"
        } else {
            "未验签"
        }
    );
    println!(
        "文件: {} 个，解压 {} 字节",
        meta.files.len(),
        meta.payload_bytes
    );
    println!("能力: {:?}", meta.manifest.requires_str());
    println!("权限:");
    for p in meta.manifest.permission_strings() {
        println!("  - {p}");
    }
    println!("出网: {:?}", meta.manifest.network_egress);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 脚手架生成的名字必须是 cargo 认的 crate 名。
    ///
    /// 回归：`new /abs/path/demo` 曾把整条路径 sanitize 成 `-abs-path-demo`，
    /// `skiff-plugin build` 里 `cargo build` 直接报
    /// `invalid character '-' in package name`（首次在 CI 的链路测试里暴露）。
    #[test]
    fn crate_name_is_valid_for_absolute_and_dotted_paths() {
        assert_eq!(crate_name("/tmp/x/demo"), "demo");
        assert_eq!(crate_name("/tmp/x/My_Plugin"), "my_plugin");
        assert_eq!(crate_name(".ci-test/demo"), "demo");
        assert_eq!(crate_name("demo"), "demo");
        // 末段以数字/减号开头时兜底加前缀，仍要是合法首字符
        assert_eq!(crate_name("/tmp/3d-viewer"), "plugin-3d-viewer");
        assert_eq!(crate_name("/tmp/.hidden"), "hidden");
        // 末段全是非法字符
        assert_eq!(crate_name("/tmp/---"), "plugin");
    }

    #[test]
    fn plugin_id_takes_last_component() {
        assert_eq!(sanitize(&dir_tail("/opt/plugins/demo")), "demo");
        assert_eq!(sanitize(&dir_tail("demo")), "demo");
    }
}
