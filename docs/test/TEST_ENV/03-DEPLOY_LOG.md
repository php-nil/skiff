# 部署流水台账（§5 历史 + §7）

> 上级索引：[TEST_ENV.md](../TEST_ENV.md) ｜ 拆自 `docs/test/TEST_ENV.md` 原 §5 历史段落 + §7 明细表（精简为一行一轮）（2026-11-16 整理）。

---

## 部署流水（按日坐标台账）

> 一轮部署一行：**日期 / 场景 / 版本坐标 / 结论**。完整断言、逐条复验与根因不在这里——当轮细节见 `docs/changes/YYYY-MM-DD.md` 与 git log，验收断言见 [04-ACCEPTANCE.md](04-ACCEPTANCE.md)。
> 2026-11-16 整理：原 §5 的历史安装段落（每轮一整段安装流水）与 §7 的明细表已合并成本表，只留坐标与结论；同一轮的证书指纹、逐条命令输出以 `changes/` 为准。

**当前生效部署见 [02-DEPLOY_STATE.md](02-DEPLOY_STATE.md)**；表内结论为「当轮结论」，不代表当前机器状态。

| 日期 | 场景 | 版本 / 坐标 | 结论 |
| --- | --- | --- | --- |
| 2026-11-20 14:17 | 覆盖升级 schema v21→v22 + budding-boot.js CSS 修正 | Run `44687309424`（head `524f880`），amd64 12.9 MB；前序 Run `44685263606`（head `fae19cd`，50 files） | schema 自动迁移 v22 ✅；curl slug→id 303 ✅；**浏览器 6 项站点详情全量复验 PASS** |
| 2026-11-20 10:58 | 空库首装（write_atomic 权限修复 + CLI 前缀分叉收口） | Run `44574500026`（head `a02f1d0`），amd64 12,821,377 B `sha256=2fd680b5…b5c` | accept.sh **48 PASS / 0 FAIL / 3 SKIP**；浏览器 9 页面 0 console error；A6e 根治 |
| 2026-11-05 | 重置库后全新安装（schema v7） | x86_64 6,987,420 B，`sha256:25559d7ec362625cee4fb608710aa84… | 安装成功 |
| 2026-11-07 早轮 | Docker / FTP 修复轮 | 资产 `f9a0b30`，x86_64 `sha256:87427aa6…` | DOCKER-01～08 复验通过 |
| 2026-11-07 | v0.1.0 最终版覆盖安装 | tag/构建 `c44eeb5` | FTP 主链路、DEPLOY-02/07/08 通过 |
| 2026-11-07 22:05 | 干净系统全新装机（Release 资产） | tag/构建 `c44eeb5` | 全新库 `user_version=11` |
| 2026-11-08 00:18 | v11→v12 升级 + 最终版复验（覆盖安装） | 旧 `c44eeb5` → 新 `a22296f` | 库自动升 12、原任务保留 |
| 2026-11-08 08:30 | 关闭 TLS 切明文 HTTP + 浏览器视觉复测 | 无重装：`listen=127.0.0.1:18080` + `allow_insecure_http=true`… | `/healthz` 200、`/<prefix>/login` 200 |
| 2026-11-08 上午 | v12→v13 升级轮（覆盖安装） | 资产 `8ede9d2` | `user_version=13` |
| 2026-11-08 12:38 | `1ead5fe` 重发版覆盖安装 + 复验 | 资产 `1ead5fe` | 库保持 13、任务/站点/运行环境全部保留 |
| 2026-11-08（三形态轮） | `9aa0381` 覆盖安装 + TLS 三形态 / 视觉 / 工作台复验 | 资产 `9aa0381` | 库保持 13 |
| 2026-11-08 15:20 | `48f57dc` 收口发版覆盖安装 + v13→v14 升级与五组复验 | 资产 `48f57dc` | `user_version=14`，`trash_entries`/`fs_quotas` 建出、旧数据保留 |
| 2026-11-08 16:55 | `8716e51` 小批收口覆盖安装 + 配置保护 / 插件日志 / 上一批回归 | 资产 `8716e51` | 库保持 v14、数据全部保留、配置 sha 未变 |
| 2026-11-08 19:59 | 机器还原后**全新安装**（空库直建 v15） | 资产 `1862702` | `sh install.sh --no-docker` 完成 |
| 2026-11-08 20:47 | `9daabd3` 复核轮覆盖安装（两处缺陷修复） | 资产 `9daabd3` | 库保持 v15、插件与运行环境保留 |
| 2026-11-08 20:47–21:55 | 工作树构建二进制覆盖（写放大三处修复 A/B 复测，**非 Release 资产**） | 同工作树 musl 构建替换 `/usr/local/bin/skiff`（备份 `/root/skiff.befo… | 三处 A/B 复测完成，数字见 `docs/test/TEST_LEDGER.md` 与 `docs/changes/2… |
| 2026-11-08 21:20 | 本地工作树构建二进制覆盖（超时可配验证用，**非 Release 资产**） | `cargo build --release --target x86_64-unknown-linux-musl… | 仅用于验证任务中心超时路径 |
| 2026-11-08 22:48 | `456e040` 三线收口发版覆盖安装（Release 资产） | 资产 `456e040` | 解压 `sh install.sh --no-docker` 成功 |
| 2026-11-08 22:48–23:12 | `456e040` Release 资产五组复验 | 同资产 | 插件面 EDQUOT **507**、超时 `error_class=timeout`、430/860/1024 三档… |
| 2026-11-09 00:31 | `97edf87` 工作台三项复验版覆盖安装（Release 资产） | 资产 `97edf87` | 解压 `sh install.sh --no-docker` 成功 |
| 2026-11-09 00:35–01:05 | `97edf87` 工作台三项 + 任务/文件主链路复验 | 同资产 | 四插件 × 三档 iframe 高度收敛（430 封顶后页级滚动条 0、滚轮内部滚动有效）、单视图老包占位消失与明暗底衬… |
| 2026-11-09 01:05 | `d197041` testkit 探针复验版覆盖安装（Release 资产） | 资产 `d197041` | 解压 `sh install.sh --no-docker` 成功 |
| 2026-11-09 01:05–01:12 | `d197041` 台账四条复验 | 同资产 | WEB-04 四条候选校验、SITE-10 三种删除路径、files 授权默认/重装、BusyGuard 403 全部通… |
| 2026-11-09 01:23–01:35 | `9e2c6b1` 修复轮覆盖安装 + 复验（Release 资产） | 资产 `9e2c6b1` | 解压 `sh install.sh --no-docker` 成功 |
| 2026-11-09 凌晨 | `2f63aeb` FTP jail 修复版覆盖安装（Release 资产） | 资产 `2f63aeb` | 库保持 v15 |
| 2026-11-09 03:25 | `607d09b` 发版覆盖安装（Release 资产） | 资产 `607d09b` | 解压 `sh install.sh --no-docker` 成功 |
| 2026-11-09 03:59 | `8acc83f` 收口发版覆盖安装（Release 资产） | 资产 `8acc83f` | 解压 `sh install.sh --no-docker` 成功 |
| 2026-11-09 04:10–04:25 | monitor 网速口径修复的插件临时替换（**非 Release 资产**） | 现编 musl 插件 `730,592 B` `dfb96a8a…` 替换 `versions/0.1.0/bin/… | 已知量对照：16s 下载 119.1 MB，期望 tx 7,428,904 B/s、插件报 7,437,136 B/s（… |
| 2026-11-09 04:29 | `9a656d5` 下载端点 404 修复版覆盖安装（Release 资产） | 资产 `9a656d5` | 解压 `sh install.sh --no-docker` 成功 |
| 2026-11-09 05:10–05:28 | `0c99464` monitor 出站速率 + testkit 缓冲探针复验版覆盖安装（Release 资产） | 资产 `0c99464` | 解压 `sh install.sh --no-docker` 成功 |
| 2026-11-09 21:20–21:55 | 机器还原后**全新安装**（`491da40`，空库直建 v15） | 资产 `491da40` | 机器还原后无 `/etc/skiff`、无 `skiff.service`、Docker 已就绪 |
| 2026-11-10 08:05–08:40 | `483f898` 防火墙 + Node 运行时发版覆盖安装（Release 资产） | 资产 `483f898` | 解压 `sh install.sh --no-docker` 成功 |
| 2026-11-10 09:03–09:15 | `c7a06b4` 升级授权补齐复验版覆盖安装（Release 资产） | 资产 `c7a06b4` | 解压 `sh install.sh --no-docker` 成功 |
| 2026-11-10 13:25 | `2defdc7` 内核健壮性轮升级覆盖安装（Release 资产，**保留旧库**验迁移） | 资产 `2defdc7` | schema **v15→v16** 自动迁移成功（`user_version=16`、`fs_quotas.trunc… |
| 2026-11-10 14:09–14:25 | `38b0bb9` 两处 FAIL 修复复验版覆盖安装（Release 资产） | 资产 `38b0bb9` | 解压 `sh install.sh --no-docker` 成功 |
| 2026-11-10 15:28 起 | `0ee9065` 商店目录（Skiff-server P1+P2）覆盖安装 + 链路实测（Release 资产） | 资产 `0ee9065` | 解压 `sh install.sh --no-docker` 成功 |
| 2026-11-10 18:38 起 | `5c15b6b` 健壮性收口轮覆盖安装 + 五条重点复验（Release 资产） | 资产 `5c15b6b` | 解压 `sh install.sh --no-docker` 成功 |
| 2026-11-10 19:46 起 | `c8bcb6e` 商店 P3 升级只读段 + 暂存 TTL 覆盖安装 + 四条重点复验（Release 资产） | 资产 `c8bcb6e` | 解压 `sh install.sh --no-docker` 成功 |
| 2026-11-10 23:03–次日 00:20 | `eea2969` 六块大范围上机验收覆盖安装（Release 资产，**保留旧库**验 v16→v17） | 资产 `eea2969` | `sh -n install.sh` OK |
| 2026-11-11 00:19–00:57 | `db7e579` 四条 FAIL 修复复验版覆盖安装（Release 资产，**保留旧库**，无 schema 变更） | 资产 `db7e579` | `sh -n install.sh` OK、`sh install.sh --no-docker` exit 0 |
| 2026-11-11 00:52–01:06 | `a735a91` SITE-15 / FW-19 修复复验版覆盖安装（Release 资产，**保留旧库**，无 schema 变更） | 资产 `a735a91` | `sh -n install.sh` OK、`sh install.sh --no-docker` exit 0 |
| 2026-11-11 01:04–01:35 | `c4e42f0` SITE-15 前导冒号修复复验版覆盖安装（Release 资产，**保留旧库**，无 schema 变更） | 资产 `c4e42f0` | `sh -n install.sh` OK、`sh install.sh --no-docker` exit 0 |
| 2026-11-11 03:45–03:58 | `e2d6240`（含 `912c1f3`）菜单下移 + FTP 权限对齐复验版覆盖安装（Release 资产，**保留旧库**，无 schema 变更） | 资产 `e2d6240` | `sh -n install.sh` OK |
| 2026-11-11 04:08–04:20 | `97694b4` 站点/任务纯结构拆分回归覆盖安装（Release 资产，**保留旧库**，无 schema 变更） | 资产 `97694b4` | `sh -n install.sh` **exit 0**、`sh install.sh --no-docker` **… |
| 2026-11-11 04:47–04:52 | `244ea76` 默认站点切换修复 + `97694b4` 两处结构拆分回归覆盖安装（Release 资产，**保留旧库**，无 schema 变更） | 资产 `244ea76` | `sh -n install.sh` **exit 0**、`sh install.sh --no-docker` **… |
| 2026-11-11 05:30–05:31 | `bf0c4fe` TLS 临时件权限 + 唯一临时名收尾覆盖安装（Release 资产，**保留旧库**，无 schema 变更） | 资产 `bf0c4fe` | `sh -n install.sh` **exit 0**、`sh install.sh --no-docker` **… |
| 2026-11-11 07:19–07:21 | `8222d4e` 证书失败路径事务性回滚 + 悬空自愈覆盖安装（Release 资产，**保留旧库**，无 schema 变更） | 资产 `8222d4e` | `sh -n install.sh` **exit 0**、`sh install.sh --no-docker` **… |
| 2026-11-11 07:46–07:53 | `810f28a` 商店目录自定义 CA（`[catalog].ca_bundle`）覆盖安装 + 自签镜像端到端（Release 资产，**保留旧库**，无 schema 变更） | 资产 `810f28a` | `sh -n install.sh` **exit 0**、`bash deploy.sh`（含 `sh install… |
| 2026-11-11 08:05–08:15 | `f863e84`（实际 head `719996e`）`skiff-server serve` 商店链路端到端对接覆盖安装（Release 资产，**保留旧库**，无 schema 变更） | 资产 `f863e84` | `sh -n install.sh` **exit 0**、`bash deploy2.sh`（含 `sh instal… |
| 2026-11-11 08:39–08:45 | `d44966f` `serve --access-log` 上机取证（**只替换 `skiff-server`**，Skiff/库/配置均未动） | 资产 `d44966f` | 304 取证 `status=304 bytes=0 cache=inm range=none` PASS |
| 2026-11-11 21:10–21:18 | 测试机快照还原后**全量重建**（形态三：Skiff + 7 插件 + 两站点 + 自签 CA + 商店产物 + serve），无旧内容可备份 | 资产 `d44966f` run `42932324929`：x86_64 主包 9,931,780 B `4b29… | `/usr/local/bin/skiff` 14,742,288 B `19a31072…13e7`、`skiff-s… |
| 2026-11-11 21:20–21:30 | 快照还原后补装宿主 `pure-ftpd` / `fail2ban` 并补跑 FTP / fail2ban 用例（**未重装 Skiff**，库/配置未变） | `apt-get install -y pure-ftpd fail2ban`：`pure-ftpd 1.0.50-… | 二者初始 `enabled+active` → 立即停用为 `inactive+disabled`，`jail.d` 仅… |
| 2026-11-11 21:38–21:48 | **R0 发布面 API 端到端验收**（`189d581` / `tools/skiff-server`）：只替换 `skiff-server`、起 api:9443、发布 seq=2 并装 testkit 0.1.5 | Release run `43030422818`（`ref=main` `8a29ea8`，代码 `189d581… | 正向：提交 201 → 重提 200 同 sid → approve 202 |
| 2026-11-11 23:03–23:13 | **R1 撤包 / 吊销 / 审计验收**（`7d4a9b4` / `tools/skiff-server`）：**主包与 `skiff-server` 双资产替换**，api:9443 起停一次，发布 seq 3→4→5 | Release run `43043521770`（`ref=main` head `7d4a9b4`）四 job… | R0 链路回归：提交 0.1.6 → 201、重提 200 同 sid、approve 202、release seq… |
| 2026-11-11 23:07–23:18 | **R1 修复复验**（`f3b6f89`）：二次替换主包与 `skiff-server`，api:9443 起停一次，发布 seq 6→10 | Release run `43116275024`（`ref=main` head `f3b6f89`）四 job… | **两处偏差均修复**：yank 后插件页「该版本已被服务端下架：R1 复验：撤包原因可见性」、安装被拒 `[revok… |
| 2026-11-12 08:41–08:48 | **§15 维护路径 + 站点文件 0.1.3 + R2a/b/c/d 发布面**（`13a054c`）：主包与 `skiff-server` 双资产替换，`official/` 同步并升级 files 0.1.3 | Release run `43128096405`（`ref=main` head `13a054c`）四 job… | 库保持 **v18**、配置 sha `86610cdb…` 未变、前缀仍 `u9kiz9qb` |
| 2026-11-12 09:25–09:41 | **R2e 分 kind 撤包 + 维护任务失败文案**（`87a636e`）：主包与 `skiff-server` 双资产替换，api:9443 起停一次，dist seq 13→15（组 D）→16（商店安装回归） | Release run `43133873020`（`ref=main` head `87a636e`）四 job… | 库保持 **v18**、配置 sha `86610cdb…` 未变、前缀仍 `u9kiz9qb` |
| 2026-11-12 10:31–10:39 | **商店安装选择器**（`085bc18`）：主包覆盖替换（`skiff-server` 未变），api:9443 起停一次，dist seq 16→25（F 组夹具） | Release run `43141656735`（`ref=main` head `085bc18`）四 job… | 库保持 **v18**、配置 sha 未变、前缀 `u9kiz9qb` |
| 2026-11-12 12:47–12:52 | **SDUI 解耦八条上机回归**（`0eeabbe`）：主包覆盖替换（`skiff-server` 未变），无 schema 迁移 | Release run `43154627962`（`ref=main` head `0eeabbe`）四 job… | 库保持 **v18**、配置 sha `86610cdb…` 未变、前缀 `u9kiz9qb` |
| 2026-11-12 14:58–15:04 | **商店目录 TTL 与降级**（`a988085`）：主包覆盖替换（`skiff-server` 未变），dist seq 25→26（TTL 夹具），临时开启 serve 访问日志后还原 | Release run `43171292002`（`ref=main` head `a988085`，其后 `ma… | 库保持 **v18**、配置 sha `86610cdb…` 未变、前缀 `u9kiz9qb` |
| 2026-11-12 15:44–15:47 | **商店抓取静默丢包超时修复**（`7e322a3`）：主包覆盖替换（`skiff-server` 未变），无 schema 迁移；两份临时实例（默认源 / 多源回落）验完即清理 | Release run `43176330913`（`ref=main` head `7e322a3`）四 job… | 库保持 **v18**、配置 sha `86610cdb…` 未变、前缀 `u9kiz9qb` |
| 2026-11-13 19:53 | **就地升级**（五个插件功能页原生化 + firewall/cron 写通路 + 全站抽查） | 出包 run `43418974721`（head `4da21cd`） | 库保持 **v19**、`integrity_check=ok`、前缀 `34h8y9da`、`/healthz` 18… |
| 2026-11-13 20:11 | **就地升级**（补验 files 原生页与概览系统状态卡） | 出包 run `43421228044`（head `3d00543`） | 库保持 **v19**、`integrity_check=ok`、前缀 `34h8y9da`、`/healthz` 均… |
| 2026-11-13 20:55 | **就地升级**（验内置任务解锁、健康卡片三态、菜单链接与三页抽查） | 出包 run `43427115488`（head `1abcf4d`） | 库保持 **v19**、`integrity_check=ok`、前缀 `34h8y9da`、`/healthz` 均… |
| 2026-11-13 21:37 | **就地升级**（budding 1.8.0-r3：游标分页、图标、代码查看器、行内删除） | 出包 run `43433228776`（head `150357f`，代码同 `91813f1`） | 库保持 **v19**、`integrity_check=ok`、前缀 `34h8y9da`、`/healthz` 均… |
| 2026-11-14 07:45 | **就地升级**（budding 1.8.0-r3 三处缺陷修复复验：审计全量行、代码查看器、菜单图标） | 出包 run `43528413516`（head `c535343`） | 库保持 **v19**、`integrity_check=ok`、前缀 `34h8y9da`、`/healthz` 均… |
| 2026-11-14 08:28 | **就地升级**（作业日志路径按 `Accept` 分流复验） | 出包 run `43533335180`（head `16cbe9a`） | 库保持 **v19**、`integrity_check=ok`、前缀 `34h8y9da`、`/healthz` 均… |
| 2026-11-14 14:36 | **全新安装**（r4 资产与概览/容器列表精修；旧安装与旧库已不存在，无备份可做） | 出包 run `43565620459`（head `008ecff`） | 空库 **v19**、`integrity_check=ok` |
| 2026-11-14 16:06 | **全新安装**（查询态页内跳转重渲染 + 列表场景空态补齐） | 出包 run `43572862985`（head `10deadf`） | 空库 **v19**、`integrity_check=ok` |
| 2026-11-14 18:49 | **全新安装**（壳主题误写 schema 根属性，全站 CompileError，未进入验收） | 出包 run `43586098454`（head `e38be99`） | 空库 **v19**、`integrity_check=ok` |
| 2026-11-14 19:25 | **全新安装**（`56e5790` 主题 CSS 变量修复 + 菜单精简 + Web 服务原生运维） | 出包 run `43589383679`（head `56e5790`） | 空库 **v19**、`integrity_check=ok` |
| 2026-11-15 00:39 | **全新安装**（`53b06bf` 交互改造 P0/P1/P2 + budding r5 + Web 安装进度持久化） | CI run `43614238123` success | 空库 **v20**、`integrity_check=ok` |
| 2026-11-15 01:20 | **全新安装**（`194d6a1` Docker 资源动作与设置保存键分离） | CI run `43618467103` success | 空库 **v20**、`integrity_check=ok` |
| 2026-11-15 01:58 | **全新安装**（`7eeb0fd` 文件管理「改属主」请求体类型修复） | CI run `43621954301` success | 空库 **v20**、`integrity_check=ok` |
| 2026-11-15 08:10 | **全新安装**（`7946e93` 界面四套预设 + 三层配色；站点/运行环境列表页内弹窗新建） | CI run `43655127248` success | 空库 **v20**、7 插件、转发层、`/healthz` 正常 |
| 2026-11-15 12:34 | **全新安装**（`d025258` 内核健壮性 C 批四项） | CI run `43669273358` success | 空库 **v20**、`integrity_check=ok`、7 插件、`/healthz` 18080/18081… |
| 2026-11-15 15:35 | **全新安装**（`3b5c433` Node 端口归属 + crash-loop 就绪判定） | CI run `43688211443` success | 空库 **v20**、`integrity_check=ok`、7 插件、`/healthz` 200 |
| 2026-11-13 08:15–08:20 | 快照还原后**全新安装**（形态三，budding 迁移 UI 验收前置） | run `43331444780`（`ref=main` head `51914a6`，代码同 `0d33a86`） | 空库直建 **v19**、`integrity_check=ok` |
| 2026-11-13 09:18 | **就地升级**（保留库/配置/插件，只换二进制；budding admin 外壳 + 插件标签修复验收） | run `43342855257`（head `d06bdab`） | 库 v19 未变、`integrity_check=ok` |
| 2026-11-13 18:20 | **就地升级**（验 Docker/Compose/运行环境/商店/文件/Web 页写操作） | run `43410030908`（head `b7771c3`） | 库 v19 未变、`integrity_check=ok` |
| 2026-11-13 14:46 | **就地升级**（验收容器终端 / 审计 / 日志） | run `43379837914`（head `4c1a1aa`） | 库 v19 未变、`integrity_check=ok` |
| 2026-11-13 09:52 | **就地升级**（复验插件标签与插件功能页占位） | run `43346854352`（head `281dc9d`） | 库 v19 未变、`integrity_check=ok` |
| 2026-11-15 23:04 | **全新安装 `v0.1.0` Release 资产 + 全量回归（含八项复跑、浏览器实操）** | Release `v0.1.0` amd64 包 12,070,181 B `114a8320…5f67`（`.sh… | 全新空库 `user_version=21`、`integrity_check=ok` |
| 2026-11-13 09:18 | **就地升级**（保留库/配置/插件，只换二进制；budding admin 外壳 + 插件标签修复验收） | run `43342855257`（head `d06bdab`） | 库 v19 未变、`integrity_check=ok` |
| 2026-11-16 23:22 | **全新安装**（`280345e` 架构收口：actor/map_err 合一份、`require_admin` 走 authz；定向回归授权/审计/认证） | run `43892800693`（head `280345e`）四 job 全绿；包 12,130,214 B `7dc33e92…f051` | 空库 **v21**、`integrity_check=ok`、7 插件、`/healthz` 18080/18081 均 200 |
| 2026-11-17 00:06 | **就地覆盖安装**（`2847580` Docker 409 结构化 + accept.sh B 组 `--password`；同时验升级路径） | run `43902115907`（head `2847580`）四 job 全绿；包 12,130,261 B `f90676b4…6894` | 库保持 **v21**、`integrity_check=ok`、配置逐字节未变、前缀 `cb5mgdk7` 未变、7 插件全在；重复建网络回 **409 `domain.conflict`** |
| 2026-11-17 08:14 | **就地覆盖安装**（`fc43f3c` UI 壳层 chrome D17：顶栏隐藏 + 三级路径条 + 账号入设置页；网站下四页精修 + Web 服务拆三级子视图 + files schema） | Release run `43984373863`（head `fc43f3c`）success；包 12,143,038 B `fd782a0…cd7f` | 库保持 **v21**、`integrity_check=ok`、配置 sha `5bfb00d9…` 未变、前缀 `cb5mgdk7` 未变、7 插件全在；八页浏览器截图验收全过 |
| 2026-11-17 13:00 | **就地覆盖安装**（`ab70234` 三级菜单重做：路径条→横向菜单；站点详情拆 6 视图、运行环境详情拆 6 视图；插件 L3 走 `views`） | Release run `44018296064`（head `ab70234`）success；包 12,144,387 B `8c53a1eb…5580` | 库保持 **v21**、`integrity_check=ok`、配置 sha `5bfb00d9…` 未变、前缀 `cb5mgdk7` 未变、7 插件全在；7 页浏览器验收全过、无 console error |
| 2026-11-17 19:51 | **就地覆盖安装**（`0816073` testkit 迁随包 schema + 壳页 `inspect` 结果图 + 壳脚本版本化 `v=3`） | Release run `44075496980`（head `0816073`）success；包 12,153,912 B `5f6bcbc7…1ee38` | 库保持 **v21**、`integrity_check=ok`、配置 sha `5bfb00d9…` 未变、前缀 `cb5mgdk7` 未变、7 插件全在；**testkit 自动升到 `api=1.3` + 4 个 schema**；四视图壳内实测无 iframe、RPC 成功/失败/越界三个结果图正确 |
| 2026-11-17 23:17 | 机器还原后**全新安装**（形态三；budding 1.8.1 + redirect + 预设色卡 + 文件插件 schema 首版） | head `a9a0b59`；包 12,109,782 B `69ee464e…f9f2` | 空库直建 **v21**、`integrity_check=ok`；前缀 **`kyuyjr44`**；7 插件；`/healthz` 18080/18081 均 200 |
| 2026-11-17 23:35 | **就地覆盖安装**（files schema 节点白名单修复 + 防火墙/FTP 插件缺陷修复首批） | head `cb81325`；包 12,120,566 B `e2648a4c…e5915e9` | 库保持 **v21**、站点/插件/授权全保留 |
| 2026-11-18 00:15 | **就地覆盖安装**（文件列表前置、防火墙状态文案、审计列宽、设置页重叠修复；`nft` 建表/建链） | head `8004dc1`；包 12,117,986 B `f43dafca…789cf` | 库保持 **v21**、`integrity_check=ok` |
| 2026-11-18 00:46 | **就地覆盖安装**（站点详情指标条去重、FTP 列表列宽） | head `286a5b7`；包 12,118,283 B `12914393…54774` | 库保持 **v21**、`integrity_check=ok`、前缀 `kyuyjr44` 未变 |
| 2026-11-18 06:10 | **就地覆盖安装**（`36840f2` 修复防火墙端口 / fail2ban 数值参数不从表单字符串解析） | Release run `44185563501`（head `36840f2`）success；包 12,121,107 B `7ac7f9ce…321e` | 库保持 **v21**、`integrity_check=ok`、配置 sha `fad99a3d…` 未变、权限 `711/711`；三站点 running、`/healthz` 双端 200；防火墙 `port.allow("59991","both")` 新增两条、按 id 逐条删除通过 |
| 2026-11-18 19:27 | **机器还原后全新安装**（旧 SSR 清理 + budding 1.8.1-r2；T1 口径） | Release run `44293570337`（head `12d8ea2`）success；包 11,951,970 B `2445983b…6e21` | 空库 **v21**、`integrity_check=ok`；前缀 **`2u3uiv85`**；7 插件（需 `allow_reduced_sandbox`）；accept.sh **49 PASS / 0 FAIL / 2 SKIP**；25 页 0 console error；`/m/core/*` 全 404 |
| 2026-11-18 19:50 | **就地覆盖安装**（表单回车修复 + 壳 `?v=5`） | Release run `44296920567`（head `bcdfd22`）success；包 11,953,417 B `523a352e…e970` | 库保持 **v21**、配置逐字节未变、前缀未变、7 插件与站点全保留；accept.sh A/D0 **34 PASS / 0 FAIL / 2 SKIP**；交互 10 项全过（含回车筛选） |
| 2026-11-18 21:15 | **就地覆盖安装**（登录页回车（壳 `?v=6`，补丁抽成 `form-enter-submit.js`）+ PHP 扩展默认超时 1800/3600 s + Docker 写路径错误分类三分法） | Release run `44310262459`（head `d0c8cc4`）success；包 11,951,277 B `fc2269cb…69b2` | 库保持 **v21**、`integrity_check=ok`、前缀 `2u3uiv85` 未变、7 插件 / 2 站点 / `php-8.5` running 全保留；accept.sh A+D0 **26 PASS / 0 FAIL / 2 SKIP**；25 页 0 console error；登录页回车直达壳页；容器重名 **409**、镜像不存在 **400** |
| 2026-11-19 08:31 | **机器还原后全新安装**（T1 口径；新建 PHP 环境可选 apk 源 + sources 插件修复 + 架构收口回归修复） | Release run `44428042366`（head `f5267ec`）success；CI `44426759812` 全绿；包 13,272,472 B `60f0b8ff…0a3e` | 空库 **v21**、`integrity_check=ok`；前缀 **`zgvvx5xz`**；官方 8 插件全装（`allow_reduced_sandbox=true`）；accept.sh A+D0 **27 PASS / 0 FAIL / 2 SKIP**；概览与运行环境新建弹窗浏览器实测通过（源下拉预选清华 TUNA） |
| 2026-11-19 09:31 | **就地覆盖安装**（sources 改原生 schema + apk 恢复自动探测） | Release run `44435964314`（head `8f78221`）success；CI `44434687589` 全绿；包 13,272,804 B `f04eafe8…dba7` | 库保持 **v21**、`integrity_check=ok`、前缀未变、8 插件全保留；`com.skiff.sources` 同版本内容自动更新；软件源原生页两表加载 + 测速出真实 MB/s；`php-8.5` 在固定 tuna 源下 **10 s 安装完成**，容器内 `/etc/apk/repositories` 指向 tuna `v3.24` |
