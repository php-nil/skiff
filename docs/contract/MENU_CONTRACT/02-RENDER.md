# 渲染契约与菜单设置持久化（§3–4）

> 上级索引：[MENU_CONTRACT.md](../MENU_CONTRACT.md) ｜ 拆自 `docs/contract/MENU_CONTRACT.md` 原 §3–4（2026-11-16 整理，正文未改）。

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
| L1 直达（概览） | `/{p}` | budding 壳页；`/{p}/m/overview` 是保留的概览 SSR 入口 |
| 内置 L2 | `/{p}/<l2-path>` | 如 `/{p}/containers`、`/{p}/runtimes`、`/{p}/plugins/discover` |
| 插件 L2 | `/{p}/m/plugin/<plugin-id>/<l2-id>` | 如上例 |
| 插件 L3 | `/{p}/m/plugin/<plugin-id>/<l2-id>/<l3-id>` | 选中对应标签 |
| 插件信息与管理 | `/{p}/plugins/<id>` | 壳页渲染，非插件页面 |
| 插件规范功能页 | `/{p}/plugins/<id>/<view>` | 等价于默认/指定视图的深链 |
| 插件 iframe 内容 | `/{p}/plugins/<id>/ui/<path>?nonce=...&menu=<l2-key>&tab=<l3-view>` | 内核托管，非用户主入口 |

- 规范 URL 是菜单身份的唯一外部表现；菜单 `href` 必须落在规范 URL 上，不得指向已删除的旧深链。
- `/{p}/m/*` 只有两条：`overview`（概览 SSR，插件槽位迁完即删）与 `plugin/*`（插件工作台）；`/{p}/m/core/*` 已删除并返回 404。
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
| 菜单集合、内置 L1/L2、URL 规则 | 内核 | 代码 | 任何人都不能由插件改 |

- 「全局 vs 按用户」结论：**顺序/隐藏/pin 一律按用户**；布局固定为侧栏，内容宽度固定铺满，均不再持久化。
- 管理员不能看到其他用户的私有排序；不做“强制模板/锁定布局”。若以后需要，再加全局策略层。

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
    "order": {
      "sites": ["core/sites/list", "core/sites/env", "core/sites/web"],
      "docker": ["core/docker/overview", "core/docker/containers"]
    },
    "hidden": ["plugin/com.skiff.guard/guard"],
    "pinned": ["core/docker/containers"]
  }
  ```
- 全局实例默认布局与内容宽度字段已删除，不再读取 `settings.menu.layout.default`。
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

### 4.5 布局（固定）

- 壳固定使用侧栏（左右）布局：`body[data-layout="left"]`，不再保存布局偏好。
- 结构不变保证：菜单注册表、菜单键、URL、L2/L3 顺序完全一致；禁止为其它布局另生成一份导航数据。
- 布局是内核功能：插件 manifest 没有 layout 字段，插件也不能通过 CSS 影响宿主布局。
- 内容区固定铺满侧栏之外的可用宽度，不提供「自适应 / 固定」切换。
- 窄屏 ≤860px 时仍走 §3.6 折叠规则，`data-layout` 仍保留但视觉统一，内容区取消侧栏留白。

### 4.6 菜单设置页

- 入口：`设置 → 菜单设置`，稳定键 `core/settings/menu`，URL `/{p}/settings/menu`。
- 内容：
  - L1 列表：显示/隐藏、上移/下移、恢复默认。
  - 每个 L1 展开后的 L2/L3：显示/隐藏、上移/下移、固定/取消固定。
  - 锁定项：`概览`、`设置`、`菜单设置`自身不可隐藏（防止把自己锁在门外）；其余可隐藏。
  - 「恢复默认」只清当前用户的 `menu.prefs`，不触碰插件声明。
- 保存：POST + CSRF；失败保持原值并显示原因；成功审计。
- 预览：设置页提供同结构的静态预览，不改变真实导航直到保存。

### 4.7 界面设置页

- 入口：`设置 → 界面`，稳定键 `core/settings/ui`，URL `/{p}/settings/ui`。
- 最小集合只有「主色」一项，按**全站**生效（全局设置，非按用户）：写入仅管理员，读取对所有登录用户开放。
- 主色存 `settings` 表 `ui.theme.primary`，`#rrggbb` 小写；空值恢复缺省 `#1867c0`。
- 下发路径：`GET /ui/v1/session → theme.primary`；壳启动时**不写入 schema 树**（budding 1.8.0-r4 未注册根级 `theme`，写入会 `CompileError` 白屏），改为向文档注入 `--v-theme-primary` / `--v-theme-on-primary` / `--bud-admin-primary` 等 CSS 变量（对 `.v-theme--light/.v-theme--dark` 用 `!important` 覆盖 Vuetify 缺省主色）；保存后当前页刷新，其他会话下次加载页面生效。
- 校验失败 400 且不落库；成功审计 `settings.ui.update`。

---
