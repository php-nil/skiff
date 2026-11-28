//! 运行环境版本清单：具体版本号（`8.5.13` / `24.4.0` / `1.99.1` …）由**镜像服务**提供。
//!
//! 目录（[`crate::sites::runtime_catalog::RUNTIME_KINDS`]）只维护「支持哪些大版本」。补丁号
//! 是镜像服务每次发版产生的新版本，写死在代码里必然滞后一版——这正是「PHP 8.5 装出来
//! 还是 8.5.12」的一半原因（另一半是拉取时跳过浮动 tag，见 `sites::spec`）。
//!
//! **路径固定、不配置**：`<镜像基址>/channels/<channel>/runtime-index/<kind>.json`，
//! 三个已实现的类型各一份（`php.json` / `node.json` / `rust.json`）。镜像基址取
//! [`crate::AppState::catalog`] 的有序镜像列表首项，channel 取同一份配置；`mirrors = []`
//! 表示不联网，只用内置副本或目录大版本。服务端产物结构见
//! `docs/plugin/SKIFF_SERVER/01-DESIGN.md` §2.2：`channels/<name>/` 是唯一可变层，
//! 所以清单落在渠道目录下而不是根目录。
//!
//! 文件格式 `skiff.server.runtime-index/v1`（schema 随产物发在
//! `deploy/skiff-server/schema/runtime-index.schema.json`）：
//!
//! ```json
//! {"schema":"skiff.server.runtime-index/v1","kind":"php","channel":"stable",
//!  "generated_at":"2026-11-24T12:00:00Z",
//!  "versions":[{"version":"8.5.13","default":true},
//!              {"version":"8.3.37","eol":"2027-12-31"}]}
//! ```
//!
//! 条目只带面板真正消费的 `version` / `eol` / `default`：`image` / `image_digest` **有意不发**
//! ——按 digest 锁定安装会把「镜像还没同步」变成硬安装失败，得单独设计失败面之后才出 v2。
//! `eol` 是**日期**不是布尔（`2027-12-31` = 该分支到这天结束支持），过期才算 EOL。
//!
//! **内置副本只对 PHP 存在**（`assets/runtime-index/php.json`，是服务端产物的快照）：
//! PHP 的补丁号差异直接影响扩展可用性，值得离线也列出来；Node.js / Rust 的可选版本是
//! LTS / stable 大版本，目录本身就是权威，回退到目录版本即可装出可用环境。不给它们造
//! 内置副本还有一个硬理由：内置副本的 `generated_at` 若比服务端产物新，回滚保护会把
//! 服务端清单**永久**判成「更旧」而拒用（见 `TEST_LEDGER`）。
//!
//! **回填顺序是硬要求**：内置副本的 `generated_at` 必须 ≤ 镜像服务的清单（先发服务端数据，
//! 再回填内置副本）；顺序反了，远端会被判「更旧」而永久拒用。
//!
//! 兼容旧形态（裸数组、`{"versions":[..]}`、`{"tags":[..]}`、`{"results":[{"name":…}]}`），
//! 因为镜像方可能还没升级产物。
//!
//! **信任边界**：该文件不签名、内网只走 HTTP。能挡住的是「镜像比内置副本更旧」
//! （`generated_at` 回退即拒用），挡不住伪造；要防伪造得把它的 sha256 写进已签名的
//! `index.json`（二期，见契约 §4.5）。

use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use skiff_store::{KIND_NODE, KIND_PHP, KIND_RUST};

use crate::sites::runtime_catalog;
use crate::AppState;

/// 成功清单的缓存时长。镜像服务发版不频繁，10 分钟足够新。
pub const CACHE_TTL_SECS: i64 = 600;
/// 失败负缓存：外网不通时压住重试，避免每次渲染都等一次超时。
pub const FAILURE_TTL_SECS: i64 = 60;
/// 每个大版本最多列多少个版本号（8.5.13 / 8.5.12 / 8.5.11 / 8.5.10）。
pub const PATCHES_PER_MAJOR: usize = 4;
/// 单次拉取硬超时：清单是锦上添花，不能把新建弹窗拖到框架超时。
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(6);
/// 响应体上限：整份清单只有几 KB，4 MiB 已远超任何合理输入。
const MAX_BODY_BYTES: u64 = 4 * 1024 * 1024;

/// 有版本清单的类型（顺序即页面字段顺序）。Go 未实现，不在目录也不取清单。
pub const KINDS: &[&str] = &[KIND_PHP, KIND_NODE, KIND_RUST];

/// 渠道目录下的固定子目录：文件名按类型定（`<kind>.json`）。
const INDEX_SUBDIR: &str = "runtime-index";
const INDEX_SCHEMA: &str = "skiff.server.runtime-index/v1";

/// 随面板发版入库的 PHP 补丁清单：镜像服务不可达时它就是可用版本来源。
///
/// 它是**服务端产物的快照**（发版时用 `skiff-server build` 的产物覆盖），不是另一份
/// 手工维护的版本表；`generated_at` 因此可以同时用于回退判定。
const BUILTIN_PHP: &str = include_str!("../assets/runtime-index/php.json");

/// 清单文件名：只认已实现的三个类型，其余返回 `None`（= 不取清单、不拼地址）。
pub fn index_file(kind: &str) -> Option<&'static str> {
    match kind {
        KIND_PHP => Some("php.json"),
        KIND_NODE => Some("node.json"),
        KIND_RUST => Some("rust.json"),
        _ => None,
    }
}

/// 一次清单查询的结果。`source` 区分「镜像服务实时清单」与回退来源。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionIndex {
    pub kind: String,
    /// `registry` = 镜像服务下发的清单；`builtin` = 内置副本（仅 PHP）/
    /// `catalog` = 目录大版本（Node.js / Rust，以及没有内置副本时）。
    pub source: String,
    /// 取数时间（unix 秒）。回退时是回退时间，用作负缓存起点。
    pub fetched_at: i64,
    /// 清单声明的生成时间（RFC3339）；旧形态没有该字段，为空串。
    #[serde(default)]
    pub generated_at: String,
    pub versions: Vec<String>,
    /// 已 EOL 的版本（清单 `eol` 日期已过期）：页面标注并排到最后，正常版本仍可按原顺序选。
    #[serde(default)]
    pub eol: Vec<String>,
    /// 镜像服务声明的默认版本；为空时页面预选首个可用项。
    #[serde(default)]
    pub default: Option<String>,
    /// 非空表示这次是回退：页面必须把原因显示出来，否则「清单少了几版」无从解释。
    /// 回退不一定等于失败（`HTTP 404` = 服务端还没发布该类型，属正常状态）。
    #[serde(default)]
    pub error: String,
    /// 是否**拉取失败**（超时/连接拒绝/坏 JSON）。决定缓存 TTL：失败 60 秒、其余 10 分钟。
    /// 与 `error` 分开是因为「服务端没发布」这种回退要按成功缓存，否则每 60 秒重打一次网络。
    #[serde(default)]
    pub failed: bool,
}

/// 解析结果：清单里真正被消费的字段。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Parsed {
    generated_at: String,
    versions: Vec<String>,
    eol: Vec<String>,
    default: Option<String>,
}

/// 一次拉取的三种结局。`NotPublished` 不是故障：服务端还没有该类型的 `runtimes/` 条目。
enum Fetched {
    List(Parsed),
    NotPublished,
}

impl VersionIndex {
    fn fallback(kind: &str, now: i64, failed: bool, note: String) -> Self {
        let (source, parsed) = match builtin_index(kind) {
            Some(parsed) => ("builtin", parsed),
            None => (
                "catalog",
                Parsed {
                    versions: runtime_catalog::kind_def(kind)
                        .map(|d| d.versions.iter().map(|v| v.to_string()).collect())
                        .unwrap_or_default(),
                    ..Default::default()
                },
            ),
        };
        Self {
            kind: kind.to_string(),
            source: source.to_string(),
            fetched_at: now,
            generated_at: parsed.generated_at,
            versions: parsed.versions,
            eol: parsed.eol,
            default: parsed.default,
            error: note,
            failed,
        }
    }

    /// 缓存是否还在有效期内（拉取失败按短 TTL 算）。
    pub fn fresh(&self) -> bool {
        let ttl = if self.failed {
            FAILURE_TTL_SECS
        } else {
            CACHE_TTL_SECS
        };
        chrono::Utc::now()
            .timestamp()
            .saturating_sub(self.fetched_at)
            < ttl
    }

    /// 某个版本是否已被镜像服务标记 EOL。
    pub fn is_eol(&self, version: &str) -> bool {
        self.eol.iter().any(|v| v == version)
    }
}

/// 只读缓存（不触网）：页面渲染用它内联下拉选项，保证首屏不被外网拖慢。
pub fn cached(state: &AppState, kind: &str) -> Option<VersionIndex> {
    read_cache(state, &cache_key(kind), kind)
}

/// 无缓存时的视图（不触网、不写缓存）：内置副本或目录大版本。
pub fn default_index(kind: &str) -> VersionIndex {
    VersionIndex::fallback(kind, 0, false, String::new())
}

fn cache_key(kind: &str) -> String {
    format!("sites.runtime_index.{kind}")
}

/// 清单地址：固定路径，由镜像基址、渠道与类型拼出；未知类型或基址缺失返回 `None`。
///
/// **不猜外部仓库**：清单只由本项目镜像服务提供，面板不查 Docker Hub 或任何第三方。
/// `[catalog].mirrors` 为空即「不联网」（与目录客户端的口径一致），此时只用内置副本/目录。
pub fn index_url(state: &AppState, kind: &str) -> Option<String> {
    let settings = state.catalog.settings();
    fixed_index_url(
        settings.mirrors.first().map(String::as_str),
        &settings.channel,
        kind,
    )
}

/// 纯函数：路径固定为 `<基址>/channels/<channel>/runtime-index/<kind>.json`。
///
/// 任一输入为空、或类型没有清单文件都返回 `None`（=`不联网`），不拼出半截地址。
pub fn fixed_index_url(base: Option<&str>, channel: &str, kind: &str) -> Option<String> {
    let file = index_file(kind)?;
    let base = base?.trim().trim_end_matches('/');
    let channel = channel.trim();
    if base.is_empty() || channel.is_empty() {
        return None;
    }
    Some(format!("{base}/channels/{channel}/{INDEX_SUBDIR}/{file}"))
}

/// 是否配了远端镜像服务（页面据此决定要不要后台预热；没配就不该反复打网络）。
pub fn remote_enabled(state: &AppState) -> bool {
    let settings = state.catalog.settings();
    fixed_index_url(
        settings.mirrors.first().map(String::as_str),
        &settings.channel,
        KIND_PHP,
    )
    .is_some()
}

/// 内置副本：只有 PHP 有（见模块注释），其它类型返回 `None` → 回退目录大版本。
fn builtin_index(kind: &str) -> Option<Parsed> {
    (kind == KIND_PHP).then(|| parse_index(KIND_PHP, "", BUILTIN_PHP).unwrap_or_default())
}

/// 解析一份清单。`expected_channel` 为空表示不校验渠道（内置副本与单元测试）。
///
/// 只认 1–3 段纯数字版本：PHP/Node 的 tag 形如 `8.5.13-fpm-alpine` / `24-alpine`，
/// 归一后分别是 3 段与 1 段。`8.5.13-zts-alpine`、`latest`、`bookworm` 一律丢弃
/// （不认识的后缀不会被剥离，于是整串带非数字字符而被挡掉）。
fn parse_index(kind: &str, expected_channel: &str, body: &str) -> Result<Parsed, String> {
    let raw: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("清单不是合法 JSON：{e}"))?;
    let mut tags: Vec<String> = Vec::new();
    let mut parsed = Parsed::default();
    // EOL 判定要「与今天比」：清单里的 `eol` 是日期，未来日期表示仍受支持。
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    match &raw {
        serde_json::Value::Array(rows) => collect_entries(rows, &mut tags, &mut parsed, &today),
        serde_json::Value::Object(map) => {
            if let Some(schema) = map.get("schema").and_then(|v| v.as_str()) {
                // 未知主版本必须整份拒绝：字段语义可能已经变了，不猜着解析。
                if schema != INDEX_SCHEMA {
                    return Err(format!("清单 schema 不受支持：{schema}"));
                }
                if let Some(file_kind) = map.get("kind").and_then(|v| v.as_str()) {
                    if file_kind != kind {
                        return Err(format!("清单类型是 {file_kind}，与请求的 {kind} 不符"));
                    }
                }
                if !expected_channel.is_empty() {
                    if let Some(channel) = map.get("channel").and_then(|v| v.as_str()) {
                        if channel != expected_channel {
                            return Err(format!(
                                "清单渠道是 {channel}，与当前渠道 {expected_channel} 不符"
                            ));
                        }
                    }
                }
                parsed.generated_at = map
                    .get("generated_at")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
            }
            for key in ["results", "tags", "versions", "items"] {
                if let Some(serde_json::Value::Array(rows)) = map.get(key) {
                    collect_entries(rows, &mut tags, &mut parsed, &today);
                }
            }
        }
        _ => {}
    }
    parsed.versions = tags
        .iter()
        .filter_map(|tag| version_from_tag(kind, tag))
        .collect();
    if parsed.versions.is_empty() {
        return Err("清单里没有可识别的版本号".to_string());
    }
    parsed.versions = sort_and_trim(kind, &mut parsed.versions);
    if parsed.versions.is_empty() {
        // 版本都合法但一个大版本都不在目录里：按「没有可用版本」处理，让上层回退。
        return Err("清单里的版本都不在目录支持范围内".to_string());
    }
    // EOL 标记只保留真正出现在清单里的版本，避免页面拿旧标记去标注新清单。
    parsed
        .eol
        .retain(|v| parsed.versions.iter().any(|x| x == v));
    if let Some(default) = &parsed.default {
        if !parsed.versions.iter().any(|v| v == default) {
            parsed.default = None;
        }
    }
    Ok(parsed)
}

/// 同时收集版本名与 v1 条目的附加字段（`eol` / `default`）。
///
/// `eol` 是**日期**（`YYYY-MM-DD`，该 cycle 的结束支持日）而不是布尔：只有已经过期
/// 才算「已 EOL」。2026-11-25 上机发现这里按「非空即 EOL」处理，把清单里 13 个 PHP
/// 版本（含刚发的 `8.6.0`）全标成了「已 EOL」——日期字段当布尔用会全量命中。
fn collect_entries(
    rows: &[serde_json::Value],
    out: &mut Vec<String>,
    parsed: &mut Parsed,
    today: &str,
) {
    for row in rows {
        match row {
            serde_json::Value::String(s) => out.push(s.clone()),
            serde_json::Value::Object(map) => {
                if let Some(serde_json::Value::String(s)) = map.get("version") {
                    out.push(s.clone());
                    if let Some(eol) = map.get("eol").and_then(|v| v.as_str()) {
                        // ISO 日期按字典序比较即可：未来日期 = 仍受支持，不进 EOL 集。
                        if !eol.is_empty() && eol <= today {
                            parsed.eol.push(s.clone());
                        }
                    }
                    if map.get("default").and_then(|v| v.as_bool()) == Some(true) {
                        parsed.default = Some(s.clone());
                    }
                } else if let Some(serde_json::Value::String(s)) = map.get("name") {
                    out.push(s.clone());
                } else if let Some(serde_json::Value::String(s)) = map.get("tag") {
                    out.push(s.clone());
                }
            }
            _ => {}
        }
    }
}

/// 从镜像 tag 里取出纯版本：`8.5.13-fpm-alpine` → `8.5.13`、`24-alpine` → `24`。
fn version_from_tag(kind: &str, tag: &str) -> Option<String> {
    let suffixes: &[&str] = match kind {
        KIND_PHP => &["-fpm-alpine", "-fpm"],
        KIND_NODE => &["-alpine", "-slim", "-bookworm"],
        KIND_RUST => &["-slim", "-alpine", "-bookworm"],
        _ => &[],
    };
    let core = suffixes
        .iter()
        .find_map(|suffix| tag.strip_suffix(suffix))
        .unwrap_or(tag)
        .trim();
    let ok = {
        let mut segments = 0;
        let mut valid = true;
        for seg in core.split('.') {
            if seg.is_empty() || !seg.bytes().all(|b| b.is_ascii_digit()) {
                valid = false;
                break;
            }
            segments += 1;
        }
        valid && (1..=3).contains(&segments)
    };
    ok.then(|| core.to_string())
}

/// 数值化排序键：`8.5.13` → (8, 5, 13)；`8.5` → (8, 5, 0)；`24` → (24, 0, 0)。
fn numeric_key(version: &str) -> (u64, u64, u64) {
    let mut parts = version.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

/// 清单版本归属的目录大版本：取目录项里最长的、等于它或作为其前缀的那一项。
///
/// 不能拿「前两段」去比对：Node 的目录项是 `24`，清单里是 `24.22.3`，按前两段算出来
/// 是 `24.22`，永远匹配不上，整份 Node 清单会被当成未知版本丢光（Rust 的 `1.99`
/// 对 `1.99.1` 同理）。PHP 的目录项本来就是 `8.5`，两种写法都能对上。
fn cycle_of<'a>(allowed: &'a [String], version: &str) -> Option<&'a str> {
    allowed
        .iter()
        .filter(|item| version == item.as_str() || version.starts_with(&format!("{item}.")))
        .max_by_key(|item| item.len())
        .map(String::as_str)
}

/// 降序 + 去重 + 只留目录支持的大版本 + 每大版本截断。
pub fn sort_and_trim(kind: &str, versions: &mut Vec<String>) -> Vec<String> {
    let allowed: Vec<String> = runtime_catalog::kind_def(kind)
        .map(|d| d.versions.iter().map(|v| v.to_string()).collect())
        .unwrap_or_default();
    versions.sort_by_key(|v| std::cmp::Reverse(numeric_key(v)));
    versions.dedup();
    let mut seen: HashMap<&str, usize> = HashMap::new();
    versions.retain(|version| {
        let Some(cycle) = cycle_of(&allowed, version) else {
            return false;
        };
        let count = seen.entry(cycle).or_insert(0);
        if *count >= PATCHES_PER_MAJOR {
            return false;
        }
        *count += 1;
        true
    });
    versions.clone()
}

/// 取某类型的清单（带缓存）。`force` 用于页面上的「刷新版本清单」。
pub async fn load(state: &Arc<AppState>, kind: &str, force: bool) -> VersionIndex {
    let now = chrono::Utc::now().timestamp();
    let key = cache_key(kind);
    if !force {
        if let Some(cached) = read_cache(state, &key, kind) {
            if cached.fresh() {
                return cached;
            }
        }
    }

    // 未知类型或没配镜像基址：不联网，直接用内置副本/目录大版本。
    let Some(url) = index_url(state, kind) else {
        let index = VersionIndex::fallback(kind, now, false, String::new());
        write_cache(state, &key, &index);
        return index;
    };

    let channel = state.catalog.settings().channel.clone();
    let kind_owned = kind.to_string();
    let url_owned = url.clone();
    let outcome =
        tokio::task::spawn_blocking(move || fetch_index(&kind_owned, &channel, &url_owned))
            .await
            .unwrap_or_else(|e| Err(format!("清单拉取任务失败：{e}")));

    let index = match outcome {
        Ok(Fetched::List(parsed)) => accept(kind, now, parsed),
        // 服务端还没有该类型的 runtimes 条目：不是故障，按成功 TTL 缓存，页面说明来源即可。
        Ok(Fetched::NotPublished) => VersionIndex::fallback(
            kind,
            now,
            false,
            format!(
                "镜像服务未发布 {} 清单（HTTP 404）",
                index_file(kind).unwrap_or("")
            ),
        ),
        Err(e) => VersionIndex::fallback(kind, now, true, format!("清单获取失败：{e}")),
    };
    write_cache(state, &key, &index);
    index
}

/// 采用远端清单；有内置副本时做回滚判定。
fn accept(kind: &str, now: i64, parsed: Parsed) -> VersionIndex {
    if let Some(builtin) = builtin_index(kind) {
        // 镜像比面板内置副本还旧，说明镜像没跟上（或同步卡住），采用它会让用户
        // 「刷新一次反而少了几版」。拒绝并说明原因。
        if !builtin.generated_at.is_empty()
            && !parsed.generated_at.is_empty()
            && parsed.generated_at < builtin.generated_at
        {
            return VersionIndex::fallback(
                kind,
                now,
                false,
                format!(
                    "镜像服务清单（{}）比内置副本（{}）更旧，已忽略",
                    parsed.generated_at, builtin.generated_at
                ),
            );
        }
    }
    VersionIndex {
        kind: kind.to_string(),
        source: "registry".to_string(),
        fetched_at: now,
        generated_at: parsed.generated_at,
        versions: parsed.versions,
        eol: parsed.eol,
        default: parsed.default,
        error: String::new(),
        failed: false,
    }
}

fn fetch_index(kind: &str, channel: &str, url: &str) -> Result<Fetched, String> {
    // 不共用商店目录的传输层：这里只需要「一次 GET + 有上限的文本体」，
    // 且要显式带 UA（部分镜像服务对匿名裸请求更严格）。
    let agent = ureq::AgentBuilder::new()
        .timeout(FETCH_TIMEOUT)
        .timeout_connect(Duration::from_secs(3))
        .redirects(3)
        .user_agent("skiff-runtime-index/1")
        .build();
    let resp = match agent.get(url).call() {
        Ok(resp) => resp,
        // 404/410 = 该类型还没发布清单（或已被移除），是正常状态而非故障。
        Err(ureq::Error::Status(code, _)) if code == 404 || code == 410 => {
            return Ok(Fetched::NotPublished);
        }
        Err(ureq::Error::Status(code, _)) => {
            return Err(format!("{url} 返回 HTTP {code}"));
        }
        Err(ureq::Error::Transport(e)) => {
            return Err(format!("请求 {url} 失败：{e}"));
        }
    };
    let mut body = String::new();
    resp.into_reader()
        .take(MAX_BODY_BYTES)
        .read_to_string(&mut body)
        .map_err(|e| format!("读取清单失败：{e}"))?;
    parse_index(kind, channel, &body).map(Fetched::List)
}

fn read_cache(state: &AppState, key: &str, kind: &str) -> Option<VersionIndex> {
    let raw = state.db().get_setting(key).ok().flatten()?;
    let mut index: VersionIndex = serde_json::from_str(&raw).ok()?;
    // 缓存与当前目录可能不同步（升级后大版本增减）；以缓存里记录的 kind 为准。
    index.kind = kind.to_string();
    Some(index)
}

fn write_cache(state: &AppState, key: &str, index: &VersionIndex) {
    if let Ok(raw) = serde_json::to_string(index) {
        // 缓存写失败不影响本次结果：下次重新拉取即可。
        let _ = state.db().set_setting(key, &raw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V1: &str = r#"{
        "schema": "skiff.server.runtime-index/v1",
        "kind": "php",
        "channel": "stable",
        "generated_at": "2026-11-24T12:00:00Z",
        "versions": [
            {"version": "8.5.13", "default": true},
            {"version": "8.5.12"},
            {"version": "8.4.28"},
            {"version": "8.3.37", "eol": "2020-01-01"}
        ]
    }"#;

    #[test]
    fn parses_v1_file_with_eol_and_default() {
        let parsed = parse_index(KIND_PHP, "stable", V1).expect("应能解析 v1");
        assert_eq!(parsed.versions[0], "8.5.13");
        assert_eq!(parsed.default.as_deref(), Some("8.5.13"));
        assert_eq!(parsed.eol, vec!["8.3.37"]);
        assert_eq!(parsed.generated_at, "2026-11-24T12:00:00Z");
    }

    /// `eol` 是日期：未来日期表示「到那天为止仍受支持」，不能标成已 EOL。
    ///
    /// 2026-11-25 上机回归：清单里每个条目都带 `eol`（如 `2030-12-31`），按「非空即
    /// EOL」解析会把 13 个 PHP 版本全标成「已 EOL」，含刚发布的默认版本。
    #[test]
    fn future_eol_date_is_not_marked() {
        let body = r#"{"schema": "skiff.server.runtime-index/v1", "kind": "php",
            "channel": "stable", "generated_at": "2026-11-25T10:07:00Z",
            "versions": [
                {"version": "8.6.0", "eol": "2030-12-31", "default": true},
                {"version": "8.5.13", "eol": "2029-12-31"},
                {"version": "8.3.37", "eol": "2020-01-01"}
            ]}"#;
        let parsed = parse_index(KIND_PHP, "stable", body).expect("应能解析");
        assert_eq!(parsed.eol, vec!["8.3.37"]);
        assert_eq!(parsed.default.as_deref(), Some("8.6.0"));
    }

    #[test]
    fn rejects_unknown_schema_major_and_wrong_channel() {
        let body = V1.replace("runtime-index/v1", "runtime-index/v2");
        assert!(parse_index(KIND_PHP, "stable", &body)
            .unwrap_err()
            .contains("不受支持"));
        assert!(parse_index(KIND_PHP, "night", V1)
            .unwrap_err()
            .contains("渠道"));
        assert!(parse_index(KIND_NODE, "stable", V1)
            .unwrap_err()
            .contains("类型"));
    }

    #[test]
    fn still_accepts_legacy_shapes() {
        for body in [
            r#"["8.5.13","8.5.12","8.4.28"]"#,
            r#"{"versions":["8.5.13","8.5.12","8.4.28"]}"#,
            r#"{"tags":["8.5.13-fpm-alpine","8.5.12-fpm-alpine","8.4.28-fpm-alpine"]}"#,
            r#"{"results":[{"name":"8.5.13-fpm-alpine"},{"name":"8.5.12-fpm-alpine"},
                           {"name":"8.4.28-fpm-alpine"},{"name":"latest"}]}"#,
        ] {
            let parsed = parse_index(KIND_PHP, "stable", body).expect("应兼容旧形态");
            assert_eq!(
                parsed.versions,
                vec!["8.5.13", "8.5.12", "8.4.28"],
                "body={body}"
            );
            assert!(parsed.generated_at.is_empty());
        }
    }

    /// Node 的 LTS tag 是单段（`24-alpine`）：剥离后缀后必须认，否则清单永远为空。
    #[test]
    fn node_and_rust_tags_are_understood() {
        let node = r#"{"schema":"skiff.server.runtime-index/v1","kind":"node","channel":"stable",
                       "generated_at":"2026-11-24T12:00:00Z",
                       "versions":[{"version":"24"},{"version":"24.4.0"},{"version":"22"}]}"#;
        let parsed = parse_index(KIND_NODE, "stable", node).expect("node 清单");
        assert_eq!(parsed.versions, vec!["24.4.0", "24", "22"], "数值降序");

        let tags = r#"{"tags":["24-alpine","22-alpine","20-alpine"]}"#;
        let parsed = parse_index(KIND_NODE, "stable", tags).expect("node tag 形态");
        assert_eq!(parsed.versions, vec!["24", "22", "20"]);

        let rust = r#"{"tags":["1.99.1-slim","1.98.0-slim"]}"#;
        let parsed = parse_index(KIND_RUST, "stable", rust).expect("rust tag 形态");
        assert_eq!(parsed.versions, vec!["1.99.1", "1.98.0"]);
    }

    #[test]
    fn trims_to_four_per_major_and_drops_unknown_major() {
        let mut input: Vec<String> = [
            "8.5.13", "8.5.12", "8.5.11", "8.5.10", "8.5.9", "8.5.8", "8.4.28", "8.1.5",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let versions = sort_and_trim(KIND_PHP, &mut input);
        assert_eq!(versions.len(), 5, "8.5 取四个 + 8.4 一个");
        assert_eq!(versions[0], "8.5.13");
        assert!(!versions.iter().any(|v| v == "8.1.5"), "8.1 不在目录");
        assert!(!versions.iter().any(|v| v == "8.5.9"), "超出每大版本上限");
    }

    /// 目录项是单段（Node `24`）或两段（Rust `1.99`）时，清单里的三段补丁号必须能认出来。
    #[test]
    fn patches_under_a_short_cycle_still_match() {
        let mut input: Vec<String> = [
            "24.22.3", "24.22.2", "24.22.1", "24.21.0", "24.20.0", "22.23.5",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let versions = sort_and_trim(KIND_NODE, &mut input);
        assert_eq!(versions.len(), 5, "24 合计取四个（跨 minor）+ 22 一个");
        assert_eq!(versions[0], "24.22.3");
        assert!(
            !versions.iter().any(|v| v == "24.20.0"),
            "24 这一组已到每大版本上限"
        );

        let mut rust: Vec<String> = ["1.99.1", "1.98.0"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            sort_and_trim(KIND_RUST, &mut rust),
            vec!["1.99.1", "1.98.0"],
            "1.99.1 要归到目录项 1.99"
        );
    }

    /// 版本全都不在目录里时按「没有可用版本」报错，让上层回退而不是给个空下拉。
    #[test]
    fn all_unknown_majors_is_an_error() {
        let err = parse_index(KIND_RUST, "stable", r#"["9.9.9"]"#).expect_err("必须报错");
        assert!(err.contains("不在目录"), "{err}");
    }

    /// 内置副本必须与服务端产物同格式：否则「同一份字节」只是口号。
    #[test]
    fn builtin_copy_is_v1_and_ready_to_use() {
        let builtin = builtin_index(KIND_PHP).expect("PHP 必须有内置副本");
        assert!(!builtin.generated_at.is_empty(), "内置副本缺 generated_at");
        assert!(
            builtin.versions.len() >= 3,
            "内置副本至少要覆盖三个大版本：{:?}",
            builtin.versions
        );
        // 首项是最高版本（清单顺序即下拉顺序，默认版本在最前）。不写死具体补丁号：
        // 内置副本随镜像服务刷新，写死就得每刷一次改一次测试。
        let first = &builtin.versions[0];
        assert!(
            builtin
                .versions
                .iter()
                .all(|version| numeric_key(version) <= numeric_key(first)),
            "首项应是最高版本，实际是 {first}：{:?}",
            builtin.versions
        );
        assert!(
            builtin
                .versions
                .iter()
                .all(|v| runtime_catalog::version_supported(KIND_PHP, v)),
            "内置副本里出现目录不支持的大版本：{:?}",
            builtin.versions
        );
        // 只有 PHP 有内置副本：给它造快照会让回滚保护误伤真实产物（见模块注释）。
        assert!(builtin_index(KIND_NODE).is_none());
        assert!(builtin_index(KIND_RUST).is_none());
    }

    /// 回退结果用内置副本而不是空列表：PHP 少一个选项都装不了。
    #[test]
    fn fallback_uses_builtin_copy_for_php() {
        let index = VersionIndex::fallback(KIND_PHP, 0, true, "清单获取失败：连接超时".into());
        assert_eq!(index.source, "builtin");
        let builtin_first = builtin_index(KIND_PHP)
            .expect("PHP 必须有内置副本")
            .versions
            .into_iter()
            .next()
            .unwrap_or_default();
        assert_eq!(
            index.versions.first().map(String::as_str),
            Some(builtin_first.as_str()),
            "回退必须用内置副本的首项（= 默认版本）"
        );
        assert!(index.error.contains("连接超时"));
        assert!(!index.fresh(), "失败结果不能长期命中");
    }

    /// Node / Rust 没有内置副本：回退到目录大版本，且按成功 TTL 缓存（不是失败）。
    #[test]
    fn fallback_uses_catalog_for_node_and_rust() {
        let now = chrono::Utc::now().timestamp();
        let node = VersionIndex::fallback(KIND_NODE, now, false, String::new());
        assert_eq!(node.source, "catalog");
        assert_eq!(node.versions, vec!["26", "24", "22", "20"]);
        assert!(node.error.is_empty());
        assert!(node.fresh(), "目录回退不是失败，应按成功 TTL 缓存");

        let rust = VersionIndex::fallback(KIND_RUST, now, false, String::new());
        assert_eq!(rust.versions.first().map(String::as_str), Some("1.100"));
        assert_eq!(default_index("go").versions, Vec::<String>::new());
    }

    /// 「服务端还没发布」不是故障：回退但按成功 TTL 缓存，避免每 60 秒重打一次。
    #[test]
    fn not_published_is_not_a_failure() {
        let now = chrono::Utc::now().timestamp();
        let index = VersionIndex::fallback(
            KIND_NODE,
            now,
            false,
            "镜像服务未发布 node.json 清单（HTTP 404）".into(),
        );
        assert!(index.fresh(), "404 回退必须按成功 TTL 缓存");
        assert!(index.error.contains("404"), "原因仍要能显示出来");
        assert_eq!(index.source, "catalog");
    }

    #[test]
    fn rejects_body_without_version_tags() {
        let err = parse_index(KIND_PHP, "stable", r#"{"results":[{"name":"latest"}]}"#)
            .expect_err("没有版本 tag 必须报错，才能让上层回退内置副本");
        assert!(err.contains("没有可识别"));
    }

    /// 路径固定：镜像基址首项 + 渠道目录 + 类型文件名；基址或渠道为空表示不联网。
    #[test]
    fn index_url_is_fixed_path_under_channel() {
        assert_eq!(
            fixed_index_url(Some("http://10.66.52.7:8080/"), "stable", KIND_PHP).as_deref(),
            Some("http://10.66.52.7:8080/channels/stable/runtime-index/php.json")
        );
        assert_eq!(
            fixed_index_url(Some(" http://mirror/skiff "), "night", KIND_NODE).as_deref(),
            Some("http://mirror/skiff/channels/night/runtime-index/node.json")
        );
        assert_eq!(
            fixed_index_url(Some("http://mirror"), "stable", KIND_RUST).as_deref(),
            Some("http://mirror/channels/stable/runtime-index/rust.json")
        );
        assert_eq!(
            index_file(KIND_RUST),
            Some("rust.json"),
            "文件名是服务端产物的契约"
        );
        assert!(
            fixed_index_url(None, "stable", KIND_PHP).is_none(),
            "mirrors 为空即不联网"
        );
        assert!(fixed_index_url(Some("  "), "stable", KIND_PHP).is_none());
        assert!(
            fixed_index_url(Some("http://m"), " ", KIND_PHP).is_none(),
            "渠道为空不拼半截地址"
        );
        assert!(
            fixed_index_url(Some("http://m"), "stable", "go").is_none(),
            "未实现的类型不拼地址"
        );
    }
}
