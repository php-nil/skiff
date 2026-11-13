# budding 上游缺口清单（1.8.0）

> **核对日期/版本**：2026-11-13，budding **1.8.0**（catalog 1.8.0 / 57 条目）。
> **状态口径**：`已完成` = 契约 / 实现 / 字段元数据 / 手册 / playground / 测试已同步并通过门禁；`待确认` = 无阻塞、等排期或产品确认；`暂缓` = 后续项，当前不实现；`范围外` = 不由本项目承担。
> 本文只保留下游需要的「能用什么、怎么用、还差什么」，历史方案与复核过程已删除。

## 状态总览

| 编号 | 任务 | 状态 | 下游可用方式 / 契约指向 |
| --- | --- | --- | --- |
| T6 | `input-text` 透传 `autocomplete` | **已完成** | 字段 `autocomplete` 原样落原生 `input`；契约 §8.2 |
| T7 | 插件 Schema 运行时安全策略 | **范围外** | 属下游 / 宿主职责，不在本项目范围 |
| T8 | 图标字体版本化与缓存 | **已完成** | `budding-icons-<version>.css/.woff2` + 稳定别名 + `budding-icons.json`；`docs/components.md`「图标字体」 |
| T9 | `virtual-list` 滚动触点与按 key 定位 | **已完成**（`stickToBottom` 暂缓） | 触点 `scroll` / `reachBottom`、指令 `scrollToKey`；契约 §10.9 |
| T10 | `table` 游标分页与追加加载 | **已完成** | 字段 `paging` / `cursorParam` / `cursorPath` / `loadMoreText`、指令 `loadMore`；契约 §10.1 |
| T11 | `table` 行级下载与选择 / 打开语义 | **已完成**（下载） | 动作 `download`、`PlatformAdapter.download?`；契约 §19.7 / §19.10 / §15.1 |
| T12 | 上传分片 / 续传 / 暂停 / 多文件进度 | **暂缓** | 后续项；当前仅单请求上传，见「还差什么」 |
| T13 | `code` 分段加载 / 跳行 / 语法清单 | **部分完成**（跳行 + 清单） | `scrollToLine` / `getLanguages` / `getThemes` 已随 `@budding/code` 落地；分段加载仍待后端协议 |
| T14 | 终端 WS 通道契约 | **已完成** | 票据、关闭码、心跳、审计边界；契约 §22.3–§22.5 |
| T15 | `select` 多选与 `checkbox` 组 | **已完成** | `select.multiple` / `chips` / `maxSelections`、`checkbox.items`；契约 §9.1 |
| T16 | 业务菜单图标纳入随包子集 | **已完成** | 17 个语义名进子集，可用字形以 `budding-icons.json` 为准 |

## 已完成项：怎么用

- **T6 `autocomplete`**：`props.autocomplete` 透传到原生 `input`；未配置时不输出属性；与 `props.type` 并存。实现 `packages/ui-vuetify/src/form/InputText.vue` + `src/fields/form.ts`。
- **T8 / T16 图标**：版本化文件长期缓存、稳定别名短缓存；`manifest.icons` 含 `version` / `files[{ file, bytes, sha256, integrity }]`；字形清单与 CSS `::before` 规则由构建门禁校验。业务菜单直接写 mdi 名，可用字形见 `budding-icons.json`。
- **T9 `virtual-list`**：`scroll` 载荷 `{ top, atTop, atBottom, direction }`（≥100ms 节流 + 尾随补发）；`reachBottom` 每次「非底部 → 底部」触发一次，带 `reason: 'user' | 'programmatic'`；`scrollToKey({ key, position? })` 按 `itemKey` 定位，未命中返回 `false`。自动跟随（`stickToBottom`）未做，宿主用这两个触点组合。
- **T10 游标分页**：
  ```json
  {
    "type": "table",
    "source": "get:/api/files",
    "props": {
      "paging": "cursor",
      "cursorParam": "cursor",
      "cursorPath": "next_cursor",
      "loadMoreText": "加载更多",
      "rowKey": "id"
    }
  }
  ```
  首屏不带游标；`loadMore()` 带上一页游标请求并追加，按 `rowKey` 去重；排序 / 筛选 / 关键字变化或 `reload` 重置回首屏；`next_cursor` 为空后按钮变为「没有更多了」，页码与总数始终隐藏。需自动触底时可组合 T9 的 `reachBottom` + `invoke` 调 `loadMore`。
- **T11 `download`**：
  ```json
  {
    "type": "download",
    "props": {
      "url": "/api/files/${record.id}/content",
      "filename": "${record.name}",
      "token": "${token}"
    }
  }
  ```
  参数：`url`（必填）、`filename`、`method`（GET 默认 / POST）、`headers`、`token`、`data`。宿主实现 `platform.download(req)` 后可支持 POST、鉴权头与数据体；缺省行为见「还差什么」。
- **T14 终端通道**：票据走一次性短 TTL query 或子协议；关闭码 `1000` / `4401` / `4403` 不重连（4401 / 4403 额外上报 `error`），其余按 `reconnectLimit` 退避；`heartbeat` 定时清理；`input` 触点只给 `{ at, count }`，不承诺内容审计。
- **T15 多值表单**：`select.multiple` + `chips` + `maxSelections`；`checkbox.items` 编组（`itemTitle` / `itemValue` / `itemsPath`）；`required` 表示至少一项。

## 还差什么

- **T12 上传（暂缓）**：现仅单请求上传，已有 `uploadApi` / `uploadField` / `uploadData` / `uploadTarget` / `uploadProgressTarget` / `maxRetries` / `retryDelayMs` 与 `cancelUpload` / `retryUpload`。缺分片、断点续传、按文件进度与暂停 / 恢复；`multiple` 当前合并为一次请求、一个总进度。
- **T13 `code`（分段加载待确认）**：`scrollToLine`、语言 / 主题清单（指令与手册 catalog）已随 `@budding/code` 落地；仍缺分段 `source` 与 `nginx` / `ini` / `dockerfile` 等语法。分段加载需后端协议先行。
- **T10 边界**：不做双向游标、不做离线缓存；游标模式只覆盖「已加载页」。
- **T11 边界**：本轮只交付 `download`；「全选所有页」（`selectStrategy: 'all'`）与 `openOn` 不在范围内。现有替代：`selected` 存 rowKey 且跨页保留；`rowClick` 可绑 `navigate`；列 `cell` 插槽可渲染图标。
- **`download` 无宿主适配器时的实际行为**：`GET` 且无 `headers` / `token` / `data` → 创建临时 `<a href download>` 并点击，由浏览器接管下载；`POST` / 自定义头 / 数据体 / 无 `document` 环境 → 直接报动作错误（走统一错误链 + toast），提示宿主提供 `platform.download`。拒绝 `javascript:` / `vbscript:` 地址。
- **T7 范围外**：URL 约束由宿主经 `platform.request` / `loadJson` / `upload` 统一拦截；动作白名单、插件签名与信任链属宿主 / 下游策略。本项目不提供 `schemaPolicy`。

## 验证口径

已完成项的证据落点：

| 条目 | 实现 / 字段 | 测试 |
| --- | --- | --- |
| T6 | `form/InputText.vue`、`fields/form.ts` | `form/form-behavior.test.ts` |
| T8 / T16 | `scripts/lib/embed-icons.ts`、`scripts/build-dist.mjs` | `scripts/lib/embed-icons.test.ts` |
| T9 | `data/VirtualList.vue`、`data/index.ts` | `data/virtual-list.test.ts` |
| T10 | `data/use-table-data.ts`、`data/TableBlock.vue`、`fields/data.ts` | `data/table-behavior.test.ts`（游标模式） |
| T11 | `capabilities/actions/builtin.ts`、`renderer-sdk/tokens.ts`、`runtime/services/platform-service.ts` | `capabilities/actions/download.test.ts`、`action-guides.examples.test.ts` |
| T14 | `packages/terminal/src/options.ts`、`Terminal.vue` | `options.test.ts`、`Terminal.test.ts` |
| T15 | `form/Select.vue`、`form/Checkbox.vue`、`fields/form.ts` | `form/form-behavior.test.ts`（多值字段） |

交付门槛：`pnpm lint`、`pnpm test`、`pnpm typecheck`、`pnpm build` 全部通过。
