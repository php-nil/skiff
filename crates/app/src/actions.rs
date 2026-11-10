//! 动作注册表：**动作 → 资源 → 权限 → 领域命令** 的唯一声明处。
//!
//! 为什么需要它：旧 UI 里「哪个角色能做什么」散落在 `require_admin`、`policy.check`、
//! `AdminUser` 提取器和 `user.can` 四处；SDUI 的动作端点还需要同一份答案来生成
//! 「可见动作」与执行前校验。把它们收成一张表后：
//!
//! - 新增写路径只在这里登记一次，权限与命令名同时确定；
//! - `crates/web` 的 `authz::authorize` 直接查表，不再各自手写角色判断；
//! - 一致性由单测守护：动作 id 唯一、Docker 动作与 `Verb::permission` 对齐、
//!   动作 × 角色矩阵与 `RoleExt::can` 一致。
//!
//! 这里**只有声明**：不做 I/O、不调用命令、不渲染。`command` 是领域命令的稳定
//! 名称（`模块::函数`），供后续动作端点分发与审计映射使用，不参与编译期绑定。

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
    /// 稳定 id；与 `action.id()` 相同，落表是为了让声明本身可直接序列化。
    pub id: &'static str,
    pub resource: Resource,
    /// `None` 表示「只需登录」，用于用户级偏好等无特权动作。
    pub permission: Option<Permission>,
    /// 领域命令的稳定名称（`模块::函数`），不参与编译期绑定。
    pub command: &'static str,
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
    ($( $name:ident = $id:literal : $resource:ident, $permission:expr, $command:literal, $dangerous:literal ; )*) => {
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
                    command: $command,
                    dangerous: $dangerous,
                },
            )*
        ];
    };
}

action_table! {
    // ---- 站点（Admin：建站、改配置、域名、SSL、备份、删除）----
    SITE_CREATE = "site.create": Site, Some(Permission::Admin), "sites::create", false;
    SITE_BATCH_CREATE = "site.batch_create": Site, Some(Permission::Admin), "sites::batch", false;
    SITE_SAVE_CONFIG = "site.save_config": Site, Some(Permission::Admin), "sites::save_config", false;
    SITE_DELETE = "site.delete": Site, Some(Permission::Admin), "sites::delete", true;
    SITE_ACTION = "site.action": Site, Some(Permission::Admin), "sites::action", false;
    SITE_SET_DEFAULT = "site.set_default": Site, Some(Permission::Admin), "sites::set_default", false;
    SITE_UPDATE_DOMAINS = "site.update_domains": Site, Some(Permission::Admin), "sites::domains", false;
    SITE_SSL = "site.ssl": Site, Some(Permission::Admin), "sites::ssl_action", false;
    SITE_SET_BACKUP = "site.set_backup": Site, Some(Permission::Admin), "sites::set_backup", false;

    // ---- 运行环境（Admin：安装、克隆、扩缩、配置、扩展、探测）----
    RUNTIME_CREATE = "runtime.create": Runtime, Some(Permission::Admin), "sites::env_add", false;
    RUNTIME_SAVE_IMAGES = "runtime.save_images": Runtime, Some(Permission::Admin), "sites::env_images_save", false;
    RUNTIME_SAVE_CONFIG = "runtime.save_config": Runtime, Some(Permission::Admin), "sites::runtime_config_save", false;
    RUNTIME_SAVE_NODE_CONFIG = "runtime.save_node_config": Runtime, Some(Permission::Admin), "sites::runtime_node_config_save", false;
    RUNTIME_SAVE_CONFIG_FILE = "runtime.save_config_file": Runtime, Some(Permission::Admin), "sites::runtime_config_file_save", false;
    RUNTIME_SAVE_PORT = "runtime.save_port": Runtime, Some(Permission::Admin), "sites::runtime_port_save", false;
    RUNTIME_CLONE = "runtime.clone": Runtime, Some(Permission::Admin), "sites::runtime_clone", false;
    RUNTIME_ACTION = "runtime.action": Runtime, Some(Permission::Admin), "sites::runtime_action", false;
    RUNTIME_SAVE_EXTENSIONS = "runtime.save_extensions": Runtime, Some(Permission::Admin), "sites::runtime_extensions_save", false;
    RUNTIME_INSTALL_EXTENSION = "runtime.install_extension": Runtime, Some(Permission::Admin), "sites::runtime_extension_install", false;
    RUNTIME_UNINSTALL_EXTENSION = "runtime.uninstall_extension": Runtime, Some(Permission::Admin), "sites::runtime_extension_uninstall", false;
    RUNTIME_BATCH_EXTENSIONS = "runtime.batch_extensions": Runtime, Some(Permission::Admin), "sites::runtime_extensions_batch", false;
    RUNTIME_INSTALL_PECL = "runtime.install_pecl": Runtime, Some(Permission::Admin), "sites::runtime_pecl_install", false;
    RUNTIME_PROBE = "runtime.probe": Runtime, Some(Permission::Admin), "sites::runtime_probe", false;
    RUNTIME_UNBIND_SITE = "runtime.unbind_site": Runtime, Some(Permission::Admin), "sites::runtime_unbind_site", false;

    // ---- 容器与 Docker 资源（权限来自 `Verb::permission`，由单测对齐）----
    CONTAINER_CREATE = "container.create": Container, Some(Permission::DockerLifecycle), "docker::execute", false;
    CONTAINER_UPDATE = "container.update": Container, Some(Permission::DockerLifecycle), "docker::execute", false;
    CONTAINER_START = "container.start": Container, Some(Permission::DockerOperate), "docker::execute", false;
    CONTAINER_STOP = "container.stop": Container, Some(Permission::DockerOperate), "docker::execute", false;
    CONTAINER_RESTART = "container.restart": Container, Some(Permission::DockerOperate), "docker::execute", false;
    CONTAINER_KILL = "container.kill": Container, Some(Permission::DockerOperate), "docker::execute", false;
    CONTAINER_REMOVE = "container.remove": Container, Some(Permission::DockerLifecycle), "docker::execute", true;
    CONTAINER_EXEC = "container.exec": Container, Some(Permission::DockerExec), "docker::execute", true;
    IMAGE_PULL = "image.pull": Image, Some(Permission::DockerLifecycle), "docker::execute", false;
    IMAGE_REMOVE = "image.remove": Image, Some(Permission::DockerLifecycle), "docker::execute", true;
    NETWORK_CREATE = "network.create": Network, Some(Permission::DockerLifecycle), "docker::execute", false;
    NETWORK_REMOVE = "network.remove": Network, Some(Permission::DockerLifecycle), "docker::execute", true;
    VOLUME_CREATE = "volume.create": Volume, Some(Permission::DockerLifecycle), "docker::execute", false;
    VOLUME_REMOVE = "volume.remove": Volume, Some(Permission::DockerLifecycle), "docker::execute", true;
    COMPOSE_UP = "compose.up": Compose, Some(Permission::DockerLifecycle), "docker::execute", false;
    COMPOSE_DOWN = "compose.down": Compose, Some(Permission::DockerLifecycle), "docker::execute", true;

    // ---- 文件管理（Admin：全部经 `skiff-fs` 管理员客户端）----
    FILE_BROWSE = "file.browse": File, Some(Permission::Admin), "admin_fs::list", false;
    FILE_READ = "file.read": File, Some(Permission::Admin), "admin_fs::read_chunk", false;
    FILE_WRITE = "file.write": File, Some(Permission::Admin), "admin_fs::write", false;
    FILE_MKDIR = "file.mkdir": File, Some(Permission::Admin), "admin_fs::mkdir", false;
    FILE_RENAME = "file.rename": File, Some(Permission::Admin), "admin_fs::rename", false;
    FILE_REMOVE = "file.remove": File, Some(Permission::Admin), "admin_fs::remove", true;
    FILE_COPY = "file.copy": File, Some(Permission::Admin), "admin_fs::copy", false;
    FILE_CHMOD = "file.chmod": File, Some(Permission::Admin), "admin_fs::chmod", false;
    FILE_CHOWN = "file.chown": File, Some(Permission::Admin), "admin_fs::chown", false;
    FILE_ARCHIVE = "file.archive": File, Some(Permission::Admin), "admin_fs::archive", false;
    FILE_EXTRACT = "file.extract": File, Some(Permission::Admin), "admin_fs::extract", true;
    FILE_UPLOAD = "file.upload": File, Some(Permission::Admin), "admin_fs::upload", false;
    FILE_TRASH_READ = "file.trash_read": File, Some(Permission::Admin), "admin_fs::trash_list", false;
    FILE_TRASH_RESTORE = "file.trash_restore": File, Some(Permission::Admin), "admin_fs::trash_restore", false;
    FILE_TRASH_PURGE = "file.trash_purge": File, Some(Permission::Admin), "admin_fs::trash_purge", true;
    FILE_TRASH_CLEAR = "file.trash_clear": File, Some(Permission::Admin), "admin_fs::trash_clear", true;
    FILE_QUOTA_READ = "file.quota_read": File, Some(Permission::Admin), "admin_fs::quota_list", false;
    FILE_QUOTA_SET = "file.quota_set": File, Some(Permission::Admin), "admin_fs::quota_set", false;
    FILE_QUOTA_CANCEL = "file.quota_cancel": File, Some(Permission::Admin), "admin_fs::quota_cancel", false;
    FILE_QUOTA_SCAN = "file.quota_scan": File, Some(Permission::Admin), "admin_fs::quota_scan", false;

    // ---- 定时任务（Admin：启停、立即执行、删除）----
    TASK_TOGGLE = "task.toggle": Task, Some(Permission::Admin), "tasks::toggle", false;
    TASK_RUN = "task.run": Task, Some(Permission::Admin), "tasks::run", false;
    TASK_DELETE = "task.delete": Task, Some(Permission::Admin), "tasks::delete", true;

    // ---- 插件（PluginManage：安装、确认、启停、移除、RPC）----
    PLUGIN_INSTALL = "plugin.install": Plugin, Some(Permission::PluginManage), "plugin::install", false;
    PLUGIN_CONFIRM = "plugin.confirm": Plugin, Some(Permission::PluginManage), "plugin::confirm", false;
    PLUGIN_TOGGLE = "plugin.toggle": Plugin, Some(Permission::PluginManage), "plugin::toggle", false;
    PLUGIN_REMOVE = "plugin.remove": Plugin, Some(Permission::PluginManage), "plugin::uninstall", true;
    PLUGIN_RPC = "plugin.rpc": Plugin, Some(Permission::PluginManage), "plugin::rpc", false;

    // ---- 设置（Admin；菜单偏好属用户级，不登记为特权动作）----
    SETTINGS_PANEL_UPDATE = "settings.panel_update": Settings, Some(Permission::Admin), "panel_settings::update", false;
    SETTINGS_KEY_IMPORT = "settings.key_import": Settings, Some(Permission::Admin), "panel_settings::import_key", false;
    SETTINGS_KEY_REMOVE = "settings.key_remove": Settings, Some(Permission::Admin), "panel_settings::remove_key", true;
    SETTINGS_DOCKER_DAEMON_SAVE = "settings.docker_daemon_save": Settings, Some(Permission::Admin), "docker_daemon::save", true;

    // ---- 网站运行环境（nginx 主配置/参数）----
    WEB_CONFIG_MAIN_SAVE = "web.config_main_save": Web, Some(Permission::Admin), "nginx::save_main", false;
    WEB_CONFIG_PARAMS_SAVE = "web.config_params_save": Web, Some(Permission::Admin), "nginx::save_params", false;

    // ---- 作业（Admin：取消、重试、读日志）----
    JOB_CANCEL = "job.cancel": Job, Some(Permission::Admin), "jobs::cancel", false;
    JOB_RETRY = "job.retry": Job, Some(Permission::Admin), "jobs::retry", false;
    JOB_LOGS = "job.logs": Job, Some(Permission::Admin), "jobs::logs", false;
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
    fn every_action_has_a_command_and_lookup_roundtrips() {
        for entry in ACTIONS {
            assert!(!entry.command.is_empty(), "{} 缺少领域命令", entry.id);
            assert!(
                entry.command.contains("::"),
                "{} 的命令应写成 模块::函数：{}",
                entry.id,
                entry.command
            );
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
