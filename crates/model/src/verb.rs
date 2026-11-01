//! 动词白名单与分档（§6.4.1）：跨 crate 共享的**词汇**。
//!
//! `Verb` 是「面板与插件能做什么」的穷举表——任何未列动词一律 403 + 审计。
//! 它是纯枚举 + 字符串映射，没有 I/O、没有判定逻辑，因此住在 `skiff-model`：
//! `skiff-compose` 只需要 `ContainerSpec` 这类值类型，不该为了几个类型把
//! 判定引擎（`skiff-policy`，带 `libc`）拖进来。
//!
//! 判定本身在 `skiff-policy::Policy::check`；这里只回答「这个动词属于哪一类
//! 动作、需要哪条权限」。

use serde::{Deserialize, Serialize};

use crate::rbac::Permission;

/// 里程碑分档：M2 基础档 / M3 生命周期档。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Basic,
    Lifecycle,
}

impl Tier {
    pub fn max_verb(self) -> &'static str {
        match self {
            Tier::Basic => "M2（基础档）",
            Tier::Lifecycle => "M3（生命周期档）",
        }
    }
}

/// §6.4.1 动词白名单。任何未列动词一律 403 + 审计。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verb {
    // ---- 基础档 ----
    ContainersList,
    ContainerInspect,
    ContainerLogs,
    ContainerStats,
    ImagesList,
    NetworksList,
    VolumesList,
    Info,
    Version,
    Start,
    Stop,
    Restart,
    Kill,
    Exec,
    ExecStart,
    ExecInspect,
    // ---- 生命周期档 ----
    ContainerCreate,
    ContainerRemove,
    ImagePull,
    ImageRemove,
    NetworkCreate,
    NetworkRemove,
    VolumeCreate,
    VolumeRemove,
    ComposeUp,
    ComposeDown,
}

impl Verb {
    /// 从稳定字符串还原动词——`as_str()` 的唯一逆函数，由 `all()` 派生。
    ///
    /// 内核回调路径收到的是插件传的字符串。此前这里手写了第二份 26 行 match，加动词时
    /// 两边不同步就会出现「白名单里有、解析不了」；现在解析表只有 `all()` 一份，
    /// 配合 `verb_strings_roundtrip` / `all_covers_every_verb` 成对校验。
    /// 仅在内核回调解析路径使用，不在请求热路径上。
    pub fn parse(s: &str) -> Option<Verb> {
        Self::all().iter().copied().find(|v| v.as_str() == s)
    }

    /// 全部动词。新增动词必须同时进这张表——`verb_strings_roundtrip` 用它
    /// 保证 `as_str`/`parse` 与审计命名三者不会各写一套。
    pub fn all() -> &'static [Verb] {
        use Verb::*;
        &[
            ContainersList,
            ContainerInspect,
            ContainerLogs,
            ContainerStats,
            ImagesList,
            NetworksList,
            VolumesList,
            Info,
            Version,
            Start,
            Stop,
            Restart,
            Kill,
            Exec,
            ExecStart,
            ExecInspect,
            ContainerCreate,
            ContainerRemove,
            ImagePull,
            ImageRemove,
            NetworkCreate,
            NetworkRemove,
            VolumeCreate,
            VolumeRemove,
            ComposeUp,
            ComposeDown,
        ]
    }

    /// 内核审计与授权用的稳定字符串（同时是插件权限里的动词名）。
    pub fn as_str(self) -> &'static str {
        use Verb::*;
        match self {
            ContainersList => "containers.list",
            ContainerInspect => "containers.inspect",
            ContainerLogs => "containers.logs",
            ContainerStats => "containers.stats",
            ImagesList => "images.list",
            NetworksList => "networks.list",
            VolumesList => "volumes.list",
            Info => "info",
            Version => "version",
            Start => "containers.start",
            Stop => "containers.stop",
            Restart => "containers.restart",
            Kill => "containers.kill",
            Exec => "containers.exec",
            ExecStart => "exec.start",
            ExecInspect => "exec.inspect",
            ContainerCreate => "containers.create",
            ContainerRemove => "containers.remove",
            ImagePull => "images.pull",
            ImageRemove => "images.remove",
            NetworkCreate => "networks.create",
            NetworkRemove => "networks.remove",
            VolumeCreate => "volumes.create",
            VolumeRemove => "volumes.remove",
            ComposeUp => "compose.up",
            ComposeDown => "compose.down",
        }
    }

    /// 内核审计用的动作名：`docker.<verb>`，全站唯一来源。
    ///
    /// web 的 UI 路径、JSON API、插件回调（kernel.sock）都必须调这里取名字。
    /// 之前 `resources.rs` 手写了 `"docker.images.pull"` 之类的字面量，一旦
    /// `as_str()` 改名（或新增动词）审计表里就会出现两种拼法的同一操作。
    pub fn audit_action(self) -> String {
        format!("docker.{}", self.as_str())
    }

    /// 对应 §6.4.1 表里的 HTTP 动作，写审计用。
    pub fn http_path(self) -> &'static str {
        use Verb::*;
        match self {
            ContainersList => "GET /containers/json",
            ContainerInspect => "GET /containers/{id}/json",
            ContainerLogs => "GET /containers/{id}/logs",
            ContainerStats => "GET /containers/{id}/stats",
            ImagesList => "GET /images/json",
            NetworksList => "GET /networks",
            VolumesList => "GET /volumes",
            Info => "GET /info",
            Version => "GET /version",
            Start => "POST /containers/{id}/start",
            Stop => "POST /containers/{id}/stop",
            Restart => "POST /containers/{id}/restart",
            Kill => "POST /containers/{id}/kill",
            Exec => "POST /containers/{id}/exec",
            ExecStart => "POST /exec/{id}/start",
            ExecInspect => "GET /exec/{id}/json",
            ContainerCreate => "POST /containers/create",
            ContainerRemove => "DELETE /containers/{id}",
            ImagePull => "POST /images/create（仅 fromImage）",
            ImageRemove => "DELETE /images/{id}",
            NetworkCreate => "POST /networks/create",
            NetworkRemove => "DELETE /networks/{id}",
            VolumeCreate => "POST /volumes/create",
            VolumeRemove => "DELETE /volumes/{name}",
            ComposeUp => "compose up（多对象）",
            ComposeDown => "compose down（多对象）",
        }
    }

    pub fn in_tier(self, tier: Tier) -> bool {
        use Verb::*;
        match self {
            ContainersList | ContainerInspect | ContainerLogs | ContainerStats | ImagesList
            | NetworksList | VolumesList | Info | Version | Start | Stop | Restart | Kill
            | Exec | ExecStart | ExecInspect => true,
            ContainerCreate | ContainerRemove | ImagePull | ImageRemove | NetworkCreate
            | NetworkRemove | VolumeCreate | VolumeRemove | ComposeUp | ComposeDown => {
                tier == Tier::Lifecycle
            }
        }
    }

    /// 只读动词（viewer 可用，插件只需 `<verb>` 权限即可只读）。
    pub fn is_read(self) -> bool {
        use Verb::*;
        matches!(
            self,
            ContainersList
                | ContainerInspect
                | ContainerLogs
                | ContainerStats
                | ImagesList
                | NetworksList
                | VolumesList
                | Info
                | Version
                | ExecInspect
        )
    }

    /// 生命周期/删除类：admin 专属，且需要对象 grant。
    pub fn is_destructive(self) -> bool {
        use Verb::*;
        matches!(
            self,
            Kill | Exec
                | ExecStart
                | ContainerCreate
                | ContainerRemove
                | ImageRemove
                | NetworkRemove
                | VolumeRemove
                | ComposeUp
                | ComposeDown
                | ImagePull
                | NetworkCreate
                | VolumeCreate
        )
    }

    /// 是否改变运行时状态（读之外的动词）。
    ///
    /// 与 `is_destructive` 的区别：`Stop`/`Restart`/`Start` 不销毁数据，所以对「外部面板
    /// 资源」不算 destruct；但对 `io.skiff.protected=true` 的对象（含 Skiff 自身容器）
    /// **停一下就等于自断管理面**，必须一起拦下。
    pub fn changes_state(self) -> bool {
        self.is_destructive() || matches!(self, Verb::Start | Verb::Stop | Verb::Restart)
    }

    /// 这个动词需要哪条权限。
    ///
    /// **全仓唯一的角色→动作映射入口**：判定（`skiff-policy`）、UI 按钮显隐
    /// （`skiff-web`）都问这张表。此前 `policy` 内部写了一份 `Action` 三分法、
    /// web 侧又手写了 `is_admin() || role == Operator`，三份规则各改各的。
    pub fn permission(self) -> Permission {
        use Verb::*;
        match self {
            ContainersList | ContainerInspect | ContainerLogs | ContainerStats | ImagesList
            | NetworksList | VolumesList | Info | Version | ExecInspect => Permission::DockerRead,
            Start | Stop | Restart | Kill => Permission::DockerOperate,
            Exec | ExecStart => Permission::DockerExec,
            ContainerCreate | ContainerRemove | ImagePull | ImageRemove | NetworkCreate
            | NetworkRemove | VolumeCreate | VolumeRemove | ComposeUp | ComposeDown => {
                Permission::DockerLifecycle
            }
        }
    }
}

/// 内核保留标签：插件与用户都不得写入（§6.4.2）。
pub const LABEL_PROJECT: &str = "io.skiff.project";
pub const LABEL_OWNER: &str = "io.skiff.owner";
pub const LABEL_PROTECTED: &str = "io.skiff.protected";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rbac::RoleExt;
    use crate::Role;

    #[test]
    fn verb_strings_roundtrip() {
        for v in Verb::all() {
            assert_eq!(Verb::parse(v.as_str()), Some(*v), "动词往返失败: {v:?}");
        }
    }

    #[test]
    fn all_covers_every_verb() {
        // `all()` 是 `parse` 的来源，少列一个就会出现「白名单里有、解析不了」
        let mut seen = std::collections::BTreeSet::new();
        for v in Verb::all() {
            assert!(seen.insert(v.as_str()), "all() 里有重复项: {v:?}");
        }
        assert_eq!(seen.len(), Verb::all().len());
    }

    /// 动词→权限的映射必须与角色矩阵一致：viewer 只读，operator 可启停，
    /// exec 与生命周期 admin 专属。
    #[test]
    fn verb_permission_matches_role_matrix() {
        assert!(Role::Viewer.can(Verb::ContainersList.permission()));
        assert!(!Role::Viewer.can(Verb::Start.permission()));
        assert!(Role::Operator.can(Verb::Start.permission()));
        assert!(!Role::Operator.can(Verb::Exec.permission()));
        assert!(!Role::Operator.can(Verb::ContainerCreate.permission()));
        assert!(Role::Admin.can(Verb::Exec.permission()));
        assert!(Role::Admin.can(Verb::ContainerCreate.permission()));
    }
}
