# 交付与验证

> 一条链：**装机 → 迭代 → 坐标 → 用例 → 未闭环**。上测试机按此顺序读，别跳步。
> 正文在 [`test/`](test/)。

## 测试体系架构（共性）

### 验证分层

| 层 | 在哪 | 做什么 | 每轮 |
| --- | --- | --- | --- |
| L1 门禁 | 本机（增量） | `fmt` / `clippy` / `test` / Linux target check | 约 90 s |
| L2 单测 | GitHub `test` job | Linux 全量单测，unix-only 用例真跑 | 约 2.5 min，可与 L2′ 并行 |
| L2′ 验收 | 测试机（全量回滚） | 装包 → 起服务 → `accept.sh` | 约 40 s |
| L3 发布物 | `release.yml`（里程碑轮） | musl 静态包 + 官方插件签名 | 约 4 min，不每轮跑 |

编译不进测试机——测试机只回答「干净系统上装完能不能用」；原因与轮次配比见 [ITERATION/01-ROUND.md](test/ITERATION/01-ROUND.md) §0–§3。

### 证据三档（全部文档通用口径）

| 档位 | 含义 |
| --- | --- |
| 已实测 | 有人跑过，记录了命令与输出 |
| 设计目标 | 方案里写下的数字或行为，**尚未实测**，不得当既有事实引用 |
| 未实现 | 代码里没有，或只留了接口并显式返回「不支持」 |

未实测的数字与能力声明只标「设计目标 / 待实测」（AGENTS.md §4.1）。

### 唯一来源分工

| 内容 | 唯一落点 |
| --- | --- |
| 坐标（地址 / 账号 / 入口前缀 / TLS / 口令文件） | [TEST_ENV/01-ACCESS.md](test/TEST_ENV/01-ACCESS.md) |
| 当前部署、部署流水、验收结论 | [TEST_ENV.md](test/TEST_ENV.md)（02 / 03 / 04） |
| 用例编号与预期结果 | [TEST_MATRIX.md](test/TEST_MATRIX.md) |
| 未闭环项 | [TEST_LEDGER.md](test/TEST_LEDGER.md) |
| 装机 / 运维 / 排障步骤 | [DEPLOY_TEST.md](test/DEPLOY_TEST.md) |
| 单轮迭代流程 | [ITERATION.md](test/ITERATION.md) |

互相引用而不复制内容：坐标不进用例，用例不记历史，PASS 不进台账。

### 维护规则

- 每次部署后必须更新 TEST_ENV（`02` 换本轮、`03` 加一行、`04` 补结论），并在汇报中交代「地址 + 账号」；缺这步视为部署未完成（AGENTS.md §4.2）。
- 台账只留「正在处理 / 待处理」；验证通过的条目当轮删除，不留历史流水账。
- 用例编号稳定：新增只追加编号，不复用已删除编号；预期值以契约为准。
- 实测值与当轮结论必须回填 TEST_ENV，**未回填即视为未验证**。
- 逐条命令输出、根因与过程流水记 `docs/changes/YYYY-MM-DD.md`，本主题只留坐标、结论与限制。

## 子文档

| 子文档 | 管什么 |
| --- | --- |
| [DEPLOY_TEST.md](test/DEPLOY_TEST.md) | 装机步骤、配置矩阵、真机验收用例（正文在 `DEPLOY_TEST/` 7 册） |
| [ITERATION.md](test/ITERATION.md) | 单轮「改代码 → 上机验证」与回滚协同（正文在 `ITERATION/`） |
| [TEST_ENV.md](test/TEST_ENV.md) | 测试机地址 / 账号 / 入口前缀 / 当前部署坐标（**唯一坐标来源**；正文在 `TEST_ENV/` 5 册） |
| [TEST_MATRIX.md](test/TEST_MATRIX.md) | 稳定编号的验收用例（按功能域分 8 册，编号跨册不变） |
| [TEST_LEDGER.md](test/TEST_LEDGER.md) | 未闭环项与下一步 |
