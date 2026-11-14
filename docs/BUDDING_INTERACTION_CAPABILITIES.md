# budding 交互能力速查

> 来源：2026-11-14 通过 budding 官方 MCP（`@budding/mcp-server` 0.2.0）实拉，`catalogVersion=1.8.0`、`contentHash=sha256:00fcc757…c700b5b`，共 57 个组件、20 个动作、300 条示例、44 篇指南。仓内产物版本以 `crates/web/src/assets.rs::BUDDING_VERSION` 为准。
> 分工：接入边界、版本事实与 JSON 契约只在 [SDUI_BUDDING_INTEGRATION.md](SDUI_BUDDING_INTEGRATION.md) 维护；上游缺口与宿主待补契约见 [BUDDING_UPSTREAM_BACKLOG.md](BUDDING_UPSTREAM_BACKLOG.md)；本文只做「怎么用」的摘录，不复述契约全文。
> 校验：文中 dialog+form、confirm+ajax+invoke、table 游标分页、virtual-list 四段完整 Schema 已用 MCP `validateSchema` 校验通过（`ok: true`，非 strict）。

## 0. MCP 接入方式

MCP 是上游仓库的开发期只读查询层，**不是 Skiff 的运行时依赖**；本项目不引入 Node、CI 不调用它。

| 项 | 做法 |
| --- | --- |
| 入口 | 上游仓库根目录 `pnpm mcp`（stdio）或 `pnpm mcp:http`（Streamable HTTP） |
| HTTP 地址 | 默认 `http://127.0.0.1:3921/mcp`；健康检查 `GET /healthz` |
| 鉴权 | 未设 `BUDDING_MCP_TOKEN` 时强制只听 `127.0.0.1`；设了 token 才允许 `BUDDING_MCP_HOST` 对外，请求带 `Authorization: Bearer <token>` |
| 环境变量 | `BUDDING_MCP_PORT` / `BUDDING_MCP_HOST` / `BUDDING_MCP_TOKEN` / `BUDDING_MCP_CATALOG` / `BUDDING_MCP_RATE_LIMIT` / `BUDDING_MCP_LOG` |
| 产物前提 | `pnpm gen:catalog` 生成 `catalog.json` / `examples.json` / `guide.json`；缺失时工具返回 `ARTIFACT_*` 错误 |
| 工具 | `listTypes`、`search`、`getComponent`、`getExample`、`searchExamples`、`getAction`、`getGuide`、`validateSchema` |
| 协议细节 | 无状态 `POST /mcp`，不提供 GET SSE；单次 POST 独立创建 Server/Transport |

推荐查询顺序：`search` 缩小范围 → `getComponent` 取字段 → `getExample` 取可运行 Schema → `getAction` 取动作参数 → `getGuide` 补概念 → `validateSchema` 交付前终检。

## 1. 弹窗与抽屉

**dialog**（居中弹窗）与 **drawer**（贴边抽屉）都是动作，不是组件。正文、页脚都是 Schema，共享图层作用域。

| 要点 | 说明 |
| --- | --- |
| 作用域 | `props.data` 派生图层数据；`body` / `footer` 内可直接读 `${record}` 等 |
| 尺寸 | `preset`（`xs`/`sm`/`md`/`lg`/`xl`/`full`）；`width` / `height` 按轴覆盖 |
| 关闭 | `close` 关最近一层；`close` 可传 `layerId` 定向关闭 |
| 交互 | 遮罩、焦点陷阱、背景隔离、滚动锁定、Esc 只关顶层由内核处理；`closeOnOutside` / `closeOnMask` / `closeOnEsc` / `showCloseButton` / `overlay` 可分别控制 |
| 抽屉 | `placement` 缺省 `right`，左右用 `width`、上下用 `height` |
| 表单 | 表单放在 `body`，提交按钮必须写在表单自己的 `content.actions`；`footer` 是表单兄弟节点，只放关闭或不依赖表单上下文的动作 |
| 非阻塞 | `dialog` / `drawer` 打开后动作链立即继续；需要「等用户选完再执行」用 `confirm`，或把后续动作放进弹窗按钮 |

新增 / 编辑的最小组合：

```json
{
  "type": "button",
  "props": { "text": "编辑", "variant": "text" },
  "action": {
    "type": "dialog",
    "props": {
      "title": "编辑站点",
      "width": 640,
      "data": { "record": "${record}" },
      "body": [{
        "type": "form",
        "props": {
          "initialValues": "${record}",
          "api": { "method": "POST", "url": "/ui/v1/sites/${record.domain}/config/save" },
          "successMessage": "已保存",
          "submitErrorFocus": "focus",
          "onSuccess": [{ "type": "close" }]
        },
        "body": [
          { "type": "input-text", "props": { "name": "domain", "label": "域名", "required": true } }
        ],
        "content": {
          "actions": [
            { "type": "button", "props": { "text": "保存", "color": "primary" }, "action": { "type": "submit" } }
          ]
        }
      }],
      "footer": [
        { "type": "button", "props": { "text": "取消", "variant": "outlined" }, "action": { "type": "close" } }
      ]
    }
  }
}
```

> 「查看」也可用 `dialog` + `card` / `text` 拼装；budding 目前没有 `descriptions` 只读详情组件，见 backlog 的 U2。

## 2. 二次确认

**confirm** 是动作，只有两个参数：

| 参数 | 类型 | 说明 |
| --- | --- | --- |
| `title` | string | 标题 |
| `content` | string | 正文，支持 `${}` |

关键语义：

- 用户取消时抛 `ActionCancelled`：**中止整条动作链**，不跑 `onError`，静默。
- `confirm` 是异步动作，后续步骤一定等用户选择后才执行。
- 复杂内容用 `dialog`，不要堆在确认框正文里。
- 宿主未注入 `platform.confirm` 时退化为浏览器原生 `window.confirm`；当前 Skiff 的 `platform` 只注入了 `loadJson` / `request` / `upload`，所以直接改用 `confirm` 动作仍会看到原生确认框。

删除的标准组合（确认 → 请求 → 刷新表格）：

```json
{
  "type": "button",
  "props": { "text": "删除", "color": "error", "variant": "text" },
  "onEvent": {
    "click": {
      "actions": [
        {
          "type": "confirm",
          "props": { "title": "确认删除", "content": "删除 ${record.name} 后不可恢复。" }
        },
        {
          "type": "ajax",
          "props": {
            "api": { "method": "DELETE", "url": "/ui/v1/items/${record.id}" },
            "messages": { "success": "已删除" }
          }
        },
        {
          "type": "invoke",
          "props": { "component": "item-table", "command": "reload" }
        }
      ]
    }
  }
}
```

> 表格不会因 `ajax` 自动刷新；需要 `invoke` 组件的 `reload`，组件必须在 Schema 上声明 `id`。
> 需要「输入指定文本才能删除」时，当前只能用 `dialog` + `form` + 服务端校验自拼，见 backlog 的 U1。

## 3. 表单与校验

**form** 统一管理字段值、校验、提交与重置；字段通过 `name` 自动写入表单数据域。

| 能力 | 用法 |
| --- | --- |
| 字段注册 | 子控件 `props.name`，支持点分嵌套路径 |
| 同步校验 | `required`、`rules.required` / `minLength` / `maxLength` / `pattern` / `min` / `max` / `validator`；`messages` 可逐条定制 |
| 异步校验 | 字段 `validateApi`（请求体 `{ name, value, values }`，返回 `{ valid, message }` 或字符串）；表单级 `validateApi`（请求体 `{ values }`） |
| 时机 | `validateOn`: `blur`（默认）/ `change` / `submit` / `manual`；`validateDebounce`、`validateTimeout`、`validateApiFailMode` |
| 联动 | `disabledOn`、`visibleOn` / `hiddenOn`；`autoFill` 在字段变化后写其它字段 |
| 提交 | `api`（缺省只做本地校验）、`submitData`、`successMessage` / `errorMessage`、`resetOnSuccess`、`onSuccess` / `onError` |
| 错误定位 | `submitErrorFocus`: `none` / `focus` / `scroll` |
| 布局 | `layout`: `vertical` / `inline` / `grid`；`columns` 配 `col` 的 `span`；操作区可置顶、吸底、主次分离、窄屏纵排 |

要点：

- 服务端字段错误**不会自动回填到控件**；目前只有表单级错误提示与 `submitErrorFocus`，字段级回填需宿主自定义动作写 `ctx.extras.form.errors`，见 backlog 的 U3。
- `submit` / `reset` 只在 form 内有效；字段值实时同步，动作里可用 `${字段名}` 读取。
- 校验错误只取第一条；需要多条规则时用 `rules.pattern` 数组，每条可带独立 `message`。
- `input-text` 的 `type`（含 `password`）与 `autocomplete` 都已原生透传，登录口令框不再需要 DOM 桥接。

## 4. 列表与分页

| 组件 | 适用 |
| --- | --- |
| `table` | 标准数据表格：本地/远程、页码或游标分页、服务端排序、行操作、展开行、自定义单元格、行内编辑、合计行、列配置、导出、树形、虚拟滚动 |
| `crud` | 查询容器：`content.filter` + `content.toolbar` + `content.bulkActions` + 内部表格；新增/编辑/删除有可抄组合 |
| `pagination` | 独立分页器；页码或每页条数变化触发 `query` 与 `change` |
| `record-list` | 按条目逐条渲染 `content.body`，行外观完全自定义 |
| `list` | 直接映射 Vuetify `VList` / `VListItem`，适合导航型列表 |
| `virtual-list` | 只挂载可视窗口的长列表，见 §8 |

表格远程分页（游标）最小写法：

```json
{
  "type": "table",
  "id": "site-table",
  "source": { "method": "GET", "url": "/ui/v1/sites" },
  "props": {
    "columns": [
      { "key": "domain", "label": "域名" },
      { "key": "state.label", "label": "状态" }
    ],
    "paging": "cursor",
    "itemsPath": "items",
    "cursorPath": "next_cursor",
    "rowKey": "domain",
    "rowTo": "/sites/${record.domain}"
  }
}
```

要点：

- 页码模式用 `itemsPath` / `totalPath` 取行集合与总数；游标模式用 `cursorParam` / `cursorPath`，首屏不传游标，空游标表示没有更多；`paging: 'cursor'` 时底部分页始终隐藏。
- 示例路径按相对资源写；实际 `rowTo` 需按入口前缀 `/{p}` 补全。
- `paramNames` 可改 `page` / `perPage` / `orderBy` / `orderDir` / `keywords` 的请求参数名。
- 行作用域：`rowActions` / `expand` / `columns[].cell` 为 `{ ...record, record, item, index }`；行点击走 `rowClick` 或 `rowTo`。
- 列级筛选用 `props.filters`；远程模式并入查询参数，空值跳过。
- 行操作、展开行、单元格区域路径分别为 `row-actions-<i>`、`expand-<i>`、`cell-<i>-<key>`。
- 表格指令：`reload`、`setData`、游标模式 `loadMore`。`rowActions` 里的删除不会自动刷新，需 `invoke reload`。

## 5. 反馈提示

| 动作 / 组件 | 用法 |
| --- | --- |
| `toast` | `{ message, level }`，`level` 为 `info` / `success` / `warning` / `error`；不打断操作 |
| `alert` | 信息提示条，四种语义、紧凑模式、可关闭、自定义图标 |
| `progress` | 线性 / 环形、确定 / 不确定、百分比、条纹、缓冲 |
| `skeleton` | 加载占位；容器自带加载态已改用骨架 |
| `empty-state` | 图标/图片 + 标题 + 说明 + 一个动作按钮 |
| 表单反馈 | `successMessage` / `errorMessage`；失败类动作默认也会弹「操作失败，请稍后重试」 |

约定：

- 需要用户决策用 `confirm`；需要展示 Schema 内容用 `dialog`；纯结果反馈用 `toast`。
- `toast` 只有消息与级别，没有位置、时长、手动关闭参数；需要常驻说明用 `alert`。

## 6. 终端

`terminal` 来自独立 Pack `@budding/terminal`，基于 xterm；页面须先注册 Pack，未安装时渲染明确降级提示。

| 参数 | 说明 |
| --- | --- |
| `wsUrl` | WebSocket 地址模板；只接受 `ws://` / `wss://`；票据由页面 `source` 写入数据域后引用 |
| `height` / `fontSize` | 高度、字号（8–32） |
| `readOnly` | 只读回放，禁止本地输入 |
| `reconnect` / `reconnectDelay` / `reconnectLimit` | 非主动断开时自动重连；关闭码 `1000` / `4401` / `4403` 不重连 |
| `heartbeat` | `{ intervalMs, payload?, idleTimeoutMs? }` |
| `resizeFormat` | `json` 发送 `{ type: "resize", cols, rows }`；`none` 不发送 |
| `theme` | `auto` / `dark` / `light` 或宿主注册主题 |
| `keyboardEscape` | Esc 失焦回页面；业务端仍应提供显式退出按钮 |

触点：`connected` / `disconnected` / `reconnecting` / `exit` / `error` / `input`（只含计数与时间，不含输入内容）。
指令：`reconnect` / `disconnect` / `focus` / `blur`。

Skiff 现状：容器终端复用既有 exec 票据与 WS，`resizeFormat: 'none'`、`reconnect: false`，手动「重新连接（重新取票）」。

## 7. 代码查看

`code` 来自独立 Pack `@budding/code`；未加载 Pack 时按「组件未发布」处理，未装高亮依赖时按纯文本工作。

| 参数 | 说明 |
| --- | --- |
| `modelValue` | 代码内容 |
| `language` | 语言；未注册回退纯文本 |
| `languages` / `languageSwitcher` | 语言切换候选与开关 |
| `readonly` | `true` 只读查看，`false` 编辑 |
| `lineNumbers` / `wrap` / `copyable` | 行号、折行、复制 |
| `folding` / `foldStrategy` / `foldRanges` / `foldedLines` | 折叠；服务端可下发精确区间，本地只做 brace / indent 启发式 |
| `tokens` | 服务端 token，优先于本地分词 |
| `saveApi` / `saveText` / `saveSilent` | 编辑保存；请求体固定 `{ value, language }` |
| `height` / `maxHeight` | 内容区高度 |

指令：`setValue` / `getValue` / `setLanguage` / `scrollToLine` / `foldAll` / `unfoldAll` / `toggleFold` / `save` / `isDirty` / `copy`。
触点：`change` / `languageChange` / `foldChange` / `save` / `saved` / `saveError` / `copy`。

Skiff 现状：文件查看与站点 / 作业 / 容器 / 运行环境日志均用只读 `code`；文件查看受 256 KiB 上限，日志为尾部窗口快照。

## 8. 虚拟列表

`virtual-list` 按行高虚拟化，只挂载可视窗口内的内容；数据入口与条目插槽语义对齐 `record-list`。

```json
{
  "type": "virtual-list",
  "id": "file-list",
  "source": { "method": "GET", "url": "/ui/v1/files?path=${query.path}" },
  "props": {
    "itemsPath": "entries",
    "itemHeight": 52,
    "height": 560,
    "itemKey": "path",
    "emptyText": "目录为空"
  },
  "content": {
    "body": [{ "type": "text", "props": { "text": "${item.name}" } }]
  }
}
```

要点：

- 条目作用域 `{ ...item, item, index }`；`itemTo` 提供点击导航与 `role="link"` / Enter / Space。
- 指令：`reload` / `setData` / `scrollTo` / `scrollToTop` / `scrollToIndex` / `scrollToKey` / `getScrollTop`。
- 触点：`scroll`（≥100ms 节流）、`reachBottom`（从非底部进入底部触发一次）。
- 不做树形 / 子项展开、拖拽排序、组件内动态行高测量；`itemHeight` 固定时窗口计算最稳定。
- 远程数据用顶层 `source` + `itemsPath`；`reload` / `setData` 与 `record-list` 一致。
- 暂无 `stickToBottom` 自动跟随，需要时用触点组合。

## 9. 容易踩的约束

1. `dialog` / `drawer` 不阻塞动作链；只有 `confirm` 会等待并在取消时中止后续。
2. 弹窗里的表单提交按钮必须在表单 `content.actions`，放在 `footer` 拿不到表单上下文。
3. 表格 / 虚拟列表的删除、编辑成功后不会自动刷新，需要 `invoke` 对应组件的 `reload`。
4. `confirm` 只有 `title` / `content`；按钮文案、危险色、输入型确认都不在契约内（backlog U1）。
5. 表单字段错误不能由服务端响应自动落到控件；只有表单级错误与焦点定位（backlog U3）。
6. 没有 `descriptions` 只读详情组件；详情页仍需 `card` + `flex` + `text` 组合（backlog U2）。
7. 页面 Schema 交付前跑一次 MCP `validateSchema`；`strict: true` 可把未声明字段升级为错误。

## 10. 本项目的采用约定

> 目的：同一个交互只保留一种写法；与 backlog 的 U1–U3、K2 配套。

1. **确认框**：Schema 一律用 `confirm` 动作；宿主先接管 `platform.confirm`（K2）。需要输入指定文本才能继续的，用 `dialog` + `form`；不新增 `window.confirm` / `window.prompt`。
2. **反馈**：操作结果统一用 `toast`（成功 / 失败 / 警告），常驻说明用 `alert`，表单错误用 `errorMessage`；不新增 `window.alert`。
3. **弹窗内表单**：字段校验交给 `form` / 字段组件的 `required` / `rules` / `validateApi`，不在按钮动作里手写；提交按钮写在表单 `content.actions`，成功后 `close`。
4. **写操作后刷新**：表格 / `crud` / 虚拟列表的删除或编辑成功后，显式 `invoke` 对应组件的 `reload`；组件必须声明稳定 `id`，不依赖自动刷新。
5. **查看详情**：优先 `dialog` + 现有布局；`descriptions`（U2）到位后统一替换手工 `card + text`。
6. **Vuetify 内部文案**：当前 `embed` 的 `locale` 不能改 Vuetify 自身的 locale（K3），不要据此承诺全中文；需要时按 K3 的上游方案推进。
7. **交付前**：新 Schema 跑一次 MCP `validateSchema`；`strict: true` 作为终检。

## 11. 与现有文档的关系

- 接入边界、产物版本、`/ui/v1` 信封、CSP、壳页与菜单：见 [SDUI_BUDDING_INTEGRATION.md](SDUI_BUDDING_INTEGRATION.md)。
- 仍缺的上游能力、宿主待补契约、当前自实现清单：见 [BUDDING_UPSTREAM_BACKLOG.md](BUDDING_UPSTREAM_BACKLOG.md)。
- 框架无关的分层与迁移阶段：见 [SDUI_HANDBOOK.md](SDUI_HANDBOOK.md)。
