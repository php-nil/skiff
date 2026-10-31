# Skiff（轻舟）· 1Panel 商店包（受限形态）

极简 Linux 服务器管理面板。**这是可选形态，主形态是宿主机 systemd 直装。**

## 这个形态的能力边界（先读再装）

容器内运行意味着若干宿主级能力**无法默认兑付**，因此能力面被显式收缩：

| 能力 | 容器形态 | 说明 |
|---|---|---|
| `docker` | ✅ 可用 | 通过挂载的 `docker.sock`，仍受动词/对象/参数三层策略约束 |
| `identity` | ✅ 可用 | 登录、会话、TOTP、RBAC、审计 |
| `fs` | ❌ 不可用 | 面板容器看到的 `/data` 不是 Engine 宿主路径，绑定挂载无法安全校验 |
| `net` | ❌ 不可用 | 容器内通常没有 nftables 权限，出网收口无法兑付 |

由此带来两条硬规则：

1. **宿主绑定挂载一律拒绝**（403）。容器创建只能使用 named volume。
2. 需要 `fs`/`net` 能力的插件**装前拒装**；已装插件若声明 `requires_full_sandbox`，需要显式打开
   `allow_reduced_sandbox` 才能跑，UI 会标为 reduced sandbox。

装之前请确认你能接受这两条；不能接受就用 systemd 直装。

## 端口与访问

- 默认映射 `18080`，安装时可在表单里改。
- 面板容器监听 `0.0.0.0:18080`（容器网络内），TLS 请由 **1Panel 的网站反向代理**终止并开启 HTTPS。
- 因此表单里有两项必须成对配置：`SKIFF_ALLOW_INSECURE_HTTP=1` 与 `SKIFF_TRUSTED_PROXIES`（可信反代网段）。
  只开明文而不给白名单，内核会**拒绝启动**（这是刻意的：远程明文管理面不可接受）。

## 安装后

```sh
# 初始管理员口令
docker exec skiff cat /data/initial-admin.txt

# 安全入口前缀（面板只在随机前缀下响应）
docker logs skiff 2>&1 | grep 入口
```

首次登录会强制改密，改密成功后 `initial-admin.txt` 自动删除。

## 数据

- 全部状态在 `./data`（1Panel 映射到应用专属目录）：`skiff.db`、`plugins/`、`backup/`。
- 升级前会自动在 `data/backup/` 留一份 `skiff.db` 快照（保留 3 份）。这是**升级回滚用**的快照，
  不等于 §5.2 要求的备份恢复演练。
- 卸载默认保留数据。

## 已知限制

- landlock / cgroups v2 限额 / nftables 出网收口 **在 MVP 未实现**（见仓库 `docs/sections/12-pending-tests.md`）。
- 插件沙箱在容器形态下为 reduced：独立 UID 有，但资源限额与出网控制没有。
- `docker.sock` 等价宿主 root —— 装这个面板就是授予其宿主 root 等价能力。三层策略约束的是
  「面板与插件能看见并操作什么」，不改变 socket 本身的权限本质。

## 链接

- 项目主页：https://example.com/skiff
- 文档：https://example.com/skiff/docs
- 仓库：https://github.com/example/skiff
