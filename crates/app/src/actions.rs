//! 动作注册表：**动作 → 资源 → 权限** 的唯一声明处。
//!
//! 为什么需要它：旧 UI 里「哪个角色能做什么」散落在 `require_admin`、`policy.check`、
//! `AdminUser` 提取器和 `user.can` 四处；SDUI 的动作端点还需要同一份答案来生成
//! 「可见动作」与执行前校验。把它们收成一张表后：
//!
//! - 新增写路径只在这里登记一次，资源与权限同时确定；
//! - `crates/web` 的 `authz::authorize` 直接查表，不再各自手写角色判断；
//! - 一致性由单测守护：动作 id 唯一、Docker 动作与 `Verb::permission` 对齐、
//!   动作 × 角色矩阵与 `RoleExt::can` 一致。
//!
//! 这里**只有声明**：不做 I/O、不调用命令、不渲染。**不登记「哪条命令服务这个动作」**：
//! 设置 / Web / 作业三组的实现在 handler 里、没有命令层，硬填一列字符串只会变成假声明
//! 与两张自说自话的清单；真要做按名字分发时，按资源组做「类型 → 函数」的编译期绑定。
//!
//! **勿按「死代码」处理**：容器/镜像/网络/卷/Compose（16 条）与插件（5 条）
//! 目前**运行期没有调用方**——容器动作走 `policy.check` + `Verb`，插件写路径走
//! `PluginManagerUser` 提取器。它们留在表里是为了让「动作 × 权限」与
//! `Verb::permission` 的交叉校验继续成立；新增 Docker/插件动作仍应先在此登记，
//! 再补对应的 `Verb`/提取器映射与单测。

use std::fmt;

use skiff_model::{Permission, Role, RoleExt};

/// 动作所属资源。与 SDUI 资源目录的 `resource` 字段一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Resource {
    Site,
    Runtime,
    Container,
    Image,
    Network,
    Volume,
    Compose,
    File,
    Task,
    Plugin,
    Settings,
    Web,
    Job,
}

impl Resource {
    pub fn as_str(self) -> &'static str {
        match self {
            Resource::Site => "site",
            Resource::Runtime => "runtime",
            Resource::Container => "container",
            Resource::Image => "image",
            Resource::Network => "network",
            Resource::Volume => "volume",
            Resource::Compose => "compose",
            Resource::File => "file",
            Resource::Task => "task",
            Resource::Plugin => "plugin",
            Resource::Settings => "settings",
            Resource::Web => "web",
            Resource::Job => "job",
        }
    }
}

/// 动作标识。只能由本模块的常量构造，避免手写字符串漂移。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Action(&'static str);

impl Action {
    const fn new(id: &'static str) -> Self {
        Self(id)
    }

    /// 稳定动作 id（`<resource>.<verb>`）。
    pub fn id(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// 一个动作的完整声明。
#[derive(Debug, Clone, Copy)]
pub struct ActionSpec {
    pub action: Action,
    /// 稳定 id；与 `action.id()` 相同，落表让声明自身可读、可直接打印。
    pub id: &'static str,
    pub resource: Resource,
    /// `None` 表示「只需登录」，用于用户级偏好等无特权动作。
    pub permission: Option<Permission>,
    /// 危险或不可逆动作：供确认流程与审计分级使用，不替代后端校验。
    pub dangerous: bool,
}

impl ActionSpec {
    /// 角色能否发起该动作。唯一口径来自 `skiff_model` 的角色→权限矩阵。
    pub fn allows(&self, role: Role) -> bool {
        match self.permission {
            Some(perm) => role.can(perm),
            None => true,
        }
    }
}

macro_rules! action_table {
    ($( $name:ident = $id:literal : $resource:ident, $permission:expr, $dangerous:literal ; )*) => {
        $(
            pub const $name: Action = Action::new($id);
        )*

        /// 全部动作。新增动作必须同时进这张表，由单测保证唯一与完整性。
        pub const ACTIONS: &[ActionSpec] = &[
            $(
                ActionSpec {
                    action: $name,
                    id: $id,
                    resource: Resource::$resource,
                    permission: $permission,
                    dangerous: $dangerous,
                },
            )*
        ];
    };
}

action_table! {
    // ---- 站点（Admin：建站、改配置、域名、SSL、备份、删除）----
    SITE_CREATE = "site.create": Site, Some(Permission::Admin), false;
    SITE_BATCH_CREATE = "site.batch_create": Site, Some(Permission::Admin), false;
    SITE_SAVE_CONFIG = "site.save_config": Site, Some(Permission::Admin), false;
    SITE_DELETE = "site.delete": Site, Some(Permission::Admin), true;
    SITE_ACTION = "site.action": Site, Some(Permission::Admin), false;
    SITE_SET_DEFAULT = "site.set_default": Site, Some(Permission::Admin), false;
    SITE_UPDATE_DOMAINS = "site.update_domains": Site, Some(Permission::Admin), false;
    SITE_SSL = "site.ssl": Site, Some(Permission::Admin), false;
    SITE_SET_BACKUP = "site.set_backup": Site, Some(Permission::Admin), false;
    // 恢复会覆盖站点目录，与删除同档：破坏性动作必须在动作表里显式标记。
    SITE_RESTORE = "site.restore": Site, Some(Permission::Admin), true;
    SITE_SET_POLICY = "site.set_policy": Site, Some(Permission::Admin), false;

    // ---- 运行环境（Admin：安装、克隆、扩缩、配置、扩展、探测）----
    RUNTIME_CREATE = "runtime.create": Runtime, Some(Permission::Admin), false;
    RUNTIME_SAVE_IMAGES = "runtime.save_images": Runtime, Some(Permission::Admin), false;
    RUNTIME_SAVE_CONFIG = "runtime.save_config": Runtime, Some(Permission::Admin), false;
    RUNTIME_SAVE_NODE_CONFIG = "runtime.save_node_config": Runtime, Some(Permission::Admin), false;
    RUNTIME_SAVE_CONFIG_FILE = "runtime.save_config_file": Runtime, Some(Permission::Admin), false;
    RUNTIME_SAVE_PORT = "runtime.save_port": Runtime, Some(Permission::Admin), false;
    RUNTIME_SAVE_PUBLISH = "runtime.save_publish": Runtime, Some(Permission::Admin), false;
    RUNTIME_CLONE = "runtime.clone": Runtime, Some(Permission::Admin), false;
    RUNTIME_ACTION = "runtime.action": Runtime, Some(Permission::Admin), false;
    RUNTIME_SAVE_EXTENSIONS = "runtime.save_extensions": Runtime, Some(Permission::Admin), false;
    RUNTIME_INSTALL_EXTENSION = "runtime.install_extension": Runtime, Some(Permission::Admin), false;
    RUNTIME_UNINSTALL_EXTENSION = "runtime.uninstall_extension": Runtime, Some(Permission::Admin), false;
    RUNTIME_BATCH_EXTENSIONS = "runtime.batch_extensions": Runtime, Some(Permission::Admin), false;
    RUNTIME_INSTALL_PECL = "runtime.install_pecl": Runtime, Some(Permission::Admin), false;
    RUNTIME_PROBE = "runtime.probe": Runtime, Some(Permission::Admin), false;
    RUNTIME_UNBIND_SITE = "runtime.unbind_site": Runtime, Some(Permission::Admin), false;

    // ---- 容器与 Docker 资源（权限来自 `Verb::permission`，由单测对齐）----
    CONTAINER_CREATE = "container.create": Container, Some(Permission::DockerLifecycle), false;
    CONTAINER_UPDATE = "container.update": Container, Some(Permission::DockerLifecycle), false;
    CONTAINER_START = "container.start": Container, Some(Permission::DockerOperate), false;
    CONTAINER_STOP = "container.stop": Container, Some(Permission::DockerOperate), false;
    CONTAINER_RESTART = "container.restart": Container, Some(Permission::DockerOperate), false;
    CONTAINER_KILL = "container.kill": Container, Some(Permission::DockerOperate), false;
    CONTAINER_REMOVE = "container.remove": Container, Some(Permission::DockerLifecycle), true;
    // exec 是 WS 流式通道，持票据直连 hijack，不经命令层。
    CONTAINER_EXEC = "container.exec": Container, Some(Permission::DockerExec), true;
    IMAGE_PULL = "image.pull": Image, Some(Permission::DockerLifecycle), false;
    IMAGE_REMOVE = "image.remove": Image, Some(Permission::DockerLifecycle), true;
    NETWORK_CREATE = "network.create": Network, Some(Permission::DockerLifecycle), false;
    NETWORK_REMOVE = "network.remove": Network, Some(Permission::DockerLifecycle), true;
    VOLUME_CREATE = "volume.create": Volume, Some(Permission::DockerLifecycle), false;
    VOLUME_REMOVE = "volume.remove": Volume, Some(Permission::DockerLifecycle), true;
    COMPOSE_UP = "compose.up": Compose, Some(Permission::DockerLifecycle), false;
    COMPOSE_DOWN = "compose.down": Compose, Some(Permission::DockerLifecycle), true;

    // ---- 文件管理（Admin：全部经 `skiff-fs` 管理员客户端，无独立命令层）----
    FILE_BROWSE = "file.browse": File, Some(Permission::Admin), false;
    FILE_READ = "file.read": File, Some(Permission::Admin), false;
    FILE_WRITE = "file.write": File, Some(Permission::Admin), false;
    FILE_MKDIR = "file.mkdir": File, Some(Permission::Admin), false;
    FILE_RENAME = "file.rename": File, Some(Permission::Admin), false;
    FILE_REMOVE = "file.remove": File, Some(Permission::Admin), true;
    FILE_COPY = "file.copy": File, Some(Permission::Admin), false;
    FILE_CHMOD = "file.chmod": File, Some(Permission::Admin), false;
    FILE_CHOWN = "file.chown": File, Some(Permission::Admin), false;
    FILE_ARCHIVE = "file.archive": File, Some(Permission::Admin), false;
    FILE_EXTRACT = "file.extract": File, Some(Permission::Admin), true;
    FILE_UPLOAD = "file.upload": File, Some(Permission::Admin), false;
    FILE_TRASH_READ = "file.trash_read": File, Some(Permission::Admin), false;
    FILE_TRASH_RESTORE = "file.trash_restore": File, Some(Permission::Admin), false;
    FILE_TRASH_PURGE = "file.trash_purge": File, Some(Permission::Admin), true;
    FILE_TRASH_CLEAR = "file.trash_clear": File, Some(Permission::Admin), true;
    FILE_QUOTA_READ = "file.quota_read": File, Some(Permission::Admin), false;
    FILE_QUOTA_SET = "file.quota_set": File, Some(Permission::Admin), false;
    FILE_QUOTA_CANCEL = "file.quota_cancel": File, Some(Permission::Admin), false;
    FILE_QUOTA_SCAN = "file.quota_scan": File, Some(Permission::Admin), false;

    // ---- 定时任务（Admin：启停、立即执行、删除）----
    TASK_TOGGLE = "task.toggle": Task, Some(Permission::Admin), false;
    TASK_RUN = "task.run": Task, Some(Permission::Admin), false;
    TASK_DELETE = "task.delete": Task, Some(Permission::Admin), true;

    // ---- 插件（PluginManage：安装、确认、启停、移除、RPC）----
    PLUGIN_INSTALL = "plugin.install": Plugin, Some(Permission::PluginManage), false;
    PLUGIN_CONFIRM = "plugin.confirm": Plugin, Some(Permission::PluginManage), false;
    PLUGIN_TOGGLE = "plugin.toggle": Plugin, Some(Permission::PluginManage), false;
    PLUGIN_REMOVE = "plugin.remove": Plugin, Some(Permission::PluginManage), true;
    PLUGIN_RPC = "plugin.rpc": Plugin, Some(Permission::PluginManage), false;

    // ---- 设置（Admin；菜单偏好属用户级，不登记为特权动作）----
    SETTINGS_PANEL_UPDATE = "settings.panel_update": Settings, Some(Permission::Admin), false;
    SETTINGS_KEY_IMPORT = "settings.key_import": Settings, Some(Permission::Admin), false;
    SETTINGS_KEY_REMOVE = "settings.key_remove": Settings, Some(Permission::Admin), true;
    SETTINGS_DOCKER_DAEMON_SAVE = "settings.docker_daemon_save": Settings, Some(Permission::Admin), true;

    // ---- 网站运行环境（nginx 主配置/参数）----
    WEB_CONFIG_MAIN_SAVE = "web.config_main_save": Web, Some(Permission::Admin), false;
    WEB_CONFIG_PARAMS_SAVE = "web.config_params_save": Web, Some(Permission::Admin), false;

    // ---- 作业（Admin：取消、重试、读日志）----
    JOB_CANCEL = "job.cancel": Job, Some(Permission::Admin), false;
    JOB_RETRY = "job.retry": Job, Some(Permission::Admin), false;
    JOB_LOGS = "job.logs": Job, Some(Permission::Admin), false;
}

/// 查表。`Action` 只能由本模块常量构造，未登记属于编码错误。
pub fn spec(action: Action) -> &'static ActionSpec {
    ACTIONS
        .iter()
        .find(|spec| spec.action == action)
        .expect("动作未在 ACTIONS 登记")
}

/// 按稳定 id 查找；供 HTTP 边界解析动作字符串（未知名返回 `None`，不 panic）。
pub fn find(id: &str) -> Option<&'static ActionSpec> {
    ACTIONS.iter().find(|spec| spec.id == id)
}

/// 某资源下的全部动作，供未来的资源描述与可见动作计算使用。
pub fn for_resource(resource: Resource) -> impl Iterator<Item = &'static ActionSpec> {
    ACTIONS.iter().filter(move |spec| spec.resource == resource)
}

#[cfg(test)]
mod tests {
    use super::*;
    use skiff_model::Verb;
    use std::collections::BTreeSet;

    #[test]
    fn action_ids_are_unique_and_namespaced() {
        let mut seen = BTreeSet::new();
        for spec in ACTIONS {
            assert!(seen.insert(spec.id), "动作 id 重复: {}", spec.id);
            assert_eq!(spec.id, spec.action.id());
            let mut parts = spec.id.split('.');
            let resource = parts.next().unwrap_or_default();
            let verb = parts.next().unwrap_or_default();
            assert!(!resource.is_empty(), "动作缺少资源前缀: {}", spec.id);
            assert!(!verb.is_empty(), "动作缺少动词: {}", spec.id);
            assert!(parts.next().is_none(), "动作 id 只允许一个点: {}", spec.id);
            assert_eq!(
                resource.to_ascii_lowercase(),
                resource,
                "动作 id 必须小写: {}",
                spec.id
            );
        }
    }

    #[test]
    fn every_action_has_an_id_and_lookup_roundtrips() {
        for entry in ACTIONS {
            assert!(!entry.id.is_empty(), "{} 缺少动作 id", entry.id);
            assert_eq!(spec(entry.action).id, entry.id);
            assert_eq!(find(entry.id).map(|s| s.action), Some(entry.action));
        }
        assert!(find("unknown.action").is_none());
    }

    #[test]
    fn docker_actions_match_verb_permission() {
        let pairs = [
            (CONTAINER_CREATE, Verb::ContainerCreate),
            (CONTAINER_UPDATE, Verb::ContainerUpdate),
            (CONTAINER_START, Verb::Start),
            (CONTAINER_STOP, Verb::Stop),
            (CONTAINER_RESTART, Verb::Restart),
            (CONTAINER_KILL, Verb::Kill),
            (CONTAINER_REMOVE, Verb::ContainerRemove),
            (CONTAINER_EXEC, Verb::Exec),
            (IMAGE_PULL, Verb::ImagePull),
            (IMAGE_REMOVE, Verb::ImageRemove),
            (NETWORK_CREATE, Verb::NetworkCreate),
            (NETWORK_REMOVE, Verb::NetworkRemove),
            (VOLUME_CREATE, Verb::VolumeCreate),
            (VOLUME_REMOVE, Verb::VolumeRemove),
            (COMPOSE_UP, Verb::ComposeUp),
            (COMPOSE_DOWN, Verb::ComposeDown),
        ];
        for (action, verb) in pairs {
            assert_eq!(
                spec(action).permission,
                Some(verb.permission()),
                "{} 的权限与 {:?} 的动词分档不一致",
                action,
                verb
            );
        }
    }

    #[test]
    fn privileged_resources_use_the_expected_permission() {
        for spec in ACTIONS {
            let expected = match spec.resource {
                Resource::Site | Resource::Runtime | Resource::File | Resource::Task => {
                    Some(Permission::Admin)
                }
                Resource::Settings | Resource::Web | Resource::Job => Some(Permission::Admin),
                Resource::Plugin => Some(Permission::PluginManage),
                Resource::Container
                | Resource::Image
                | Resource::Network
                | Resource::Volume
                | Resource::Compose => None,
            };
            if let Some(expected) = expected {
                assert_eq!(
                    spec.permission,
                    Some(expected),
                    "{} 的权限口径与资源 {} 不一致",
                    spec.id,
                    spec.resource.as_str()
                );
            }
        }
    }

    #[test]
    fn action_role_matrix_matches_existing_rules() {
        // 与现有 handler 口径一致的抽样：admin 全通；operator 可操作容器但不可
        // 生命周期/exec；viewer 只读（本表未登记只读动作，故全部拒绝）。
        let cases = [
            (SITE_DELETE, Permission::Admin),
            (FILE_WRITE, Permission::Admin),
            (TASK_RUN, Permission::Admin),
            (SETTINGS_PANEL_UPDATE, Permission::Admin),
            (JOB_RETRY, Permission::Admin),
            (PLUGIN_INSTALL, Permission::PluginManage),
            (CONTAINER_START, Permission::DockerOperate),
            (CONTAINER_REMOVE, Permission::DockerLifecycle),
            (CONTAINER_EXEC, Permission::DockerExec),
        ];
        for (action, permission) in cases {
            let spec = spec(action);
            for role in [Role::Admin, Role::Operator, Role::Viewer] {
                assert_eq!(
                    spec.allows(role),
                    role.can(permission),
                    "{} 在 {:?} 下的判定与权限矩阵不一致",
                    action,
                    role
                );
            }
        }
    }

    #[test]
    fn every_action_is_reachable_by_at_least_one_role() {
        for spec in ACTIONS {
            assert!(
                [Role::Admin, Role::Operator, Role::Viewer]
                    .into_iter()
                    .any(|role| spec.allows(role)),
                "{} 没有任何角色可执行",
                spec.id
            );
        }
    }

    #[test]
    fn resources_are_stable_strings() {
        for resource in [
            Resource::Site,
            Resource::Runtime,
            Resource::Container,
            Resource::Image,
            Resource::Network,
            Resource::Volume,
            Resource::Compose,
            Resource::File,
            Resource::Task,
            Resource::Plugin,
            Resource::Settings,
            Resource::Web,
            Resource::Job,
        ] {
            assert!(!resource.as_str().is_empty());
            assert_eq!(resource.as_str(), resource.as_str().to_ascii_lowercase());
        }
    }

    #[test]
    fn actions_are_grouped_by_resource() {
        for spec in for_resource(Resource::File) {
            assert_eq!(spec.resource, Resource::File);
        }
        assert!(for_resource(Resource::Site).count() >= 5);
        assert!(for_resource(Resource::File).count() >= 10);
    }
}
