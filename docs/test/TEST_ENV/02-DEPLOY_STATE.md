# 当前部署与 Release 资产（§5 当前轮 + §6）

> 上级索引：[TEST_ENV.md](../TEST_ENV.md) ｜ 拆自 `docs/test/TEST_ENV.md` 原 §5 当前生效一轮 + §6，历史轮次已压成 03-DEPLOY_LOG.md 的坐标表（2026-11-16 整理）。

---

## 5. 当前部署

> **2026-11-19 09:31 就地覆盖安装 `8f78221`（当前生效；库/配置/插件/运行环境全保留）**
> - 背景：本轮三项——新建 PHP 环境可选 apk 镜像源、软件源插件复查修复（编译错误 / `r.base` / 原生 schema）、测试机还原后重新部署。Release run `44435964314`（head `8f78221`）success（同提交 CI `44434687589` 全绿）；前一次首装为 Release run `44428042366`（head `f5267ec`），见 03-DEPLOY_LOG。
> - 机器状态：还原后为干净系统——无旧二进制/配置/库、18080/18081 空闲、Docker 29.7.2 在位、`/etc/skiff-baseline` 不存在；按 T1 口径（Docker 已装）首装后在同机覆盖升级。
> - 资产：`skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` **13,272,804 B** `f04eafe89684f69b3ca1dff4783738ed7938401a753ac695ac3977b51ff1dba7`（资产 id `765733618`；API `digest` / 本地 sha256 / 测试机 `sha256sum` 三方一致）。
> - 安装：`install.sh --no-docker --tls off --listen 127.0.0.1:18080` **exit 0**（首装 + 就地覆盖各一次）。`skiff.service` 与 `skiff-http-fwd.service`（`0.0.0.0:18081 → 127.0.0.1:18080`，脚本 `/root/iter/skiff-http-fwd.py`）均 active。
> - 二进制：`/usr/local/bin/skiff` **19,179,408 B** `e428451579aa8e198034898662683e43323114c5e52fad03a701a95e9d59bfc4`。
> - 库：`user_version=21`、`integrity_check=ok`；users 1 / plugins **8** / sites 0 / site_runtimes 1（`php-8.5` running）。配置 `/etc/skiff/config.toml` 已加 `allow_reduced_sandbox = true`（否则官方插件全被「要求完整沙箱」拒装），sha256 `59791959b24138b17a83c473433054c2ff9eec9ed752449b4d8becfe41234129`。权限 `711 /www/skiff`、`711 plugins/`、`600 skiff.db`、`711 /run/skiff` 保持。
> - 面板：入口前缀 **`zgvvx5xz`**；管理员 `admin` / `wGQcAXtSDKLDaxBs`（`/www/skiff/initial-admin.txt` 仍在，B 组在 TLS/prod 档跳过故未改密）。`/healthz` 18080 与 18081 均 **200**，登录页 **200**。
> - 插件：官方 8 个包（7 官方 + testkit）全部装成；`com.skiff.sources` 同版本内容随发布包自动更新（journal：`官方插件内容已随发布包更新（同版本重装）`）。
> - 软件源：apk 固定为 `https://mirrors.tuna.tsinghua.edu.cn/alpine`（`/www/runtime/cache/apk/.skiff-mirror` 带 `# skiff-fixed`）；容器 `/etc/apk/repositories` 为 tuna `v3.24/{main,community}`。
> - 运行环境：`php-8.5`（`php:8.5-fpm-alpine`，PHP 8.5.12）**10 s 装完**（固定 tuna 源），容器 `skiff-php-php-8.5` running；站点本轮未建。
> - 浏览器实测（真实资产无拦截）：概览页、运行环境新建弹窗（软件包源字段预选「清华 TUNA」）、软件源插件原生页（两表 5 行数据、`测速全部` 出真实 MB/s）均 **0 console error**。

## 6. Release 资产台账（当前资产；历史坐标见 03-DEPLOY_LOG.md）

测试期只有 `v0.1.0` 一个 tag，**每次 dispatch 都用 `--clobber` 覆盖同一 Release 的同名资产**，因此「某轮的资产 sha」只在该轮有效；对比历史请查 [03-DEPLOY_LOG.md](03-DEPLOY_LOG.md) 的坐标表。

- **当前资产来源**：Release run `44435964314`（dispatch `ref=main`，head `8f78221`，`publish=true`、`release_tag=v0.1.0`），success。
- **tag 与资产不同源**：tag `v0.1.0` 仍指向 `ef7058b2cdc622f300cdc97c4466e482d105740c`，与资产构建提交（`8f78221`）不同——这是「不移动 tag + dispatch 覆盖资产」的约定结果。

| 资产 | 大小 | sha256 | 资产 id |
| --- | --- | --- | --- |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz` | 13,272,804 B | `f04eafe89684f69b3ca1dff4783738ed7938401a753ac695ac3977b51ff1dba7` | `765733618` |
| `skiff-0.1.0-x86_64-unknown-linux-musl.tar.gz.sha256` | 111 B | （侧车，内容同上摘要） | `765733585` |
| `skiff-0.1.0-aarch64-unknown-linux-musl.tar.gz` | 同轮 | 见同轮 `checksums.txt` | 同轮上传 |
| `skiff-server-{x86_64,aarch64}-unknown-linux-musl` | 同轮 | 见同轮 `checksums.txt` | 同轮上传 |

- **下载与核对**：开发机直接打 `api.github.com/repos/php-nil/skiff/releases/assets/<id>` + `Accept: application/octet-stream`（缓存 `%TEMP%\skiff-deploy\pkg2\`）；装机前本地 sha256 / API `digest` 一致，`scp` 到测试机后 `sha256sum` 再核对。
- **包内容**：`official/` 含 8 个官方插件包（`com.skiff.{cron,files,firewall,ftp,guard,monitor,sources,testkit}`）+ `public-key.txt`；aarch64 两件本轮未实机验证。
