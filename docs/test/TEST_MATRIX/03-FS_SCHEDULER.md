# F 文件系统原语 / G 调度器

> 上级索引：[TEST_MATRIX.md](../TEST_MATRIX.md) ｜ 拆自 `docs/test/TEST_MATRIX.md` 原 F–G（2026-11-16 整理，正文未改）。

---

## F. 文件系统原语

> 重跑触发：改了路径授权代数、原子写/路径锁、回收站、配额、审计分级、插件 `fs:*` 能力或管理员文件页。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| FS-01 | 插件只能读授权根内文件 | `[上机]` testkit 已装且授 `fs:read` + `path:` 根 | 调 testkit `fs_read` 读授权根内文件；读根外文件 | 根内 200 且返回内容；根外 400 并审计 `fs.read deny` | □通过 / □失败 | `crates/plugin/src/capability.rs`、`crates/model/src/path.rs` |
| FS-02 | 路径越界与符号链接逃逸 | `[本机+上机]` 单测 + 临时目录 | `cargo test -p skiff-fs --locked`；上机用 `../`、绝对路径、指向根外的软链测试 | 全部拒绝；规范化后仍在根内的路径才放行 | □通过 / □失败 | `crates/fs`、`crates/model/src/path.rs` |
| FS-03 | 原子写与并发路径锁 | `[本机]` 单测 | `cargo test -p skiff-fs --locked` | 写入走临时文件+rename；并发写同一路径不出现半文件；失败不留 `.tmp` | □通过 / □失败 | `crates/fs/src/atomic.rs`、`crates/fs/src/lock.rs` |
| FS-04 | 回收站移动/恢复/清理 | `[本机]` 单测；`[上机]` 管理员会话可打开 `/$P/files/trash` | `pwsh tools/gate.ps1 -Task test -Pkg skiff-fs`；上机删除文件后进回收站，恢复、彻底删除、过期清理 | 删除进回收站、可恢复、过期清理；恢复后原路径内容一致；父目录缺失/同名冲突有可读提示 | □通过 / □失败 | `crates/fs/src/trash.rs`、`crates/app/src/admin_fs.rs`、`crates/web/handlers/files.rs` |
| FS-05 | 全局与插件配额 | `[本机]` 单测；`[上机]` 管理员会话可打开 `/$P/files/quotas` | `pwsh tools/gate.ps1 -Task test -Pkg skiff-fs`；设置目录树字节/文件数上限后写入 | 超全局/插件配额被拒；目录树超限返回 `fs.quota_exceeded` 且消息含配额根与用量；回收站对象不计入目录树用量（恢复回补） | □通过 / □失败 | `crates/fs/src/quota.rs`、`crates/fs/src/ops/mod.rs`、`crates/app/src/admin_fs.rs` |
| FS-06 | 审计分级与目视筛选 | `[上机]` 已产生 fs 操作 | 打开 `/$P/audit`，用关键字框筛 `fs.`，再翻页找 result=deny/fail 行 | 每条含 actor、channel、action、target、result；关键字命中 actor/ip/action/target/detail；翻页走 `GET /ui/v1/audit?q=&limit=&cursor=`（`limit` ≤200，默认 50） | □通过 / □失败 | `crates/plugin/src/capability.rs`、`crates/web/src/handlers/ui/audit.rs`、`crates/web/static/budding/budding-boot.js` |
| FS-07 | 内核文件管理页基本操作（第一步） | `[上机]` 管理员会话，`/$P/files` 可打开 | 浏览目录；打开文本并修改保存；新建目录/文件；重命名；删除；再打开 >256 KiB 文件 | 操作成功；大小/时间正确；>256 KiB 只读且保存禁用；删除二次确认；保存失败时编辑内容保留 | □通过 / □失败 | `crates/web/src/handlers/ui/files.rs`、`crates/app/src/admin_fs.rs`、`crates/web/static/budding/budding-boot.js` |
| FS-08 | 数据根硬拒与权限位 | `[上机]` 已安装 | 用内核文件页尝试列/读/写 `/www/skiff/skiff.db`、`/www/skiff/plugins`、`/www/skiff/official`；检查数据根权限 | 即使管理员也返回 403 `fs.protected`；绕过软链仍拒绝；数据根与 plugins 为 0711；DB 文件非全局可写 | □通过 / □失败 | `crates/app/src/admin_fs.rs`、`crates/plugin/src/fsgrant.rs`、`deploy/install.sh`、`docs/test/DEPLOY_TEST.md` §1.1 |
| FS-09 | 旧深链 301 与插件菜单撤销 | `[上机]` `com.skiff.files` 已装 | 访问 `/$P/plugins/com.skiff.files/main`；查看「工具」菜单与全站导航 | 返回 301 且 `Location: /$P/files`；菜单中只有内核「文件管理」入口，无插件文件管理入口 | □通过 / □失败 | `crates/web/src/handlers/plugins.rs`、`crates/app/src/menu.rs`、`crates/web/src/handlers/mod.rs` |
| FS-10 | 回收站页面主链路（第二步） | `[上机]` 管理员会话，`/$P/files/trash` 可打开 | 在文件页删除文件/目录 → 打开回收站；恢复、故意制造同名冲突后覆盖恢复、彻底删除、清空 | 删除后进回收站且原路径消失；恢复后内容一致；冲突提示可读且需二次确认覆盖；清空二次确认并说明不可恢复；列表清空 | □通过 / □失败 | `crates/web/src/handlers/files.rs`、`crates/web/static/budding/budding-boot.js`（回收站 L3）、`/files/trash/list` 接口 |
| FS-11 | 配额页面与执行（第二步） | `[上机]` 管理员会话，`/$P/files/quotas` 可打开 | 设置目录树字节/文件数上限 → 在文件页写入超限；修改上限；取消上限；点「重新统计」 | 超限写入返回 `fs.quota_exceeded` 且含原因；页面显示已用/上限；取消后不再拦截且列表隐藏；重扫耗时提示出现 | □通过 / □失败 | `crates/web/src/handlers/files.rs`、`crates/web/static/budding/budding-boot.js`（配额 L3）、`/files/quotas/list` 接口、`crates/app/src/admin_fs.rs` |
| FS-12 | 回收站/配额审计与受保护路径 | `[上机]` 已产生回收站/配额操作 | 删除、恢复、彻底删除、清空、设置/取消配额后打开 `/$P/audit`；再尝试把配额根设为数据根/`plugins/` | 审计出现 `fs.recycle.*` / `fs.quota.*` 且不含文件内容；受保护根被拒（403 `fs.denied/protected`） | □通过 / □失败 | `crates/fs/src/audit.rs`、`crates/app/src/admin_fs.rs`、`handlers/settings.rs` |
| FS-13 | 内核运行配置保护（加固） | `[本机]` 单测；`[上机]` 管理员会话，文件页可浏览 `/etc` | 在文件页对 `/etc/skiff/config.toml` 执行删除、改名、覆盖保存；再对 `/etc/nginx/nginx.conf` 执行同样操作；尝试把配额根设为 `/etc/skiff` | 配置/TLS 路径三类操作均 403 且提示「内核运行配置或数据目录受保护」；`/etc/nginx/nginx.conf` 仍可操作；配额设置/扫描对 `/etc/skiff` 均 403 | □通过 / □失败 | `crates/fs/src/path.rs`、`crates/fs/src/ops/mutate.rs`、`crates/app/src/admin_fs.rs` |
| FS-14 | 宿主只读/容量类 I/O 错误映射 | `[本机]` 单测；`[上机]` systemd `ProtectSystem` 生效，管理员会话可浏览 `/etc` | 本机 `cargo test -p skiff-fs -p skiff-plugin`；上机在文件页写保护区外只读路径（如 `/etc/skiff-iter-probe.txt`），有只读挂载时再写该挂载点；ENOSPC/EDQUOT 构造不出则如实标未实测 | `EROFS` → 403 `fs.read_only` 且文案含「只读」，不再 500；`EACCES`/`EPERM` → 403 `fs.denied`；`ENOSPC`/`EDQUOT` → 507 `fs.no_space`；未归类 I/O 仍 500；插件能力面同一口径 | □通过 / □失败 | `crates/fs/src/error.rs`、`crates/plugin/src/fsops.rs`、`crates/web/src/handlers/ui/files.rs`、`crates/web/static/budding/budding-boot.js` |

最小回归命令：

```bash
cargo test -p skiff-fs --locked
cargo test -p skiff-plugin capability --locked
curl -ks -o /dev/null -w 'audit=%{http_code}\n' -b "$J" "$B/$P/audit"
curl -ks -o /dev/null -w 'files=%{http_code}\n' -b "$J" "$B/$P/plugins/com.skiff.files/main"
```

**2026-11-06 实测（测试机 10.10.15.4）**

| 编号 | 结果 | 证据 |
| --- | --- | --- |
| FS-01 | ☑通过 | testkit `fs_read /proc/version` 200（bytes=213）；`fs_read /etc/passwd` 400，内核 403「路径不在已授权的 path: 根内」；审计行 `plugin:com.skiff.testkit | uds | fs.read | /etc/passwd | deny | 路径不在已授权的 path: 根内` |
| FS-02 | ☐部分通过（仅上机） | `/www/../etc`、`/proc/../etc/passwd`、根内指向 `/etc` 的目录/文件软链、经软链写穿透均 400/403；运行态判定实际在 `crates/plugin/src/fsgrant.rs`（canonicalize + 授权根），`skiff-fs` 未被任何 crate 依赖；本机 `cargo test -p skiff-fs` 未执行（开发机无 Rust 工具链） |
| FS-03 | ☐未执行 | 仅本机单测；开发机无 `cargo`/`rustup`；运行态写路径是 `crates/plugin/src/fsops.rs::write_chunk` 直接 `set_len(0)`+写，无临时文件/rename/路径锁 |
| FS-04 | ☐未执行 | 仅本机单测；运行时无回收站存储/入口（`/www/skiff` 下无 trash 目录），files 删除为永久删除 |
| FS-05 | ☐未执行 | 仅本机单测；运行时无配额配置/入口（`config.toml` 无 trash/quota 键） |
| FS-06 | 审计分级与目视筛选 | `[上机]` 已产生 fs 操作 | 打开 `/$P/audit`，用关键字框筛 `fs.`，再翻页找 result=deny/fail 行 | 每条含 actor、channel、action、target、result；关键字命中 actor/ip/action/target/detail；翻页走 `GET /ui/v1/audit?q=&limit=&cursor=`（`limit` ≤200，默认 50） | □通过 / □失败 | `crates/plugin/src/capability.rs`、`crates/web/src/handlers/ui/audit.rs`、`crates/web/static/budding/budding-boot.js` |
| FS-07 | 内核文件管理页基本操作（第一步） | `[上机]` 管理员会话，`/$P/files` 可打开 | 浏览目录；打开文本并修改保存；新建目录/文件；重命名；删除；再打开 >256 KiB 文件 | 操作成功；大小/时间正确；>256 KiB 只读且保存禁用；删除二次确认；保存失败时编辑内容保留 | □通过 / □失败 | `crates/web/src/handlers/ui/files.rs`、`crates/app/src/admin_fs.rs`、`crates/web/static/budding/budding-boot.js` |
| FS-08 | ☑本机集成通过（2026-11-07，内核文件页） | 旧结论（files 插件按 `path:/www` 可写数据根）已被内核接管取代：`AdminFs::guard` 对 `data/`、`data/plugins`、`data/official` 一律 403，集成测试断言列 `data_dir/plugins` 返回 403；Unix 软链绕回单测同样拒绝。上机权限位复核仍待发版 | 发版后按 FS-08 上机复验：管理员在 `/files` 打开数据根必须 403；软链逃逸仍拒绝；数据根 0711/DB 0600 不变 |

> 稳定性：开跑前后 6 次探活（`/$P/` 307、`/api/v1/docker/containers` 401）一致，`NRestarts=0`，未遇到重启/502；测试期间 `monitor` 被并行会话停用，不影响 F 章判定。

> 2026-11-07 补充（FS-07 编辑器关闭回归）：`plugins/files` 编辑器弹窗的关闭按钮 / Esc / 点遮罩三条路径全部失效。根因是 `.editor-dialog` 上的作者样式 `display:flex` 盖掉了 UA 的 `dialog:not([open]) { display:none }`，`close()` 只移除 `open` 属性，窗口整块留在页面上。修复为 `display` 只挂 `.editor-dialog[open]`、补 `dialog:not([open]) { display:none !important }`、遮罩点击接入 `requestCloseEditor`（未保存内容先确认）、关闭后焦点回列表、确认框防重复 `showModal`。本地复现台（同 CSP + mock RPC，真浏览器）已验证：关闭按钮 / Esc / 遮罩均能收起、未保存内容先确认、保存后重开内容持久、>256 KiB 只读且禁用保存；测试机面板内复验待发版（浏览器工具无法越过自签证书）。

## G. 调度器

> 重跑触发：改了任务模型、内置任务、站点备份开关、手动触发、超时/重试/并发、内存缓冲/回执或插件唤醒链路。

| 编号 | 检查目标 | 前置条件 | 操作步骤 | 预期结果 | 判定 | 涉及模块或代码位置 |
| --- | --- | --- | --- | --- | --- | --- |
| TASK-01 | 任务列表与详情状态 | `[上机]` 已登录 | 打开 `/$P/tasks`；点任一任务详情 | 壳页 200；`GET /ui/v1/tasks` 与 `/ui/v1/tasks/{id}` 200 且含 kind/source/调度/状态/下次运行与历史 | □通过 / □失败 | `crates/web/src/handlers/tasks.rs`、`crates/web/src/handlers/ui/tasks.rs`、`crates/web/static/budding/budding-boot.js`（tasks 页） |
| TASK-02 | 内置任务标记与删除入口 | `[上机]` 新库或已有内置任务 | 打开列表找 `builtin:db-vacuum`；检查是否有删除按钮；如需可再直接 POST `/$P/tasks/builtin:db-vacuum/delete` 观察 | 列表可见并标「内置」；壳页不为内置任务渲染删除按钮。注：服务端 `delete_task` 当前未拦截 builtin，直接 POST 仍会删除（如要硬保护需另开事项） | □通过 / □失败 | `crates/app/src/tasks.rs`、`crates/web/src/handlers/tasks.rs`、`crates/web/static/budding/budding-boot.js`（tasks 页） |
| TASK-03 | 站点备份开关 | `[上机]` 有站点 | 站点详情打开备份开关；关闭；刷新站点页与任务页 | 开关 303；启用后出现 `builtin:site-backup:<domain>`（每日 03:00），关闭后任务保留但 `enabled=false`；站点页开关与任务页状态一致；无「保留天数」参数 | □通过 / □失败 | `crates/app/src/tasks.rs`（`set_site_backup`）、`templates/site_detail.html` |
| TASK-04 | 手动触发并写历史 | `[上机]` 任一启用任务 | 点「立即执行」；刷新详情；看 `task_runs` | 303；出现 running 记录；完成后 success/failed 与耗时、输出摘要可见 | □通过 / □失败 | `crates/app/src/tasks.rs`、`crates/sched` |
| TASK-05 | 超时、重试与退避 | `[本机]` 调度器单测 | `cargo test -p skiff-sched timeout --locked`；`cargo test -p skiff-sched backoff --locked` | 超时被取消并记 timeout；按重试次数与退避间隔重跑；最终状态正确 | □通过 / □失败 | `crates/sched/src/scheduler.rs`、`docs/contract/KERNEL_SCHEDULER_CONTRACT.md` |
| TASK-06 | 缓冲回执、统计与限速 | `[本机]` app 单测；`[上机]` testkit 已装并授 `scheduler:read` + `scheduler:manage` | 调 testkit `buffer_probe {"id":"plugin:com.skiff.testkit:busy-probe"}`；`buffer_push {"id":"…","count":N}`（N≤100，溢出用大 `point` 或超容量）；再 `buffer_probe` 对比；`cargo test -p skiff-app buffer --locked` | `buffer_probe` 原样回报 `points/bytes/capacity_bytes/seq/dropped_*/rejected_*/high_watermark`；`buffer_push` 汇总 `accepted/rejected/dropped_*/signals`，回执字段与内核一致；每任务 100 点/秒限速生效；`count>100` 本地 -32602；无 `manage` 时 push 被拒。注：内核只为声明 `sample_method` 的任务建缓冲，`busy-probe` 已声明（容量 4096 B、默认停用不自动采样） | □通过 / □失败 | `plugins/testkit`、`crates/app/src/tasks.rs`、`crates/sched/src/buffer.rs` |
| TASK-07 | 插件任务唤醒 | `[本机]` sched 单测；`[已核对]` 有入口 | `cargo test -p skiff-sched plugin --locked`；cron 新建生成 `plugin:com.skiff.cron:<key>`、`source=plugin:<target>` | 插件任务按调度唤醒、超时/失败回执；插件不可用时记 blocked 而非 failed | □通过 / □失败 | `crates/core/src/server.rs`、`crates/sched`、`crates/app/src/tasks.rs` |
| TASK-08 | 任务页权限与 CSRF | `[上机]` viewer 与 admin 各一 | viewer 打开任务页；尝试启停/立即执行/删除；admin 不带 CSRF 执行 | viewer 只读；写操作 403；admin 无 CSRF 403，带 CSRF 303 | □通过 / □失败 | `crates/web/src/handlers/tasks.rs`、`crates/model/src/rbac.rs` |
| TASK-09 | 调度预设选择与编辑反解 | `[本机]` `skiff-cron` 单测；`[上机]` 已装 cron 插件 | 本机跑 `cargo test -p skiff-cron --locked`；列表页右上角「新建任务」弹窗逐个选预设并核对表达式/人话预览；保存后在列表「编辑」逐项确认回显；自定义表达式填非法值 | 6 类预设各自生成标准 5 段表达式且预览为人话；编辑时预设能回显成对应表单；匹配不到预设落「自定义」并保留原表达式；非法表达式有明确报错且拒绝保存；界面标注本地时区、不支持秒级与 `@reboot` | □通过 / □失败 | `crates/cron/src/preset.rs`、`plugins/cron/src/main.rs`、`plugins/cron/ui/app.js` |

最小回归命令：

```bash
cargo test -p skiff-sched --locked
cargo test -p skiff-app tasks --locked
curl -ks -o /dev/null -w 'tasks=%{http_code}\n' -b "$J" "$B/$P/tasks"
curl -ks -o /dev/null -w 'task_detail=%{http_code}\n' -b "$J" "$B/$P/tasks/builtin:db-vacuum"
```

**2026-11-06 实测（测试机 10.10.15.4）**

| 编号 | 结果 | 证据 |
| --- | --- | --- |
| TASK-01 | ☑通过 | 列表 200，`builtin:db-vacuum` 显示 kind/source/调度/状态/下次运行；详情历史可见 |
| TASK-02 | ☑通过（待发版） | 列表标「内置」；模板不渲染删除按钮；内核已补服务端硬拦（`Scheduler::delete` 拒 `builtin:`，能力面 `task.delete` 同拒），发版后按台账复验 RPC 删除 → 403 |
| TASK-03 | ☑通过 | 开关 303；`scheduled_tasks` 行 `id=builtin:site-backup:matrix.local`、`kind=builtin:site-backup`、`source=site:matrix.local`、`schedule_json={"type":"cron","expr":"0 3 * * *","tz":...}`、`payload={"domain":"matrix.local"}`、`enabled=1`；关闭后 `enabled=0` 且 8 条历史保留；重开恢复 `enabled=1`。环境无 OpenResty，站点为库内夹具，未触发真实 vhost |
| TASK-04 | ☑通过 | 手动触发 303；`task_runs` 落 `trigger=manual`/`result=ok`（站点备份产出 tar）；插件任务手动触发同样落 `trigger=manual` |
| TASK-05 | ☑通过（本机） | `cargo test -p skiff-sched --locked` 45 项通过：超时取消、退避重试、单飞、`queue_one`、冷却期、错过策略、`retain_across_ticks=false` |
| TASK-06 | ☑通过（上机，2026-11-09） | 官方包 `0c99464` 上机：`buffer_probe` 基线 `points=0/capacity_bytes=4096`；`buffer_push count=100` → `accepted=100, dropped_points=17, dropped_bytes=833, signals=[high_watermark,overflow]`；再 probe `points=83, bytes=4067, seq=100, head_seq=17, tail_seq=99, high_watermark=true`；同秒再推 1 点被 400 拒（每任务 100 点/秒限速）；`count=101` 本地 `-32602`；70KB 大点 `accepted=false, bytes=70043, reason=point_too_large`。`rejected_points/rejected_bytes` 未能构造：超限点与限速在入缓冲前即被拒，统计恒 0。旧包（无 `manage`）`buffer_push` 403「缺少 scheduler:manage」已验 |
| TASK-07 | ☑部分通过 | `cron` `jobs.list/toggle/run` 200；`jobs.remove` 200 且行删除（`{"removed":true}`）；内核任务详情/历史页 200；插件任务按调度唤醒、失败回执 `result=fail`/`error_class=executor_failed`、手动触发均通过；`cron` 代他人注册被 `source != owner` 拒，修复（`source=owner`）已在本机待发版，见台账 |
| TASK-08 | ☑通过 | viewer 只读；写操作 403；admin 无 CSRF 403、带 CSRF 303 |
| TASK-09 | ☑通过（2026-11-07） | `skiff-cron` 14 项单测（预设→表达式、越界拒绝、往返反解、等价写法、自定义回退、描述文案）全绿；测试机 10.10.15.4 真浏览器 41 项断言全过：6 类预设分别落 `*/5 * * * *`、`0 */2 * * *`、`0 3 * * *`、`30 9 * * 1,3,5`、`0 4 1 * *`、`0 9 * * 1-5` 且预览为人话；编辑「改调度」逐项回显对应表单；自定义 `0,30 8-18 * * 1-5` 原样保留；非法 `0 25 * * *` 在新建与编辑两处均报错并拒绝保存；验收任务已清理。截图 `dist/iter/shots/{list,edit-card}.png` |

**调度器补充实测（本轮）**

| 项 | 结果 | 证据 |
| --- | --- | --- |
| 插件门三态 | ☑通过 | 未安装 `NotInstalled`、禁用 `Disabled`、`api=1.1` 且 `plugins.api=1.1` 时 `ApiMismatch`；均只写 `task.blocked` 审计，不产生 failed 运行 |
| 执行期回调 scheduler | ☐未上机 | testkit 已提供 `busy_probe`（默认停用的 `busy-probe` 任务，启用后「立即执行」即构造执行期回调），待上机复验；代码核对 `crates/core/src/kernel/capability.rs:134-141`：`is_interop_busy || tasks.is_executing` → 403 + `scheduler.deny` |
| 超时晚到结果 | ☑通过（本机） | `timeout_enters_sixty_second_cooldown` 新增断言：`timeout` 结算后 `finish(同一 run_id)` 返回 `Protocol`，任务保持 `Failed`，终态不被覆盖；上机未单独构造晚到回执 |
| 缓冲能力面 | ☑通过（上机，2026-11-09） | 容量/条数/字节/丢弃/高水位/seq 全对上：4096 B 容量下 100 点推入后保留 83 点/4067 B、丢 17 点/833 B（drop-oldest，单点约 49 B）；限速与本地 `count` 上限、无 `manage` 403 均通过。`rejected_*` 统计无法构造（拒绝发生在入缓冲前） |
| monitor / guard 迁移 | ☐未上机（代码完成） | manifest 改 `[[task]]`，内核启动/安装种入 `plugin:com.skiff.monitor:sample` 与 `plugin:com.skiff.guard:reconcile`；`run_cron` 已删，插件无 `tick`。上机断言：`scheduled_tasks` 两条、source 前缀正确、无 `plugin.cron_deprecated` 审计、重装不重复、卸载任务消失 |
| monitor 采样/落盘 | ☐未上机 | 内核每 60s 调 `sample`（只更新 `prev.json`），每 5 分钟调 `execute` 追加 `samples.jsonl`（一行一点，超 480 行压缩回 240 行）并回 `persist`；写盘失败时缓冲保留、下一轮重试；`snapshot` 返回 `scheduler.driver=kernel`，基线独立写 `prev_ui.json`，不截断采样窗口 |
| monitor 网络速率口径 | ☑通过（2026-11-09，官方包 `0c99464`） | 全速窗口：`/proc/net/dev` tx 差 210,657,428 B / 10 s → 期望 21,065,742.8 B/s，插件 `net_tx_rate=21,065,742.8 B/s`（差 0.00%，`net_tx` 与宿主逐字节一致）。限速长窗口 106 s（200 MB，`scp -l 16384`）：整窗均值 1,985,505 B/s；面板 UI 每约 60 s 自身的 `snapshot` 轮询把末段窗口截为 54 s，插件报 1,943,259.9 B/s，对同 54 s 期望差 0.00%、对整窗均值差 2.13%。同窗内内核 60 s 采样推进 `prev.json`（3060→3120），`prev_ui.json` 未被内核改写——两份基线独立，内核采样不截断 UI 窗口 |
| guard 单路径与语义 | ☐未上机 | 每分钟 `reconcile`；异常容器重启/冷却/小时预算/未授权只报不动与迁移前一致；UI 无「立即巡检」入口；`status.scheduler.driver=kernel` |
| manifest 任务升级刷新 | ☑通过（本机 + 上机，2026-11-09） | 本机 `cargo test -p skiff-app manifest_task --locked`：`*/5`→`*/10`、`execute`→`run`、采样 60→120、超时 30→45 后任务行同步更新；管理员停用的 `enabled=0`、`created_by=system:manifest`、`created_at` 与 `task_runs` 历史全部保留；撤掉声明后任务行被回收。上机（`ef7058b`，真实 0.1.0→0.2.0 签名包）：`expr` `* * * * *`→`*/2 * * * *`、采样 3600→1800、容量 4096→8192、超时 10→15 全部落库；`enabled=1`、`created_by`、`created_at=1794172700` 与 2 条运行历史保留 |
| 升级补齐新增授权 | ☑通过（本机 + 上机，2026-11-09） | 本机 `cargo test -p skiff-plugin upgrade_ --locked`：0.1.0→0.2.0 新声明 `docker:restart`/`docker:inspect` 后 grants 补齐，敏感档写审计 `plugin.grant_upgrade`（detail 含动词与 `0.1.0 → 0.2.0`）；只读新增不写该审计；未再声明的 perm 被过滤；同版本重装保留既有授权。上机：grants 7→8 补 `fs:write`，审计 detail = `升级新增授权: fs:write；版本 0.1.0 → 0.2.0（敏感: fs:write）`；卸载重装 0.1.0 后回到 7（`fs:write` 被过滤）。「用户未勾选项不翻案」无 CLI 拒绝交互，构造不出，仅本机单测覆盖 |
| 卸载回收插件任务 | ☑通过（上机，2026-11-09） | `skiff plugin remove com.skiff.testkit` 输出「已卸载（delete_data=false，回收任务 1 条）」，`scheduled_tasks` 中 `source=plugin:com.skiff.testkit` 行立即消失、重启不再出现；`task_runs` 历史按设计保留；重装 0.1.0 后按 manifest 重新种入（`enabled=0`、`created_by=system:manifest`、新 `created_at`） |
