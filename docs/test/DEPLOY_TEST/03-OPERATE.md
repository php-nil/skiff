# 升级、回滚、卸载、运维命令与权限自检（§2.5–2.10）

> 上级索引：[DEPLOY_TEST.md](../DEPLOY_TEST.md) ｜ 拆自 `docs/test/DEPLOY_TEST.md` 原 §2.5–2.10（2026-11-16 整理，正文未改）。

---

### 2.5 升级

```sh
systemctl stop skiff     # 停机备份最稳；不停也能备，但插件 data/ 可能被写一半（§6.6 F1）
skiff backup
sh install.sh --no-systemd      # 只换二进制
systemctl start skiff
systemctl status skiff --no-pager
```

数据目录不动；SQLite 迁移幂等、向前，启动自动执行并写 `PRAGMA user_version`。若库的 schema 版本高于二进制支持版本，启动会拒绝并报 `SchemaTooNew`——这是防降级写坏数据，不是 bug。

**官方插件随升级同步**：`install.sh` 更新 `/www/skiff/official/` 后，`skiff serve` 在启动早期
遍历该目录，只对**库内已安装**的插件做比对：包版本更高，或同版本但版本目录内容（文件集合 +
SHA-256）与包不一致时，按普通重装路径安装。`data/` 不覆盖，管理员既有授权原样沿用，
按新 manifest 补齐新增声明的权限与对象 hint（只增不减；敏感动词与任何新增对象授权写
`plugin.grant_upgrade` 审计，记录执行者、来源与版本），已停用插件保持停用；
未安装的插件仍不自动安装（首次安装必须逐条确认权限）。
单个包失败只写 `[warn]` 并继续启动，日志会打印可人工重试的 `skiff plugin install <包>` 命令。

### 2.6 回滚

```sh
systemctl stop skiff
# 1) 换回旧二进制
install -m 0755 skiff-<旧版本>/skiff /usr/local/bin/skiff
# 2) 数据库若已被新版本迁移过，需要回滚备份（备份里带版本与校验和）
ls /www/skiff/backup/
# 3) 用备份替换（停机状态下操作，禁止对活动库直接覆盖）
cp /www/skiff/backup/skiff-<ts>.db /www/skiff/skiff.db
rm -f /www/skiff/skiff.db-wal /www/skiff/skiff.db-shm
systemctl start skiff
```

**注意**：SQLite 迁移只向前，不提供降级脚本。跨 schema 版本回滚必须走备份；备份恢复演练属 M5，**尚未实测**（§8）。

### 2.7 卸载

```sh
sh uninstall.sh            # 保留 /www/skiff 与 /etc/skiff
sh uninstall.sh --purge    # 删程序 + 数据 + 配置（需手输 YES 确认）
```

插件 UID 不是系统用户，`--purge` 时按 200000–200999 扫描 passwd 清理（一般扫不到，属正常）。
**复测建议**：`sh uninstall.sh --purge` 后再从 §2.2 装一次，能验证「干净机器首启」这条路。

### 2.8 常用运维命令

| 命令 | 用途 | 退出码 |
|---|---|---|
| `skiff health` | 探本机 `/healthz` | 0 健康 / 非 0 异常 |
| `skiff doctor` | 只读检查数据目录/配置/TLS/socket | 0 |
| `skiff version` | 版本与架构 | 0 |
| `skiff audit 50` | 最近 50 条审计 | 0 |
| `skiff backup [--out <path>]` | `VACUUM INTO` + 复制插件 data，保留 3 份 | 0 |
| `skiff token create ci` | 建 API Token（仅创建时打印一次） | 0 |
| `skiff plugin list` | 列出已装插件 | 0 |
| `skiff plugin install <pkg> --yes [--grant <perm>] [--resource <obj>]` | 非交互安装；默认采用 manifest 声明的权限与 `resource_hints`（官方包「装完即可用」），显式 `--grant`/`--resource` 覆盖为最小授权 | 0 |
| `skiff plugin remove <id> [--delete-data]` | 卸载，默认保留数据 | 0 |
| `skiff plugin config <id>` | 列出该插件 `config_schema` 声明项与当前值（secret 打码） | 0 |
| `skiff plugin config <id> <key> <value>` | 写入一项配置；是否 secret 由 manifest 决定，键必须在 `config_schema` 内 | 0 |
| `skiff-plugin keygen/new/build/pack/verify` | 插件打包与验签 | 0 |

### 2.9 插件测试物准备（E 组前置）

E 组要装真包，得先造一把测试密钥和一个 demo 插件。**都在目标机跑**（或在构建机跑完把 `.wbpkg` 拷过来）：

```sh
# 1) 生成插件骨架。`new` 会自己生成 <dir>/skiff-signing.key，manifest.key_id 就是它的指纹
skiff-plugin new demo --id com.example.demo --name Demo --sdk-path /root/skiff/crates/sdk

# 2) 编译（产物落到 demo/bin/<arch>/plugin）
skiff-plugin build --dir demo

# 3) 打包。--key 必须与 manifest.key_id 配对，用别的私钥会直接报「指纹不一致」
skiff-plugin pack --dir demo --key demo/skiff-signing.key --out /root/demo-0.1.0.wbpkg

# 4) 装前自查（与内核安装时同一套校验）
skiff-plugin verify /root/demo-0.1.0.wbpkg --pubkey <公钥hex>
```

几个必须知道的点：

| 点 | 说明 |
|---|---|
| SDK 未发布 | `new` 默认找当前目录下的 `crates/sdk`；不在仓库里跑就必须传 `--sdk-path`，否则 Cargo.toml 里的 `skiff-sdk = "0.1"` 解析不到 |
| 公钥从哪来 | `new` 和 `pack` 都会打印`公钥(hex)`与指纹；装包前要在面板「设置 → 信任公钥」导入，未导入=拒装 |
| 包内有什么 | 只有 `bin/ ui/ migrations/` 与 `manifest.toml`；`target/`、`Cargo.toml`、私钥**不进包**（实测 28.9MB → 461KB） |
| 架构 | `build` 按宿主架构产出 `bin/<arch>/plugin`；目标机是 arm64 就在 arm64 机器上 build |
| 目标机没 Rust | 在构建机跑完 `pack`，把 `.wbpkg` 拷到目标机，只做第 4 步验签 |

导入公钥后装包：

```sh
skiff plugin install /root/demo-0.1.0.wbpkg --yes \
  --grant docker:containers.list \
  --grant docker:containers.inspect \
  --grant docker:containers.logs
skiff plugin list
```

**官方插件（随包携带，E 组推荐先用它们冒烟）**——发布包 `official/` 里是 7 个已签名的包 = 6 个官方插件（guard/monitor/files/cron/ftp/firewall）+ `com.skiff.testkit` 架构测试台（原 nginx 插件已于 2026-11-04 内核化为「网站」一级功能，不再随包），装机时已复制到 `/www/skiff/official/`：

```sh
ls /www/skiff/official            # com.skiff.{guard,monitor,cron,firewall,ftp,testkit}-0.1.0.wbpkg + com.skiff.files-0.1.3.wbpkg + public-key.txt
# public-key.txt 不需要手动导入：首次启动会自动写入信任库（见 §2.11 注）
# 仅在公钥缺失/指纹不符时，才需显式执行：
#   skiff trust import /www/skiff/official/public-key.txt --label official

# 主机监控：只读 /proc
skiff plugin install /www/skiff/official/com.skiff.monitor-0.1.0.wbpkg --yes

# 容器守护：manifest 已声明权限；容器对象按需显式追加
skiff plugin install /www/skiff/official/com.skiff.guard-0.1.0.wbpkg --yes \
  --resource container:nginx

# 文件管理：默认授权 path:/www（resource_hints 预勾选），装完即可浏览；
# 想缩小范围就显式覆盖，例如 --resource path:/var/www
skiff plugin install /www/skiff/official/com.skiff.files-0.1.3.wbpkg --yes

# 计划任务：manifest 已声明 scheduler:read/manage/run/admin 与 interop:call；
# 要调度别的插件再追加 --resource plugin:<id>
skiff plugin install /www/skiff/official/com.skiff.cron-0.1.0.wbpkg --yes \
  --resource plugin:*

# 架构测试台（多视图 / 槽位 / RPC 成功失败 / 越界 / 互操作）
skiff plugin install /www/skiff/official/com.skiff.testkit-0.1.0.wbpkg --yes
```

> `--yes` 不再「权限默认全拒」，而是采用 manifest 声明 + `resource_hints`；显式 `--grant`/`--resource` 会把该维度收窄到只给列出的项（少一项即运行时 403，插件会把「该去勾哪个授权」写进错误文案）。确认页同样默认勾选这些声明值，取消勾选即按最小授权安装。**重装/升级按同一插件 id 沿用既有授权**（不重置为 manifest 默认值、manifest 新增声明不自动授予）；**只有显式卸载才清空授权**，重装后无需重新授权。

### 2.10 权限链自检（装完必查）

装完不查权限，插件测试会以「EACCES」「连不上」的形式随机失败，且报错发生在插件侧，很难反推。逐条跑：

```sh
# 目录与 socket
ls -ld /www/skiff /www/skiff/plugins /run/skiff
stat -c '%a %U:%G %n' /run/skiff/kernel.sock
# 期望：drwx--x--x root root …；660 root skiff-plugins

# 插件装好后，逐段看路径可达性
namei -l /www/skiff/plugins/com.example.demo/data
# 代码目录与二进制必须可穿越/可执行（缺这一条 = 降权进程 Permission denied）
ls -ld /www/skiff/plugins/com.example.demo/versions/*/
ls -l /www/skiff/plugins/com.example.demo/versions/*/bin/*/plugin
# 期望：目录 755 root:root；二进制至少 owner 有 x 位（修后为 755）

# 插件跑起来后看它的真实身份（不是 passwd 用户，是哈希 UID）
pid=$(pgrep -f 'plugins/com.example.demo/versions' | head -1)
grep -E '^(Uid|Gid|Groups)' /proc/$pid/status
# 期望：Uid/Gid 都是 200xxx；Groups 里含 skiff-plugins 的 gid
getent group skiff-plugins | cut -d: -f3

# 主进程仍是 root（它要能 setuid）
ps -o user,group,args -p $(systemctl show -p MainPID --value skiff)
```
