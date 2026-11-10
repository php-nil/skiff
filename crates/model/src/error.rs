//! 稳定错误码：传输层与服务端 UI（SDUI）共用的**词汇**。
//!
//! 为什么放在 model：错误码要被 `web` 的 HTML 页、`/api/v1` 与未来的动作端点
//! 同时引用，判定和文案却各自不同。这里只回答「有哪些错误、属于哪一类、
//! 默认是什么 HTTP 语义、是否可重试」，不含任何 I/O、框架或文案。
//!
//! 与 `skiff-fs` 的 `fs.*` 错误码同一思路：**机器码稳定、可分支、不复用**。
//! 具体业务原因（如 `site.domain_invalid`）由领域层另行定义，这里只划大类，
//! 避免把几十个领域错误塞进共享词汇层。

use std::fmt;

/// 错误大类。与产品口径一致：形状、未授权、领域冲突、未找到、依赖不可用、内部错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCategory {
    /// 请求本身不成立：缺字段、类型错误、越界。
    Shape,
    /// 未登录、越权、token scope 不足。
    Unauthorized,
    /// 与当前领域状态冲突：重复、状态不允许、被保护。
    Conflict,
    /// 目标不存在。
    NotFound,
    /// 下游依赖不可用：Docker、插件、磁盘空间、超时。
    Unavailable,
    /// 内核自身缺陷或未预期状态。
    Internal,
}

impl ErrorCategory {
    /// 机器码前缀。`ErrorCode::as_str()` 必须以它开头，由单测守护。
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCategory::Shape => "shape",
            ErrorCategory::Unauthorized => "auth",
            ErrorCategory::Conflict => "domain",
            ErrorCategory::NotFound => "lookup",
            ErrorCategory::Unavailable => "dependency",
            ErrorCategory::Internal => "internal",
        }
    }
}

impl fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 稳定错误码。
///
/// 新增只允许往后加；已发布的码不得改语义或复用。领域层若要区分具体原因，
/// 在其 crate 内另行定义（如 `fs.*`），不要在这里堆业务名词。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// 请求形状不成立（缺字段、类型错、超出范围）。
    InvalidShape,
    /// 形状成立但某个字段的值不合法；配合 `field` 定位。
    InvalidField,
    /// 未认证：缺少或失效的会话/令牌。
    Unauthenticated,
    /// 已认证但无权限。
    Forbidden,
    /// API Token 缺少所需 scope。
    ScopeMissing,
    /// 与当前状态冲突（重复创建、已被占用等）。
    Conflict,
    /// 当前状态不允许该动作（如已结束的作业不能取消）。
    InvalidState,
    /// 目标不存在。
    NotFound,
    /// 下游依赖不可用（Docker 未就绪、插件进程异常）。
    Unavailable,
    /// 下游超时。
    Timeout,
    /// 空间不足（磁盘/配额）。
    NoSpace,
    /// 内核内部错误。
    Internal,
}

impl ErrorCode {
    /// 全部错误码。新增变体必须同步进这张表，由单测保证唯一且可枚举。
    pub fn all() -> &'static [ErrorCode] {
        use ErrorCode::*;
        &[
            InvalidShape,
            InvalidField,
            Unauthenticated,
            Forbidden,
            ScopeMissing,
            Conflict,
            InvalidState,
            NotFound,
            Unavailable,
            Timeout,
            NoSpace,
            Internal,
        ]
    }

    pub fn category(self) -> ErrorCategory {
        use ErrorCode::*;
        match self {
            InvalidShape | InvalidField => ErrorCategory::Shape,
            Unauthenticated | Forbidden | ScopeMissing => ErrorCategory::Unauthorized,
            Conflict | InvalidState => ErrorCategory::Conflict,
            NotFound => ErrorCategory::NotFound,
            Unavailable | Timeout | NoSpace => ErrorCategory::Unavailable,
            Internal => ErrorCategory::Internal,
        }
    }

    /// 稳定机器码。发布后不得改名。
    pub fn as_str(self) -> &'static str {
        use ErrorCode::*;
        match self {
            InvalidShape => "shape.invalid",
            InvalidField => "shape.field",
            Unauthenticated => "auth.unauthenticated",
            Forbidden => "auth.forbidden",
            ScopeMissing => "auth.scope_missing",
            Conflict => "domain.conflict",
            InvalidState => "domain.invalid_state",
            NotFound => "lookup.not_found",
            Unavailable => "dependency.unavailable",
            Timeout => "dependency.timeout",
            NoSpace => "dependency.no_space",
            Internal => "internal",
        }
    }

    /// 默认 HTTP 状态。传输层可覆盖，但默认值只在这里维护。
    pub fn status(self) -> u16 {
        use ErrorCode::*;
        match self {
            InvalidShape | InvalidField => 400,
            Unauthenticated => 401,
            Forbidden | ScopeMissing => 403,
            NotFound => 404,
            Conflict | InvalidState => 409,
            Internal => 500,
            Unavailable => 503,
            Timeout => 504,
            NoSpace => 507,
        }
    }

    /// 重试是否有意义（客户端可据此决定是否自动重试）。
    pub fn retryable(self) -> bool {
        matches!(self, ErrorCode::Unavailable | ErrorCode::Timeout)
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn all_is_complete_and_unique() {
        let mut seen = BTreeSet::new();
        for code in ErrorCode::all() {
            assert!(seen.insert(code.as_str()), "错误码重复: {code}");
        }
        assert_eq!(seen.len(), ErrorCode::all().len());
    }

    #[test]
    fn machine_codes_are_prefixed_by_category() {
        for code in ErrorCode::all() {
            assert!(
                code.as_str().starts_with(code.category().as_str()),
                "{code} 的前缀与类别 {} 不一致",
                code.category()
            );
            assert!(
                code.as_str().split('.').all(|p| !p.is_empty()),
                "错误码不能有空段: {code}"
            );
        }
    }

    #[test]
    fn statuses_are_valid_http_codes() {
        for code in ErrorCode::all() {
            let status = code.status();
            assert!(
                (400..=599).contains(&status),
                "{code} 的 HTTP 状态 {status} 不在错误区间"
            );
        }
    }

    #[test]
    fn only_transient_dependency_failures_are_retryable() {
        for code in ErrorCode::all() {
            // 空间不足同属依赖类，但重试无意义；只有「暂时不可用/超时」可重试。
            let expected = matches!(code, ErrorCode::Unavailable | ErrorCode::Timeout);
            assert_eq!(code.retryable(), expected, "{code} 的可重试口径不一致");
        }
    }

    #[test]
    fn category_prefixes_are_stable() {
        assert_eq!(ErrorCategory::Shape.as_str(), "shape");
        assert_eq!(ErrorCategory::Unauthorized.as_str(), "auth");
        assert_eq!(ErrorCategory::Conflict.as_str(), "domain");
        assert_eq!(ErrorCategory::NotFound.as_str(), "lookup");
        assert_eq!(ErrorCategory::Unavailable.as_str(), "dependency");
        assert_eq!(ErrorCategory::Internal.as_str(), "internal");
    }
}
