# 内核 UI 交互基准（UI_CONTRACT）

> 适用范围：`crates/web` 渲染的内核页面（模板 + `static/skiff.css` / `static/skiff.js`）。
> 插件 iframe 内部不受本规范强制约束，插件视图边界见 [PLUGIN_UI_CONTRACT.md](PLUGIN_UI_CONTRACT.md)。
> 版本：v1（2026-11-08），全站内核页面已按基准对齐（逐页对齐记录见 `docs/changes/`）；新增或改造页面一律按本基准执行。

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

## 15. Web 服务安装进度（2026-11-14）

- **唯一口径**：进度来自任务中心 `web.install` 作业的持久化快照；`/ui/v1/web`、`/ui/v1/web/install`、旧 SSR 与任务详情共用同一构建函数，页面不再出现内存进度。
- **展示**：状态（排队/进行中/已完成/失败/中断）、当前步骤与百分比、发起人与角色、来源 IP、开始/结束时间、耗时、每步状态与错误摘要；有历史作业时给「查看任务详情」入口。
- **锚点**：进度块的稳定 `className` 必须写在 schema 节点顶层（SDK decorate 属性），不能放进 `props`；后者只进组件 props，不会落到 DOM，轮询将找不到目标。
- **点击安装**：提交成功后立即刷新进入当前作业视图，不在旧页面上弹阻塞提示，避免上一次「已完成」结果继续可见。
- **轮询**：仅当 `status` 为 `queued/running` 时启动，2 秒一次、递归 `setTimeout`（上一请求结束再排下一次）；只更新进度块文本，不整页刷新。
- **停止与暂停**：进入 `succeeded/failed/cancelled` 即停止并保留失败记录，整页刷新一次以同步运维按钮；离开 `/sites/web` 立即清理定时器；`document.hidden` 时暂停，恢复可见后先补一次。
- **网络失败**：保留上次进度，退避到 5 秒重试，不把失败误报为完成。
- **重启**：活跃作业在启动收尾中置为 `failed/interrupted`，`running` 步骤改 `failed` 并补结束时间；页面显示「安装已中断」与原因，历史步骤仍可查。

