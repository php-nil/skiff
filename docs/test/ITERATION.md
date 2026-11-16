# 迭代验证回路

> 状态：**已上机**。本文是「改代码 → 上机验证」的唯一依据。交付门槛见 [AGENTS.md](../../AGENTS.md) §4.2 / [CI.md](../ops/CI.md) §2，工作流见 [CI.md](../ops/CI.md)。
> 坐标（地址 / 账号 / 口令文件 / 入口前缀）只在 [TEST_ENV/01-ACCESS.md](TEST_ENV/01-ACCESS.md)，本文不落明文。
> 正文在 [`ITERATION/`](ITERATION/)。

## 一轮怎么跑

L1 门禁（验证分层见 [test.md](../test.md)）→ 提交并触发迭代包 → 装到测试机 → 按 [01-ROUND.md](ITERATION/01-ROUND.md) §5 验收口径判 PASS / FAIL → 未闭环记账到 [TEST_LEDGER.md](TEST_LEDGER.md)，实测值与当轮结论回填 [TEST_ENV.md](TEST_ENV.md)（**未回填即视为未验证**）。

## 文档地图

| 章节 | 文件 | 内容要点 |
| --- | --- | --- |
| §0–5 | [01-ROUND.md](ITERATION/01-ROUND.md) | 分层依据、测试机、基线、轮次、单轮步骤、验收口径（分组与判定） |
| §6–8 | [02-ISSUES_ENV.md](ITERATION/02-ISSUES_ENV.md) | 已知限制、测试机与凭据口径、内网代理、工具落点 |
