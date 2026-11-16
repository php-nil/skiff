# 部署与测试（运维视角 · 索引）

> 主文档：[DESIGN.md](../DESIGN.md) ｜ 本文件：部署与验收测试导航 ｜ 设计依据：§5 部署 / §6 安全 / §9 里程碑 / §12 待实测
> 正文在 [`DEPLOY_TEST/`](DEPLOY_TEST/)，按 §编号分册，**编号沿用拆分前原文**。

本文只讲**怎么装、怎么验**，不重复设计论证；结论按**证据三档**（已实测 / 设计目标 / 未实现）标注，口径见 [test.md](../test.md)。

§12 的每一项在本文要么有一条对应验收用例，要么落在 §8 的未实现清单里——不存在“既没测也没记”的项。

## 读法

上测试机按 §0 → §2 顺序走；改代码后的单轮验证走 [`ITERATION.md`](ITERATION.md)；坐标（地址 / 账号 / 入口前缀）只在 [`TEST_ENV.md`](TEST_ENV.md)，用例预期只在 [`TEST_MATRIX.md`](TEST_MATRIX.md)。

## 文档地图

| 章节 | 文件 | 内容要点 |
| --- | --- | --- |
| §0–1 | [01-PREGATE.md](DEPLOY_TEST/01-PREGATE.md) | 上机前的三道闸、环境与前置条件（目标机 / 构建机 / 开发机） |
| §2.1–2.4 | [02-INSTALL.md](DEPLOY_TEST/02-INSTALL.md) | 取包与校验、解包安装、首启与首次登录、TLS 三态与监听形态（含反代超时） |
| §2.5–2.10 | [03-OPERATE.md](DEPLOY_TEST/03-OPERATE.md) | 升级、回滚、卸载、常用运维命令、插件测试物准备、权限链自检 |
| §2.11–2.14 | [04-PLUGIN_SITE.md](DEPLOY_TEST/04-PLUGIN_SITE.md) | 官方插件导入与安装、插件页多视图、vhost 语义、站点 HTTPS、镜像源落盘 |
| §3–5 | [05-FORMS_DEV.md](DEPLOY_TEST/05-FORMS_DEV.md) | 形态 B（1Panel 商店包）、本地开发与冒烟、配置速查 |
| §6 | [06-CASES.md](DEPLOY_TEST/06-CASES.md) | 验收测试用例（A–H 组） |
| §7–8 | [07-TROUBLESHOOT.md](DEPLOY_TEST/07-TROUBLESHOOT.md) | 故障排查、未实现 / 未验证清单（不得当事实引用） |


