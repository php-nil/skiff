# 菜单模型、归属与声明契约（§0–2）

> 上级索引：[MENU_CONTRACT.md](../MENU_CONTRACT.md) ｜ 拆自 `docs/contract/MENU_CONTRACT.md` 原 §0–2（2026-11-16 整理，正文未改）。

---

> 状态：**契约 v1 · P1/P2 已落地**。§11/§12 记录实现差异；正文其余部分仍为设计口径。
> 适用：内核 Web UI、插件 manifest、插件中心、菜单设置、固定页槽位。
> 基线：`crates/pkgfmt/src/manifest.rs`、`crates/app/src/menu.rs`、`crates/web/src/handlers/ui/menu.rs`、`plugins/*/manifest.toml`。
> 契约版本：v1 · 2026-11-03（P1 同日落地，P2 同日补记）。
>
> **2026-11-13 原生化**：budding 后台外壳（admin + outlet）不再用 iframe 承载插件功能页；
> 已声明 `schema`（`api = "1.3"`）的视图由壳页原生渲染；未声明的旧视图走
> `/{p}/plugins/{id}/{view}?legacy=1` 的 iframe 工作台。
> 菜单继续下发 `plugin/*` 入口；本文的三级归属与固定 L1 仍然有效，「内核渲染 L3 标签栏 +
> 插件内容 iframe」只对 `?legacy=1` 工作台有效，新壳用页内按钮行承担 L3 入口。
> 插件 UI 的交付方向改为 Schema（见 [`BUDDING_UPSTREAM_BACKLOG.md`](../../sdui/BUDDING_UPSTREAM_BACKLOG.md) T7）。

## 0. 四个结论

1. **三级归属**：一级 6 项由内核封闭固定（概览 / 网站 / Docker / 插件 / 工具 / 设置），插件不能新增或改名一级；插件只能在固定一级下声明二级，并只为自己的二级声明三级。
2. **L3 渲染方式**：三级由**内核渲染成工作台内的标签栏**，插件只提供被选中的内容页（iframe），不自行绘制导航 chrome。这样标签样式、当前态、权限、深链和窄屏行为都由内核统一控制。
3. **声明入口**：插件在 `manifest.toml` 用 `[[menu]]` + `[[menu.items]]` 声明 L2/L3，用 `[[mount]]` 声明固定页槽位；仅 `[[ui]]` 的老包走明确的降级映射，不要求重打包。
4. **设置作用域**：顺序、隐藏/显示、固定（pin）按**用户**保存；壳布局固定为侧栏（左右），不再作为可配置项；插件只能改声明默认值，不能读写任何用户偏好，也不能改布局实现。

---

## 1. 模型与归属

### 1.1 层级语义

| 层级 | 语义 | 最大数量 | 说明 |
|---|---|---|---|
| L1 | 功能域 / 模块入口 | 固定 6 项 | 概览、网站、Docker、插件、工具、设置；集合与 id 不可由插件改变 |
| L2 | 一个可独立完成的任务或工作台 | 每个 L1 内有序列表 | 系统内置项 + 插件在固定 L1 下声明项；不再允许「页内锚点」冒充 L2 |
| L3 | 同一个工作台内的视图 / 标签 | 每个 L2 最多 12 个 | 只允许拥有该 L2 的插件声明；内核渲染标签栏，插件渲染内容 |

深度硬上限为 3；不存在 L4。一个 L2 可以有目标页，也可以有 L3；有 L3 时 L2 的默认目标等价于第一个可见 L3。

### 1.2 归属矩阵

| 对象 | 定义权 | 渲染权 | 插件能否修改 |
|---|---|---|---|
| L1 集合、顺序基准、标签、图标 | 内核 | 内核 | 否 |
| 系统内置 L2（Docker 容器等） | 内核 | 内核 | 否 |
| 插件 L2 | 插件 manifest | 内核 | 只能改自己的声明 |
| 插件 L3 标签栏 | 插件 manifest | 内核 | 只能改自己的 L3 |
| L2/L3 内容页 | 插件声明 + 内核校验（schema），旧页回退 iframe | 壳页原生渲染 schema；legacy 用 iframe | 是（仅自己的 `ui/`） |
| 菜单设置、布局实现 | 内核 | 内核 | 否 |

**硬规则**：插件只负责「声明菜单」与「对应菜单呈现的内容」。内核负责所有可见导航 DOM、样式、交互、当前态、权限过滤与 URL。插件不得输出导航 HTML/CSS/JS 到父页面。

### 1.3 L3 为什么由内核渲染

结论：**内核渲染导航 chrome，插件只声明内容结构（schema）；旧包仍以 iframe 承载内容。**

理由：

- 视觉一致：标签栏继承内核字体、间距、颜色、焦点环、窄屏折叠规则；插件换 UI 不会让菜单样式漂移。
- 内核统一控制：当前态、权限过滤、隐藏/排序、固定、深链都由同一份菜单注册表计算，避免插件各自实现一套。
- 可访问性：`aria-current`、键盘方向键、Escape、焦点归还只在核心里维护一次。
- 可迁移：内核以后把上下结构改成左右结构或调整密度，插件无需改包。
- 安全边界清晰：插件内容始终在沙箱 iframe 里；标签栏不在 iframe 内，插件无法覆盖或伪装系统导航。

### 1.4 一级空态规则（「工具」）

- L1 **集合固定**，不因插件缺失而从注册表删除；但渲染时按下面规则处理可见性。
- `概览`、`网站`、`Docker`、`插件`、`设置` 有内核自带的固定 L2，正常渲染。
  - `网站` 的 L2 为内核固定的「网站列表 / 运行环境 / Web 服务」，不依赖任何插件（原 `com.skiff.nginx` 已内核化）。Web 服务的稳定键为 `core/sites/web`，路由 `/{p}/sites/web`；页面内部可说明底层实现（当前为 OpenResty），但菜单与路由不绑定具体软件名。
- `工具` 是**插件支撑型一级**：
  - 有可见 L2：正常渲染分组。
  - 没有任何已启用提供者：L1 仍然显示，展开后只有一条**空态项**「未启用工具插件」，附「去插件中心」链接；点击空态项进入插件中心，不产生 404。
  - 有提供者但全部被当前用户角色/条件过滤：对当前用户隐藏该 L1（不是空态，避免泄露不可见能力）。
  - 用户在菜单设置里手动隐藏：按用户偏好隐藏。
- 工具插件已安装但未启用：与「未安装」同为空态，但文案区分「已安装未启用」并给启用入口。
- **本契约取代旧规则「应用 0 项隐藏一级」**：工具默认以可行动空态常驻；用户可自行隐藏；网站为内核功能，始终可用。

---

## 2. 声明契约

### 2.1 Manifest 新增结构

在现有 `Manifest` 上新增两个可选数组：

```rust
pub struct ManifestMenu { /* L2 */ }
pub struct MenuItem { /* L3 */ }
pub struct ManifestMount { /* 固定页槽位 */ }

pub struct Manifest {
    // ...现有字段不变...
    #[serde(default)]
    pub menu: Vec<ManifestMenu>,
    #[serde(default)]
    pub mount: Vec<ManifestMount>,
}
```

现有 `Manifest` 使用 `#[serde(deny_unknown_fields)]`；实现本契约时必须同步扩展结构体，否则 `[[menu]]` / `[[mount]]` 会在解析阶段被拒。菜单声明要求 `api = "1.2"`（`[[mount]]` 自 `1.1` 起可用）；`api = "1.0"` / `1.1` 的老包继续走 `[[ui]]` 降级。
>
> **路由与视图分工**见 [`docs/contract/PLUGIN_UI_CONTRACT.md`](../PLUGIN_UI_CONTRACT.md)：`/{p}/plugins/{id}` 是内核信息与管理页，插件功能页固定为 `/{p}/plugins/{id}/<view>`；本文只负责菜单树、深链与偏好。

### 2.2 字段表

#### L2：`[[menu]]`

| 字段 | 类型 | 必填 | 默认 | 约束 |
|---|---|---|---|---|
| `id` | string | 是 | — | 本插件内稳定的局部 id：`[a-z0-9][a-z0-9-]{0,31}`；全仓唯一键由内核拼成 `plugin/<plugin-id>/<id>` |
| `parent` | string | 是 | — | **只能是固定一级 id**：`sites` / `tools` / `plugins` / `docker` / `settings`；不允许挂到另一个 L2 下 |
| `label` | string | 是 | — | 1–32 字符，纯文本；内核转义，不允许 HTML |
| `order` | integer | 否 | `100` | 同一父级内升序；冲突用稳定键裁决 |
| `icon` | string | 否 | `dot` | 只能取内核内置图标集；未知值记 warning 并降级为 `dot`，不加载任何外部资源 |
| `roles` | string[] | 否 | `["viewer","operator","admin"]` | 只能是这三个角色；空数组合法表示任何人不可见（不建议） |
| `gates` | string[] | 否 | `["plugin_enabled"]` | 只能是 `plugin_enabled` / `docker_available`；多项 AND |
| `default_visible` | bool | 否 | `true` | 默认是否出现在菜单；用户可覆盖 |
| `pinnable` | bool | 否 | `true` | 用户是否可把它固定到「固定区」快捷条；插件不能强制固定 |
| `view` | string | 否 | `main` | 该 L2 的默认视图名；无 L3 时是唯一视图。映射包内 `ui/<view>.html` |
| `items` | table[] | 否 | `[]` | L3 列表；见下 |

#### L3：`[[menu.items]]`

| 字段 | 类型 | 必填 | 默认 | 约束 |
|---|---|---|---|---|
| `id` | string | 是 | — | 本 L2 内稳定局部 id，规则同 L2 |
| `label` | string | 是 | — | 1–32 字符，纯文本 |
| `order` | integer | 否 | `100` | 同一 L2 内升序 |
| `icon` | string | 否 | `dot` | 内置图标集 |
| `view` | string | 否 | 等于 `id` | 视图名；映射包内 `ui/<view>.html`，视图名在插件内必须唯一 |

L3 不重复声明 `roles` / `gates`：继承所属 L2。若将来需要单独控制，再增加字段，不在 v1 引入。

#### 槽位：`[[mount]]`

| 字段 | 类型 | 必填 | 默认 | 约束 |
|---|---|---|---|---|
| `slot` | string | 是 | — | 只能取内核槽位注册表（见 §5） |
| `ui` | string | 是 | — | 包内相对路径，必须存在 |
| `height` | string | 否 | `md` | `sm` / `md` / `lg`，对应固定高度；禁止插件自定义像素高度 |
| `refresh` | integer | 否 | `0` | 仅 `0` / `30` / `60` / `300` 秒；0 表示不自动刷新 |
| `roles` | string[] | 否 | 全角色 | 同 L2 |

#### 稳定键格式

| 层级 | 键 | 示例 |
|---|---|---|
| L1 | 固定 id | `overview` / `sites` / `docker` / `plugins` / `tools` / `settings` |
| 内置 L2 | `core/<l1>/<l2>` | `core/docker/containers` |
| 插件 L2 | `plugin/<plugin-id>/<local-id>` | `plugin/com.skiff.monitor/monitor` |
| 插件 L3 | `<L2键>/<local-id>` | `plugin/com.skiff.monitor/monitor/history` |

稳定键用于设置持久化、DOM `data-menu-key`、URL、当前态判定；**不使用 label 或数组下标**。

### 2.3 校验规则

安装/加载时逐条校验，失败即拒绝安装（返回 `ManifestError`，错误码见括号）：

1. `parent` 必须是六个固定 L1 id 之一。（`MENU_PARENT_UNKNOWN`）
2. 插件不能声明 L1；`id` 不能以 `core` 开头，不能包含 `/`、`.` 之外的冒用拼接。（`MENU_ID_INVALID`）
3. L2 的完整键必须全局唯一；L3 的完整键必须在其 L2 内唯一。（`MENU_ID_CONFLICT`）
4. 深度只能是 L2/L3；`items` 内不得再出现 `items`。（`MENU_DEPTH_EXCEEDED`）
5. `label` 1–32 字符、去空白后非空、不得包含控制字符。（`MENU_LABEL_INVALID`）
6. `icon` 必须在内置枚举内；未知值 warning 并降级 `dot`，不拒装。（`MENU_ICON_UNKNOWN`）
7. `view` 必须是 `[a-z0-9][a-z0-9-]{0,31}` 的视图名；映射出的 `ui/<view>.html` 必须存在于包内 `ui/`。（`MENU_VIEW_INVALID` / `MENU_VIEW_MISSING`；同一插件内视图名重复为 `MENU_VIEW_CONFLICT`）
8. `roles` 只能包含 `viewer` / `operator` / `admin`；空数组合法。（`MENU_ROLE_INVALID`）
9. `gates` 只能包含 `plugin_enabled` / `docker_available`；内置项额外允许 `route_available`。（`MENU_GATE_INVALID`）
10. `order` 必须在 `-10000..=10000`；超出拒装，防止极端排序破坏默认值。（`MENU_ORDER_INVALID`）
11. 一个插件至少声明一个 `[[ui]]`；`[[menu]]` 的 `view` 映射文件必须可托管。没有 `[[ui]]` 的插件不能声明菜单。（`MENU_NO_UI_ENTRY`）
12. 同一 L2 的 L3 数量 ≤12；`items` 超过则拒装。（`MENU_TOO_MANY_ITEMS`）

### 2.4 示例插件完整声明

> 原 `com.skiff.nginx` 插件已**内核化**：网站一级与「网站列表 / 运行环境 / Web 服务」由内核自带，
> 不再作为插件声明菜单。下面的 `com.skiff.example` 仅演示键格式（L2 + L3 + 槽位），
> 任何仍挂在固定一级下的插件都适用。

```toml
# 现有字段保持不变（id/name/version/key_id/api/...）。
api = "1.2"

[[ui]]
label = "示例面板"
path = "main.html"

# 一级固定为「工具」（id = tools），插件只在这里声明 L2。
[[menu]]
id = "panel"
parent = "tools"
label = "示例面板"
order = 10
icon = "globe"
roles = ["admin", "operator", "viewer"]
gates = ["plugin_enabled"]
default_visible = true
pinnable = true
view = "main"

  # L3 由内核渲染成工作台标签栏；插件只处理内容。
  [[menu.items]]
  id = "overview"
  label = "总览"
  order = 10
  icon = "list"
  view = "overview"

  [[menu.items]]
  id = "history"
  label = "历史采样"
  order = 20
  icon = "archive"
  view = "history"

  [[menu.items]]
  id = "settings"
  label = "设置"
  order = 30
  icon = "settings"
  view = "settings"
```

渲染结果：

- 顶栏/侧栏：`工具` → `示例面板`。
- 信息页：`/{p}/plugins/com.skiff.example`（内核渲染身份/授权/管理，见 `PLUGIN_UI_CONTRACT.md`）。
- 工作台：内核标签栏 `总览 · 历史采样 · 设置`，内容 iframe
  `/{p}/plugins/com.skiff.example/ui/overview.html?nonce=...&menu=plugin/com.skiff.example/panel&tab=overview`。
- 深链：`/{p}/plugins/com.skiff.example/overview`（视图名直接寻址；`?legacy=1` 才是 iframe 工作台）。

### 2.5 插件卸载后的残留声明

菜单**不做物化存储**：

- 每次渲染从「内置注册表 + 已安装插件 manifest」实时合并；插件记录删除后，其 `plugin/<id>/*` 键立即从渲染结果消失。
- 卸载事务内删除 `user_prefs` 中该插件命名空间的键（或按 `menu.prefs` JSON 内的 owner 前缀裁剪），并写审计 `plugin.menu.cleanup`。
- 重新安装同一 id 时按默认声明重建，不恢复旧隐藏/排序偏好（避免“幽灵设置”）。
- 启动时做一次孤儿清理：删除引用了不存在 `plugin/<id>/...` 键的用户偏好。
- 不做“墓碑菜单”：不存在“插件已卸载但菜单还在”的状态。

### 2.6 旧包兼容：只有 `[[ui]].label` 如何降级

旧包没有 `[[menu]]` 时，内核合成菜单：

1. `[[ui]]` 第一条 → 一个 L2：
   - 稳定键：`plugin/<plugin-id>/legacy`。
   - `label` = `[[ui]].label`（空则用插件 `name`）。
   - `icon` = `[[ui]].icon`（合法时）。
   - `parent` = 官方白名单映射（见 §7.3），未知插件默认 `tools`。
   - `order` = `1000`；同一父级内按 `plugin-id` 字典序稳定排序。
   - `roles` = 全角色；`gates` = `["plugin_enabled"]`；`pinnable` = true。
2. `[[ui]]` 其余条目 → 该 L2 的 L3，`id = legacy-2`、`legacy-3`…，各用 `path` 对应的视图名，`label` 用各自 label。
3. 无 `[[ui]]` 的插件：不产生菜单，只出现在插件中心。
4. 新包一旦声明至少一个 `[[menu]]`，`[[ui]]` 条目不再参与菜单合成；`view` 统一映射 `ui/<view>.html`，旧包的默认视图 `main` 回落到原 `ui/index.html`（见 `PLUGIN_UI_CONTRACT.md` §5）。
5. 旧包不要求重打包；升级到新包后声明替换降级结果，用户偏好按稳定键尽量保留；`legacy` 键变成孤儿后由清理任务删除。

---
