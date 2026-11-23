# 渲染契约与菜单设置持久化（§3–4）

> 上级索引：[MENU_CONTRACT.md](../MENU_CONTRACT.md) ｜ 拆自 `docs/contract/MENU_CONTRACT.md` 原 §3–4（2026-11-16 整理，正文未改）。

---

## 3. 渲染契约

菜单的**数据**由内核决定，**DOM 由 budding 壳页渲染**；内核不再输出菜单 HTML，也不随包发菜单 CSS。

### 3.1 内核输出：`GET /ui/v1/menu`

```
{ "data": { "groups": [ { "key", "label", "icon", "href", "items": [...], "empty": {...} } ] } }
```

- `groups[]`：固定 L1（`overview` / `sites` / `docker` / `tools` / `settings`…），顺序由内核注册表决定，前端不重排。
- `groups[].href`：L1 直达链接（`overview` → `/{p}`）；有子项时该 `href` 只用于高亮判定，不作导航目标。
- `items[]`：L2 / 插件项，字段 `{key, label, icon, href, plugin, plugin_id, default_view, views[]}`。
  - `key` 是菜单身份（`core/<l2>`、`plugin/<id>/<l2>`）；设置、测试、前端只认 key，不解析 label。
  - `href` 一律是规范路径（§3.2）；插件项由 `plugin_id` + `default_view` 拼成 `/{p}/plugins/<id>/<view>`。
  - `views[]` 是插件声明的视图清单，壳页据此生成页内横向三级菜单（当前页有子视图才出现，全量平铺 + 当前项强调）。
- `empty`：分组空态文案与行动链接（插件全部停用/未安装时下发，指向插件中心）。

菜单 DOM（类名、aria、展开/收起交互、窄屏抽屉）全部由 budding 运行时实现；内核只保证 JSON 的字段与顺序，
不再定义 `<nav id="skiff-menu">` 结构。

### 3.2 深链 URL 形状

| 页面 | 规范 URL | 说明 |
|---|---|---|
| L1 直达（概览） | `/{p}` | budding 壳页（数据走 `GET /ui/v1/overview`） |
| 内置 L2 | `/{p}/<l2-path>` | 如 `/{p}/docker/containers`、`/{p}/sites/runtimes`、`/{p}/plugins/discover` |
| 插件 L2 | `/{p}/plugins/<plugin-id>/<view>` | 有 schema 由壳页原生渲染；`?legacy=1` 才是 iframe 工作台 |
| 插件 L3 | 同上 + 页内横向三级菜单 | 视图名直接寻址；iframe 工作台按视图名选中标签 |
| 插件信息与管理 | `/{p}/plugins/<id>` | 壳页渲染，非插件页面 |
| 插件 iframe 内容 | `/{p}/plugins/<id>/ui/<path>?nonce=...&menu=<l2-key>&tab=<view>` | 内核托管，非用户主入口 |

- 规范 URL 是菜单身份的唯一外部表现；菜单 `href` 必须落在规范 URL 上。
- 深链必须能在无 JS 下打开；JS 只做同页增强（更新 iframe / pushState），不能成为唯一入口。
- URL 中 `plugin-id` 含 `.`，不再二次编码；视图名只用 `[a-z0-9-]`。
- 不允许把 label、`ui` 文件路径直接拼进菜单 URL。

### 3.3 当前态判定

1. 壳页按 URL 反推菜单身份：`/{p}` → `overview`；`/{p}/<l2-path>` → 对应 `core/<l2>`；
   `/{p}/plugins/<id>/<view>` → `plugin/<id>/<l2>` 且选中 `<view>`。
2. L2 命中：该 L2 高亮；有 L3 但 URL 未带具体视图时，选中第一个可见视图。
3. 不用「路径前缀包含」猜当前态，必须查路由表；侧栏高亮与 L3 选中都由同一份 key 驱动。
4. 插件 iframe 内的当前态由插件按 `?menu=&tab=` 自渲染，父页面不注入。

### 3.4 与 iframe 托管 UI 的关系

- 插件内容仍走沙箱路由：`/{p}/plugins/<id>/ui/<path>?nonce=...`，CSP 保持
  `frame-ancestors 'self'`、`connect-src 'none'`、`script-src 'self'` 等既有收紧项。
- 视图切换：
  - 默认无 JS：整页打开视图 URL，iframe 带 `?menu=&tab=`，插件按 query 渲染。
  - 有 JS：内核更新 URL + `iframe.src`（`ui` 变化时）或 `postMessage({type:"skiff.menu", menu, tab})`（复用同一 iframe），插件可保留状态。
- 插件不得：注入父页面样式/脚本、修改父 DOM；`target="_top"` 或跳出 iframe 伪装系统导航；
  加载外链 CSS/JS/字体/图片（沿用 `network_egress=[]` 与 CSP）；依赖导航 DOM 结构或 CSS 类名。
- 父页面只接受带本次 nonce 的 `skiff.rpc`，不因菜单消息放宽。

### 3.5 硬边界

- 菜单数据只经 `/ui/v1/menu` 下发，且已过 RBAC：未授权项不得「先下发再 403」。
- 插件 label/icon 只作文本与内置图标渲染；禁止 HTML、SVG 字符串、data URL、外链图标。
- 插件 iframe 不能 `postMessage` 修改菜单，只能接收 `skiff.menu`。
- 内核不再渲染菜单 DOM、不再发菜单 CSS；壳层样式在 `static/skiff-shell.css`，只作用于壳页自身。

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
- 窄屏折叠规则由 budding 运行时实现（抽屉/就手风琴），内核只给菜单数据，不参与断点判断。

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
- 三个字段按**全站**生效（全局设置，非按用户）：主色 `ui.theme.primary`、菜单底色 `ui.theme.menu`、页面底色 `ui.theme.page`，均存 `#rrggbb` 小写；`ui.theme.menu_custom` 控制菜单底色是否自定义。写入仅管理员，读取对所有登录用户开放。
- 缺省值（方案「深潜」）：主色 `#17a98e`、页面底色 `#f4f6f5`、菜单底色 `#0c2422`。`menu_custom` **缺省为真**——默认形态是深色导航 + 浅色内容；只有显式存 `"0"`（用户关掉开关）才按页面亮度自动取白/近黑。空值一律回落上述缺省。
- 预设三套，点「套用」一次写入三个颜色：`深潜`（= 缺省）、`石墨`（`#5a4ae3` / 纯白导航）、`灯塔`（`#b8f04b` / 近黑底）。预设自身必须满足主色在菜单底色上 ≥ 4.5:1，否则会被服务端校正成另一个色，色卡与结果对不上（`settings_page` 用例拦这一条）。
- 下发路径：`GET /ui/v1/session → theme.primary`；壳启动时**不写入 schema 树**（budding 1.8.0-r4 未注册根级 `theme`，写入会 `CompileError` 白屏），改为向文档注入 `--v-theme-primary` / `--v-theme-on-primary` / `--bud-admin-primary` 等 CSS 变量（对 `.v-theme--light/.v-theme--dark` 用 `!important` 覆盖 Vuetify 缺省主色）；保存后当前页刷新，其他会话下次加载页面生效。
- 校验失败 400 且不落库；成功审计 `settings.ui.update`。


---
