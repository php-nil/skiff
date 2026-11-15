# 内核自身架构体检（七维）

> 状态：**现行审计稿**（2026-11-15，只读盘点；基线 `35477e2`，工作区另有他人未提交改动，未参与判定）。
> 范围：**只从内核自身角度**看 `crates/` 与 `xtask/`（16 个 crate + 1 个工具），不含前端/模板/静态资源实现细节，不逐个点评官方插件；插件**体系**（manifest、签名、Supervisor、能力面划分）在范围内。
> 与既有文档的关系：`ARCH_REVIEW.md` 是历史六轮审查（14 crate 基线，行号已腐化，只读存档）；[KERNEL_UI_BOUNDARY_AUDIT.md](KERNEL_UI_BOUNDARY_AUDIT.md) 是内核↔UI 边界专项（本轮只做增量，不重复其 18 条）。本文是**内核七维体检 + 可执行批次**。
> 方法：逐 crate 读 `Cargo.toml` 与模块树、按关键词定位（`unwrap/expect/panic`、`Command::new`、`timeout`、`trait`、`allow(dead_code)`、`fs::rename`、`nginx -t` 等），对关键结论逐条打开源码核对行号。**未运行任何 cargo 命令**，行为相关结论均标注验证方式。

## 0. 一页结论

| 指标 | 实测值 |
| --- | --- |
| crate 数 | 16（根 workspace）+ `xtask` + `tools/skiff-server` |
| Rust 源文件 / 行数 | 216 个 / 115,937 行（含测试） |
| 体量前三 | `web` 35,107、`app` 34,198、`plugin` 9,548（行） |
| 本文问题数 | **18 条**：必须改 5、值得改 9、可选 4 |
| 生产路径 panic 点 | 约 30 处（其余 `unwrap/expect` 全在 `#[cfg(test)]` 内） |
| Cargo feature 开关 | **0 个**（全仓无 `[features]`） |
| 依赖环 | **无**（`app`/`web`/`core` 单向） |

**总判断：分层与防错纪律是好的，问题集中在「边界没有写下来」和「同一机制有第二份实现」。**

1. **架构红线没有漂移，但依赖表已经失真**：`docs/sections/08-engineering.md` 的依赖表仍是 14 crate 时代的口径，`app` 实际还依赖 `pkgfmt/sched/fs`（并直接用 `ureq`/`rustls`/`ed25519-dalek` 出网），`web` 实际依赖 `sched` 却**一行都没用**。红线靠这张表评审，表错了评审就失效。
2. **两处「同一机制两份实现」值得立刻收口**：① 特权执行通道 `run_host` 在 `core/kernel/ftp.rs` 与 `core/kernel/firewall.rs` 逐行重复（安全相关代码双份）；② 能力面权限通配 `"<cap>:*"` 的匹配在 ftp、firewall 两处各写一遍，而 `plugin::capability::has_perm` 只做精确匹配，语义并不统一。
3. **异步路径上还有同步阻塞点**：证书上传在 async worker 上直接 fork `openssl` 四次且无超时（同文件的「生成自签证书」却正确用了 `spawn_blocking`）；`systemctl restart docker` 本身无超时。`worker_threads=2`，这类阻塞的代价被放大。
4. **能力面实现分布与 AGENTS 红线不一致**：AGENTS 写「插件能力面实现只在 `crates/plugin`」，但 `scheduler` 能力面唯一实现在 `app/tasks.rs`，`ftp`/`firewall` 的特权执行在 `core`。**事实是合理的（依赖方向所迫），错的是没有把例外写下来。**
5. **旧审计「JSON 依赖 SSR 展示模型」只修了一半**：`ui/runtimes.rs`、`ui/mod.rs` 仍以 `templates::RuntimeRow` / `NetworkRowView` / `PluginView` 为输入。

**若只能先做三件事**：删 `web` 的死依赖 → 合并 `run_host` → 把证书校验移出 async worker。三项都零对外契约变化。

---

## 1. 七维结论表

| 维度 | 结论 | 关键证据 |
| --- | --- | --- |
| 1 模块划分 | **基本合理，两个过热文件** | `app/src/sites.rs` 类型/布局/安装锁/进度混放；`store/src/db.rs` 连接+20 版迁移+备份同文件 |
| 2 职责边界 | **主线清楚，边界靠注释维持** | `app` 自述「不含表现层」（[lib.rs](file:///c:/dev/skiff/crates/app/src/lib.rs#L1-L5)），但 `views/sites.rs` 输出中文文案与 CSS class |
| 3 依赖关系 | **无环、无 feature 分叉，但文档失真** | `web` 依赖 `skiff-sched` 零引用；`08-engineering.md` 表缺 `app→pkgfmt/sched/fs`、`plugin→axum` |
| 4 复用性 | **有三类真实重复** | `run_host` ×2、权限通配匹配 ×2（+1 处语义不同）、`nginx -t` 候选校验 ×2 |
| 5 过度设计 | **轻微，只有一处明显空转发** | `web/src/menu.rs` 6 行纯 `pub use`；其余单实现 trait 均有测试替身或依赖倒置理由 |
| 6 与 UI 解耦 | **读模型已下沉，但展示属性上浮** | `app/src/views/*` 是 SSR/JSON 共用读模型（好）；`status_class`/`status_label` 留在 app（坏） |
| 7 健壮性 | **整体强，仍有 4 个具体缺口** | 生产 panic 少；缺口＝阻塞调用、systemd 超时残留、同步审计、docker 重启无超时 |

---

## 2. 问题清单

> 每条格式：现象与位置 → 为什么是问题 → 影响面 → 修法建议 → 风险与验证 → 优先级。
> 优先级三档：**必须改**（正确性/红线/零风险收益）、**值得改**（一致性或可维护性，改动小）、**可选**（收益有限或需先设计）。

### 必须改

#### 问题 1：`web` 声明了 `skiff-sched`，但全 crate 零引用

**现象**：[crates/web/Cargo.toml](file:///c:/dev/skiff/crates/web/Cargo.toml#L16) 有 `skiff-sched = { workspace = true }`；对 `crates/web` 全目录递归检索 `skiff_sched`，命中 **0** 次（`src/`、`tests/` 都查过）。

**为什么是问题**：依赖表是架构红线的评审入口（`08-engineering.md` §依赖方向）。一条不存在的依赖会让「谁依赖谁」的判断长期带噪；也让 `web` 的编译单元多拉一个下层 crate（含 chrono/cron），与「占用低」目标相反。

**影响面**：编译依赖图、CI 缓存、依赖评审。

**修法**：删除该行；同时用 `cargo tree -p skiff-web` 复核没有传递性隐形使用（`skiff-app` 已转出任务相关只读结构，`web` 不需要直接依赖）。

**风险与验证**：零风险。`cargo check -p skiff-web --all-targets` + 全量测试。

**优先级**：必须改。

#### 问题 2：特权执行通道 `run_host` 在 FTP 与防火墙两处逐行重复

**现象**：
- [ftp.rs](file:///c:/dev/skiff/crates/core/src/kernel/ftp.rs#L255-L323) `run_host`：`systemd-run --wait --pipe --collect` + stdin 管道 + `timeout` + `wait_with_output`，单元名前缀 `skiff-ftp-`。
- [firewall.rs](file:///c:/dev/skiff/crates/core/src/kernel/firewall.rs#L174-L242) `run_host`：同上，逐行同构，仅单元名前缀 `skiff-firewall-` 与 `--description` 文案不同。

**为什么是问题**：这是**以 root 身份、绕过主服务沙箱执行宿主命令**的唯一通道。两份实现意味着以后补「超时后清理瞬态单元」「统一输出截断」「统一审计」时，很容易只改一处；而这类差异不会编译失败、也不会被现有测试抓到（两份都只在 Linux 上跑）。

**影响面**：FTP 与防火墙两个能力命令面的安全与可观测性；任何对执行通道的加固（见问题 10）。

**修法**：在 `crates/core/src/kernel/` 下新增 `host_run.rs`（同 crate 内，模块可见性 `pub(super)`），签名固定为
`run_host(prefix: &str, description: &str, cmd, args, stdin, timeout_secs) -> Result<CommandOutput, String>`；
`ftp.rs` / `firewall.rs` 的 `Host::run` 各留一行转调。保持两处 `CommandOutput` 类型不变（分别是 `skiff_plugin::ftp` / `firewall` 的类型，只需在调用处转换字段）。

**风险与验证**：纯移动 + 参数化；风险在于两种 `CommandOutput` 是不同 crate 的类型，需一处字段映射。验证：`cargo check -p skiff --all-targets`（Linux 目标的 clippy 在 CI 上复核 `cfg(target_os="linux")` 分支）；现有单测不覆盖该分支 → 在 `crates/core/src/kernel/tests.rs` 补一条「同一 helper 生成的 argv 以 `--` 分隔命令与参数」的纯函数测试（把 argv 拼装抽成纯函数后即可在 Windows 验证）。

**优先级**：必须改。

#### 问题 3：证书上传在 async worker 上同步 fork `openssl`，且无超时

**现象**：[sites/crud.rs](file:///c:/dev/skiff/crates/app/src/sites/crud.rs#L1306-L1356) `upload_cert` 是 `async fn`，其中 `std::fs::write`（1332/1334）与 `verify_cert_pair`（1338）都在调用线程上同步执行；`verify_cert_pair` 内部 `std::process::Command::output()` 连续 fork `openssl` **4 次**（[sites.rs](file:///c:/dev/skiff/crates/app/src/sites.rs#L280-L303)），没有任何超时。同文件的 `generate_self_signed` 则正确地把 `openssl` 放进 `tokio::task::spawn_blocking`（[crud.rs](file:///c:/dev/skiff/crates/app/src/sites/crud.rs#L1261-L1284)）。

**为什么是问题**：`worker_threads=2`（`DESIGN` §0 显式配置）。一次挂住的 `openssl`（私钥带口令交互、熵不足、NFS 卡住）会永久占掉一半运行时；两次并发上传即可让面板所有请求停摆，而 HTTP 30 秒超时只会把响应变成 504，阻塞并不会消失。

**影响面**：证书上传、以及任何复用 `verify_cert_pair` 的路径；可用性（不是安全性）。

**修法**：把「写临时文件 + `verify_cert_pair` + 失败清理」整段包进 `spawn_blocking`（与 `generate_self_signed` 完全一致的形状），`async fn` 里只等待结果。`openssl` 调用本身可加 `timeout`（std 无原生支持时用 `tokio::process` 或 `spawn_blocking` + 外部 `timeout` 语义），最小改动是只做 `spawn_blocking`。

**风险与验证**：行为不变（同输入同输出）。验证：`cargo test -p skiff-app`（`sites/tests.rs` 已有证书成对写入与失败回滚用例），并新增一条「`upload_cert` 在校验失败时清理两个临时文件」的用例（当前该路径 1339-1344 已有逻辑，但没有断言）。

**优先级**：必须改。

#### 问题 4：依赖表与实现漂移，红线评审入口失真

**现象**：`docs/sections/08-engineering.md` 的依赖表（[08-engineering.md](file:///c:/dev/skiff/docs/sections/08-engineering.md#L51-L67)）与实际 `Cargo.toml` 不一致：

| crate | 文档写 | 实际还依赖 |
| --- | --- | --- |
| `app` | `model`、`store`、`identity`、`docker`、`plugin` | `pkgfmt`、`sched`、`fs`、`ureq`、`webpki-roots`、`rustls-pemfile`、`ed25519-dalek`、`sha2`、`tar` |
| `web` | `app`、`model`、`store`、`identity`、`docker`、`plugin`、`pkgfmt` | `sched`（未使用，见问题 1） |
| `plugin` | `pkgfmt`、`wire`、`store`、`model` | `axum`、`http-body-util`、`tokio`、`libc` |
| `core` | 「以上全部」 | 另有 `rustls`、`axum-server` |

**为什么是问题**：AGENTS 与 §8 都把「依赖方向单向」列为硬红线，而这根红线的可执行形式就是这张表。表缺项之后：`app` 已经变成「装配层 + 站点域 + 目录客户端（唯一出网组件）+ 菜单/主题」的复合体，却仍被描述成 `AppState` 加登录流程；评审时看不出 `app` 的重量级依赖。

**影响面**：所有后续架构评审、新人理解、依赖升级（`ureq`/`rustls` 的安全更新责任落在哪个 crate）。

**修法**：更新表，并补两行事实说明：① `app` 是唯一出网 crate（商店目录客户端），依赖 `ureq`/`rustls-pemfile`；② `plugin` 依赖 `axum` 仅用于能力面 handler 的 HTTP 形状（判定层 `fsops` 刻意不依赖它，见文件注释）。**只改文档，不动代码。**

**风险与验证**：零代码风险。核对方式：逐 crate `Get-Content Cargo.toml` 对照，本文附录 A 已给出实测清单。

**优先级**：必须改。

#### 问题 5：能力面实现分布与 AGENTS 红线口径冲突（事实合理，文档缺例外）

**现象**：
- AGENTS「单一来源」写：插件能力面实现**只在** `crates/plugin`（[AGENTS.md](file:///c:/dev/skiff/AGENTS.md) 三、红线）。
- 实际分布：`fs`/`interop` 判定在 `plugin/src/capability.rs`、`fsops.rs`、`interop.rs`；`ftp`/`firewall` 的**判定与编排**在 `plugin/src/ftp.rs`、`firewall.rs`，但**特权执行**在 `core/src/kernel/{ftp,firewall}.rs`；`scheduler` 能力面的**唯一实现**在 [app/tasks.rs](file:///c:/dev/skiff/crates/app/src/tasks.rs#L1-L16)（`tasks.capability(...)`，被 [core/kernel/capability.rs](file:///c:/dev/skiff/crates/core/src/kernel/capability.rs#L182) 调用）；`docker`/`identity`/`config` 在 core。

**为什么是问题**：代码分布有充分理由（`plugin` 不能依赖 `app`/`sched`，也不能持有 root 执行通道），但红线文字与事实相反。后果是：下一轮评审会把这个「违规」当新问题重提；或者有人真的把 scheduler 能力面搬进 `plugin`，从而让 `plugin → sched` 增量依赖悄悄发生。

**影响面**：架构评审效率、后续能力面新增时的落位判断。

**修法**：把红线改为可判定的表述：「**能力面的权限判定与命令编排**只在 `crates/plugin`；HTTP 形状与宿主特权执行属于内核装配（`core`/`app`），逐项登记在表里」。同时在 `docs/sections/08-engineering.md` 增一张「能力面 × 实现位置」小表（fs/interop → plugin；ftp/firewall → plugin 判定 + core 执行；scheduler → app；docker/identity/config → core）。**只改文档。**

**风险与验证**：零代码风险。验证：按表逐条跳到源码确认函数存在（本文已核对）。

**优先级**：必须改。

---

### 值得改

#### 问题 6：能力面权限通配匹配两处重复、一处语义不同

**现象**：
- [ftp.rs](file:///c:/dev/skiff/crates/core/src/kernel/ftp.rs#L118-L120)：`g.kind == "perm" && (g.value == "ftp:*" || g.value == format!("ftp:{method}"))`
- [firewall.rs](file:///c:/dev/skiff/crates/core/src/kernel/firewall.rs#L51-L53)：同构，`firewall:*`
- [plugin/capability.rs](file:///c:/dev/skiff/crates/plugin/src/capability.rs#L68-L76)：`has_perm` **只做精确匹配**，不支持 `fs:*`

**为什么是问题**：同一份「授权项如何命中方法」的规则，三处两种语义。写授权 UI 或安装确认时，`ftp:*` 会放开全部 FTP 方法，而 `fs:*` 不会放开任何 `fs` 方法（必须逐条授权）——这个差异没有任何文档，只能读代码发现。

**影响面**：插件安装时的授权确认、能力面拒绝审计的正确性、新增能力面的实现成本。

**修法**：在 `skiff-model`（`Grant` 已有词汇）或 `skiff-policy` 提供纯函数 `perm_allows(grant_value: &str, requested: &str) -> bool`，规则显式写成「精确命中或 `prefix:*`」；`ftp`/`firewall`/`has_perm` 三处改为调用它。**先补 3 条单测锁定语义**（精确、通配、不匹配），再替换。

**风险与验证**：如果刻意让 `fs` 不支持通配，则应在函数里显式表达（例如按能力面白名单），而不是靠「没实现」。验证：`cargo test -p skiff-policy`（零 Docker、零 SQLite，符合 AGENTS §四建议）+ `cargo test -p skiff-plugin`。

**优先级**：值得改。

#### 问题 7：`app` 的职责边界没有定义，站点域已占 10.5k 行

**现象**：`app` 34,198 行 / 47 文件；站点相关 6 个文件合计约 10.5k 行：`sites.rs`(1327)、`sites/crud.rs`(2578)、`sites/runtime.rs`(2960)、`runtime_catalog.rs`(1819)、`nginx.rs`(1363)、`sites_vhost.rs`(514)。而 `app/src/lib.rs` 的模块表（[lib.rs](file:///c:/dev/skiff/crates/app/src/lib.rs#L7-L27)）只列名字，没有一句话说明「什么该进 app、什么该去别处」；`08-engineering.md` 的 `app` 描述仍是「应用上下文（AppState/WebConfig）与用例层（登录流程）」。

**为什么是问题**：没有准入规则时，默认行为就是「能编译就塞 app」。站点域已经是事实上的「第二核心业务域」，但它在概念上没有名字，因此没有独立评审对象；`app/theme.rs`（294 行，纯表现规则）就是这种默认行为的产物（见问题 9）。

**影响面**：新增业务域时的落位、代码评审粒度、`app` 编译时间。

**修法**（建议按此顺序，不拆 crate）：
1. 在 `app/src/lib.rs` 顶部写准入规则：**用例编排、领域服务、读模型**可以进；**路由/模板/HTTP 形状**不进；**跨域通用词汇**进 `model`。
2. 把站点域在文档里命名为「站点域（sites）」，并在 `08-engineering.md` 的 `app` 描述里列出域清单（sites / nginx / tasks / catalog / files-admin / menu / theme）。
3. 代码层面只做「文件内分层」：把 `sites.rs` 中与类型无关的部分（`InstallLocks`/`InstallProgress`/`SiteEnvStatus`）迁到 `sites/install.rs`（纯移动）。

**风险与验证**：第 3 步是纯移动，`sites.rs` 的 `pub use` 保持对外路径不变；验证 `cargo check -p skiff-app --all-targets` 与 `cargo test -p skiff-app`。

**优先级**：值得改（文档优先，代码可选）。

#### 问题 8：`sites.rs` 与 `sites_vhost.rs` 双向再导出，模块切分按历史批次而非职责

**现象**：[sites_vhost.rs](file:///c:/dev/skiff/crates/app/src/sites_vhost.rs#L1-L16) 只含纯函数，但注释写明「`sites.rs` 用 `pub use crate::sites_vhost::{...}` 再导出，上层仍按 `skiff_app::sites::*` 旧路径调用」；同时 `sites_vhost.rs` 又 `use crate::sites::{default_php_name, runtime_container_socket, PhpRuntimeConfig, ...}`。两个模块互相引用（同 crate，不是循环依赖，但形成认知上的双向）。

**为什么是问题**：读代码时无法从模块名判断「vhost 渲染到底在哪」；新增站点配置项时，需要同时理解两边的再导出关系。这也让「纯函数可脱开装配层单测」的收益被旧路径掩盖。

**影响面**：站点/证书/vhost 相关改动的心智成本；后续把站点域进一步模块化的成本。

**修法**：保留 `sites_vhost` 作为唯一实现，删除 `sites.rs` 的再导出，调用方直接 `crate::sites_vhost::validate_domain`；或反向——把纯函数并入 `sites/vhost.rs` 子模块。两种都是一次机械替换，**建议选后者**（与 `sites/{crud,runtime,layout,logs,spec}` 的子模块命名一致）。

**风险与验证**：纯路径替换，编译器会指出所有调用点；`cargo test -p skiff-app`（`sites/tests.rs` 已覆盖 vhost 渲染）。

**优先级**：值得改。

#### 问题 9：读模型带上了展示文案与 CSS class

**现象**：[app/src/views/sites.rs](file:///c:/dev/skiff/crates/app/src/views/sites.rs#L11-L27) 把领域状态映射为 `("已停止", "")`、`("运行中", "ok")`、`("环境异常", "danger")`，并放进 `SiteSummary.status_label` / `status_class`（同文件 171-203 行还有 HTTPS 的文案与 class）。`app/src/lib.rs` 开头明确「这里不放路由、模板和中间件」（[lib.rs](file:///c:/dev/skiff/crates/app/src/lib.rs#L1-L5)）。

**为什么是问题**：`app` 是 SSR 与 `/ui/v1` 的共同下层。把中文文案和 CSS 类名放进去，等于让 JSON API 的字段值依赖前端样式词表；换文案或换配色体系会触及「非表现层」crate 并影响 JSON 消费者。当前 `SiteSummary` 被两边共用，改一处两边同时变，这种耦合不容易被发现。

**影响面**：站点列表 SSR/JSON 文案；主题/样式调整的波及面。

**修法**：读模型只保留**领域枚举**（`SiteRuntimeStatus` 已在 `sites/spec.rs` 定义，是好的），把 `status_label`/`status_class` 的映射移到 `web`（SSR 模板与 `/ui/v1` DTO 各取所需，但共用同一张映射表也行——关键是它在 `web` 内）。属于「同一操作两套编排」的反面：读模型共享、展示映射上移。

**风险与验证**：JSON 字段值不变（映射结果保持逐字节一致），只是搬家。验证：`crates/web/tests/main_flow.rs` 中站点列表相关断言（含冻结字段用例）+ SSR 页面渲染断言。

**优先级**：值得改。

#### 问题 10：`systemctl restart docker` 无超时

**现象**：[docker_daemon.rs](file:///c:/dev/skiff/crates/app/src/docker_daemon.rs#L157-L177)：`tokio::process::Command::new("systemctl").args(["restart","docker"]).status().await` 没有超时；之后 `docker_ready` 最多 8 次 `list_containers`（每次 bollard 默认 30 秒），最坏可拖到约 4 分钟。HTTP 侧 30 秒后已返回超时，但后台仍持锁、仍在重试与回滚。

**为什么是问题**：用户看到「保存失败/超时」时，实际可能正在重启 Docker 或已回滚——状态与提示不一致；且 `MirrorApply` 的返回被丢掉后，审计/页面都拿不到最终结论。这是「超时语义」与「真实副作用」不一致的典型。

**影响面**：设置页镜像源保存；极端情况下 Docker 被反复重启（`restart_docker` 失败后再次调用，134 行）。

**修法**：`status()` 外套 `tokio::time::timeout(60s)`，超时返回「systemctl 未在 60 秒内返回（Docker 可能正在重启）」，并跳过后续重试；把「已发起重启但结果未知」作为显式状态返回（不改对外字段，只改内部 `Result` 文案与日志）。同时 `docker_ready` 的 8 次循环改为总预算（例如 30 秒）而不是「8 × 30 秒」。

**风险与验证**：需要 Docker 宿主机验证（测试机）。验证：手动把 docker 服务设为不可启动，确认接口在 60 秒内返回而不是拖到 4 分钟；补一条「systemctl 超时路径返回明确错误」的单测（把命令执行抽成可注入的闭包）。

**优先级**：值得改。

#### 问题 11：`systemd-run` 超时后只杀客户端，瞬态单元可能继续执行

**现象**：`run_host`（[ftp.rs](file:///c:/dev/skiff/crates/core/src/kernel/ftp.rs#L287-L317)、[firewall.rs](file:///c:/dev/skiff/crates/core/src/kernel/firewall.rs#L206-L236)）使用 `--wait --pipe` + `kill_on_drop(true)` + `tokio::time::timeout`。超时发生时，`kill_on_drop` 只能终止本地 `systemd-run` 客户端；`--collect` 的瞬态单元在 systemd 里仍然运行（这是 `systemd-run --wait` 的语义）。

**为什么是问题**：能力面收到「超时失败」，但宿主机命令可能已经部分生效（例如 pure-ftpd 配置写了一半、fail2ban 规则已加）。调用方会按「失败」重试或回滚，而真实状态是「进行中」。这是重启/中断后状态一致性最容易出问题的一类。

**影响面**：FTP/firewall 能力调用的结论可信度；重试的幂等性依赖各命令自身。

**修法**：超时分支里先 `systemctl stop <unit>`（或 `systemd-run --unit=...` 记录 unit 名后 `reset-failed`），再返回错误；错误文案区分「命令超时且已尝试终止」。**需要 Linux 验证**，且要确认 `--wait` 被 kill 后 unit 名仍可解析（unit 名由 `SEQ` 生成，已在本地变量中）。

**风险与验证**：属于特权路径改动，需在测试机用一条 `sleep 120` 的假命令验证：超时后 `systemctl list-units 'skiff-*'` 无残留。**本轮只列方案，不做。**

**优先级**：值得改（需上机验证，故不进零契约批次）。

#### 问题 12：审计写入在 async handler 上同步抢单连接锁

**现象**：`AppState::audit_or_log`（[lib.rs](file:///c:/dev/skiff/crates/app/src/lib.rs#L416-L434)）与 `CapabilityHost::audit` 都是同步函数，内部 `db.audit(...)` 走 `Db` 的**单连接 `std::sync::Mutex`**；它们在 async handler 中被直接调用（例如 [core/kernel/capability.rs](file:///c:/dev/skiff/crates/core/src/kernel/capability.rs#L227-L235)、[ftp.rs](file:///c:/dev/skiff/crates/core/src/kernel/ftp.rs#L122-L131)、[firewall.rs](file:///c:/dev/skiff/crates/core/src/kernel/firewall.rs#L109-L116)）。`Db::conn_lane` 在等待超过 50ms 时会 `warn`（[db.rs](file:///c:/dev/skiff/crates/store/src/db.rs#L101-L115)），说明作者已知这条路径会排队。

**为什么是问题**：`worker_threads=2`；一次磁盘抖动或一个长事务（备份、VACUUM 走 `maintenance` 车道）会让审计调用阻塞 worker。它不丢数据，也不越权，但把「存储抖动」放大成「页面无响应」。`app/src/db_async.rs` 已经提供了投递到阻塞池的机制，审计路径却没有用。

**影响面**：所有写操作后的审计（几乎每个 handler 末尾）；极端情况下 2 个 worker 同时卡在审计上。

**修法（需先定口径）**：两种方案，二选一，不要混用：
- A：`audit_or_log` 改成 `async fn` 内部 `db_async::run`，调用点加 `.await`——改动面大但语义最干净；
- B：保留同步函数，但在其中 `tokio::task::block_in_place`（仅 multi-thread runtime 可用）——改动最小，但会临时扩容 worker，与「显式 2 线程控内存」的目标相冲突，需在文档里说明取舍。

建议 A，但先确认「审计必须在响应前落库」这一现行语义是否要继续（现在确实是同步写，失败只记日志）。

**风险与验证**：改变调用时序（仍然在响应前）。验证：`cargo test --workspace`；加一条慢库用例（人为持锁 200ms）确认其它请求不被拖住——需要可注入的 `Db` 等待钩子，成本中等。**本轮只列方案。**

**优先级**：值得改（需设计取舍）。

#### 问题 13：旧审计问题 3 未闭环——JSON 仍依赖 SSR 展示模型

**现象**：[handlers/ui/runtimes.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui/runtimes.rs#L33) 与 `:152` 调 `handlers::sites::runtime_row(...)`，返回 [templates::RuntimeRow](file:///c:/dev/skiff/crates/web/src/templates.rs#L370)；[handlers/ui/mod.rs](file:///c:/dev/skiff/crates/web/src/handlers/ui/mod.rs#L181) `network_item(v: crate::templates::NetworkRowView)`、`:313` `plugin_item(view: crate::templates::PluginView)`。

**为什么是问题**：`KERNEL_UI_BOUNDARY_AUDIT.md` 问题 3 的结论仍然成立——JSON 的输入是「模板行结构」，模板字段为渲染服务（文案、截断、CSS class），删/改模板字段会同时改 API 输出。批次 0 只完成了拆分，没有完成这层的反转。

**影响面**：运行环境、网络、插件三类 JSON 的稳定性；budding 字段冻结（只增不改）无法靠编译期保证。

**修法**：把 `runtime_row` / `network_view` / `plugin_view` 的**聚合结果**上移为一个与模板无关的读模型（可放 `app::views`，与问题 9 的「展示映射上移」相反方向：数据上移、文案下移），模板与 DTO 各自映射。这是 `KERNEL_UI_BOUNDARY_AUDIT.md` 批次 1.2「抽 app 单一读取入口」的延续。

**风险与验证**：字段值必须不变。验证：`/ui/v1` 冻结字段用例 + SSR 渲染断言。

**优先级**：值得改（与旧审计 1.2 合并处理，不单独开工）。

#### 问题 14：`nginx -t` 候选校验有两套

**现象**：
- [nginx.rs](file:///c:/dev/skiff/crates/app/src/nginx.rs#L668-L706)：主配置走「候选文件 → 容器内 `nginx -t -c candidate` → 失败回滚 → 解析告警行」。
- [sites/crud.rs](file:///c:/dev/skiff/crates/app/src/sites/crud.rs#L2320-L2376)：vhost 走「校验目录 + 候选 .conf + 临时 main → `nginx -t -c` → 清理 → `error_line`/`diff_summary`」。

**为什么是问题**：两者共享同一套语义（候选、容器内校验、错误行解析、清理），但各自实现；新增一种「需要校验的配置」（例如未来 PHP 池）会再抄一遍。差异也是真实的（vhost 需要包一层 main），所以不能简单合并，需要参数化。

**影响面**：配置校验路径的一致性；清理逻辑（临时文件残留）现在有两份。

**修法**：抽 `validate_in_nginx(container_main_path, extra_include: Option<&str>) -> Result<Warnings, SiteError>`，把候选目录/临时文件生命周期收在一处；两处调用点只决定「候选内容怎么来」。属于零对外契约，但改动面在站点核心路径，建议单独一批并配回归用例。

**风险与验证**：`cargo test -p skiff-app`（已有 nginx reload 回退/候选清理用例）+ 测试机保存一次主配置与一次 vhost。

**优先级**：可选（收益真实，但当前两份都能工作；排在问题 9/13 之后）。

---

### 可选

#### 问题 15：`write_if_changed` 与 `write_atomic` 各写一遍 tmp+chmod+rename

**现象**：[sites/layout.rs](file:///c:/dev/skiff/crates/app/src/sites/layout.rs#L255-L278) 自己实现 `sibling_temp` + `write` + `set_permissions` + `rename`；[atomic.rs](file:///c:/dev/skiff/crates/app/src/atomic.rs#L24-L40) 已有 `write_atomic(path, bytes, mode)`。两者在同 crate，前者只多了「内容相同则跳过」。

**修法**：`write_if_changed` 保留「读旧内容比较」的前置判断，然后直接调 `crate::atomic::write_atomic`（错误信息补 path 上下文）。**同 crate 内，零契约。**

**风险与验证**：错误文案变化只在内部日志/表单提示；`sites/tests.rs` 已有 `write_if_changed` 行为用例。

**优先级**：可选（属于顺手整理）。

#### 问题 16：`web/src/menu.rs` 是 6 行空转发层

**现象**：[menu.rs](file:///c:/dev/skiff/crates/web/src/menu.rs) 全文只有 `pub use skiff_app::menu::*;`，注释说「路径兼容的再导出」；`web` 内 23 处 `crate::menu::...` 调用。

**为什么是可选项而不是问题**：它的存在有历史原因（菜单领域下移到 `app` 时避免一次大 diff），且没有行为。但它属于用户点名的「空转发层」，会让 `grep menu` 在两个 crate 都命中。

**修法**：一次机械替换为 `skiff_app::menu::` 并删除文件。收益低、diff 分散（20+ 文件），**建议与其它 web 改动合并时顺手做，不单独提交**。

**优先级**：可选。

#### 问题 17：`image_versions` 负缓存无上限、不随镜像删除清理

**现象**：[app/lib.rs](file:///c:/dev/skiff/crates/app/src/lib.rs#L116-L120) `image_versions: Mutex<HashMap<String, (Option<String>, Instant)>>`；键是 `image_id`，注释说明不可变，但没有容量上限与删除钩子。

**为什么只是可选**：键空间等于「曾经出现过的镜像 id」，单机通常几十到几百，内存量级可忽略；删除镜像后条目会残留但不会被读到（除非 id 复用，实际不会）。

**修法**：加容量上限（例如 512，与 `EXEC_TICKET_MAX` 同风格），或删除镜像时 `retain`。**先不做**，等有实测数据。

**优先级**：可选。

#### 问题 18：`store/src/db.rs` 连接管理 + 20 版迁移 + 备份同文件

**现象**：`db.rs` 1137 行；`CURRENT_SCHEMA_VERSION = 20`（[db.rs](file:///c:/dev/skiff/crates/store/src/db.rs#L38-L39)），迁移 SQL 与连接/备份逻辑同处一文件，而仓储方法已按聚合拆到 `db/*.rs`（19 个子模块）。

**为什么只列可选**：这是「已按正确方向拆过一半」的文件；迁移 SQL 是只增不改的流水，独立成文件后 `db.rs` 会回到连接/备份的本职。但纯移动收益有限，且是冲突热点（schema 变更频繁）。

**修法**：`db/migrate.rs` 纯移动，`db.rs` 只保留 `open/configure/conn/backup`。与下一次 schema 变更一起做，避免两次冲突。

**优先级**：可选。

---

## 3. 非问题清单（下轮不要重复讨论）

| # | 候选点 | 为什么不是问题 |
| --- | --- | --- |
| N1 | 14 个 trait 里多数只有一个生产实现（`CapabilityHost`→`AppState`、`RoleExt`→`Role`、`ParseAction`→`String`） | `CapabilityHost` 是**依赖倒置的必要抽象**（`plugin` 不能依赖 `app`，否则成环，见 [capability.rs](file:///c:/dev/skiff/crates/plugin/src/capability.rs#L14-L16)）；`Clock`/`Executor`/`TaskStore`/`Transport`/`FtpHost`/`FirewallHost` 都有测试替身（本次逐个 grep 确认 impl 数 ≥2）；`RoleExt` 是扩展方法惯用法，删除只会把调用点变长。**建议全部保留。** |
| N2 | 重复提交/幂等 | `JobService::submit` 的同目标唯一由 **SQLite 部分唯一索引**兜底，并在 [store/src/db/jobs.rs](file:///c:/dev/skiff/crates/store/src/db/jobs.rs#L135-L192) 映射为 `JobError::Busy`；[app/src/jobs.rs](file:///c:/dev/skiff/crates/app/src/jobs.rs#L726-L751) 有用例。不是靠调用方自觉。 |
| N3 | 生产代码大量 `unwrap` | 精确统计（排除 `#[cfg(test)]` 之后）：生产路径约 30 处，全部在不变量之后（`serde_json` 对纯结构、静态 `HeaderValue` 常量、`Arc::try_unwrap` 刚构造、`ChildGuard` Drop）。`store`/`plugin`/`web` 的热路径没有裸 `unwrap`。**纪律良好。** |
| N4 | `cfg(unix)` / `cfg(target_os="linux")` 分叉风险 | 全仓无 Cargo feature；平台分支只有两类：非 unix 的 stub（`kernel::spawn` 打日志返回）与 linux-only 的宿主能力（ftp/firewall/降权）。Windows CI 跑 `cargo test --workspace`（[ci.yml](file:///c:/dev/skiff/.github/workflows/ci.yml#L128-L149)），Linux 跑 clippy `-D warnings`。**风险已被 CI 双向覆盖。** |
| N5 | 重启后状态一致性 | 三条链路都有明确收尾：作业中断结算（`JobService::recover_interrupted`，[jobs.rs](file:///c:/dev/skiff/crates/app/src/jobs.rs#L258-L282)）、插件停服（`stop_all`，[main.rs](file:///c:/dev/skiff/crates/core/src/main.rs#L210-L211) + systemd `KillMode=mixed`）、安装锁只在进程内（重启即丢，但作业已标中断）。**剩余的是「业务半完成态」，契约已声明需人工核对，不是缺陷。** 建议只在 `TEST_MATRIX` 补一条「安装中重启」用例。 |
| N6 | 请求超时口径 | 非流式 30 秒（[middleware.rs](file:///c:/dev/skiff/crates/web/src/middleware.rs#L43-L62)），SSE/WS 不挂（正确）；Docker 客户端默认 30 秒（[client.rs](file:///c:/dev/skiff/crates/docker/src/client.rs#L325)），探活 3 秒。**分层正确。** |
| N7 | `open_dir_nofollow` 是 `#[allow(dead_code)]` | 作者注释明确「尚未接入 `validate_bind_source`，保留实现是为了不在下次重构时重新发明」（[paths.rs](file:///c:/dev/skiff/crates/policy/src/paths.rs#L203-L211)）。这是**技术储备**，不是抽象过度；但应登记为「已实现未接入」，且对外不得声明 TOCTOU 已消除（现文档已如此写）。**保留。** |
| N8 | `views/runtimes.rs::list` 是一行包装 | 它承载了「取数失败按空列表降级」的明确口径（与 SSR/JSON 原有行为一致），是读模型入口，不是空转发。 |
| N9 | `skiff_store::NewSite` 与 `skiff_app::sites::NewSite` 同名 | 层次不同：前者是仓储插入行（`root_dir` 已是最终路径），后者是建站输入（`root_dir` 是可选原始输入、含 `aliases`）。转换在一处完成。**不改名**，但在 `app` 侧文档里注一句「非存储类型」。 |
| N10 | `plugin::ipc::MAX_FRAME` 再导出 `skiff_wire::MAX_FRAME` | 有注释说明「实现唯一来源是 wire，此处转出保留旧路径」，且是常量不是逻辑。与问题 16 的纯转发不同。 |
| N11 | `web` 的 `tempfile` 在 `[dependencies]` | 不是误放：`handlers/files.rs` 用 `tempfile::tempdir()` 承载上传暂存目录（[files.rs](file:///c:/dev/skiff/crates/web/src/handlers/files.rs#L297)）。**保留。** |

---

## 4. 批次计划（可执行顺序与边界）

> 判定口径沿用旧审计：**零契约** = 不改 URL、状态码、错误码、JSON 字段、审计动作名、SSR 文案。

### 批次 A：零契约整理（可立即做，本次执行范围）

| # | 内容 | 文件 | 行为不变保证 |
| --- | --- | --- | --- |
| A1 | 删 `web` 死依赖 `skiff-sched`（问题 1） | `crates/web/Cargo.toml` | 全 crate 零引用；编译即证明 |
| A2 | 抽 `kernel::host_run`，ftp/firewall 各留转调（问题 2） | `crates/core/src/kernel/{mod,ftp,firewall,host_run}.rs` | argv 拼装抽纯函数并补单测；两份调用点参数化 |
| A3 | `upload_cert` 的写盘+校验移入 `spawn_blocking`（问题 3） | `crates/app/src/sites/crud.rs` | 同输入同输出；补失败清理断言 |
| A4 | `write_if_changed` 复用 `write_atomic`（问题 15） | `crates/app/src/sites/layout.rs` | 内容相同仍跳过；错误文案仅内部 |
| A5 | 依赖表与能力面分布表更新（问题 4、5） | `docs/sections/08-engineering.md`、`AGENTS.md` | 只改文档 |

**门槛**：`tools\bin\cargo.cmd fmt` → `clippy --workspace --all-targets -D warnings` → `test --workspace`（Windows 口径）；Linux 专属分支由 CI 复核。每项独立提交（A1/A2/A3/A4 可各一条，A5 一条）。

### 批次 B：低风险但需一次聚焦（下一批，不在本次）

| # | 内容 | 前置 |
| --- | --- | --- |
| B1 | 权限通配匹配单一实现（问题 6） | 先定 `fs` 是否支持 `fs:*`（建议：支持，与 `ftp:*` 对齐；如不支持则在函数里显式白名单） |
| B2 | 站点域模块收口（问题 7 第 3 步 + 问题 8） | 纯移动，需一次大范围 `pub use` 清理 |
| B3 | 展示映射上移（问题 9） | 与 `/ui/v1` 字段冻结用例一起改 |
| B4 | 读模型与模板解耦（问题 13） | 与旧审计 1.2 合并，先定「读模型放 app 还是 web」 |

### 批次 C：需设计取舍或上机验证（只列方案）

| # | 内容 | 阻塞点 |
| --- | --- | --- |
| C1 | 审计写入改投递（问题 12） | 需决定「审计是否必须在响应前落库」 |
| C2 | `systemd-run` 超时清理单元（问题 11） | 需 Linux 测试机验证 |
| C3 | `systemctl restart docker` 超时语义（问题 10） | 已完成：60s 超时 + 30s 探测总预算（未上机实测） |
| C4 | `nginx -t` 校验合并（问题 14） | 部分完成：执行通道已统一，候选生命周期各自保留 |
| C5 | `plugin → axum` 解耦（§3 陈述，未单列问题） | 需要先定义能力面的传输无关返回类型；改动面大，建议随「能力面协议演进」一起做 |
| C6 | `store/db.rs` 迁移拆文件（问题 18） | 与下次 schema 变更合批，避免冲突 |

---

## 5. 未验证项

1. **问题 11 的 unit 残留**：`systemd-run --wait` 被 kill 后瞬态单元是否继续运行，本文按 systemd 语义推断，**未在测试机实测**。
2. **问题 10 的最坏耗时**：已由代码上限约束（systemctl 60s + 探测 30s），但「docker.sock 存在而守护进程无响应」时的真实表现**未在 Docker 宿主机实测**。
3. **问题 12 的实际阻塞概率**：`Db` 的 50ms 等待告警是否在真实负载下触发，**未采集日志**。
4. **问题 14 的合并后行为**：`nginx_test_candidate` 只改「怎么执行」，SSR 保存主配置与 vhost 的既有用例全过；**未上机验证真实 OpenResty 容器内的错误行解析**（沿用原逻辑，无变化）。
4. **问题 7 的站点域行数占比**：按文件归属统计（6 个文件 10.5k 行），未做函数级归属；`runtime_catalog` 是否应算「站点域」还是「运行环境目录」，取决于后续是否独立命名。
5. **`web` 之外是否还有死依赖**：本次只核到 `skiff-sched`；`cargo-machete` 类工具未跑，其余依赖按人工抽样判断。

---

## 6. 批次执行记录

| 批次 | 内容 | 状态 | 提交 |
| --- | --- | --- | --- |
| A | 删 `web` 死依赖、`kernel::host_run` 收口、证书校验移入阻塞池、`write_if_changed` 复用、依赖表与能力面分布表 | 已完成 | `c843cc8`、`b7b4a49`、`00ead89`、`c58cf4c`、`cf19780` |
| B1 | 权限匹配单一实现：`Grant::permits` / `Grant::permits_wildcard`（问题 6） | 已完成 | 见本轮提交 |
| B2 | 站点域模块收口：`sites/install.rs` 拆出安装进度/互斥/环境探测；`sites_vhost.rs` 并入 `sites/vhost.rs`（问题 7 第 3 步、问题 8） | 已完成 | 见本轮提交 |
| B3 | 展示映射上移：`SiteSummary` 只带领域事实（`runtime_state`/`cert_ready`），中文文案与 CSS class 由 `skiff-web` 映射（问题 9） | 已完成 | 见批次 B 提交 |
| B4 | B 阶段新发现：`unix_now` 三份合并到 `time_fmt::unix_now`；`type_label` 两份合并到 `web::handlers::sites::type_label`（零契约） | 已完成 | 见本轮提交 |
| C3 | `systemctl restart docker` 加 60s 超时（超时文案明确「结果未知」）；`docker_ready` 改为 30s 总预算、单次探测受剩余预算约束（问题 10） | 已完成（未上机实测） | 见本轮提交 |
| C4 | `nginx -t` 执行通道统一为 `AppState::nginx_test_candidate`（问题 14 的「怎么跑」部分） | 已完成（生命周期未合并，见下） | 见本轮提交 |
| C7 | `fs` 是否支持 `fs:*` 通配 | **已关闭：不支持**，理由写入 [KERNEL_FS_CONTRACT.md](KERNEL_FS_CONTRACT.md) §2.1 | 本轮文档提交 |

**C4 的收敛边界**：只统一「容器内怎么执行 `nginx -t`」（命令数组、错误映射）。主配置与 vhost 的**候选生命周期没有合并**——前者成功后要 `rename` 上线并重载，后者校验完即删除，且 vhost 需要一层包含候选文件的临时主配置。强行合并会把两条路径的状态机揉在一起，收益小于风险；问题 14 因此降级为「执行通道已统一，生命周期保持各自实现」。

**C3 的残余风险**：超时后 `systemctl` 客户端被杀，但 systemd 作业可能仍在执行，调用方拿到的是「结果未知」。这与问题 11 同源，最终处置需要上机实测（见 §5）。

**B1 的语义边界**：本轮只把「如何命中授权项」收口为一个函数，**没有**改变任何能力面的通配策略——`permits_wildcard` 目前只有 ftp / firewall 调用，`fs` / `docker` / `scheduler` 仍是逐条精确授权（与原实现逐字节一致）。「是否让 `fs` 也支持 `fs:*`」属于安全语义扩张，未决策，已列入批次 C。

**B 阶段新发现（已在本轮收口）**：

- `unix_now()` 原有三份（`app/src/auth.rs`、`app/src/sites/install.rs`、`app/src/sites/crud.rs`），已合并到 [time_fmt.rs](file:///c:/dev/skiff/crates/app/src/time_fmt.rs) 的 `unix_now`，三处改为调用。
- `type_label` 原有两份（app 读模型与 web 详情页），已只保留 `web/src/handlers/sites.rs::type_label`；`SiteSummary` 不再携带 `type_label`，SSR 与 `/ui/v1` 各自从 `site_type` 映射，字段值不变。

**批次 C 待决策项**：原 C7「`fs` 是否支持 `fs:*` 通配」已按产品决定**关闭：不支持**，契约已写明「`fs:*` 不匹配任何方法」及三条理由；判定入口仍是 `Grant::permits_wildcard`，只是 fs 侧不调用。

**仍只出方案、本轮未实现的两项**：

| 项 | 可选路径 | 代价 |
| --- | --- | --- |
| C1 审计写入投递（问题 12） | A：`audit_or_log` 改 `async fn` + `db_async` 投递，调用点全部加 `.await` | 改动面大（几乎每个 handler 末尾），语义最干净；需要确认「审计必须在响应前落库」是否保留 |
| | B：保留同步函数，内部 `block_in_place` | 改动 1 处；但会临时扩容 worker，与「显式 2 线程控内存」目标冲突，需在 §7 记录例外 |
| C2 `systemd-run` 超时后瞬态单元处置（问题 11） | A：超时分支先 `systemctl stop <unit>` 再返回 | 需 Linux 测试机验证 unit 名可解析、stop 不会误杀后续命令 |
| | B：给 `systemd-run` 增加 `--property=RuntimeMaxSec=`，由 systemd 自身兜底 | 依赖 systemd 版本；超时语义与本地 `tokio::time::timeout` 可能不一致，需要统一 |
| | C：保持现状，只在错误文案与审计里标注「结果未知」 | 零风险、零收益，状态一致性仍靠人工核对 |

---

## 附录 A：crate 体量与实测依赖（2026-11-15）

| crate | 文件 | 行数 | 直接依赖（实测） |
| --- | --- | --- | --- |
| `app` | 47 | 34,198 | model/store/identity/docker/plugin/pkgfmt/sched/fs + tokio/ureq/rustls-pemfile/webpki-roots/ed25519-dalek/sha2/hex/tar/toml/chrono/base64 |
| `web` | 54 | 35,107 | app/store/model/identity/docker/plugin/pkgfmt/**sched（未用）** + axum/tower-http/askama/rust-embed |
| `plugin` | 12 | 9,548 | store/pkgfmt/wire/model + axum/http-body-util/tokio/libc |
| `fs` | 23 | 8,372 | serde/sha2/tar/flate2/zip + cfg(unix) libc |
| `sched` | 11 | 5,470 | chrono/serde/serde_json/thiserror/cron |
| `core` | 15 | 5,255 | 全部 + rustls/axum-server/anyhow |
| `store` | 20 | 5,198 | model/rusqlite/thiserror/tracing |
| `pkgfmt` | 6 | 3,715 | cron/model/serde/toml/ed25519-dalek/sha2/zstd |
| `docker` | 4 | 2,240 | policy/compose/model/bollard |
| `model` | 9 | 1,697 | serde（仅 derive） |
| `sdk` | 3 | 1,412 | pkgfmt/wire |
| `policy` | 3 | 1,154 | model + libc（路径校验） |
| `compose` | 1 | 754 | model/serde_yaml |
| `cron` | 2 | 710 | chrono |
| `identity` | 5 | 611 | store/model/argon2/rand/sha2/sha1/hmac/subtle |
| `wire` | 1 | 496 | serde_json/thiserror |

（行数含 `#[cfg(test)]`；与 `08-engineering.md` 的依赖表对照可复现问题 4。）
