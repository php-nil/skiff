# 测试矩阵、变更影响面与待拍板决策（§8–10 + 附）

> 上级索引：[SITE_RUNTIME_CONTRACT.md](../SITE_RUNTIME_CONTRACT.md) ｜ 拆自 `docs/contract/SITE_RUNTIME_CONTRACT.md` 原 §8–10 + 附（2026-11-16 整理，正文未改）。

---

## 8. 测试矩阵

| # | 场景 | 断言 | 层 | 阶段 |
| --- | --- | --- | --- | --- |
| T1 | 站点类型与运行时绑定 | `runtime` 缺 `runtime_id` 拒绝；`static`/`proxy` 带 `runtime_id` 拒绝 | 单测 | P1 |
| T2 | kind 一致性 | PHP 站点绑 Node 环境拒绝；Node 站点绑 PHP 环境拒绝 | 单测 | P1 |
| T3 | 运行环境未就绪 | 停止/异常环境不能新建绑定站点；错误含环境状态 | 单测/集成 | P1 |
| T4 | 共享 PHP socket | 两个 PHP 站点使用同一 `socket_path`；vhost 不硬编码路径 | 集成 | P1 |
| T5 | 进程型端口与地址归属 | Node `9200–9299`、Rust `9100–9199` 容器内端口不重叠；端口唯一、区间耗尽拒绝、删除后释放；改端口越界/占用拒绝；默认不发布宿主端口，`publish_port` ≥1024、唯一且显式开启 | 单测 | P1 |
| T5a | 站点维护态 | 巡检判定不可用 → vhost 变成 503 维护页且不出现 `proxy_pass`/`fastcgi_pass`；恢复后写回容器地址；地址与登记值不一致按终态失败、不改指实际地址 | 单测 + 集成 | P1 |
| T5b | 站点网段选择与冲突 | 首选 `172.18.0.0/16` 与既有 Docker 网络或宿主网卡重叠 → 按固定候选序列回退到第一个不重叠的 `/16`，回退原因写明占用者并持久化；宿主网卡地址同样参与重叠判断；候选段全部被占用或同名网络网段无法识别 → 显式报错并列出候选段/冲突来源；不静默换段、不删他人网络；池划分/地址分配跟随实际网段 | 单测（CIDR 与选择逻辑）+ 上机 | P1 |
| T6 | 反向代理校验 | 非 http(s)、带 userinfo、缺 host、回环到自身均拒绝；WebSocket 头生成 | 单测/集成 | P1 |
| T7 | 静态站无运行时依赖 | 无任何运行环境也可创建、启停、访问 200 | 集成 | P1 |
| T8 | 删除被引用运行环境 | 有绑定站点时拒绝；错误列出域名；无绑定时删除成功且不删站点 | 单测/集成 | P1 |
| T9 | 停止运行环境的级联 | PHP / Node.js 环境停止后站点显示“环境停止”，恢复后回到运行中；不修改站点自身期望状态 | 集成 | P1 |
| T10 | 默认版本 | 同 kind 至多一个默认；删除/切换默认规则；向导默认选中 | 单测 | P1 |
| T11 | 向导校验 | 域名格式/重复、别名冲突、目录越界、空字段、上游非法逐项报错 | 集成/浏览器 | P1 |
| T12 | 权限矩阵 | viewer/operator 无写操作；admin 可写；未授权 URL 404；CSRF 缺失拒绝 | 集成 | P1 |
| T13 | 审计事件 | 建站/启停/删除/环境配置/重建事件名与目标正确；无敏感内容 | 单测/集成 | P1 |
| T14 | 路由与深链 | 规范 URL 与旧别名均 200；runtime 详情/标签页；未知 id 404 | 集成 | P1 |
| T15 | Docker 边界 | 受管容器带徽章；Docker 操作后运行环境页显示漂移；双链正确 | 集成/浏览器 | P1 |
| T16 | 状态漂移 | 外部停止容器后页面显示“状态漂移”；“仅同步状态”不重启容器 | 集成 | P1 |
| T17 | 迁移 | v6→v8 幂等；旧站点映射到 `php-8.5`；默认站/SSL/别名不丢；`name` 回填 | 单测 | P1 |
| T18 | Go 未实现 | 目录与向导不出现 Go；提交 `kind=go` 返回“未知运行环境类型” | 集成/浏览器 | P1 |
| T19 | Node.js 健康检查 | 启动/重建超时置 error、保留 `server.log`、不无限重试；恢复后 running；GET 非 2xx/3xx 判定失败 | 集成 | P1 |
| T20 | 多实例 PHP | 同 kind 不同 `name` 的 socket/容器互不覆盖；站点按 `runtime_id` 指向各自 socket；同版本多实例允许 | 单测/集成 | P1 |
| T21 | Node.js 命令与端口边界 | 启动命令拒绝 `;`、`|`、`$`、反引号；`PORT`/`NODE_ENV`/`HOST` 不可覆盖；健康检查路径必须 `/` 开头 | 单测 | P1 |
| T22 | Node.js 站点绑定 | 站点目录不生成 `package.json` / `server.js`（无 `index.php`）；绑定只写 `runtime_id`，vhost 反代到环境端口；解绑/删站点不影响环境代码目录与进程 | 集成 | P1 |
| T23 | 共享环境保护 | 有绑定站点时：停止/重启/重建/改端口先列出域名并要求 `confirm=1`；无 `confirm` 不执行；删除环境被拒绝 | 单测/集成 | P1 |
| T24 | 克隆环境 | 复制 `config_json` 与 `app/`（跳过 `node_modules` / `target`），分配同 kind 新端口；重名/目录已存在拒绝；PHP 拒绝克隆 | 单测/集成 | P1 |

浏览器验收至少覆盖：向导三类分支、校验报错、运行环境列表筛选、PHP / Node.js 详情标签切换、
配置保存失败回滚提示、Go 未接入提示、站点列表窄屏无横向滚动。

---

## 9. 变更影响面

### 9.1 数据库

- 当前 `CURRENT_SCHEMA_VERSION = 19`（2026-11-12 核对）；本契约涉及的站点/运行环境表自 v8 起持续扩展，当前基线为 v19。
- > 2026-11-12 更正：本节此前写作「= 8 / 目标 v8」，与实际不符；仅修正文档口径，不涉及表结构变更。
- `site_runtimes`：
  ```sql
  CREATE TABLE site_runtimes (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK(kind IN ('php','node','go')),
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    image TEXT NOT NULL,
    image_digest TEXT NOT NULL DEFAULT '',
    desired_state TEXT NOT NULL DEFAULT 'running'
      CHECK(desired_state IN ('running','stopped')),
    last_status TEXT NOT NULL DEFAULT 'stopped'
      CHECK(last_status IN ('installing','running','stopped','error')),
    socket_path TEXT,
    is_default INTEGER NOT NULL DEFAULT 0,
    config_json TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE(kind, name)
  );
  CREATE UNIQUE INDEX idx_runtime_default ON site_runtimes(kind) WHERE is_default = 1;
  ```
- `sites` 表重建：删除 `php_version`，新增 §1.1 字段；外键 `runtime_id REFERENCES site_runtimes(id) ON DELETE RESTRICT`
  （SQLite 需开启外键；服务层仍做“有绑定站点禁止删除”校验，不能只靠 FK）。
- 迁移策略：v8 重建 `site_runtimes`，按 `id` 回填 `name`；内置 `php-8.5` 的 socket 收敛为
  `/www/runtime/php/php-8.5/php-fpm.sock`。旧站点 `runtime_id` 保持不变，不做旧固定 socket 路径迁移。

### 9.2 文件与目录

- `/www/sites/<域名>/` 布局不变；运行环境目录为 `/www/runtime/<kind>/<name>/`。
- **vhost 必须从 DB `socket_path` 生成**；该字段只由实例 `name` 推导，不按版本号硬编码。
- 删除站点仍只删该站目录；删除运行环境不删站点目录。

### 9.3 菜单与导航

- `core/sites/list`、`core/sites/env`、`core/sites/web`（原 `core/sites/nginx`，测试期直接换键、不兼容旧链接）三项不变；运行环境详情不新增 L2/L3。
- 运行环境详情返回列表时保留筛选与当前 kind；深链落在 `env` 菜单当前态。
- 不改变顶部/侧栏布局与菜单偏好结构。

### 9.4 概览健康块

- 概览的服务健康块新增/扩展“网站运行环境”信号：运行环境异常或状态漂移时显示计数与受影响站点数；
  点击跳 `core/sites/env`。
- 无运行环境 / 无站点时显示明确空态，不写死数字；Go 未接入，不伪造指标。

### 9.5 现有代码与模板（仅登记影响，不在本轮修改）

| 位置 | 影响 |
| --- | --- |
| `crates/store/src/db.rs` / `db/sites.rs` | v7 迁移、`site_runtimes` CRUD、`sites` 字段重建 |
| `crates/app/src/sites.rs` | 按类型生成 vhost、环境端口分配、PHP socket 取 `runtime_id`、Node.js / Rust 进程模型与健康检查 |
| `crates/web/src/handlers/sites.rs` | 向导三步、运行环境详情、校验、权限、审计改名 |
| `crates/web/templates/site_new.html` / `site_env.html` / `sites.html` / `site_detail.html` | 三类分支、环境列表与详情、站点类型列 |
| `crates/web/src/menu.rs` | 仅确认 `core/sites/*` 稳定键，不新增 L2 |
| `docs/test/DEPLOY_TEST.md` / `docs/test/TEST_LEDGER.md` | 目录、socket、端口、验收用例同步 |
| `docs/archive/ui/site-runtime-preview.html` | 本契约的可点预览，不参与产品构建 |

### 9.6 1Panel 对照取舍

| 1Panel 能力 | Skiff 取舍 |
| --- | --- |
| 网站 → 运行环境（PHP / Node.js / Rust 版本安装） | 采纳；但运行环境页只做业务视角，原始容器仍归 Docker |
| 建站先选“运行环境/静态/反代” | 采纳，作为三步向导第一步 |
| PHP 扩展、ini、FPM 进程池 | 采纳；P1 ini/FPM 可改，扩展只读，P2 受控重建 |
| Node/Go 项目一键部署、源码构建 | 不照抄；Node 只做「环境内 `npm start` + 反代」，Go 未接入且只运行已编译二进制，面板不做 `go build` |
| 数据库创建、SSL 一键申请（Let’s Encrypt） | 不做；自签/上传证书保留现状；向导不放假开关 |
| 应用商店、Compose 编排 | 不做；避免与内核“三核心”边界冲突 |

---

## 10. 需要拍板的决策（本轮回复中给推荐）

### Q1. P1 是否必须包含 Node.js / Go 站点？

- 已定：**Node.js 已接入；Go 不包含且已摘除**。2026-11-09 Node.js 恢复 `RUNTIME_KINDS` 目录、
  安装/启停/重建、站点绑定、专属配置与健康检查；Go 仍从目录与新建弹窗移除，提交 `kind=go` 直接拒绝。
- 理由：Node 的「单进程 + 端口 + 反代」链路已闭环，与 PHP 的 FastCGI 模型并列；Go 缺少构建/运行链路，
  做“能跑”必然假实现。

### Q2. PHP socket 路径：固定 `/www/php/php-fpm.sock`，还是每实例独立路径？

- 已定：**每实例独立路径 `/www/runtime/php/<name>/php-fpm.sock`**，由实例 `name` 推导；
  内置 `php-8.5` 即 `/www/runtime/php/php-8.5/php-fpm.sock`。不同实例绝不共用 socket。
- 理由：路径只依赖 `name` 后，同版本多实例（`php-8.5` / `php-8.5-a`）天然隔离；vhost 按站点
  `runtime_id` 取对应实例 socket，不按版本号推导，也不再散落硬编码。

### Q3. Node.js / Go 的运行模型：每站点独立容器，还是每环境一个常驻进程？

- 已定（2026-11-09 收口为方案 A）：**一个环境 = 一个应用进程 = 一个固定代码目录**
  `/www/runtime/<kind>/<name>/app`，配固定端口 + 反代，与 Rust 一致；环境提供版本、镜像、
  启动命令、`NODE_ENV`、健康检查、环境变量、端口与代码目录，站点只存 `runtime_id`。
- 理由：Node 是单进程常驻模型，一个环境一个端口最贴近「启动命令 + 监听端口 + 反代」；
  也避免引入第二套站点级容器生命周期。**不采用**“绑定站点即切换源码目录”的做法：多站点共享
  时目录归属含糊、删除站点会误删代码、克隆也无从下手。代价是同环境多站点共享进程与端口，
  需在 UI 上显式提示影响面并二次确认；站点级隔离与独立日志留作 P2（见 §7）。

### Q4. 删除运行环境时存在绑定站点，如何处理？

- 推荐：**直接拒绝删除**，列出绑定域名并给“改绑/删除站点”入口；不提供隐式解绑或级联删除。
- 理由：运行环境是共享资源，隐式操作会一次影响多个站点；显式阻断更符合“保存 ≠ 生效、操作可追溯”的基调。

---

## 附：状态文案与空态

| 场景 | 文案 |
| --- | --- |
| 无运行环境 | `还没有运行环境。先安装 PHP 8.5，再创建需要运行时的网站。` |
| 环境停止 + 有绑定站点 | `环境已停止，3 个站点当前不可访问。启动环境后会恢复。` |
| 环境异常 | `环境异常：socket 不可连接。查看日志 / 重建环境` |
| Node.js / Rust 未就绪 | `环境未就绪：端口未监听或健康检查未通过。查看日志 / 重建环境` |
| Go（未实现） | 列表与向导不出现 Go；直接提交 `kind=go` 返回 `未知运行环境类型` |
| proxy 无文档根 | `反向代理站点不创建文档根，只生成转发配置。` |
| 无绑定站点 | `该环境暂无站点。去网站列表新建。` |

---
