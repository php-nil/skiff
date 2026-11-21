//! 固化校验（§8）：把「上架与安装链路最高频的失败原因」用代码堵住。
//!
//! 三个子命令：`check` 跑前两项，`verify-pkg` 是独立的包校验入口。
//! 1. `check-1panel`：compose 里每个 `${VAR}` 必须在版本级 data.yml 的 formFields 有同名
//!    `envKey`；反向也查（声明了却没用到 = 用户填了不生效，同样是坑）。
//! 2. `check-compose`：用内核同一个解析器（`skiff_compose`）解析样例 compose，
//!    未支持的字段必须**显式报错**，而不是被静默忽略。这份解析器就是运行时那份，
//!    所以这里通过 = 运行时也通过。
//! 3. `verify-pkg`：`.wbpkg` 的结构/哈希清单/签名输入编码与内核安装路径完全一致
//!    （同一份 `skiff_pkgfmt` 实现）。
//!
//! 退出码：0 全通过，1 有失败项。CI 默认跑 `cargo run -p xtask -- check`。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use skiff_pkgfmt::package::{verify_file, VerifyOptions, DEFAULT_LIMITS};

const HELP: &str = "\
xtask — Skiff 发布与上架校验

用法:
  xtask check                       依次跑 check-1panel 与 check-compose（CI 用这个）
  xtask check-channel               宿主通道闸：进程执行只能经 skiff_host、
                                    内核文件写只能经内核文件通道（CI 用这个）
  xtask check-1panel [--dir <apps/skiff>]
                                    商店包 compose ↔ data.yml 变量对齐
  xtask check-compose <file.yml...>   Compose MVP 字段子集校验（含未支持字段报错）
  xtask verify-pkg <file.wbpkg> [--pubkey <hex>] [--allow-unsigned]
                                    .wbpkg 摘要/结构/签名校验
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("check");
    let rest = if args.is_empty() {
        &args[..]
    } else {
        &args[1..]
    };

    let ok = match cmd {
        "-h" | "--help" | "help" => {
            println!("{HELP}");
            true
        }
        "check" => check_all(rest),
        "check-channel" => check_channel(),
        "check-1panel" => check_1panel(rest),
        "check-compose" => check_compose(rest),
        "verify-pkg" => verify_pkg(rest),
        other => {
            eprintln!("未知子命令: {other}\n\n{HELP}");
            false
        }
    };
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn opt<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
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

/// 宿主通道闸。
///
/// 两条硬规则（「不能有例外」的机械落点）：
/// 1. `crates/app` 与 `crates/core` 的非测试代码**不得出现 `Command::new`**：
///    宿主机进程的唯一入口是 `skiff_host`（超时 / 取消 / 清理只有一份）。
/// 2. 内核自身的文件写**只能经内核文件通道**（`AdminFs::write_kernel` 等）。
///    这条暂时是「只减不增」的计数闸：基线固定，迁移一处就下调一处。
///
/// 插件进程的 `Command::new` 不在此列（`crates/plugin/src/supervisor.rs` 拉起
/// 插件是进程生命周期管理，不是宿主命令执行）。
fn check_channel() -> bool {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace 根")
        .to_path_buf();
    let mut ok = true;

    // 规则 1：进程执行唯一入口
    for dir in ["crates/app/src", "crates/core/src"] {
        for (file, text) in rust_files(&root.join(dir)) {
            if before_tests(&text).contains("Command::new") {
                println!(
                    "✗ {}: 出现 Command::new —— 宿主机进程只能经 skiff_host",
                    rel(&root, &file)
                );
                ok = false;
            }
        }
    }
    if ok {
        println!("✓ 进程执行通道：app/core 无非测试的直接 Command::new");
    }

    // 规则 2：内核文件写入口（只减不增）
    let mut writes = 0usize;
    let mut files = 0usize;
    for (file, text) in rust_files(&root.join("crates/app/src")) {
        let name = file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        // 底层原语自身与审计兜底不算绕过：它们**就是**通道的实现。
        if matches!(name, "atomic.rs" | "audit.rs") {
            continue;
        }
        // 独立测试文件整份跳过：其中的临时文件操作不是产品路径。
        if name == "tests.rs" {
            continue;
        }
        let hits = count_fs_writes(&before_tests(&text));
        if hits > 0 {
            files += 1;
            writes += hits;
        }
    }
    match FS_WRITE_BASELINE {
        Some(baseline) if writes > baseline => {
            println!(
                "✗ 内核文件写：{files} 个文件共 {writes} 处，超过基线 {baseline} —— 新写入必须经 write_kernel"
            );
            ok = false;
        }
        Some(baseline) => {
            println!("✓ 内核文件写：{files} 个文件共 {writes} 处（基线 {baseline}，只减不增）");
        }
        None => println!("内核文件写：{files} 个文件共 {writes} 处（未设基线）"),
    }
    ok
}

/// 内核文件写的基线。**只能下调**：每迁一处到 `write_kernel` 就减一处。
///
/// 2026-11-21 重定 **96 处（13 个文件）**：`panel_settings` 的 `config.toml`
/// 已改走 `AdminFs::write_kernel`（所有可写键零直写）。剩余计数按两类看：
/// **内核文件**必须继续迁 `write_kernel`；**宿主 nginx/php 配置**本就应直写，
/// 但必须在命令层留业务审计（如 `nginx.config.save` / `nginx.params.save`）。
const FS_WRITE_BASELINE: Option<usize> = Some(96);

/// 递归收集 `.rs` 文件。
fn rust_files(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(rust_files(&path));
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            if let Ok(text) = std::fs::read_to_string(&path) {
                out.push((path, text));
            }
        }
    }
    out
}

/// 只保留测试模块之前的代码：单测里的临时文件操作不是产品路径。
///
/// 不能见到 `#[cfg(test)]` 就截断——它也可能是文件**中间**的 test-only 辅助模块
/// （例如 `crates/app/src/commands/mod.rs` 的 `test_support`），在那里截断会让闸门
/// 漏掉后半段的真实代码。判据收紧为「属性之后第一个非空、非注释行是 `mod tests`」，
/// 也就是本仓约定的「测试模块放文件末尾」。
fn before_tests(text: &str) -> String {
    let mut cut = text.len();
    for (i, _) in text.match_indices("#[cfg(test)]") {
        let is_tests_mod = text[i..]
            .lines()
            .skip(1)
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with("//"))
            .is_some_and(|l| l.starts_with("mod tests") || l.starts_with("mod tls_tests"));
        if is_tests_mod {
            cut = i;
            break;
        }
    }
    text[..cut].to_string()
}

/// 统计直接文件写（写 / 建目录 / 删 / 改名 / 复制）与底层写入口。
///
/// 口径（2026-11-19 重定）：除 `std::fs::*` 的别名写法（`use std::fs;` 后
/// `fs::write(...)`）外，还计入 `File::create` / `OpenOptions` 与
/// `write_atomic` 调用方——它们是同一件事的不同写法，漏计等于闸门失明。
fn count_fs_writes(body: &str) -> usize {
    [
        "fs::write",
        "fs::create_dir_all",
        "fs::remove_file",
        "fs::remove_dir_all",
        "fs::rename",
        "fs::copy",
        "File::create",
        "OpenOptions",
        "write_atomic",
    ]
    .iter()
    .map(|needle| body.matches(needle).count())
    .sum()
}

fn rel(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .display()
        .to_string()
}

fn check_all(args: &[String]) -> bool {
    let mut ok = true;
    println!("== 1/3 商店包变量对齐 ==");
    ok &= check_1panel(args);
    println!();
    println!("== 2/3 仓库内 Compose 样例字段子集 ==");
    ok &= check_compose(&[]);
    println!();
    println!("== 3/3 宿主通道（进程执行 / 内核文件写）==");
    ok &= check_channel();
    println!();
    if ok {
        println!("全部通过 ✅");
    } else {
        println!("存在失败项 ❌");
    }
    ok
}

// ── 1. 1Panel 商店包：compose ${VAR} ↔ formFields envKey ──────────────

fn check_1panel(args: &[String]) -> bool {
    let root = PathBuf::from(opt(args, "--dir").unwrap_or("deploy/1panel/apps/skiff"));
    if !root.is_dir() {
        // 没带商店包时不算失败：主形态是 systemd 直装（§5.1）
        println!("跳过：{}/ 不存在（可选形态）", root.display());
        return true;
    }

    let mut ok = true;
    let mut checked = 0usize;

    for entry in std::fs::read_dir(&root).into_iter().flatten().flatten() {
        let ver_dir = entry.path();
        if !ver_dir.is_dir() {
            continue;
        }
        let compose = ver_dir.join("docker-compose.yml");
        let data = ver_dir.join("data.yml");
        if !compose.exists() || !data.exists() {
            println!(
                "跳过 {}（缺 docker-compose.yml 或 data.yml）",
                ver_dir.display()
            );
            continue;
        }
        checked += 1;
        let ver = ver_dir.file_name().and_then(|s| s.to_str()).unwrap_or("?");

        let compose_text = std::fs::read_to_string(&compose).unwrap_or_default();
        let used = compose_vars(&compose_text);
        let declared = match form_env_keys(&data) {
            Ok(v) => v,
            Err(e) => {
                println!("✗ {}：data.yml 解析失败：{e}", ver_dir.display());
                ok = false;
                continue;
            }
        };

        // 必须由 1Panel 注入、不在 formFields 里声明的内置变量
        let builtin: BTreeSet<&str> = ["CONTAINER_NAME"].into_iter().collect();

        for v in &used {
            if builtin.contains(v.as_str()) || declared.contains(v) {
                continue;
            }
            println!(
                "✗ {ver}: compose 使用了 ${{{v}}}，但 data.yml 的 formFields 没有同名 envKey —— \
                 参数传不进去（上架最常见的失败原因）"
            );
            ok = false;
        }
        for d in &declared {
            if !used.contains(d) {
                println!("! {ver}: formFields 声明了 {d}，但 compose 里没用到（用户填了不生效）");
                ok = false;
            }
        }
        // 端口变量必须带 PANEL_APP_PORT_ 前缀，否则 1Panel 不做占用校验
        for v in &used {
            if (v.contains("PORT") || v.contains("port")) && !v.starts_with("PANEL_APP_PORT_") {
                println!("✗ {ver}: 端口变量 {v} 必须以 PANEL_APP_PORT_ 开头");
                ok = false;
            }
        }
        if ok {
            println!(
                "✓ {ver}: {} 个变量 ↔ {} 个 formFields 对齐",
                used.len(),
                declared.len()
            );
        }
    }

    if checked == 0 {
        println!("跳过：没有版本目录");
    }
    ok
}

/// 从 compose 文本提取 `${VAR}`（忽略 `${VAR:-default}` 的默认值部分）。
fn compose_vars(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'$' && bytes[i + 1] == b'{' {
            let start = i + 2;
            let mut end = start;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
                end += 1;
            }
            if end > start {
                out.insert(text[start..end].to_string());
            }
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

/// 版本级 data.yml → formFields 的 envKey 集合。
fn form_env_keys(data_yml: &Path) -> Result<BTreeSet<String>, String> {
    let text = std::fs::read_to_string(data_yml).map_err(|e| e.to_string())?;
    let v: serde_yaml::Value = serde_yaml::from_str(&text).map_err(|e| e.to_string())?;
    let fields = v
        .get("additionalProperties")
        .and_then(|a| a.get("formFields"))
        .and_then(|f| f.as_sequence())
        .ok_or_else(|| "缺少 additionalProperties.formFields".to_string())?;
    let mut out = BTreeSet::new();
    for f in fields {
        if let Some(k) = f.get("envKey").and_then(|k| k.as_str()) {
            out.insert(k.to_string());
        }
    }
    Ok(out)
}

// ── 2. Compose MVP 字段子集 ─────────────────────────────────────────

fn check_compose(args: &[String]) -> bool {
    let files = positional(args);
    let files = if files.is_empty() {
        // 默认扫仓库里的样例（deploy/compose/*.yml）
        let mut found = Vec::new();
        for dir in ["deploy/compose", "examples/compose"] {
            if let Ok(rd) = std::fs::read_dir(dir) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.extension()
                        .map(|x| x == "yml" || x == "yaml")
                        .unwrap_or(false)
                    {
                        found.push(p.display().to_string());
                    }
                }
            }
        }
        found.sort();
        found
    } else {
        files
    };

    if files.is_empty() {
        println!("跳过：没有 Compose 样例（deploy/compose/*.yml）");
        return true;
    }

    let mut ok = true;
    for f in &files {
        let text = match std::fs::read_to_string(f) {
            Ok(t) => t,
            Err(e) => {
                println!("✗ {f}: 读不到：{e}");
                ok = false;
                continue;
            }
        };
        match skiff_compose::parse(&text, "xtask-check") {
            Ok(p) => println!("✓ {f}: {} 个服务，字段全在 MVP 子集内", p.services.len()),
            Err(e) => {
                println!("✗ {f}: {e}");
                ok = false;
            }
        }
    }
    ok
}

// ── 3. .wbpkg 校验 ──────────────────────────────────────────────────

fn verify_pkg(args: &[String]) -> bool {
    let Some(pkg) = positional(args).into_iter().next() else {
        println!("跳过：未提供 .wbpkg");
        return true;
    };
    let pubkey = opt(args, "--pubkey").map(str::to_string);
    let allow_unsigned = args.iter().any(|a| a == "--allow-unsigned");

    match verify_file(
        Path::new(&pkg),
        &VerifyOptions {
            trusted_public_key: pubkey,
            allow_unsigned,
            limits: DEFAULT_LIMITS,
            arch: skiff_pkgfmt::host_arch(),
        },
    ) {
        Ok(meta) => {
            println!(
                "✓ {pkg}: {} {} / {} 个文件 / 解压 {} 字节 / 签名 {}",
                meta.manifest.id,
                meta.manifest.version,
                meta.files.len(),
                meta.payload_bytes,
                if meta.signature_verified {
                    "已验"
                } else {
                    "未验（allow-unsigned）"
                }
            );
            true
        }
        Err(e) => {
            println!("✗ {pkg}: {e}");
            false
        }
    }
}
