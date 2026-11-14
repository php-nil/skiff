# 内核 UI 交互基准（UI_CONTRACT）

> 适用范围：`crates/web` 渲染的内核页面（模板 + `static/skiff.css` / `static/skiff.js`）。
> 插件 iframe 内部不受本规范强制约束，插件视图边界见 [PLUGIN_UI_CONTRACT.md](PLUGIN_UI_CONTRACT.md)。
> 版本：v1（2026-11-08）。v1 只强制应用于「运行环境」「新建网站」「Web 服务」三处，其余页面见 §13 待对齐清单。

## 1. 三条硬规则

1. **每页只有一个主操作**：放在页面右上角，`.btn.primary`；不得下沉到列表头部或表格上方。
2. **危险操作必须二次确认**：统一走 `[data-confirm-dialog]`，文案含对象与后果，按钮顺序固定（取消在左、危险在右）。
3. **结构、类名、行为钩子统一**：页面骨架固定为「`page-head` → 提示条 → `toolbar`（可选）→ 内容」，行为一律用 `data-*` 钩子，不绑在纯样式类上。

## 2. 页面标题与副标题

```html
<div class="page-head">
  <h1>运行环境</h1>
  <span class="muted small">语言运行时与容器实例；Web 服务在「Web 服务」页单独管理。</span>
  <div class="actions">
    <button class="btn primary" data-runtime-new>新建运行环境</button>
  </div>
</div>
```

- `h1` 是页面名（列表页）或对象名（详情页）；副标题用 `.muted.small`，一句话说清「这页能做什么 / 数据来源」。
- 详情页 `h1` 放对象名，`id` / 版本放副标题，不重复 `h2` 同名词。
- 返回列表用 `.page-head` 内的 `ghost` 链接（左起第一个），不放独立面包屑组件。
- 页面内分块标题统一 `panel-head > h2`，字号小于 `h1`，不加主操作按钮。

## 3. 提示条（成功 / 警告 / 错误）

- 位置固定：`page-head` 之后、`toolbar` / 内容之前；一次最多一条 `error` + 一条 `notice`。
- 分类：
  - **错误**：`<div class="alert">`，常驻不自动消失，红色语义；必须包含「对象 + 原因 + 下一步」。
  - **成功**：`<div class="alert ok" data-toast>`，约 4.2 秒后自动消失；由 `data-toast` 统一接管。
  - **警告/状态说明**：`<div class="alert warn">` 或 `.banner warn`，常驻直到状态改变。
- 字段级校验用 `.field-error`，不要用 `alert`；请求级失败才用 `alert`。
- 来源：写操作结果统一用服务端一次性消息（flash，按会话短期存储，页面渲染一次即消费）：
  POST → 303 到无参数地址，渲染时从会话取一条成功/失败。**禁止**把结果塞进
  `?notice=` / `?error=` / `?ok=1`（2026-11-08 起统一改 flash；刷新会重放、链接带脏数据、错误进历史与日志）。
  AJAX 响应体也可直达提示条。抽屉内 `data-drawer-post` 表单：fetch 跟随 303 后
  直接用回跳返回的整页 HTML 重绘面板，并把同一份 HTML 里的提示条带回抽屉
  （面板本身在提示条之外，只取面板会丢反馈）。**禁止 `alert()` / `window.alert`**。
- 文案反例：「操作失败」「出错了」。正例：「安装任务未启动：该实例已有任务在执行，请等待完成后再试。」

## 4. 主操作与次操作

- **主操作**：`.page-head .actions > .btn.primary`，每页最多一个。
- **次操作**：同一 `.actions` 内，`ghost` / 链接，按「低危 → 高危」从左到右；危险动作（删除）放最右并加 `danger`。
- **行内操作**：单行操作放表格「操作」列；批量操作放 `toolbar` 右侧。
- `.page-head .actions` 固定 `margin-left:auto`，保证在任何副标题长度下都贴右；窄屏见 §10。
- 禁止把主操作放在 `panel-head`、表格上方或空态之外的正文中间。

## 5. 列表页工具栏

```html
<div class="toolbar">
  <form class="search" method="get" action="/{prefix}/sites" role="search">
    <input type="search" name="q" value="{{ keyword }}" placeholder="搜索域名、别名、站点目录或上游">
    <button class="btn" type="submit">搜索</button>
    <a class="ghost" href="...">清除</a>
  </form>
  <div class="actions">...</div>  <!-- 右侧：刷新 / 批量 -->
</div>
```

- **搜索**：放在最左；回车或点「搜索」提交 GET，参数名统一 `q`；**不做逐键即时过滤**（数据量大时以服务端为准）。
  - 有筛选时标题旁显示 `.count`（「筛选结果」/`N 条`）；无结果空态给「清除搜索」。
- **筛选**：`select` 放搜索右侧，变更后立即提交（GET），同一页只保留一个主筛选。
- **刷新**：放 `toolbar` 右侧；普通页面用同 URL 的 GET（保留 `q`），容器等已接 htmx 的页面可用 `hx-get` 局部刷新。
- **批量**：选中行后才出现，放最右；批量危险操作必须 `data-confirm`。
- 列表页若数据量小且无筛选需求，至少保留「计数 + 刷新」，不强制堆搜索框。

## 6. 表格列宽与溢出

- 表格必须包在 `.table-wrap` 内，窄屏横向滚动兜底；表格用 `.grid`（紧凑列表加 `.compact`）。
- 操作列固定 `th.col-actions` / `td.actions-cell`，宽度 1%、`white-space:nowrap`，禁止按钮换行撑破布局。
- 长文本列（路径、镜像、上游、命令）必须加 `max-width` + 省略号 + `title` 全文；标识、版本、短状态用 `.mono` / `.tag` / `.state`。
- 窄屏**不隐藏列**，只允许横向滚动；对象标识列（名称/域名）不可被省略。
- 单元格内多行信息用同一 `td` 内的 `.muted.small`，不为每行信息加一列。

## 7. 空态

```html
<div class="empty">
  <strong>还没有运行环境</strong>
  <p class="hint">点击右上角「新建运行环境」，选择 PHP / Node.js / Rust 并安装后即可绑定站点。</p>
  <button class="btn primary" data-runtime-new>新建运行环境</button>
</div>
```

- 结构：`strong` 一句结论 + `p.hint` 解释/引导 + 一个主操作（可选）。
- 区分三种空态：
  - 真空列表 → 引导创建（主操作）；
  - 搜索/筛选无结果 → 「清除搜索」；
  - 依赖未就绪（Docker / Web 服务 / 无可用运行环境）→ 用 `.empty.error-state` 并给安装入口。
- 空态里不放大段文档、不复制页面副标题。

## 8. 弹窗 vs 抽屉

| 场景 | 选择 | 宽度 | 关闭方式 |
| --- | --- | --- | --- |
| 新建/编辑单对象、字段 ≤ 8、确认类 | `<dialog class="modal">` | 480（窄）/ 720（表单） | ×、Esc、取消按钮 |
| 详情、多页签、日志、长表单 | `<dialog class="drawer">` | 720 常规 / 840 宽 / 92vw 窄屏 | ×、Esc；footer 常驻 |
| 列表内就地查看的作业详情（日志 + 进度） | `<dialog class="modal job-dialog">`（宽版，正文独立滚动） | 920 / 92vw 窄屏 | ×、Esc；footer 常驻 |

- **footer 常驻**：长表单弹窗（`.site-new-dialog` / `.runtime-dialog`）整窗纵向 flex，`.modal-body` 滚动、`.modal-actions` 固定在底部；普通小弹窗内容不滚动，动作区仍用 `.modal-actions`。抽屉用 `.drawer-foot`（`sticky`）吸底；主操作在右、取消在左；按钮顺序遵守 §9。
- 点遮罩**不关闭**（防误触），仅 `×` / `Esc` / 取消关闭；关闭后焦点回到触发元素。
- 同一页面同类弹窗只允许一个实例；抽屉内容由服务端渲染 HTML 注入 `[data-drawer-content]`，不再二次组装 DOM。
- 任务中心「查看详情」走宽版 `modal.job-dialog`：同源拉取 `/{p}/jobs/<id>` 后取 `<main>` 注入 `[data-job-dialog-body]`，字段与日志只维护一份；日志由 `attachJobLogs` 统一接管（首屏已渲染尾巴，之后 SSE 增量），关闭弹窗即断开 SSE；日志为空时服务端渲染 `[data-job-log-empty]`「暂无日志」并在首条日志到达时隐藏，不留空白区块。独立详情页保留为深链与取消/重试的后跳目标。
- 弹窗标题用 `h2`，不重复页面 `h1`；弹窗内不再嵌套 `page-head`。

## 9. 危险操作二次确认

- 统一组件：`[data-confirm-dialog]`；文案放在触发按钮的 `data-confirm`（同一表单多动作）或表单 `data-confirm`（整表操作）。
- **措辞**：`确认<动作><对象>？<后果>`。示例：
  - 「确认停止 Web 服务？所有站点将立即不可访问。」
- **按钮顺序**：取消（`ghost`，左）→ 确认动作（`danger`，右）；确认按钮文案用动词（删除/停止/卸载），不用「确定」。
- **共享运行环境**：一个 Node.js / Rust 环境只有一个应用进程与固定端口。有站点绑定时，运行环境详情页顶部
  常驻提示「已被 N 个站点使用」，停止/重启/重建/改端口先展示受影响域名列表与确认按钮（`confirm=1`），
  不直接执行；删除环境直接拒绝。绑定向导的运行环境选项与站点详情同时标注占用站点数。
- **删除站点走专用确认页**（不可恢复）：`{entry}/sites/<域名>/delete` 先展示将被删除的具体量
  （目录大小、文件数、关联定时任务数、运行环境绑定），再要求输入域名确认。Node.js / Rust 站点不再
  额外提示“删源码”：应用代码在运行环境的 `app/`，删站点只解除绑定。
  - 表单挂 `data-type-confirm="<域名>"`，输入框 `data-type-confirm-input`，提交按钮 `data-type-confirm-submit`；
    JS 在输入 trim 后与域名完全一致前保持 `disabled`。
  - 服务端必须校验提交的 `confirm` 字段等于域名，不能只依赖前端禁用。
  - **已对齐（2026-11-09）**：`site_delete.html` 按钮顺序改为「取消（`ghost`，左）→ 删除站点（`danger`，右）」，
    与 `[data-confirm-dialog]` 一致；`data-type-confirm` 输入校验与提交按钮 `disabled` 逻辑不变。
    同轮把 `plugin_confirm.html` 的「确认并安装 → 取消」也改为「取消 → 确认并安装」；全仓其余确认类页面
    （容器、镜像/网络/卷、插件卸载、回收站/配额、Compose、任务取消）均已是取消在左。
  - 其余危险操作仍走 `[data-confirm-dialog]`；新代码禁止新增 `window.confirm` 分支。

## 10. 窄屏断点（430 / 860 / 1024）

| 断点 | 降级规则 |
| --- | --- |
| ≤ 1024 | 副标题允许换行；`.actions` 仍贴右上；表格保留全部列并横向滚动。 |
| ≤ 860 | `.page-head` 改为纵向：标题/副标题在上，`.actions` 整行、按钮右对齐；抽屉宽度 92vw；`toolbar` 搜索独占一行、右侧操作换行；表格横向滚动。 |
| ≤ 430 | 主要按钮整行、最小触达高度 44px；弹窗/抽屉 92vw；表格横向滚动；禁止隐藏关键操作，必要时折叠为「更多」。 |

- 禁止用 `display:none` 隐藏关键操作；用换行、整行、横向滚动解决空间不足。

## 11. 命名与样式类前缀

- 页面骨架：`.page-head`、`.toolbar`、`.panel`、`.panel-head`、`.empty`、`.alert`、`.table-wrap`。
- 业务前缀：`runtime-*`（运行环境）、`site-*`（站点）、`web-*`（Web 服务）、`task-*`、`plugin-*`。
- 行为钩子统一 `data-*`：`data-runtime-new`、`data-runtime-open`、`data-drawer-post`、`data-confirm`、`data-type-confirm`、`data-toast`、`data-tab`。
- 新增类名必须带业务前缀；禁止 `.left`、`.right`、`.red`、`.big` 这类无语义名。
- 行为钩子与样式类解耦：JS 只监听 `data-*`，CSS 只匹配语义类，避免改名互相破坏。

## 12. 本轮应用范围（已完成）

1. **运行环境**：`page-head` 右上角主操作、计数 + 刷新工具栏、空态、详情抽屉 footer 常驻与安装提示、窄屏降级；列表不再展示未安装的种子行与 Web 服务。
2. **新建网站**：弹窗标题/副标题、footer 顺序、运行环境下拉只列已就绪实例、无可用实例时的引导空态、窄屏 92vw。
3. **Web 服务**：在 `Web 服务` 页内新增容器运维区（安装/启动/停止/重启/删除），危险操作走统一确认；运行环境列表不再出现 Web 服务行。

## 13. 现有页面盘点与待对齐清单

> 状态更新（2026-11-08 第二批）：下表标「已对齐」的页面都在本地 Windows 实例上用
> CDP 按 430 / 860 / 1024 三档实测过，页级无横向溢出、主操作在右上、空态可操作；
> Docker / nginx 未就绪的页面只验证到其 gate 空态。

| 页面 | 现状 | 与基准差距 | 优先级 |
| --- | --- | --- | --- |
| 运行环境（第一批） | 主操作在 `panel-head`、无工具栏、种子行占位 | 已按 §2/§5/§7/§8 对齐 | — |
| 新建网站（第一批） | 运行环境选项含未安装项、无空态引导 | 已按 §4/§7/§8 对齐 | — |
| Web 服务（第一批） | 旧 SSR 有容器运维，原生页只有配置/日志 | 已在原生页补齐容器运维区：未安装只显示「安装」，已安装显示启动/停止、重启、重新安装、删除；危险操作确认，安装中展示 `install.steps` 并可刷新进度 | — |
| 网站列表（第二批） | `page-head` 主操作、工具栏计数/筛选/刷新、空态引导 | 已按 §2/§4/§5/§6/§7 对齐 | — |
| 计划任务（第二批） | 计划任务不支持手动新建（站点/插件注册），空态改为引导去站点开设备份 | 已按 §5/§7 对齐；不再补「新建任务」主操作 | — |
| 容器列表（第二批） | 主操作在页头，计数/筛选/刷新拆进 `.toolbar` | 已按 §2/§4/§5/§6/§7 对齐；Docker 不可用时为 gate 空态 | — |
| 容器新建 / Compose（第二批） | 新建页与 Compose 页按 §2/§4/§7/§9/§10 搭建 | 已按基准对齐 | — |
| 插件中心（第二批） | 成功提示接 `data-toast`；已安装页有工具栏与主操作；卸载走统一确认 | 已按 §2/§4/§5/§7/§9 对齐；发现/可更新是工具页签，主操作是页内的「上传并校验」 | — |
| 插件信息 / 工作台（第二批） | 内层标题改 `.panel-head`；关联菜单长路径折行；工作台 iframe 高度收敛、主题占位；Docker 不可用给 warn gate，异常容器入口保留 | 已按 §2/§8/§10 对齐；2026-11-08 在 `456e040` 上实测三小项并修复高度/占位/gate 的四个问题，**新代码待下一版 Release 上机复验** | — |
| 镜像 / 网络 / 卷（第二批） | 原只有表格，无主操作、无空态 | 已重写：页头主操作 + 弹窗 + 计数刷新 + `col-actions` + gate 空态 | — |
| 回收站 / 配额（第二批） | 脚本内 `window.confirm` | 已改走统一确认 `skiffConfirm`（对象 + 后果 + 动词） | — |
| 文件管理（阶段二） | 左右双栏 + 行操作菜单；目录下载服务端打包 | 已按 §2/§4/§5/§7/§8/§9/§11 对齐 | — |
| 站点详情（第二批） | 多页签 + 分步操作 | `.detail-head` 已并入 `.page-head`；删除仍走独立输入域名确认页（§9 的强确认变体），按钮顺序已按 §9 对齐（2026-11-09） | — |
| 任务 / 作业详情（第二批） | 详情头部原为 `.detail-head` | 已并入 `.page-head`；无动作的「空」区块不再要求引导按钮 | — |
| 设置 / 审计（第二批） | 设置页信任公钥表未包 `.table-wrap` | 已包 `.table-wrap` + `col-actions`；审计保持只读，无主操作 | — |
| 设置 → 界面（新增） | 壳主题无配置入口 | 最小集合只提供主色（`#RRGGBB`）；表单沿用写卡片 + 保存后刷新反馈，非管理员只读并说明原因；按全站生效 | — |
| 运行时详情直接页（第二批） | 与抽屉共用模板，但直接页无顶部返回入口 | 已补「返回运行环境列表」`ghost`；抽屉内隐藏 | — |

## 14. 验收清单（第二批）

- [x] 列表页统一 `page-head → 提示条 → toolbar（计数/筛选/刷新）→ 表格/空态`。
- [x] 每页至多一个 `.btn.primary`，固定在页头右上；≤430 整行 44px。
- [x] 提示条紧跟 `page-head`；错误常驻、成功 `data-toast` 约 4.2 秒消失；全仓无 `alert()`。
- [x] 表格统一 `.table-wrap`，操作列 `col-actions` / `actions-cell`，窄屏横向滚动而非压列。
- [x] 危险操作统一走 `[data-confirm-dialog]`（表单 `data-confirm` 与脚本 `window.skiffConfirm` 共用外观）。
- [x] 430/860/1024 三档实测：20 个页面 + 任务详情 + 插件信息 + 工作台，页级无横向溢出。
- [x] 工作台 iframe 高度收敛：短内容贴合（下限 200px），长内容以「视口高 − 宿主面板顶边 − 面板边框/外边距 − main 下内边距 − 页脚」封顶并在 iframe 内部滚动；切 UI 文件时高度回落并重新上报。
  - `456e040` 实测（问题基线）：testkit/cron/ftp 的 iframe 高度等于内容高（466 / 566 / 635 @ 430），长内容把整页撑出滚动条；guard 内容 304 却固定 320，留 16px 空白。
  - 新规则本地同源码 mock 实测：短内容 260→260（三档）；长内容 1600 → 430/860/1024 = 576 / 576 / 607，iframe 内部滚动、页级无第二条滚动条。
  - `65b10b9` 上机实测（四插件 × 三档）：860/1024 全部贴合（内容高＝iframe 高，页级滚动 0）；430 封顶生效（cron 566→530、ftp 656→470），但整页仍多 1px（`scrollHeight 901 / clientHeight 900`），`innerWidth − clientWidth = 15` 说明真的出了一条页级滚动条。
  - 修复：可用高度改从 `.workbench-panel` 量起并扣掉面板边框/外边距；封顶后再用真实 `scrollHeight` 做一次溢出校正（亚像素累计会稳定多出 1px）。
  - `97edf87` 复验：430 宽 `innerWidth − clientWidth = 0`、`scrollHeight 900 / innerHeight 900`（页级滚动条消失）；iframe 中心滚轮 `ΔY=400` 后截图变化而父页面 `scrollY` 保持 0（内部滚动有效）；860/1024 维持贴合。
- [x] 主题占位：加载前占位底色与 iframe 底衬跟随主题（light `#f6f8fa` / dark `#0b0d11`），占位文案 `正在加载插件界面…` 在 iframe 之上可见；切换主题时占位与已加载页同步。
  - `456e040` 实测：占位底色正确，但无 L3 标签的单视图/老包页面 `data-loaded` 永不置位、且文案被 iframe 底衬盖住。
  - `97edf87` 复验：单视图（testkit `single`，`tabs=0`）加载后 `::before` 为 `display:none`，明暗底衬 `rgb(246,248,250)` / `rgb(11,13,17)` 一致；`readystatechange=interactive` 时主题已落地，首帧 body 底色 `rgb(244,245,247)` / `rgb(14,17,22)`，无闪白。
- [x] Docker 不可用 gate：工作台 warn、概览 banner、容器/镜像/网络/卷 gate 均带「前往 Docker 设置」出口，不出现裸连接错误或白屏；守护进程恢复后 3 秒内自动解除 gate。
  - `456e040` 实测（停 `docker.socket` + `docker`）：容器页 503「服务不可用」、镜像/网络页显示 `hyper client error`，工作台与概览仍按启动时的缓存判断为可用。
  - `97edf87` 复验：概览（`-L` 200，gate 文案 4 处）与 `/containers`、`/images`、`/networks`、`/volumes` 均 200、各 2 处 gate、`hyper client error` 0；恢复后 gate 归零、三容器自动 `Up`、skiff PID 不变。guard 工作台页按 manifest `gates=["docker_available"]` 随菜单隐藏、深链 404（设计行为）。
  - 修复＝后台 3 秒探活 + `available()` 动态化 + 入口 handler 早退 gate + 模板补出口；单测覆盖四个入口返回 200 且含出口。

