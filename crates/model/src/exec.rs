//! 宿主命令执行的词汇：`bin:` 授权前缀 + 二进制风险分类，**纯代数、零 I/O**。
//!
//! 为什么放这里而不是 `skiff_plugin::execgrant`：风险分类既要被**运行期判定**
//! （`execgrant`，要 canonicalize 才能判）使用，也要被**装包校验**（`pkgfmt`）
//! 使用；后者不能依赖 `skiff-plugin`（那条链会拖进 rusqlite）。放在词汇层，
//! 两边共用一份名单，不会出现「打包时放行、运行时拒绝」的两套口径。
//!
//! 分类只有三档，刻意不做更细的分级：
//!
//! - [`BinRisk::Wrapper`]：**拒绝**。这类二进制的天职就是「把参数变成任意执行」
//!   （shell / 解释器 / `find -exec` / `systemd-run` / 编辑器与分页器的逃逸口）。
//!   一旦授权，`bin:` 授权粒度这个模型就整体失效，故不提供「确认后可授权」的路径。
//! - [`BinRisk::RootEquivalent`]：**允许但标记**。授出即等价授出 root（能任意写
//!   文件 / 起服务 / 改账号）。这不是缺陷，是这套模型的固有粒度：真实边界是
//!   「谁写的插件包 + 装之前看了确认页 + 每次执行留审计」。
//! - [`BinRisk::Scoped`]：影响面限于某个子系统（`fail2ban-client` / `pure-pw` /
//!   `nft`），出错最多让该子系统不可用。
//!
//! 名单是**黑名单**，不是白名单：新二进制不需要内核发版，只需要管理在安装时
//! 确认一条 `bin:` 授权。名单只需要覆盖「白名单永远补不完的包装器类」。

/// `bin:` 对象授权前缀。值与 `path:` 同形态：一个**绝对路径**，指向可执行文件。
pub const BIN_PREFIX: &str = "bin:";

/// 二进制风险档位（见模块文档）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinRisk {
    /// 包装器 / 解释器：拒绝授权与执行。
    Wrapper,
    /// 授出即等价 root。
    RootEquivalent,
    /// 影响面限于某个子系统。
    Scoped,
}

impl BinRisk {
    pub fn as_str(self) -> &'static str {
        match self {
            BinRisk::Wrapper => "wrapper",
            BinRisk::RootEquivalent => "root-equiv",
            BinRisk::Scoped => "scoped",
        }
    }
}

/// 包装器名单：它们能把「一个参数」解释成「一段程序」。
///
/// 覆盖五类，都是 `bin:` 模型下没有正当用途、只有逃逸用途的：
/// 1. shell（`sh` / `bash` / …）：`-c` 直接任意执行；
/// 2. 解释器（`python` / `perl` / …）：同上，且版本号后缀无法穷举，用前缀判；
/// 3. 带执行能力的工具（`find -exec`、`awk system()`、GNU `sed e`、`xargs`、`env`）；
/// 4. 编辑器与分页器（`vi` / `less` / `man` 都有 shell 逃逸）；
/// 5. 二次派发器（`systemd-run` / `su` / `sudo` / `chroot` / `crontab` / `at`）。
///
/// 第 5 类里 `systemd-run` 尤其要紧：内核自己就是用它在跑特权命令，插件若能
/// 直接调它，等于拿到一条完全不受 `bin:` 约束的 root 通道。
pub const WRAPPERS: &[&str] = &[
    // 1. shell
    "sh",
    "bash",
    "dash",
    "zsh",
    "ksh",
    "mksh",
    "ash",
    "csh",
    "tcsh",
    "fish",
    // 3. 带执行能力的工具
    "env",
    "xargs",
    "find",
    "awk",
    "gawk",
    "mawk",
    "nawk",
    "sed",
    "expect",
    "parallel",
    // 4. 编辑器 / 分页器 / 调试器
    "vi",
    "vim",
    "nvim",
    "nano",
    "emacs",
    "ed",
    "ex",
    "less",
    "more",
    "most",
    "man",
    "watch",
    "gdb",
    "strace",
    "ltrace",
    // 5. 二次派发 / 提权 / 计划任务 / 命名空间
    "systemd-run",
    "su",
    "sudo",
    "doas",
    "chroot",
    "unshare",
    "nsenter",
    "runuser",
    "crontab",
    "at",
    "batch",
    "ssh",
    "busybox",
    "socat",
    "nc",
    "ncat",
    "netcat",
    "telnet",
    "script",
    // 2. 解释器本体（版本号后缀由 [`is_wrapper_bin`] 的前缀规则兜）
    "perl",
    "ruby",
    "node",
    "nodejs",
    "php",
    "lua",
    "tclsh",
    "wish",
    "pwsh",
    "powershell",
];

/// 解释器前缀：`python3.11` / `python2` 这类带版本后缀的名字不在名单里。
const INTERPRETER_PREFIXES: &[&str] = &["python", "perl5", "ruby", "node"];

/// 授出即等价 root 的二进制（允许，但审计与确认页要标出来）。
///
/// 判据只有一条：**能以 root 身份写任意文件**或**能让 root 跑任意代码**。
/// 满足它的途径出奇地多（`cp` + `chmod` 就能种一个 setuid root 二进制），
/// 所以这份名单注定不完整——它只用于标注风险，不承担拦截职责。
pub const ROOT_EQUIVALENT: &[&str] = &[
    "apt",
    "apt-get",
    "aptitude",
    "dpkg",
    "systemctl",
    "service",
    "cp",
    "mv",
    "install",
    "ln",
    "tar",
    "rsync",
    "dd",
    "tee",
    "chmod",
    "chown",
    "chgrp",
    "truncate",
    "passwd",
    "chpasswd",
    "useradd",
    "usermod",
    "userdel",
    "groupadd",
    "gpasswd",
    "update-alternatives",
];

/// 取路径的文件名（授权值与请求值都按它比名单）。
///
/// 传进来的是绝对路径（`/usr/bin/python3.11`）；Windows 开发机上的 `\` 也一并
/// 认掉，免得单测里两种分隔符各写一套。
pub fn bin_name(path: &str) -> &str {
    let cut = path.rfind(['/', '\\']).map(|i| i + 1).unwrap_or(0);
    &path[cut..]
}

/// 这个二进制是否属于「包装器」类（拒绝授权与执行）。
pub fn is_wrapper_bin(path: &str) -> bool {
    let name = bin_name(path);
    if name.is_empty() {
        return false;
    }
    WRAPPERS.contains(&name) || INTERPRETER_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// 这个二进制是否授出即等价 root（用于审计与确认页标注，不用于拦截）。
pub fn is_root_equivalent(path: &str) -> bool {
    ROOT_EQUIVALENT.contains(&bin_name(path))
}

/// 风险档位：包装器优先（拒绝），其余按名单分两档。
pub fn bin_risk(path: &str) -> BinRisk {
    if is_wrapper_bin(path) {
        BinRisk::Wrapper
    } else if is_root_equivalent(path) {
        BinRisk::RootEquivalent
    } else {
        BinRisk::Scoped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bin_name_takes_last_component_for_both_separators() {
        assert_eq!(bin_name("/usr/bin/fail2ban-client"), "fail2ban-client");
        assert_eq!(bin_name("C:\\Windows\\system32\\cmd.exe"), "cmd.exe");
        assert_eq!(bin_name("bash"), "bash");
        assert_eq!(bin_name("/usr/bin/"), "");
    }

    /// 版本号后缀不能漏：`python3.11` 与 `python3` 一样是包装器。
    #[test]
    fn versioned_interpreters_are_still_wrappers() {
        for p in [
            "/usr/bin/python3.11",
            "/usr/bin/python2",
            "/usr/bin/perl5.36",
            "/usr/bin/node",
        ] {
            assert!(is_wrapper_bin(p), "{p} 应判为包装器");
        }
    }

    /// 五类包装器各抽一个，防止名单被误删。
    #[test]
    fn wrapper_classes_are_covered() {
        for p in [
            "/bin/bash",
            "/usr/bin/env",
            "/usr/bin/find",
            "/usr/bin/awk",
            "/usr/bin/vi",
            "/usr/bin/less",
            "/usr/bin/systemd-run",
            "/usr/bin/crontab",
            "/usr/bin/socat",
        ] {
            assert_eq!(bin_risk(p), BinRisk::Wrapper, "{p} 应判为包装器");
        }
    }

    /// 真正要授权的宿主工具不该被误伤。
    #[test]
    fn real_host_tools_are_not_wrappers() {
        for p in [
            "/usr/bin/fail2ban-client",
            "/usr/sbin/pure-pw",
            "/usr/sbin/nft",
            "/usr/bin/tee",
        ] {
            assert_ne!(bin_risk(p), BinRisk::Wrapper, "{p} 不是包装器");
        }
    }

    /// 授出即 root 的按名单标出；`nft` / `fail2ban-client` 属子系统级。
    #[test]
    fn root_equivalent_flags_filesystem_and_service_tools() {
        assert_eq!(bin_risk("/usr/bin/apt-get"), BinRisk::RootEquivalent);
        assert_eq!(bin_risk("/usr/bin/systemctl"), BinRisk::RootEquivalent);
        assert_eq!(bin_risk("/usr/bin/cp"), BinRisk::RootEquivalent);
        assert_eq!(bin_risk("/usr/sbin/nft"), BinRisk::Scoped);
        assert_eq!(bin_risk("/usr/bin/fail2ban-client"), BinRisk::Scoped);
    }

    /// 空名不算包装器（空值由调用方按「非法」拒，不在这里混一个错误分类）。
    #[test]
    fn empty_name_is_not_wrapper() {
        assert!(!is_wrapper_bin("/usr/bin/"));
        assert_eq!(bin_risk("/"), BinRisk::Scoped);
    }
}
