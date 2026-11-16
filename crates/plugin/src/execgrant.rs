//! 宿主命令执行授权（`bin:` 对象 grant）的唯一判定实现。
//!
//! 与 [`crate::fsgrant`] 同构：能力面只有一条 `exec` 路由、一个权限串 `exec:run`；
//! 真正决定「能跑什么」的是**安装时逐条确认过的 `bin:` 对象授权**。`exec:run`
//! 与 `bin:/usr/bin/fail2ban-client` 是两把钥匙，缺一不可。
//!
//! 判定顺序（缺一不可，任何一步失败都记 `deny` 审计）：
//!
//! 1. **形式**：绝对路径、无 `..` 段、无 NUL、非空；
//! 2. **存在性**：必须能 canonicalize（→ 软链按真实路径说话：授权 `/bin/systemctl`
//!    在 usr-merge 的发行版上与 `/usr/bin/systemctl` 等价）；
//! 3. **本体**：普通文件、有执行位、**不被组/其他人写**、属主为 root。这一条挡的是
//!    「插件先用自己的 `path:` 授权在自己的目录里放一个假 `systemctl`，再让内核以
//!    root 执行它」——那是 `bin:` 模型独有的交叉提权面，`fs` 单独用没有；
//! 4. **不在内核数据根内**：与 `fs` 的受保护前缀同一口径（`fsgrant::is_protected`），
//!    防止「把二进制放进插件自己的 data/ 再授权它」绕过第 3 条；
//! 5. **不是包装器**：`bash` / `python3` / `systemd-run` / `find` 这类二进制的天职就是
//!    把参数变成任意执行，名单见 [`skiff_model::exec`]（打包期同一份名单已拦一次）；
//! 6. **命中授权**：canonical 路径必须**精确等于**某个 `bin:` 授权的 canonical 路径。
//!    刻意不支持目录级授权（`bin:/usr/bin` 等于授权整台机器）与通配。
//!
//! 第 6 步之后不再校验 argv：授权一个二进制就是授权它作为 root 能做的一切，
//! 这是本模型明示的粒度（详见 `docs/plugin/HOST_EXEC_DESIGN.md` §3）。

use std::path::{Path, PathBuf};

use skiff_model::{has_parent_segment, is_absolute, is_wrapper_bin, BIN_PREFIX};
use skiff_store::Grant;

pub use skiff_model::{bin_risk, BinRisk};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecError {
    /// 形式非法（相对路径、含 `..`、含 NUL、空）
    Illegal(&'static str),
    /// 目标不存在 / 不可 canonicalize
    Missing(String),
    /// 存在但不是普通文件
    NotExecutable,
    /// 存在但本体不可信（非 root 属主 / 组或其他人可写 / 无执行位）
    NotTrusted(&'static str),
    /// 落在内核数据根内
    Protected,
    /// 包装器 / 解释器
    Wrapper,
    /// 未授权
    NotGranted,
}

impl ExecError {
    /// 审计里的结果列：越权类记 `deny`，环境类记 `fail`。
    pub fn audit_result(&self) -> &'static str {
        match self {
            ExecError::NotGranted | ExecError::Wrapper | ExecError::Protected => "deny",
            _ => "fail",
        }
    }
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecError::Illegal(m) => write!(f, "{m}"),
            ExecError::Missing(m) => write!(f, "二进制不可用: {m}"),
            ExecError::NotExecutable => write!(f, "不是可执行文件"),
            ExecError::NotTrusted(m) => write!(f, "二进制不可信: {m}"),
            ExecError::Protected => write!(f, "不允许执行内核数据根内的文件"),
            ExecError::Wrapper => {
                write!(f, "包装器/解释器不可授权（可把参数变成任意执行）")
            }
            ExecError::NotGranted => write!(f, "该二进制未获 bin: 授权"),
        }
    }
}

/// 从 grant 列表里取出 `bin:` 授权值（原始字符串，未 canonicalize）。
pub fn bins_of(grants: &[Grant]) -> Vec<String> {
    grants
        .iter()
        .filter(|g| g.kind == "object" && g.value.starts_with(BIN_PREFIX))
        .map(|g| g.value[BIN_PREFIX.len()..].to_string())
        .collect()
}

/// 授权值转 canonical 路径；不存在或不是文件的一律丢弃（授权随之失效）。
///
/// 与 `fsgrant::canonical_roots` 同一取舍：管理员把二进制删了/换了，授权立刻失效，
/// 不会因为库里存了旧字符串而继续放行。
pub fn canonical_bins(grants: &[Grant]) -> Vec<PathBuf> {
    bins_of(grants)
        .into_iter()
        .filter_map(|b| std::fs::canonicalize(b).ok())
        .filter(|p| p.is_file())
        .collect()
}

/// 把插件给的二进制路径解析成可执行的真实路径（判定顺序见模块文档）。
pub fn resolve(data_dir: &Path, grants: &[Grant], requested: &str) -> Result<PathBuf, ExecError> {
    let canon = resolve_candidate(data_dir, requested)?;
    check_owner(&canon)?;
    authorize(&canon, grants)?;
    Ok(canon)
}

/// 形式 + 存在 + 本体类型 + 包装器 + 数据根：只看「这个路径指向什么」。
///
/// 与「属主可信」拆开，是因为那一步能否验证取决于跑测试的是谁：它要求文件属主
/// 是 root，普通用户（CI runner）造不出来。拆开后授权命中与本体可信各自可断言，
/// 不必让一条用例同时依赖特权。
fn resolve_candidate(data_dir: &Path, requested: &str) -> Result<PathBuf, ExecError> {
    if requested.is_empty() {
        return Err(ExecError::Illegal("二进制路径不能为空"));
    }
    if requested.contains('\0') {
        return Err(ExecError::Illegal("二进制路径含 NUL"));
    }
    if has_parent_segment(requested) {
        return Err(ExecError::Illegal("二进制路径不得含 .."));
    }
    if !is_absolute(requested) {
        return Err(ExecError::Illegal("二进制路径必须是绝对路径"));
    }

    let canon = std::fs::canonicalize(requested).map_err(|e| ExecError::Missing(e.to_string()))?;
    if !canon.is_file() {
        return Err(ExecError::NotExecutable);
    }
    if is_wrapper_bin(canon.to_string_lossy().as_ref()) {
        return Err(ExecError::Wrapper);
    }
    if crate::fsgrant::is_protected(data_dir, &canon) {
        return Err(ExecError::Protected);
    }
    Ok(canon)
}

/// 授权命中：canonical 路径必须**精确等于**某条 `bin:` 授权的 canonical 路径。
fn authorize(canon: &Path, grants: &[Grant]) -> Result<(), ExecError> {
    if canonical_bins(grants).iter().any(|b| b == canon) {
        Ok(())
    } else {
        Err(ExecError::NotGranted)
    }
}

/// 文件本体是否可信：root 属主、组/其他人不可写、有执行位。
///
/// 非 unix 平台没有这套元数据（生产只跑 Linux），直接放行——判定仍由授权那一环
/// 兜底；这样 Windows 开发机上 `cargo test` 也能跑形式与授权部分。
#[cfg(unix)]
fn check_owner(canon: &Path) -> Result<(), ExecError> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(canon).map_err(|e| ExecError::Missing(e.to_string()))?;
    if meta.uid() != 0 {
        return Err(ExecError::NotTrusted("属主不是 root"));
    }
    if meta.mode() & 0o022 != 0 {
        return Err(ExecError::NotTrusted("组或其他人可写"));
    }
    if meta.mode() & 0o111 == 0 {
        return Err(ExecError::NotTrusted("缺少执行位"));
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_owner(_canon: &Path) -> Result<(), ExecError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(value: &str) -> Grant {
        Grant::object(value.to_string())
    }

    /// 造一个「已授权」的临时可执行文件；返回（二进制目录，内核数据根，文件路径）。
    ///
    /// 两个目录**必须分开**：二进制落在数据根下会被 [`ExecError::Protected`] 拦掉，
    /// 那是另一个用例要验的东西。非 unix 上 `check_owner` 是空实现，所以这些用例
    /// 在开发机上同样能跑。
    fn fixture(name: &str) -> (tempfile::TempDir, tempfile::TempDir, PathBuf) {
        let bin_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let bin = bin_dir.path().join(name);
        std::fs::write(&bin, b"#!/bin/true\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        (bin_dir, data_dir, bin)
    }

    #[test]
    fn bins_come_only_from_bin_grants() {
        let grants = vec![
            grant("path:/etc/fail2ban"),
            grant("bin:/usr/bin/fail2ban-client"),
            grant("container:web"),
        ];
        assert_eq!(bins_of(&grants), vec!["/usr/bin/fail2ban-client"]);
    }

    #[test]
    fn missing_or_non_file_grants_are_dropped() {
        let grants = vec![
            grant("bin:/definitely/not/here"),
            grant(&format!("bin:{}", std::env::temp_dir().display())),
        ];
        assert!(canonical_bins(&grants).is_empty());
    }

    #[test]
    fn form_errors_are_rejected_before_touching_the_fs() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve(dir.path(), &[], "relative/x"),
            Err(ExecError::Illegal("二进制路径必须是绝对路径"))
        );
        assert_eq!(
            resolve(dir.path(), &[], "/usr/../bin/x"),
            Err(ExecError::Illegal("二进制路径不得含 .."))
        );
        assert_eq!(
            resolve(dir.path(), &[], ""),
            Err(ExecError::Illegal("二进制路径不能为空"))
        );
    }

    /// 授权只看「canonical 路径是否精确命中某条 `bin:`」。
    ///
    /// 单独断言这一步而不是整条 [`resolve`]：整条要过「属主是 root」，普通用户
    /// （CI runner）造不出 root 属主的文件，那样这条用例就变成「只有 root 过得去」。
    #[test]
    fn authorize_requires_an_exact_grant() {
        let (_bin_dir, _data_dir, bin) = fixture("tool");
        let path = bin.display().to_string();
        // 比较一律用 canonical：Windows 上的前缀只出现在 canonicalize 结果里
        let canon = std::fs::canonicalize(&bin).unwrap();
        assert_eq!(authorize(&canon, &[grant(&format!("bin:{path}"))]), Ok(()));

        // 同一个二进制，没有授给它
        assert_eq!(
            authorize(&canon, &[grant("bin:/usr/bin/other")]),
            Err(ExecError::NotGranted)
        );
    }

    /// 整条判定：属主可信时命中授权；不可信时**先**在信任闸拒。
    ///
    /// 两种结果都是正确行为，断言按实际属主分流——不为了让用例变绿去放宽信任闸。
    #[test]
    fn resolve_hits_the_grant_only_for_a_trusted_binary() {
        let (_bin_dir, data_dir, bin) = fixture("tool");
        let path = bin.display().to_string();
        let grants = vec![grant(&format!("bin:{path}"))];
        let canon = std::fs::canonicalize(&bin).unwrap();

        match resolve(data_dir.path(), &grants, &path) {
            Ok(got) => assert_eq!(got, canon),
            Err(ExecError::NotTrusted("属主不是 root")) => {}
            other => panic!("未预期的判定结果：{other:?}"),
        }
    }

    /// 授权目录等于授权整台机器：`bin:` 只对文件生效，目录一律不收。
    #[test]
    fn directory_grant_does_not_authorize_children() {
        let (bin_dir, _data_dir, bin) = fixture("tool");
        let grants = vec![grant(&format!("bin:{}", bin_dir.path().display()))];
        assert!(canonical_bins(&grants).is_empty());
        let canon = std::fs::canonicalize(&bin).unwrap();
        assert_eq!(authorize(&canon, &grants), Err(ExecError::NotGranted));
    }

    #[test]
    fn wrappers_are_rejected_even_when_granted() {
        let (bin_dir, data_dir, _) = fixture("placeholder");
        let bin = bin_dir.path().join("bash");
        std::fs::write(&bin, b"x").unwrap();
        let path = bin.display().to_string();
        let grants = vec![grant(&format!("bin:{path}"))];
        assert_eq!(
            resolve(data_dir.path(), &grants, &path),
            Err(ExecError::Wrapper)
        );
    }

    /// 插件自己 `data/` 里的二进制：即便被授权也不执行（先于授权判定拒）。
    #[test]
    fn binaries_inside_kernel_data_root_are_rejected() {
        let (bin_dir, _data_dir, bin) = fixture("tool");
        let path = bin.display().to_string();
        let grants = vec![grant(&format!("bin:{path}"))];
        // 数据根设成二进制所在目录：canonicalize 后 bin 落在其下
        assert_eq!(
            resolve(bin_dir.path(), &grants, &path),
            Err(ExecError::Protected)
        );
    }

    #[test]
    fn missing_binary_reports_missing() {
        let dir = tempfile::tempdir().unwrap();
        let err = resolve(dir.path(), &[], "/definitely/not/here/x").unwrap_err();
        assert!(matches!(err, ExecError::Missing(_)));
        assert_eq!(err.audit_result(), "fail");
    }
}
