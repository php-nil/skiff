# 测试环境访问信息（唯一来源 · 索引）

> 本文是测试机访问信息、当前部署坐标与最近验收结论的**唯一来源**。`ITERATION.md`、`DEPLOY_TEST.md`、`docs/changes/` 不再重复地址、账号、入口前缀、证书与资产摘要，只留相对路径引用。
> 部署约定见 [AGENTS.md](../../AGENTS.md) §4.2：**任何一次向测试机部署后，必须更新本文，并在汇报中交代「地址 + 账号」；缺这步视为部署未完成。**
> **本文件是索引**：内容按用途拆在 [`TEST_ENV/`](TEST_ENV/)，**坐标只在 [`01-ACCESS.md`](TEST_ENV/01-ACCESS.md)**，其余册引用不复写。

## 按用途查

| 要查什么 | 去哪 | 说明 |
| --- | --- | --- |
| 机器地址、面板 URL、入口前缀、TLS 形态、管理员账号与口令文件、SSH | [01-ACCESS.md](TEST_ENV/01-ACCESS.md) | **唯一坐标来源**，含三种 TLS 形态的回切路径 |
| 当前生效部署（版本、二进制与资产摘要、现网保留现场） | [02-DEPLOY_STATE.md](TEST_ENV/02-DEPLOY_STATE.md) | 只保留当前一轮；旧轮次进部署流水 |
| 历史部署流水 | [03-DEPLOY_LOG.md](TEST_ENV/03-DEPLOY_LOG.md) | 一行一轮：日期 / 场景 / 版本坐标 / 结论 |
| 验收结论 | [04-ACCEPTANCE.md](TEST_ENV/04-ACCEPTANCE.md) | 当前轮 + 最近两轮完整结论 + 历史轮次台账 |
| 工具链验收、数据备份与清理 | [05-TOOLCHAIN-DATA.md](TEST_ENV/05-TOOLCHAIN-DATA.md) | `skiff-server` publish / sync、库与备份 |

## 维护规则

- 部署后各册怎么更新见 [test.md](../test.md)「维护规则」；本文特有的一条：**只在坐标变化时**动 `01-ACCESS.md`（重装导致入口前缀 / 口令 / 监听形态变化即算）。
