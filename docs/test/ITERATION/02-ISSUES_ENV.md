# 已知限制、测试机与凭据、工具落点（§6–8）

> 上级索引：[ITERATION.md](../ITERATION.md) ｜ 拆自 `docs/test/ITERATION.md` 原 §6–8（2026-11-16 整理，正文未改）。

---

## 6. 已知限制

### 6.1 明文 HTTP 下测不了登录

`dev_http == false` 时 cookie 带 `Secure`，明文 HTTP 下 curl 与浏览器都不回传。故 `accept.sh` 需两档：dev 档走 `--dev`（cookie 降级为 `skiff_dev_session`，可用 `curl -c/-b`），prod 档配自签 TLS + `curl -k` 验真实 `Secure`/`SameSite` 属性。
2026-11-03 已按 prod 档在测试机跑通（同端口 TLS、cookie jar 登录与改密），结果见
[`DEPLOY_TEST/02-INSTALL.md`](../DEPLOY_TEST/02-INSTALL.md) §2.4.2。

## 7. 测试机与凭据

地址、SSH 用户/密钥/公钥、面板 URL/入口前缀/TLS、管理员口令文件与读取方式、当前部署版本与资产摘要，**统一见 [TEST_ENV.md](../TEST_ENV.md)**；本文不重复，也不保存明文口令。

- 公钥必须先加入 `authorized_keys`，再打基线快照；顺序反了回滚后即失联。
- 首连只做环境探测、不装任何东西：`uname -m/-r`、`/etc/os-release`、`nproc`、`free -h`、`df -h`、`id -u`、`sudo -n true`、`ss -lntp`、`curl -I linuxmirrors.cn`、`apt-get -s update`、landlock ABI、cgroups v2、`systemd --version`。结果记 [TEST_ENV/03-DEPLOY_LOG.md](../TEST_ENV/03-DEPLOY_LOG.md)。
- 回滚方式：人工手动操作，需要时告知机器所有者。

### 7.1 内网 HTTP 代理（GitHub 出网缺口就靠它）

内网有五个 HTTP 代理，都能到 `github.com:443`。2026-11-03 实测（HEAD 一个空响应，**只验通断不跑流量**）：

| 代理              | 本机→github.com | 测试机→github.com | 备注            |
| --------------- | ------------- | ------------- | ------------- |
| `10.66.10.22:1010` | 200 / 0.53 s  | 200 / 1.85 s  | **首选**        |
| `10.66.10.12:1010` | 200 / 0.69 s  | 200 / 0.68 s（偶发 403，重试即可） | 次选 |
| `10.66.10.9:1010`  | 200 / 1.37 s  | 200 / 1.89 s  | 可用          |
| `10.66.10.21:1010` | 200 / 4.83 s  | 200 / 2.34 s  | 偏慢，兜底       |
| `10.66.10.13:1010` | 200 / 1.42 s  | **超时 8 s**    | 本机可用、测试机到不了 |

用法：`git` 只认环境变量，不认 `curl -x`；**必须**加 `-c http.sslBackend=openssl`，否则 Windows 的 schannel 过代理会握手失败。

```sh
export HTTPS_PROXY=http://10.66.10.22:1010
export HTTP_PROXY=$HTTPS_PROXY
git -c http.sslBackend=openssl push origin <branch>
curl -x $HTTPS_PROXY -sS -I https://github.com          # 单次探活
```

实测旁证：同一条 `git push` 之前用本机代理端口（`127.0.0.1:50121` 之类）一直失败，换成 `10.66.10.22:1010` 后**一次成功**
（`* [new branch] iter/first-install -> iter/first-install`）。`git ls-remote` 也通，返回 `main` 的 commit。

**本机已固化**（2026-11-07）：不再依赖手打 `-c`，全局配置把代理与 sslBackend 都限定在 `github.com` 作用域，其它流量不受影响：

```sh
git config --global http.https://github.com.proxy http://10.66.10.22:1010
git config --global http.https://github.com.sslBackend openssl
```

复测：`git ls-remote origin refs/heads/main` 未加任何 `-c` 直接返回 SHA。五个代理当日通断（`curl -x ... -I https://github.com`）：
`.22` 200 / 3.1 s、`.12` 200 / 2.26 s、`.13` 200 / 1.84 s、`.9` 200 / 2.47 s、`.21` 200 但 10 s 内只收 81 KB（传输超时，仅作兜底）。

**GCM 不继承 git 的代理配置**（2026-11-09 实测）：`http.<url>.proxy` 只作用于 git 自身，`git-credential-manager`
是独立进程，系统代理关闭且无 `HTTPS_PROXY` 时它自己出不去，登录流程会停在「Connect to GitHub」。需要给 GCM 单独设
`HTTPS_PROXY`。当日实测：直连 `github.com:443` 超时，经 `10.66.10.12:1010` 返回 200，本机 git 配置当前用的也是 `.12`。
认证侧的另一个坑（`credential.useHttpPath` 导致键不匹配、同样表现为弹登录框）见 [DEV_ENV.md](../../ops/DEV_ENV.md) §1。

## 8. 工具落点

| 文件                           | 作用                                                                            | 状态 |
| ---------------------------- | ----------------------------------------------------------------------------- | --- |
| `deploy/accept.sh`           | §5 可执行版，PASS/FAIL/SKIP + 退出码                                                  | **已建并实跑**（preflight / A / B / D0；D1/D2/E/F/C/G 未实现，缺则报 SKIP） |
| `tools/iter.sh`              | 单轮编排：门禁 → 取包 → 回滚协同 → 装 → 验收 → 报告                                        | **已建**；gate/fetch/run 三档实跑过，`rollback-wait` 未实测；`fetch` 只从 Actions artifact 取包（无 Release 模式），交付验证取包按 §4 第 4 步走 Release 资产 |
| `tools/linux-check.sh`       | 本地 `cargo check --target x86_64-unknown-linux-gnu`，挡 `cfg(unix)` 编译错          | **已建并实跑**：全量（含 `plugins/` 第二 workspace）**50 s**；`--quick` 只查主 workspace |
| `.github/workflows/iter.yml` | 迭代本地快包：只编 musl x86_64、`--skip-plugins`、`cancel-in-progress: true`、artifact 留 3 天 | **已首次 dispatch 实跑**：main@41beba5 / run 41616343492，出包 **118 s**、产物 9.4 MB；**不再作为交付验证来源** |

`release.yml` 已改为「测试期手动 dispatch、资产覆盖同一既有 Release、不新建 tag」（见
[CI.md](../../ops/CI.md) §4）；`iter.yml` 只负责本地快包，`cancel-in-progress: false` 仍留在
`release.yml` 上防止发出半个包。

**已定默认值**：rustc 1.98.1（与 `ci.yml` 一致）· target `x86_64-unknown-linux-musl` · 分支 `iter/<topic>` 稳定后 squash 进 `main` · 失败即停不重试 · 报告追加 `docs/changes/YYYY-MM-DD.md`。
