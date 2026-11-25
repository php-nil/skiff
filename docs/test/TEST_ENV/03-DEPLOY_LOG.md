# 部署流水台账（§5 历史 + §7）

> 上级索引：[TEST_ENV.md](../TEST_ENV.md) ｜ 拆自 `docs/test/TEST_ENV.md` 原 §5 历史段落 + §7 明细表（精简为一行一轮）（2026-11-16 整理）。

---

## 部署流水（按日坐标台账）

> 一轮部署一行：**日期 / 场景 / 版本坐标 / 结论**。完整断言、逐条复验与根因不在这里——当轮细节见 `docs/changes/YYYY-MM-DD.md` 与 git log，验收断言见 [04-ACCEPTANCE.md](04-ACCEPTANCE.md)。
> 2026-11-16 整理：原 §5 的历史安装段落（每轮一整段安装流水）与 §7 的明细表已合并成本表，只留坐标与结论；同一轮的证书指纹、逐条命令输出以 `changes/` 为准。

**当前生效部署见 [02-DEPLOY_STATE.md](02-DEPLOY_STATE.md)**；2026-11-19 及更早的流水在 [03-DEPLOY_LOG/01-ARCHIVE.md](03-DEPLOY_LOG/01-ARCHIVE.md)；表内结论为「当轮结论」，不代表当前机器状态。

| 日期 | 场景 | 版本 / 坐标 | 结论 |
| --- | --- | --- | --- |
| 2026-11-26 06:02 | **就地升级三：文案与错误页修复**（`7810675` → `a915b6e`，不清库、不换前缀） | amd64 12,542,260 B `c038e313…0279b`（CI `45687189385` 绿 / Release `45688148056`）；前缀仍 `5e3yz2yr` | ✅ exit 0、`config.toml` 逐字节相同、插件/容器保留；`accept.sh` 47 PASS / 0 FAIL / 5 SKIP；`round.sh` 106 / `round_ext.sh` 30 / `round_ext2.sh` 20 全 0 FAIL；浏览器 44 路由 **44 PASS**；`/sites/runtimes/none` 显示新落地页、PHP 配置保存消息改为「重启 PHP 容器」 |
| 2026-11-26 03:28 | **就地升级二：回收站 CompileError 修复**（`070ff1d` → `7810675`） | amd64 12,541,372 B `d7fb85d8…ac0c7`（CI `45683167379` 绿 / Release `45683912296`）；前缀仍 `5e3yz2yr` | ✅ exit 0、配置零改动；`/files/trash` 0 CompileError；恢复 / 覆盖恢复（onError 弹确认）/ 彻底删除三条路径实测通过；`accept.sh` 47 / 0 / 5 |
| 2026-11-26 03:00 | **就地升级一：`gzip_types` 空值修复**（`92949ea` → `070ff1d`） | amd64 12,539,101 B `87a828e5…`（Release `45678981070`）；前缀仍 `5e3yz2yr` | ✅ exit 0、配置零改动、7 插件与 2 容器保留；保存性能参数返回「已通过 nginx -t」，主配置只剩 `gzip on;`；`accept.sh` 49 / 0 / 3 |
| 2026-11-26 02:28 | **还原后干净首装**（形态三：明文 18080） | Release `45673872060`（head `92949ea`）amd64 12,538,527 B `6c246450…`，前缀 **`5e3yz2yr`** | ✅ preflight 8/8 PASS；`install.sh --listen 0.0.0.0:18080 --tls off --no-start` exit 0；`accept.sh` **50 PASS / 0 FAIL / 2 SKIP**；7 官方插件全装、`php-8.5` running、OpenResty 装好、三站点 200 |
| 2026-11-25 20:55 | **就地升级两跳：换到含 11-25 源码的包**（`7e3146f` → `a0f27c8`，不清库、不换前缀） | amd64 12,538,544 B `2d0e29b7…2ce0a6`（CI `45616188710` 绿 / Release `45616962452`）；前缀仍 `8au3xewy`；镜像源同前 | ✅ 两跳 exit 0、`config.toml` 零改动、`official/` 7 包、MIRROR-01~09 全过（三类清单 23 条、篡改/异钥拒绝、断源降级）；⚠ `accept.sh` 全量未跑 |
| 2026-11-25 19:18 | **干净重装：开发机当服务端（`skiff-server serve`）+ 测试机当客户端**（换包不换代，包同 `c841074`；先备份再 `uninstall.sh --purge`） | amd64 12,923,388 B `c905859d…9451b2`；前缀 `gk5y33tq` → **`8au3xewy`**；镜像源 `http://10.66.52.7:8080` | ✅ 安装 exit 0、doctor 六项 ok、商店「已从 http://10.66.52.7:8080 更新到 seq 1」+ 23 条运行环境、304 与断源降级通过；⚠ 面板侧版本清单未验（该包无 `/ui/v1/runtimes/versions`），`accept.sh` 全量未跑 |
| 2026-11-24 11:24 | **机器还原后干净首装**（budding 1.8.7 同步 + 页头主操作迁移 + SUBVIEWS 双源合一 + writePost 模板；形态三） | Release run `45317927914`（head `c841074`）amd64 12,923,388 B `c905859d…9451b2`，前缀 `gk5y33tq` | ✅ accept.sh **48 PASS / 0 FAIL / 4 SKIP**；权限位全对；B 组完整 CSRF→登录→改密链路通过；D0 Docker 29.7.2 |
| 2026-11-22 22:47 | **还原后干净首装 + budding 1.8.5 同步 / 弹窗页脚提交 / 新建入口弹窗化**（首轮发现目录失败提示丢失输入 → 修复后三次升级复验） | 首装 Run `45028320777`（head `e3066e6`）`38eb2d64…096a`；中间 Run `45030832428`（head `5fa12e2`）`dec1254e…a42e`；最终 Run `45033701573`（head `76412df`）amd64 12,971,630 B `ddd297af…0e18`；前缀 `dwkaf8si` | ✅ accept.sh 三轮均 **49 PASS / 0 FAIL / 3 SKIP**；浏览器概览、新建站点、防火墙弹窗+提交（18081/tcp）、新建目录成功/失败复验、公钥导入弹窗 0 CompileError；插件 schema 页脚 reset/close/submit 与 no-op api 注入确认；仅 firewall 插件可装（cron 官方包缺失） |
| 2026-11-22 16:09 | **还原后干净首装 + `row_button` CompileError 修复**（`json!({"actions":…})` 包装改为 `Vec<Value>`） | Run `45000648749`（head `946619d`）amd64 `eaaa57df…e3b`，前缀 `7kcyc858` | ✅ accept.sh **50 PASS / 0 FAIL / 2 SKIP**；8 插件全装；7 页面 0 CompileError / 0 console error |
| 2026-11-21 23:10 | **还原后干净首装 + budding 1.8.4 同步**（`content.breadcrumbs` 移除适配、shared 组件基线瘦身、表单属性补全） | Run `44916947385`（head `a3722fb`）amd64 12,725,968 B `44d8081e…54f5`，前缀 `kfzehjpk` | ✅ accept.sh **46 PASS / 0 FAIL / 4 SKIP**；8 插件全装；43 路由 **42 PASS**（1 条为故意访问不存在环境的预期 400）、0 console error；容器终端 WS、code pack 高亮、备份恢复、口令重置均实测通过 |
| 2026-11-21 11:45 | **登录页视觉重做 + 滚动条修复**（合成卡片、正常控件密度、取消固定高度/内层滚动） | Run `207`（head `5221b4c`）amd64 12,913,254 B `37492742…75ac`，前缀 `88qfm2s8` | ✅ 960×571 深/浅双主题真实浏览器：标题完整、三字段 + 通栏实心按钮、`#skiff-login=282px`、页面与右列 `clientHeight==scrollHeight`、无滚动条、0 console error；admin 登录正常；`abc7b0c`/`b44aac8`/`cb1b421` 三次中间构建未通过验证，未计入 |
| 2026-11-21 10:18 | **还原后干净安装 + 弹窗 schema 修复轮**（GitHub Release 当前资产 `5bec69b` → 修复 `d5680b6` 重发版升级） | Run `202`（head `5bec69b`）amd64 12,913,533 B `3c6d40b6…b424`；Run `203`（head `d5680b6`）amd64 12,913,194 B `152e5902…e257`，前缀 `88qfm2s8` | ✅ accept.sh **49 PASS / 0 FAIL / 2 SKIP**；8 插件全装；31 页巡检发现 3 页 `CompileError`（dialog `props.body`）→ `d5680b6` 修复并升级复验 3/3 PASS（含新建目录写操作），0 console error |
| 2026-11-21 01:00 | **干净重装 v23 最终版**（登录页 body overflow:hidden !important + budding 1.8.3 同步） | Release run `197`（head `26f53ec`），amd64 21,125,136 B `sha256=bf1ce05f…2162d`，前缀 `ewtdhtgx` | ✅ 960×571 视口 html/body scrollHeight==clientHeight 无垂直滚动条；浏览器验收 4/4 PASS；curl HTML + CSS 规则全链验证；库 v22 |
| 2026-11-21 00:53 | 磁盘覆盖 v23（登录页滚动条三轮回缩，非重装） | head `26f53ec`，前缀 `nitrz26v` | ✅ 960×571 无垂直滚动条，登录正常 |
| 2026-11-20 23:34 | **登录页 CSS 修复**（测试机还原后全新安装，`6d98116` 登录页布局收紧） | Release run `194`（head `6d98116`），amd64 18,016,272 B `sha256=948b9312…7977` | ✅ 1080px 视口下无垂直滚动条；登录成功；库 v22 |
| 2026-11-20 22:18 | **UI 巡检第 4 轮**（删除运行环境后跳回列表） | Run `44750632405`（head `ffc0aa2`），amd64 12,897,732 B `sha256=fbb96094…6fbb` | 删除 `uitest-del` 后落在 `/sites/runtimes`，无「运行环境不存在」错误页；库 v22、8 插件保留 |
| 2026-11-20 22:06 | **UI 巡检第 3 轮**（配置文件编辑 query 跳转整页加载） | Run `44748817699`（head `ca632b1`），amd64 12,897,514 B `sha256=6f736521…52fc` | PHP `php.ini` 编辑器渲染、内容 148 字符；审计重置仍 PASS；库 v22 |
| 2026-11-20 21:53 | **UI 巡检第 2 轮**（代码编辑器表单 / 沙箱文案 / 弹窗吸底 + 配置入口 / 审计重置） | Run `44747016695`（head `6262cfa`），amd64 12,897,607 B `sha256=15089996…99d9` | 文件新建/编辑落盘 13 B/18 B；Compose 校验 200；沙箱文案「降权（要求完整…）」；弹窗按钮在 960×571 视口内；审计重置 PASS；配置文件编辑器仍未渲染 |
| 2026-11-20 21:31 | **UI 巡检第 1 轮修复**（`code` 表单空串 / 沙箱 / 弹窗吸底） | Run `44743449302`（head `30c2c39`），amd64 12,897,589 B `sha256=5976ccbb…99b8` | 文件内容落盘、Compose 校验、沙箱文案、弹窗按钮四项复验 PASS；库 v22 |
| 2026-11-20 20:58 | 机器还原后**全新安装** + 首轮逐页巡检 | Run `44739127639`（head `2095d22`），amd64 12,897,492 B `sha256=204004b3…7da05` | accept.sh **44 PASS / 0 FAIL / 5 SKIP**；8 插件全装；20+ 页面 0 console error；发现 6 类交互缺陷进入修复 |
| 2026-11-20 19:00 | **插件权限修复**（无新发版；逐插件 `--yes` 重装以恢复 manifest `resource_hints`，去掉统一 `--resource path:/www`） | 同资产 `9d8d7a4` | ✅ 51/51 页面 PASS，monitor 主机指标与进程列表恢复，0 console error |
| 2026-11-20 18:26 | 机器还原后全新安装 + 5 个门禁修复 commit 收尾 | Run `44717281516`（head `9d8d7a4`），amd64 12,895,953 B `sha256=4211de8c…8acd` | accept.sh **44 PASS / 0 FAIL / 5 SKIP**；浏览器 8 页面 0 console error；8 官方插件全装；L1 门禁全绿 |
| 2026-11-20 14:17 | 覆盖升级 schema v21→v22 + budding-boot.js CSS 修正 | Run `44687309424`（head `524f880`），amd64 12.9 MB；前序 Run `44685263606`（head `fae19cd`，50 files） | schema 自动迁移 v22 ✅；curl slug→id 303 ✅；**浏览器 6 项站点详情全量复验 PASS** |
| 2026-11-20 10:58 | 空库首装（write_atomic 权限修复 + CLI 前缀分叉收口） | Run `44574500026`（head `a02f1d0`），amd64 12,821,377 B `sha256=2fd680b5…b5c` | accept.sh **48 PASS / 0 FAIL / 3 SKIP**；浏览器 9 页面 0 console error；A6e 根治 |
| 2026-11-23 18:58 | **机器还原后全新安装**（budding 1.8.6 同步 + 表单事件提交；形态三） | Release run `45158992891`（head `9f43aee`）success；包 12,962,955 B `f11172ee…a6106c937` | 空库 **v22**、`integrity_check=ok`；前缀 **`28bwmusa`**；8 官方包待装（需 `allow_reduced_sandbox`）；accept.sh 首跑 35 PASS（B 组因缺 `allow_insecure_http` 整组 SKIP），补配置后 **50 PASS / 0 FAIL / 2 SKIP** |
| 2026-11-23 19:39 | **就地覆盖安装**（`5a48b83` 修复插件拒装删源包） | CI `45161514099` 绿；Release run `45162438597`（head `5a48b83`)；包 12,962,844 B `4f032486…aea55ed` | 库保持 **v22**、配置 sha 不变、前缀不变；8 个官方插件全部 installed（装完 `/www/skiff/official` 仍 9 个文件，修复生效）；accept **49 PASS / 0 FAIL / 3 SKIP** |
| 2026-11-23 20:16 | **就地覆盖安装**（`9c1be6c` 修复弹窗提交失败静默 + CI 断言同步） | CI `45169444034` 绿；Release run `45170361272`（head `9c1be6c`）；包 12,964,003 B `0d5069ef…7424d62` | 库保持 **v22**、`integrity_check=ok`、8 插件与官方目录保留；accept **49 PASS / 0 FAIL / 3 SKIP**；浏览器实路由 50/50 0 console error、关键交互 15/15；同名建目录失败原地回显「目标已存在」 |
