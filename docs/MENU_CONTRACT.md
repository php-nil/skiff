# Skiff 菜单契约（Menu Contract）

> 状态：**契约 v1 · P1/P2 已落地**。§11/§12 记录实现差异；正文其余部分仍为设计口径。
> 适用：内核 Web UI、插件 manifest、插件中心、菜单设置、固定页槽位。
> 基线：`crates/pkgfmt/src/manifest.rs`、`crates/web/src/handlers/mod.rs`、`crates/web/templates/base.html`、`plugins/*/manifest.toml`。
> 契约版本：v1 · 2026-11-03（P1 同日落地，P2 同日补记）。
>
> **2026-11-13 原生化**：budding 后台外壳（admin + outlet）不再用 iframe 承载插件功能页；
> 已声明 `schema`（`api = "1.3"`）的视图由壳页原生渲染，未声明的旧视图仍走旧 SSR 工作台。
> 菜单继续下发 `plugin/*` 入口；本文的三级归属与固定 L1 仍然有效，「内核渲染 L3 标签栏 +
> 插件内容 iframe」只对旧 SSR 页有效，新壳用页内按钮行承担 L3 入口。
> 插件 UI 的交付方向改为 Schema（见 [`BUDDING_UPSTREAM_BACKLOG.md`](BUDDING_UPSTREAM_BACKLOG.md) T7）。

## 0. 四个结论

1. **三级归属**：一级 6 项由内核封闭固定（概览 / 网站 / Docker / 插件 / 工具 / 设置），插件不能新增或改名一级；插件只能在固定一级下声明二级，并只为自己的二级声明三级。
2. **L3 渲染方式**：三级由**内核渲染成工作台内的标签栏**，插件只提供被选中的内容页（iframe），不自行绘制导航 chrome。这样标签样式、当前态、权限、深链和窄屏行为都由内核统一控制。
3. **声明入口**：插件在 `manifest.toml` 用 `[[menu]]` + `[[menu.items]]` 声明 L2/L3，用 `[[mount]]` 声明固定页槽位；仅 `[[ui]]` 的老包走明确的降级映射，不要求重打包。
4. **设置作用域**：顺序、隐藏/显示、固定（pin）与上下/左右布局按**用户**保存；实例默认布局存全局设置；插件只能改声明默认值，不能读写任何用户偏好，也不能改布局实现。

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
> **路由与视图分工**见 [`docs/PLUGIN_UI_CONTRACT.md`](PLUGIN_UI_CONTRACT.md)：`/{p}/plugins/{id}` 是内核信息与管理页，插件功能页固定为 `/{p}/plugins/{id}/<view>`；本文只负责菜单树、深链与偏好。

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
- 深链：`/{p}/m/plugin/com.skiff.example/panel/overview`；规范功能页 `/{p}/plugins/com.skiff.example/overview`。

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

## 3. 渲染契约

### 3.1 DOM 结构

内核输出一份语义稳定的结构（上下/左右只改 `data-layout` 和 CSS 类，不改结构）：

```html
<nav id="skiff-menu" data-layout="top" aria-label="主导航">
  <ul class="menu-l1" role="list">
    <li class="menu-l1-item" data-menu-key="overview">
      <a class="menu-link" href="/{p}/m/overview" data-menu-key="overview">概览</a>
    </li>
    <li class="menu-l1-item" data-menu-key="tools">
      <button type="button" class="menu-trigger" aria-expanded="false"
              aria-haspopup="true" aria-controls="skiff-menu-panel-tools">工具</button>
      <ul id="skiff-menu-panel-tools" class="menu-l2" hidden>
        <li data-menu-key="plugin/com.skiff.monitor/monitor">
          <a class="menu-link" href="/{p}/m/plugin/com.skiff.monitor/monitor"
             aria-current="page">主机监控</a>
        </li>
      </ul>
    </li>
  </ul>
</nav>
```

L3 标签栏（在工作台页面内，不在 iframe 内）：

```html
<div class="workbench-tabs" role="list" aria-label="主机监控工作台">
  <a class="workbench-tab" data-menu-key="plugin/com.skiff.monitor/monitor/overview"
     href="/{p}/m/plugin/com.skiff.monitor/monitor/overview"
     aria-current="page">总览</a>
  <a class="workbench-tab" data-menu-key="plugin/com.skiff.monitor/monitor/history"
     href="/{p}/m/plugin/com.skiff.monitor/monitor/history">历史采样</a>
</div>
<iframe class="workbench-frame" src="...?...&menu=...&tab=overview"></iframe>
```

约定：

- 所有可交互项带 `data-menu-key`；设置、测试、JS 只读这个属性，不解析 label。
- 当前项 `aria-current="page"`；所属 L1 带 `data-current="true"`（视觉高亮）。
- 关闭的 L2 面板带 `hidden`，确保不进入 Tab 顺序。
- 空态项使用 `class="menu-empty"`，仍带 `data-menu-key="empty"`，不是链接（按钮/纯文本 + 链接到插件中心）。
- 插件入口不额外套自定义 `<div>`；`namespace` 通过 `data-owner-kind="plugin|core"` 暴露，便于样式与测试。

### 3.2 稳定 id / 可访问名

- DOM `id` 只用于 `aria-controls` 关联，格式 `skiff-menu-panel-<l1>`；菜单身份以 `data-menu-key` 为准。
- 一级触发器使用 `<button>`；二级/三级使用 `<a href>`，保证中键/复制链接/无 JS 可用。
- 不把菜单做成 `role="menu"` 复合组件：这里是站点导航，不是应用菜单；使用 nav + 链接 + `aria-current`，减少 ARIA 状态错误。

### 3.3 深链 URL 形状

| 页面 | 规范 URL | 说明 |
|---|---|---|
| L1 直达（概览） | `/{p}/m/overview` | 也接受旧 `/{p}` |
| 内置 L2 | `/{p}/m/core/<l1>/<l2>` | 如 `/{p}/m/core/docker/containers` |
| 插件 L2 | `/{p}/m/plugin/<plugin-id>/<l2-id>` | 如上例 |
| 插件 L3 | `/{p}/m/plugin/<plugin-id>/<l2-id>/<l3-id>` | 选中对应标签 |
| 插件信息与管理 | `/{p}/plugins/<id>` | 内核渲染，非插件页面 |
| 插件规范功能页 | `/{p}/plugins/<id>/<view>` | 等价于默认/指定视图的深链 |
| 插件 iframe 内容 | `/{p}/plugins/<id>/ui/<path>?nonce=...&menu=<l2-key>&tab=<l3-view>` | 内核托管，非用户主入口 |

- 规范 URL 是菜单身份的唯一外部表现；旧路由保留为别名（§7.2），由内核解析到同一菜单键。
- 深链必须能在无 JS 下打开；JS 只做同页增强（更新 iframe / pushState），不能成为唯一入口。
- URL 中 `plugin-id` 含 `.`，不再二次编码；`l2/l3` 只用 `[a-z0-9-]`。
- 不允许把 label、`ui` 文件路径直接拼进菜单 URL。

### 3.4 当前态判定

1. 先由 URL 解析出 `menu-key`（规范 URL）或经旧路由映射表（§7.2）得到 `menu-key`。
2. L3：`menu-key` 命中某 L2 的某个 L3 → 该 L3 `aria-current`，其 L2/L1 同时高亮。
3. L2：`menu-key` 命中 L2 → 该 L2 高亮；若该 L2 有 L3 但 URL 未带 L3，默认选中第一个可见 L3。
4. L1 直达：只高亮 L1。
5. 不用“路径前缀包含”猜当前态；必须查注册表。旧 `active` 字符串（如 `plugin:<id>`）只存在于迁移适配层。

### 3.5 与 iframe 托管 UI 的关系

- 插件内容仍然走现有沙箱路由：`/{p}/plugins/{id}/ui/{path}?nonce=...`，CSP 保持
  `frame-ancestors 'self'`、`connect-src 'none'`、`script-src 'self'` 等既有收紧项。
- L2/L3 切换：
  - 默认无 JS：整页打开 L3 URL，iframe 带 `?menu=&tab=`，插件按 query 渲染。
  - 有 JS：内核更新 URL + `iframe.src`（`ui` 变化时）或 `postMessage({type:"skiff.menu", menu, tab})`（同一 iframe 时），插件可保留状态。
- 插件不得：
  - 注入父页面样式/脚本、修改父 DOM；
  - 使用 `target="_top"` 或跳出 iframe 伪装系统导航；
  - 加载外链 CSS/JS/字体/图片（沿用 `network_egress=[]` 与 CSP）；
  - 依赖导航 DOM 结构或 CSS 类名；只能依赖 `menu`/`tab` query 与 `skiff.menu` 消息。
- 父页面只接受带本次 nonce 的 `skiff.rpc`，不因菜单消息放宽。

### 3.6 交互规则（修掉悬停下拉缺陷）

**结论：点击展开，悬停不展开。**

- 桌面：
  - `hover` 只做视觉高亮，**不打开** L2 面板；不再用 `mouseenter` 自动展开。
  - 点击 / Enter / Space 切换；再次点击收起。
  - 展开后点击另一个 L1：先关旧面板再开新面板，只保留一个。
  - `Escape` 关闭当前面板并把焦点还给触发器。
  - 点击面板外、Tab 焦点离开整个分组、切走窗口 → 关闭。
  - 关闭时加 `hidden`，从 Tab 顺序移除；`aria-expanded` 同步。
- 键盘：
  - 触发器之间左右/上下方向键移动（按布局方向）；Home/End 到首尾。
  - 打开时聚焦第一个可用 L2；L2 内上下移动；Enter 进入；Escape 回到触发器。
  - 不使用自动聚焦陷阱；Tab 可正常离开。
- 窄屏（≤860px）：
  - 上下/左右两种布局都折叠为「菜单按钮 + 抽屉/就地展开」，L1 纵向列表，L2 手风琴；
  - 不使用 hover 浮层；点 L1 展开/收起；
  - 抽屉关闭时焦点回菜单按钮；`Escape` 关闭；
  - L3 标签栏横向滚动，不换行挤压 iframe。
- 触屏：只用点击；不监听 `mouseenter`。
- 动效：只做透明度/位移 ≤120ms；`prefers-reduced-motion` 下取消。

### 3.7 硬边界

- 零依赖、同源静态资源、无 CDN、无内联脚本；菜单交互只在 `/static/skiff.js`，样式只在 `/static/skiff.css`。
- 插件 label/icon 只能作为文本/内置图标渲染；禁止 HTML、SVG 字符串、data URL、外链图标。
- 菜单 DOM 由内核模板输出；插件 iframe 不能 `postMessage` 修改菜单，只能接收 `skiff.menu`。
- 任何菜单变更都必须经 RBAC 过滤后再输出 DOM；未授权项不得“先渲染再 403”。

---

## 4. 菜单设置与持久化

### 4.1 作用域

| 设置 | 作用域 | 默认来源 | 谁能改 |
|---|---|---|---|
| L1/L2/L3 顺序 | 按用户 | 内置注册表 + manifest `order` | 用户自己；插件只能改声明默认值 |
| 显示/隐藏 | 按用户 | `default_visible` + gates + roles | 用户自己 |
| 固定（pin） | 按用户 | `pinnable` | 用户自己；插件不能强制 |
| 上下/左右布局 | 按用户 | 全局 `menu.layout.default`，缺省 `left` | 用户自己；插件不能改 |
| 内容宽度（自适应/固定） | 按用户 | 缺省 `auto`（铺满）；固定上限缺省 1440px | 用户自己；插件不能改 |
| 实例默认布局 | 全局 | `settings.menu.layout.default` | 管理员 |
| 菜单集合、内置 L1/L2、URL 规则 | 内核 | 代码 | 任何人都不能由插件改 |

- 「全局 vs 按用户」结论：**顺序/隐藏/pin/布局/内容宽度一律按用户**；全局只保留实例默认布局。
- 管理员不能看到其他用户的私有排序；P1 不做“强制模板/锁定布局”。若以后需要，再加全局策略层。

### 4.2 存放位置

推荐 **DB 表**，不放 config：

```sql
-- schema v2
CREATE TABLE IF NOT EXISTS user_prefs (
  user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  key TEXT NOT NULL,
  value_json TEXT NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (user_id, key)
);
```

- 用户菜单偏好存 `key = 'menu.prefs'`，值为一个 JSON：
  ```json
  {
    "v": 1,
    "layout": "left",
    "content_width": "auto",
    "content_max": 1440,
    "order": {
      "sites": ["core/sites/list", "core/sites/env", "core/sites/web"],
      "docker": ["core/docker/overview", "core/docker/containers"]
    },
    "hidden": ["plugin/com.skiff.guard/guard"],
    "pinned": ["core/docker/containers"]
  }
  ```
- 全局实例默认布局存现有 `settings` 表：`settings.menu.layout.default = "top" | "left"`，未设置时按 `left`。
- 为什么不用 config：需要按用户、事务写入、立即生效、随用户删除级联；config 是启动期文件，不适合高频个人偏好。
- 菜单设置页只写当前用户 `user_prefs`；插件无法通过 manifest/API 写这些键。
- 每次保存写审计 `menu.prefs.update`（不记录完整 JSON，只记录变更键数）。

### 4.3 默认值与合并算法

默认 L1 顺序：

```text
overview(0) → sites(100) → docker(200) → plugins(300) → tools(400) → settings(900)
```

默认 L2/L3 顺序：内置注册表 `order` + manifest `order`。

合并算法（必须确定性）：

1. 取某父级的**可见默认列表**（按 `order`，tie-break：`owner_rank` 内置 0/插件 1，再按稳定键字典序）。
2. 若用户有该父级的 `order`：保留其中仍存在的键，按用户顺序排列。
3. 新出现的键（插件升级新增、首次安装）插入到「默认顺序中它前一个已存在键」之后；若前面没有锚点，插入到最前。
4. 用户顺序里缺失但默认存在的键，按默认顺序插到相应位置（不能静默消失）。
5. `hidden` 在排序之后应用；隐藏项保留位置，重新显示时回原位。
6. `pinned` 只影响固定区，不改变层级与主菜单顺序。
7. 同一位置冲突：`owner_rank` → 稳定键字典序；插件不能通过 `order` 抢到内置项之前以外的方式改变裁决。

### 4.4 插件升级/新增项

- 插件新增 L2/L3 后，已有用户不会被打乱：新键按 §4.3 插入默认相对位置。
- 插件改名只改 label，不改变稳定键；用户排序不受影响。
- 插件改 `id` 视为新条目；旧键成为孤儿，按 §2.5 清理。
- 插件删除条目：用户偏好里的该键移除；不影响其他项。
- 隐藏项被插件升级删除后，偏好自动清理，不保留“隐藏一个不存在项”。

### 4.5 布局切换

- 值：`top`（上下） / `left`（左右）；缺省读全局默认，最后兜底 `left`。
- 存 `user_prefs.menu.prefs.layout`；切换即时生效。
- 结构不变保证：菜单注册表、菜单键、URL、L2/L3 顺序完全一致，只改根节点 `data-layout` 与 CSS；禁止为 left 另生成一份导航数据。
- 布局是内核功能：插件 manifest 没有 layout 字段，插件也不能通过 CSS 影响宿主布局。
- 内容宽度与布局独立：`body[data-content-width="auto"]` 铺满可用宽度；`"fixed"` 时用 `--content-max`（默认 1440px）约束 `main` / `.foot`。左右布局的固定宽度在侧栏之外的剩余空间居中。
- 窄屏 ≤860px 时两种布局都走 §3.6 折叠规则，`data-layout` 仍保留但视觉统一，内容区取消侧栏留白。

### 4.6 菜单设置页

- 入口：`设置 → 菜单设置`，稳定键 `core/settings/menu`，URL `/{p}/settings/menu`。
- 内容：
  - 布局：上下 / 左右（单选，缺省左右）。
  - 内容宽度：自适应（铺满）/ 固定（可填 960–4096px 的最大宽度，缺省 1440px）。
  - L1 列表：显示/隐藏、上移/下移、恢复默认。
  - 每个 L1 展开后的 L2/L3：显示/隐藏、上移/下移、固定/取消固定。
  - 锁定项：`概览`、`设置`、`菜单设置`自身不可隐藏（防止把自己锁在门外）；其余可隐藏。
  - 「恢复默认」只清当前用户的 `menu.prefs`，不触碰插件声明。
- 保存：POST + CSRF；失败保持原值并显示原因；成功审计。
- 预览：设置页提供同结构的静态预览，不改变真实导航直到保存。

---

## 5. 固定页面的插件挂载点

### 5.1 槽位注册表

槽位由内核**封闭注册**，插件不能自造名字：

| 槽位 | 页面 | 语义 | 替换/追加 | 默认降级 |
|---|---|---|---|---|
| `overview.health` | 概览 | 顶部系统健康块 | **替换**内核默认健康块 | 内核原生健康块 |
| `overview.actions` | 概览 | 行动面板补充卡 | 追加（最多 2 个） | 不显示 |
| `overview.side` | 概览 | 右栏补充卡 | 追加（最多 1 个） | 不显示 |
| `sites.header` | 网站工作台 | 站点页顶部提示 | 追加（最多 1 个） | 不显示 |

- 新增槽位必须改内核槽位表并写契约；插件不能通过声明“发明”槽位。
- 槽位命名固定 `page.slot`，全小写、点分。

### 5.2 声明与渲染

```toml
[[mount]]
slot = "overview.health"
ui = "health.html"
height = "sm"
refresh = 60
roles = ["admin", "operator", "viewer"]
```

- 内核渲染 `<iframe src="/{p}/plugins/{id}/ui/{ui}?nonce=...&slot=overview.health">`。
- 尺寸：`sm=96px`、`md=180px`、`lg=320px`；内容超出在 iframe 内滚动，不允许撑开宿主页面。
- 刷新：`0/30/60/300` 秒；内核重设 `src`（带新 nonce）；`0` 表示只在页面打开时加载。
- 失败降级：
  - 插件未启用 / 进程未运行 / 声明 gate 不满足 → 不渲染 iframe；`overview.health` 回退内核原生块，其他槽位显示空。
  - iframe 5 秒未 `load` 或插件返回 5xx → 显示固定卡片「插件不可用」+ 重试按钮 + 「查看日志」（异常容器/插件日志）。
  - 单个槽位失败不得影响页面其他区域或整页 500。
- CSP / 安全：沿用 §3.5；iframe 沙箱不获得父页面权限；槽位内容不能读 Cookie、不能发外网。
- `overview.health` 替换时，内核仍保留「Docker 不可用」等系统级警示；插件不能隐藏系统警示。

### 5.3 monitor 最小示例

```toml
api = "1.1"

[[ui]]
label = "主机监控"
path = "index.html"

[[menu]]
id = "monitor"
parent = "tools"
label = "监控"
order = 20
icon = "activity"
view = "overview"

  [[menu.items]]
  id = "overview"
  label = "总览"
  order = 10
  view = "overview"

  [[menu.items]]
  id = "cpu"
  label = "CPU / 内存"
  order = 20
  view = "cpu"

  [[menu.items]]
  id = "disk"
  label = "磁盘"
  order = 30
  view = "disk"

[[mount]]
slot = "overview.health"
ui = "health.html"
height = "sm"
refresh = 60
roles = ["admin", "operator", "viewer"]
```

- `health.html` 是 monitor 包内新文件；没有该文件时安装校验明确失败。
- monitor 禁用后，概览自动回退内核健康块，不需要用户手动清理。

---

## 6. 权限与安全

### 6.1 角色显隐

- L2 `roles` 是**可见性上限**，不是授权来源；真正操作仍由 RPC 策略与插件 grants 判定。
- 渲染前按当前用户角色过滤；未命中角色 → 该 L2/L3 不输出 DOM，不出现“点进去 403”的入口。
- 若一个 L1 的全部 L2 因角色不可见 → 该 L1 对当前用户隐藏（`工具` 例外见 §1.4：无提供者才显示空态；`网站` 由内核固定 L2）。
- 直接访问未授权菜单 URL：返回 404（不泄露存在性）；插件 RPC 仍返回 403/策略错误。
- `roles = []`：任何用户不可见；允许但安装警告。

### 6.2 插件不能注册系统内置 id

- 插件 L2/L3 的完整键强制带 `plugin/<plugin-id>/` 前缀；内置键强制 `core/` 前缀。
- 插件 `parent` 只能是固定 L1，不能挂到 `core/<l1>/<l2>` 下；因此无法把自己的项塞进 Docker/设置的内置 L2。
- 插件 `id` 不能含 `/`；不能声明 `id = "core"` 或任何 `core/*` 键。
- 内置 L1/L2 的 label、icon、URL、顺序基准由内核固定；插件 label 不参与路由。

### 6.3 id 冲突与冒用

- 两个插件声明同一完整键：安装校验失败 `MENU_ID_CONFLICT`，不静默覆盖。
- 同一插件 L3 重复：安装失败。
- 插件 label 与内置 label 重名：允许，但 DOM 有 `data-owner-kind="plugin"`，插件中心卡片显示真实插件名；视觉上不得伪装成系统项（禁止插件提供自定义图标绕过，图标只能内置枚举）。
- 插件卸载/降级后旧键不残留（§2.5）。
- 审计：安装/拒绝/卸载/菜单偏好变更都记录；菜单声明变更随插件安装审计留存 manifest 摘要。

---

## 7. 兼容与迁移

### 7.1 当前实现基线

- `Manifest` 目前只有 `[[ui]]`（`UiEntry { label, path, icon }`）与 `deny_unknown_fields`，没有 `menu` / `mount`。
- 导航目前由 `crates/web/src/handlers/mod.rs::nav()` 生成 `Vec<NavGroup>`；`base.html` 渲染 `.nav-group`、`.nav-toggle`、`.nav-menu`。
- 当前一级为 概览 / 应用 / Docker / 插件中心 / 设置；「应用」来自 `nav_plugins()`（已启用 + 有 UI）。
- 当前插件页只有 `/plugins/{id}` 一个详情页，iframe 指向 `/plugins/{id}/ui/{path}?nonce=...`；没有 L3。
- 当前 active 值：`dashboard`、`containers`、`images`、`networks`、`volumes`、`plugins`、`settings`、`audit`、`plugin:<id>`。
- 当前设置只有单页 `/settings` + `/audit`，没有菜单设置页。

### 7.2 旧路由 → 新菜单键

| 旧路由/active | 新菜单键 | 新 URL | 备注 |
|---|---|---|---|
| `/{p}` / `dashboard` | `overview` | `/{p}/m/overview` | L1 直达 |
| `/{p}/containers` / `containers` | `core/docker/containers` | `/{p}/m/core/docker/containers` | 保留 `?filter=error` |
| `/{p}/containers/{id}` | `core/docker/containers` | 同上 | 详情页仍走旧路由，当前态归容器 |
| `/{p}/images` / `images` | `core/docker/images` | `/{p}/m/core/docker/images` | |
| `/{p}/networks` / `networks` | `core/docker/networks` | `/{p}/m/core/docker/networks` | |
| `/{p}/volumes` / `volumes` | `core/docker/volumes` | `/{p}/m/core/docker/volumes` | |
| `/{p}/plugins` / `plugins` | `core/plugins/installed` | `/{p}/plugins`（旧深链 `/{p}/m/core/plugins/installed` 保留） | 页内 tab 走原生路径 |
| `/{p}/plugins?tab=discover` | `core/plugins/discover` | `/{p}/plugins/discover`（旧深链 `/{p}/m/core/plugins/discover` 保留） | L2 直达；壳页内呈现「发现」标签，旧 `?tab=discover` 兼容 |
| `/{p}/plugins?tab=updates` | `core/plugins/updates` | `/{p}/plugins/updates`（旧深链 `/{p}/m/core/plugins/updates` 保留） | L2 直达；壳页内呈现「可更新」标签，旧 `?tab=updates` 兼容 |
| `/{p}/plugins/{id}` / `plugin:<id>` | `plugin/<id>/<declared-l2>`；旧包 `plugin/<id>/legacy` | 由声明决定 | 不再有统一「应用」 |
| `/{p}/settings` / `settings` | `core/settings/security` | `/{p}/m/core/settings/security` | |
| `/{p}/settings/menu`（新增） | `core/settings/menu` | `/{p}/m/core/settings/menu` | 菜单设置 |
| `/{p}/audit` / `audit` | `core/settings/audit` | `/{p}/m/core/settings/audit` | |
| 旧「应用」菜单 | 解散 | — | 网站已内核化；cron/monitor/guard → `工具`，files → `网站`（站点文件） |
| `/{p}/compose`（规划） | `core/docker/compose` | `/{p}/m/core/docker/compose` | 无后端前 `route_available` 不渲染 |
| `/{p}/docker`（规划） | `core/docker/overview` | `/{p}/m/core/docker/overview` | P3 |
| `/{p}/settings/docker`（原 `/{p}/docker/settings`，旧路径 308） | `core/docker/settings` | `/{p}/m/core/docker/settings` | 页面 URL 归「设置」资源，菜单键仍是 Docker 设置；URL 规则见 [`URL_CONTRACT.md`](URL_CONTRACT.md) |

兼容策略：

- 旧路由继续可用，**不 301**，直接内部解析到新菜单键；避免用户书签断裂。
- 当前 `active` 字符串迁移为 `route → menu-key` 映射表，迁移完成后删除 `APP_ORDER` 与 `nav()` 旧硬编码。
- 旧「应用」菜单不再出现；`工具` 按 §1.4 渲染；`网站` 由内核固定的三个 L2 渲染。

### 7.3 6 个官方插件应声明什么

| 插件 | L1 | 插件 L2 | L3 | 固定页槽位 | 批次 |
|---|---|---|---|---|---|
| `com.skiff.files` | 网站 `sites` | `files` 站点文件 | 无（单视图 `main`） | — | B3 |
| `com.skiff.cron` | 工具 `tools` | `cron` 任务 | 任务列表 / 执行记录 / 设置 | — | B3 |
| `com.skiff.monitor` | 工具 `tools` | `monitor` 监控 | 总览 / 历史采样 / 进程 / 网络 | `overview.health` | B2 |
| `com.skiff.guard` | 工具 `tools` | `guard` 守护 | 容器 / 事件 / 设置 | — | B3 |
| `com.skiff.firewall` | 工具 `tools` | `firewall` 防火墙 | 防火墙规则 / fail2ban | — | 已上线（0.1.0，未验收） |
| `com.skiff.ftp` | 工具 `tools` | `ftp` FTP 管理 | 账号 / 运行状态 | — | 已上线（0.1.0） |

说明：

- 网站一级由内核自带（网站列表 / 运行环境 / Web 服务），原 `com.skiff.nginx` 已内核化，不再声明插件菜单。
- files/cron/monitor/guard/ftp 全部归「工具」，不再作为一级或二级「应用」的平铺项。
- L3 的 `view` 必须对应真实存在的 `ui/<view>.html`；没有对应视图就先不声明，避免假标签。

### 7.4 分批

1. **B1 内核先行**：注册表、manifest 解析/校验、旧路由映射、旧 `[[ui]]` 降级、L1/L2 渲染；不引入 L3 与设置页。
2. **B2 高价值路径**：内核网站（网站列表 / 运行环境 / 设置 + 运行环境安装）、monitor（工具 + `overview.health` 槽位）；同时落 user_prefs 与布局切换。
3. **B3 全量**：files/cron/guard/ftp 声明、插件中心 L2 拆分、Docker 规划页、pin/固定区与管理员默认。

---

## 8. 验收口径与测试矩阵

### 8.1 契约级断言

| # | 场景 | 断言 | 层级 |
|---|---|---|---|
| C1 | 合法 manifest `[[menu]]` | schema 解析成功；L2/L3 稳定键正确 | 单测 |
| C2 | `parent` 非固定 L1 | 安装拒绝 `MENU_PARENT_UNKNOWN` | 单测/集成 |
| C3 | 插件声明 `core/...` 或重复键 | 拒绝 `MENU_ID_CONFLICT`，无覆盖 | 单测 |
| C4 | L3 深度超限 / 数量超限 | 拒绝 `MENU_DEPTH_EXCEEDED` / `MENU_TOO_MANY_ITEMS` | 单测 |
| C5 | `view` 非法/文件不存在/同插件重复 | 拒绝 `MENU_VIEW_INVALID` / `MENU_VIEW_MISSING` / `MENU_VIEW_CONFLICT` | 单测 |
| C6 | 旧包只有 `[[ui]]` | 合成 `plugin/<id>/legacy`；旧 nginx 包兼容映射到 `sites`，其余映射到 `tools` | 单测/集成 |
| C7 | 网站 L1 的内核固定 L2 | 始终出现网站列表 / 运行环境 / Web 服务，三项均 200，无 404 | 集成 |
| C8 | 0 个工具提供者 | 工具 L1 显示空态 | 集成 |
| C9 | 插件未启用 | 其 L2 不出现在导航；插件中心可启用 | 集成 |
| C10 | 角色不匹配 | 菜单不渲染；直接访问菜单 URL 404 | 集成 |
| C11 | 顺序偏好 | 用户顺序生效；新项按合并算法插入 | 单测/集成 |
| C12 | 隐藏偏好 | 隐藏项不渲染，但保留排序位置 | 集成 |
| C13 | 布局切换 | 同一菜单键/URL 集合，`data-layout` 变化，结构不变 | 集成 |
| C14 | 深链 | `/{p}/m/...` 命中正确 L1/L2/L3 当前态 | 集成 |
| C15 | L3 无 JS | iframe query 含 `menu`/`tab`，`tab` = 视图名，插件可渲染 | 集成/浏览器 |
| C16 | 槽位失败 | monitor 禁用后概览回退原生健康块；页面 200 | 集成 |
| C17 | 卸载清理 | 插件删除后菜单消失；`user_prefs` 无该前缀 | 单测/集成 |
| C18 | 悬停 | hover 不展开；click 才展开；Escape 关闭并还焦点 | 浏览器 |
| C19 | 窄屏 | ≤860px 折叠；无 hover 浮层；L3 横向滚动 | 浏览器 |
| C20 | CSP | 菜单页无内联脚本/外链；插件 iframe CSP 仍收紧 | 集成/静态扫描 |

### 8.2 需要补的单测/集成测试方向

- `manifest` 单测：新增字段解析、所有错误码、旧 `[[ui]]` 合成。
- 纯函数注册表测试：`builtin + plugin declarations → effective tree`，覆盖 role/gate/hidden/order/merge/uninstall。
- Web 集成：用临时 DB 种插件记录 + manifest，GET `/{p}`、`/{p}/m/...`，断言 DOM 与当前态。
- 设置集成：POST 菜单偏好（CSRF）→ 重新 GET 断言顺序/隐藏/布局；越权用户不能改别人。
- 槽位集成：插件禁用/模拟加载超时 → 回退块；`overview.health` 不破坏原生数据块。
- 浏览器：桌面上下/左右、窄屏、键盘、hover 不展开、Escape、焦点归还、L3 切换后 iframe URL。
- 安全：恶意 label（HTML）、外部 icon、`target.ui` 越界、伪造 `core` 键、重复键、越权角色。

三关口径沿用仓库规则：`cargo fmt --all`、`cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`、`cargo test --workspace --locked`。

---

## 9. 分阶段实现计划

### P1：注册表与声明层（不改视觉交互大框架）

- 扩展 `skiff-pkgfmt::Manifest`：`[[menu]]` / `[[menu.items]]` / `[[mount]]` 结构 + 校验 + 错误码。
- 内核新增内置菜单注册表（六 L1、Docker/插件/设置内置 L2；规划项标 `route_available=false`）。
- 用注册表替换 `handlers::nav()` 的硬编码；保留旧 `active` → 新键映射；旧 `[[ui]]` 降级。
- 产出：现有页面仍可打开；导航结构变为六 L1；工具空态可验收；网站内核固定 L2 可验收；所有契约单测通过。
- 风险：`deny_unknown_fields` 导致旧内核不识别新包（预期）；当前「应用」入口消失需同步测试与文档；旧 active 映射漏项会丢当前态。

### P2：L3、设置与槽位

- 新增 `user_prefs` 表（schema v2）与菜单设置页 `/settings/menu`。
- 内核 L3 标签栏 + 规范 URL + iframe `menu/tab` + `skiff.menu` 消息。
- 上下/左右布局（同一 DOM，`data-layout`），窄屏折叠与交互修正（点击展开、无 hover 打开）。
- monitor 等官方插件声明 L2+L3；monitor 声明 `overview.health` 槽位；概览页接入槽位失败降级。
- 产出：工具 → 主机监控 → L3 切换 → iframe 内容；排序/隐藏/布局保存生效；概览可显示 monitor 健康槽位。
- 风险：iframe 状态保持与无 JS 降级；用户偏好合并；槽位加载超时的视觉抖动；CSP 回归。

### P3：全量迁移与规划项

- files/cron/guard/monitor/ftp 声明迁移；插件中心 L2 拆分；旧「应用」代码删除。
- Docker 概览 / 编排 / 设置 页面与内置 L2（编排无后端前保持 `route_available=false`，不出现死链）。
- pin/固定区、管理员实例默认、菜单设置细节（拖拽、恢复默认、预览）。
- 产出：6 个官方插件全部走新契约；旧 `[[ui]]` 仅作兼容；契约测试矩阵全绿。
- 风险：官方插件 UI 需配合 `tab`/`menu` query，否则 L3 是空标签；Docker 编排缺少后端可能导致长期隐藏项；用户偏好迁移与清理遗漏。

---

## 10. 开放问题（最多 3 条）

### Q1. 工具在 0 提供者时，默认显示空态还是自动隐藏？

- 说明：`网站` 已内核化、L2 固定，不再参与本问题；本问题只针对插件支撑的 `工具`。
- 推荐：**默认显示空态**（固定一级集合不变，空态给出「去插件中心」），用户可在菜单设置里隐藏。
- 影响：导航始终是六项；测试要覆盖空态；与旧「应用 0 项隐藏」规则相反，需确认。
- 若选自动隐藏：六项固定会退化为动态集合，与「一级固定」冲突。

### Q2. L3 切换是同一 iframe 内消息切换，还是每个 L3 独立 iframe/页面？

- 推荐：**同一 iframe + `tab` query + `skiff.menu` 消息**；`ui` 不同时才换 `src`。
- 理由：保留插件内部状态、避免每次切标签重建 iframe、深链仍可用。
- 影响：插件 UI 必须监听 `skiff.menu` 或读 query；如果插件坚持多 HTML 文件，也能通过 `ui` 字段工作，但状态不保留。

### Q3. 菜单偏好只做按用户，还是管理员需要全局策略/锁定？

- 推荐：**P2 只做按用户 + 全局默认布局**；不做强制模板与锁定。
- 理由：改动面最小、符合“高度自定义”，避免引入权限模型外的“策略层”。
- 影响：每个用户可自行隐藏/排序；若以后企业需要统一模板，再增加 `menu.policy` 与管理员锁定，属于 P4。

---

## 11. P1 落地记录（与 §9 计划的差异）

用户定稿 Q1–Q3 后，P1 的交付范围被扩大到「声明层 + 渲染 + 菜单设置 + 布局」，原计划中的 P2 部分项前移。实际落地：

**已完成**
- `skiff-pkgfmt`：`[[menu]]` / `[[menu.items]]` / `[[mount]]` 解析与校验；错误码 `MENU_PARENT_UNKNOWN` / `MENU_ID_INVALID` / `MENU_ID_CONFLICT` / `MENU_VIEW_INVALID` / `MENU_VIEW_MISSING` / `MENU_VIEW_CONFLICT` / `MENU_ROLE_INVALID` / `MENU_GATE_INVALID` / `MENU_ORDER_INVALID` / `MENU_LABEL_INVALID` / `MENU_TOO_MANY_ITEMS` / `MENU_NO_UI_ENTRY` / `MENU_API_VERSION` / `MOUNT_*`；安装验包时校验视图文件确实在 `ui/` 内。
- 内核菜单注册表 `crates/app/src/menu.rs`（领域逻辑，`crates/web/src/menu.rs` 只做再导出）：六个固定 L1、内置 L2、启用插件声明合并、角色/能力门、旧包 `[[ui]]` legacy 降级、稳定键与规范深链。
- 渲染：同一份菜单 DOM，`body[data-layout]` 在 `top`/`left` 间切换；左侧栏分组内联展开；点击为主、悬停 320ms 辅助、Escape/焦点/空白关闭、方向键导航、≤860px 抽屉。
- 菜单设置页 `/settings/menu`：L1/L2 上移/下移、隐藏/显示、固定/取消固定、恢复默认、布局切换；偏好写 `user_prefs`（schema v2），仅当前用户。
- 空态：工具无提供者保留一级并给出「去插件中心」动作；已安装未启用时直接指向已安装列表；网站为内核固定 L2，无空态。
- 深链 `/{p}/m/...` 与旧路由并存；插件信息页 `/{p}/plugins/<id>` 与功能页 `/{p}/plugins/<id>/<view>` 分离（见 §13 与 `PLUGIN_UI_CONTRACT.md`）。
- 卸载插件时清理所有用户偏好中的 `plugin/<id>/...` 残留。
- 固定项：偏好里的 pin 在导航顶部渲染为固定快捷区（P1 提前落地）。

**明确未做（留 P2/P3）**
- L3 标签栏未渲染：`[[menu.items]]` 已解析、已进入菜单树并生成规范 URL，但 `/{p}/m/plugin/<id>/<l2>/<l3>` 仍落到该 L2 工作台；`skiff.menu` / `tab` query 未接入。
- `[[mount]]` 只解析与校验，概览页尚未渲染 `overview.health` 等槽位。
- 菜单设置只有上移/下移，没有拖拽；没有管理员全局策略/锁定（Q3 留 P3）。
- Docker 的概览/编排/设置仍未注册（无后端路由，避免死链）。
- 官方插件未改 manifest：monitor 等继续走 legacy；L2/L3 声明迁移留 P2。

**测试**
- `skiff-pkgfmt`：合法菜单/槽位解析、非法 parent/id/target/roles/gates/order/slot/refresh、重复 id、L3 超限、1.0 包声明菜单拒装、包内 ui 资产校验。
- `skiff-store`：`user_prefs` 按用户隔离、upsert、按键枚举、删除。
- `skiff-web` 单测：legacy 映射、声明合并与 docker gate、用户顺序 + 新项锚点插入、锁定项不可隐藏、`default_visible` 首次保存语义、卸载键清理。
- `skiff-web` 集成：六 L1/空态/插件二级高亮、菜单设置隐藏与按用户隔离、布局切换、恢复默认、深链与旧路由、viewer 不渲染审计、卸载清理。

---

## 12. P2 落地记录（L3 标签栏 + 固定页槽位 + 官方插件迁移）

**L3 标签栏（契约 §1.3 / §3.1）**
- 内核在插件工作台渲染 `.workbench-tabs`；每个标签是真实 `<a href="/{p}/m/plugin/<id>/<l2>/<l3>">`，无 JS 可直达。
- 三个标签共用同一个 iframe：同一 `ui` 文件时只 `postMessage({type:"skiff.menu", nonce, menu, tab})`，不重载（不白屏、不重复鉴权）；声明了不同 `ui` 的标签才换 iframe 文件。
- iframe URL 统一为 `/plugins/<id>/ui/<ui>?nonce=...&menu=plugin/<id>/<l2>&tab=<tab>`；`menu` 使用完整稳定键，插件侧也接受局部 id。
- 角色/能力门在解析阶段过滤：不可见 L2 或未知 L3 一律 404，不渲染后再做 403。
- 旧 `legacy` 深链在插件升级为声明式后落到首个可见 L2，旧 URL 不 404。
- 插件 UI 侧新增 `setView(tab)`：无 `tab` 参数（插件中心直接打开）时保持旧行为「全部视图可见」；有 `tab` 时只显示对应 `[data-view]`。

**固定页槽位（契约 §5）**
- `crates/app/src/menu.rs::mount_views` 按槽位收集「已启用 + 角色可见」的 `[[mount]]`，返回渲染所需 nonce / rpc / csrf / 白名单。
- 内核模板 `mount_card.html` 统一负责卡片外框、标题、`sm/md/lg` 尺寸、失败降级与重试；插件只提供 iframe 内容。
- `overview.health` 有挂载时替换内核默认健康块；插件停用、角色不可见或未声明时自动回落原生健康块。
- `overview.actions` / `overview.side` 同机制渲染；当前官方插件只声明 `overview.health`。
- 失败降级：启用但界面未起来时，5s 超时显示 `data-mount-fail` 区块（重试 + 插件详情/日志入口）；`refresh != 0` 时父页面定时发 `skiff.refresh`。
- monitor 的 `health.html` 在无采样/进程未启动时显示明确空态文本，不渲染空白卡。

**官方 4 插件迁移（`api = "1.1"` + `[[menu]]`；P3 已升至 `1.2`，见 §13）**

| 插件 | 一级 | 二级 | 三级 | 槽位 |
|---|---|---|---|---|
| files | 网站 `sites` | 站点文件 `plugin/com.skiff.files/files` | 无（单视图） | — |
| cron | 工具 `tools` | 计划任务 `plugin/com.skiff.cron/jobs` | 任务列表 / 新建任务 | — |
| monitor | 工具 `tools` | 主机监控 `plugin/com.skiff.monitor/monitor` | 总览 / 历史采样 | `overview.health` |
| guard | 工具 `tools` | 容器守护 `plugin/com.skiff.guard/guard` | 容器 / 事件 | — |

- P3 起 `[[ui]].path` 指向 `main.html`（旧包回落 `ui/index.html`）；插件中心入口改为信息页 + 规范功能页。
- 未声明「磁盘」「执行记录」「守护设置」等视图：插件 UI 没有对应内容时不造假标签（契约 §7.3）。
- 工具内顺序：cron 20 / monitor 30 / guard 40；`files` 已迁到「网站」下（`order` 60），
  与内核「工具 → 文件管理」互不占位。内核文件管理仍是独立的管理员客户端（含回收站/配额/上传下载）。

**测试**
- `skiff-pkgfmt`：官方 4 包全部解析、菜单/槽位引用的 `ui` 文件存在、monitor 声明 `overview.health`。
- `skiff-web` 集成：L3 深链（默认第一个 L3、切换 `tab`、单个 iframe、不存在/角色不可见 404、`legacy` 回落）、槽位成功渲染（`data-slot` / nonce / `data-refresh` / 失败降级块）、停用与角色过滤回落原生健康块。

**P2 未做（留 P3）**
- 菜单设置页未列出 L3（L3 由声明决定，不单独隐藏/排序）。
- 槽位只实现 `overview.*` 与 `sites.header` 的通用渲染；`sites.header` 暂无插件声明。
- 拖拽排序、管理员全局布局策略、孤儿偏好启动清理任务仍未做。

---

## 13. P3 落地记录（信息页 / 功能页拆分 + `view` 视图模型）

**路由与视图**
- `/{p}/plugins/<id>` 改为内核渲染的插件信息与管理页，不再承载 iframe 工作台。
- 插件功能页固定为 `/{p}/plugins/<id>/<view>`；`/<view>` 缺省时取第一个可见 L2 的首个视图（旧包取 `main`）。
- `/{p}/m/plugin/<id>/<l2>[/<l3>]` 仍是菜单深链：L3 的 `tab` 参数即视图名，`l3` 为空时取该 L2 默认视图。
- 菜单深链与规范功能页都只选视图，不再出现第二套 `target.ui` / `tab` 语义。

**manifest**
- `[[menu]]` 增加 `view`（默认 `main`）；`[[menu.items]]` 增加 `view`（默认等于 `id`）。
- 视图名映射 `ui/<view>.html`；同一插件内视图名必须唯一（`MENU_VIEW_CONFLICT`），文件缺失拒装（`MENU_VIEW_MISSING`）。
- 无 L3 的 L2 只有隐式主视图，内核不渲染标签栏。
- 旧包（只有 `[[ui]]`，无 `[[menu]]`）：`main` 指向原 `ui/index.html`，其余 `[[ui]]` 按原顺序合成 `legacy-2`… 标签。

**官方插件**
- files → 网站下的单视图 `main`（`ui/main.html`，站点内容目录）；cron → `list` / `new`；guard → `containers` / `events`；monitor → `overview` / `history`（保留 `overview.health` 槽位）。
- 新增 `plugins/testkit`（`com.skiff.testkit`）作为架构验收插件，覆盖多视图、单视图、槽位、RPC、能力面与主题。

**测试**
- `skiff-web` 集成：信息页字段与管理动作、`/plugins/<id>/<view>` 规范路由、未知视图 404、无 L3 不渲染标签栏、旧包 `ui/index.html` 回落、testkit 多视图与 `overview.actions` 槽位。
- 完整契约与测试矩阵见 [`docs/PLUGIN_UI_CONTRACT.md`](PLUGIN_UI_CONTRACT.md)。

**内核「工具 → 文件管理」的 L3（回收站合并）**
- 回收站不再作为独立二级菜单项，改为内核固定二级 `core/files`（文件管理）的 L3 `core/files/trash`；二级只剩「文件管理 / 配额管理」。
- 两个页面由内核渲染同款 `.workbench-tabs`（`data-menu-key` + `aria-current`），不新造菜单机制；无 JS 可直达。
- 旧入口不 404：页面路由 `/{p}/files/trash` 与深链 `/{p}/m/core/files/trash` 原样保留（后者即新的 L3 深链）；用户偏好里遗留的 `core/files/trash` 排序/固定键由 `apply_order` 与固定项过滤自动丢弃。
- 菜单设置页仍只列 L1/L2（§12 口径），不提供 L3 单独隐藏/排序。

---

## 附录 A：内置一级与默认 L2 注册表

| L1 id | 标签 | 类型 | 默认 L2 |
|---|---|---|---|
| `overview` | 概览 | 直达 | —（页面内槽位） |
| `sites` | 网站 | 分组 | 内核固定：网站列表 / 运行环境 / Web 服务（不依赖插件） |
| `docker` | Docker | 分组 | 概览 / 容器 / 镜像 / 网络 / 卷 / 编排 / 设置（概览、编排、设置 P3，`route_available=false` 前不渲染） |
| `plugins` | 插件 | 分组 | 已安装 / 发现 / 可更新 |
| `tools` | 工具 | 分组 | 插件声明；无提供者显示空态 |
| `settings` | 设置 | 分组 | 安全设置 / 菜单设置 / 审计日志 |

内置图标集（v1）：`dot`、`overview`、`globe`、`docker`、`plug`、`tools`、`settings`、`file`、`clock`、`activity`、`shield`、`container`、`image`、`network`、`disk`、`archive`、`list`、`user`、`lock`。

## 附录 B：非目标

- 本文件不实现任何代码、不改模板/样式/后端/测试。
- 不定义 Docker Compose 领域能力；编排页只在 P3 且后端具备后出现。
- 不改变插件安装/授权/验签流程；菜单声明校验只是 manifest 校验的新增部分。
- 不允许插件自定义一级、任意 HTML/CSS 导航、外部图标或外链资源。
