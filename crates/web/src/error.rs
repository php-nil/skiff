use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use serde::Serialize;
use skiff_model::ErrorCode;

/// UI 侧错误：统一渲染错误页；API 侧用 `ApiError` 包一层输出 JSON。
#[derive(Debug, Clone)]
pub enum AppError {
    NotFound(String),
    Unauthorized(String),
    Forbidden(String),
    /// CSRF 令牌缺失/失效/不匹配。HTTP 仍是 403（不改变既有页面行为），
    /// 但错误码独立为 `auth.csrf_invalid`，JSON 路径可提示「刷新后重试」而非「无权限」。
    Csrf(String),
    BadRequest(String),
    /// 随包 schema 运行期校验失败：客户端输入（插件包）不成立，按 400 处理，
    /// 错误码独立为 `shape.plugin_schema`，不再与内核 500 混在一起。
    PluginSchema(String),
    /// 插件包声明的内核 API 与当前内核不兼容，错误码 `shape.plugin_api`。
    PluginApi(String),
    /// 形状成立但字段值不合法；`field` 用于让客户端定位到具体控件。
    InvalidField {
        field: String,
        message: String,
    },
    Conflict(String),
    /// 容量类失败（磁盘/文件系统配额用尽）：单独一个变体是为了让插件能力面
    /// 与内核面同口径返回 507，而不是被代理层归成 400。
    NoSpace(String),
    Unavailable(String),
    /// 处理超时（等待 Docker 引擎等下游过久）。单独一个变体是为了给 504 配一条
    /// 说人话的文案，而不是让浏览器看到一个空 408。
    Timeout(String),
    Internal(String),
    Redirect(String),
}

impl AppError {
    pub fn status(&self) -> StatusCode {
        match self {
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            AppError::Forbidden(_) | AppError::Csrf(_) => StatusCode::FORBIDDEN,
            AppError::BadRequest(_)
            | AppError::PluginSchema(_)
            | AppError::PluginApi(_)
            | AppError::InvalidField { .. } => StatusCode::BAD_REQUEST,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::NoSpace(_) => StatusCode::INSUFFICIENT_STORAGE,
            AppError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            AppError::Timeout(_) => StatusCode::GATEWAY_TIMEOUT,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::Redirect(_) => StatusCode::SEE_OTHER,
        }
    }

    /// 稳定错误码：机器分支只认它，不认 HTTP 状态或文案。
    ///
    /// `Redirect` 不是错误，`ApiError` 会单独把它交回重定向响应；这里给内部码兜底。
    pub fn error_code(&self) -> ErrorCode {
        match self {
            AppError::NotFound(_) => ErrorCode::NotFound,
            AppError::Unauthorized(_) => ErrorCode::Unauthenticated,
            AppError::Forbidden(_) => ErrorCode::Forbidden,
            AppError::Csrf(_) => ErrorCode::CsrfInvalid,
            AppError::BadRequest(_) => ErrorCode::InvalidShape,
            AppError::PluginSchema(_) => ErrorCode::PluginSchemaInvalid,
            AppError::PluginApi(_) => ErrorCode::PluginApiIncompatible,
            AppError::InvalidField { .. } => ErrorCode::InvalidField,
            AppError::Conflict(_) => ErrorCode::Conflict,
            AppError::NoSpace(_) => ErrorCode::NoSpace,
            AppError::Unavailable(_) => ErrorCode::Unavailable,
            AppError::Timeout(_) => ErrorCode::Timeout,
            AppError::Internal(_) | AppError::Redirect(_) => ErrorCode::Internal,
        }
    }

    /// 字段定位（仅形状/字段类错误有值）。
    pub fn field(&self) -> Option<&str> {
        match self {
            AppError::InvalidField { field, .. } => Some(field),
            _ => None,
        }
    }

    /// 统一 JSON 错误载荷：后续动作端点直接复用它，不再各拼一套 `json!`。
    pub fn envelope(&self) -> ErrorEnvelope {
        let code = self.error_code();
        ErrorEnvelope {
            error: ErrorBody {
                code: code.as_str(),
                status: self.status().as_u16(),
                message: self.message().to_string(),
                field: self.field().map(ToString::to_string),
                retryable: code.retryable(),
            },
        }
    }

    pub fn message(&self) -> &str {
        match self {
            AppError::NotFound(m)
            | AppError::Unauthorized(m)
            | AppError::Forbidden(m)
            | AppError::Csrf(m)
            | AppError::BadRequest(m)
            | AppError::PluginSchema(m)
            | AppError::PluginApi(m)
            | AppError::Conflict(m)
            | AppError::NoSpace(m)
            | AppError::Unavailable(m)
            | AppError::Timeout(m)
            | AppError::Internal(m)
            | AppError::Redirect(m) => m,
            AppError::InvalidField { message, .. } => message,
        }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.status().as_u16(), self.message())
    }
}

impl std::error::Error for AppError {}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        if let AppError::Redirect(to) = &self {
            return Redirect::to(to).into_response();
        }
        let status = self.status();
        let title = match status.as_u16() {
            401 => "未登录",
            403 => "无权访问",
            404 => "页面不存在",
            409 => "冲突",
            507 => "空间不足",
            503 => "服务不可用",
            504 => "请求超时",
            _ => "请求失败",
        };
        // 统一走 error.html，不要再在代码里拼 HTML：模板改了样式这里会同时生效。
        let page = crate::templates::ErrorPage {
            code: status.as_u16(),
            title: title.to_string(),
            message: self.message().to_string(),
            prefix: crate::templates::entry_prefix().to_string(),
        };
        match crate::handlers::render(page) {
            Ok(mut resp) => {
                *resp.status_mut() = status;
                resp
            }
            // 模板渲染失败（理论上不会）时给纯文本兜底，绝不把错误页再抛回 error.rs
            Err(_) => (
                status,
                [("content-type", "text/plain; charset=utf-8")],
                format!("{} {}", status.as_u16(), title),
            )
                .into_response(),
        }
    }
}

/// 统一 JSON 错误载荷。后续动作端点与 `/api/v1` 共用，禁止再各自拼 `json!`。
#[derive(Debug, Clone, Serialize)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorBody {
    /// 稳定机器码，见 `skiff_model::ErrorCode`。
    pub code: &'static str,
    /// HTTP 状态，仅作冗余提示；分支判断用 `code`。
    pub status: u16,
    pub message: String,
    /// 字段定位；仅形状/字段类错误有值。
    pub field: Option<String>,
    pub retryable: bool,
}

/// API 错误：JSON 输出，避免前端解析 HTML。
#[derive(Debug)]
pub struct ApiError(pub AppError);

impl From<AppError> for ApiError {
    fn from(e: AppError) -> Self {
        ApiError(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // 重定向不是错误：交回 HTML 侧处理，避免出现 303 + JSON 的怪组合。
        if matches!(self.0, AppError::Redirect(_)) {
            return self.0.into_response();
        }
        (self.0.status(), Json(self.0.envelope())).into_response()
    }
}

impl From<skiff_store::StoreError> for AppError {
    fn from(e: skiff_store::StoreError) -> Self {
        use skiff_store::StoreError as Store;
        match &e {
            // 业务结果：记录不存在 / 唯一约束冲突，按 404 / 409 返回，别混成 500。
            Store::NotFound(_) => AppError::NotFound(e.to_string()),
            Store::Conflict(_) => AppError::Conflict(e.to_string()),
            // 依赖故障：SQLite / IO / 锁忙。可重试，返回 `dependency.unavailable`，
            // 让前端能区分「数据库暂时不可用」和「代码 500」。
            Store::Sqlite(_) | Store::Io(_) | Store::Busy(_) => {
                tracing::error!(error = %e, "数据库依赖失败");
                AppError::Unavailable(format!("数据库暂不可用：{e}"))
            }
            Store::SchemaTooNew { .. } | Store::Decode(_) => AppError::Internal(e.to_string()),
        }
    }
}

/// 菜单领域错误：数据层失败沿用 StoreError 的映射，序列化失败按 500。
impl From<skiff_app::menu::MenuError> for AppError {
    fn from(e: skiff_app::menu::MenuError) -> Self {
        match e {
            skiff_app::menu::MenuError::Store(e) => e.into(),
            skiff_app::menu::MenuError::Internal(m) => AppError::Internal(m),
        }
    }
}

/// 表单编码非法属于客户端输入问题，统一 400；错误里带位置与成因，便于排查。
impl From<crate::util::FormDecodeError> for AppError {
    fn from(e: crate::util::FormDecodeError) -> Self {
        AppError::BadRequest(format!("请求体不是合法的 URL 编码：{e}"))
    }
}

impl From<skiff_plugin::PluginError> for AppError {
    fn from(e: skiff_plugin::PluginError) -> Self {
        match e {
            skiff_plugin::PluginError::Rejected(m) => AppError::BadRequest(m),
            other => AppError::Internal(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_errors() -> Vec<(AppError, ErrorCode, u16)> {
        vec![
            (AppError::NotFound("x".into()), ErrorCode::NotFound, 404),
            (
                AppError::Unauthorized("x".into()),
                ErrorCode::Unauthenticated,
                401,
            ),
            (AppError::Forbidden("x".into()), ErrorCode::Forbidden, 403),
            (AppError::Csrf("x".into()), ErrorCode::CsrfInvalid, 403),
            (
                AppError::BadRequest("x".into()),
                ErrorCode::InvalidShape,
                400,
            ),
            (
                AppError::PluginSchema("schema 校验失败".into()),
                ErrorCode::PluginSchemaInvalid,
                400,
            ),
            (
                AppError::PluginApi("插件要求内核 API 1.4".into()),
                ErrorCode::PluginApiIncompatible,
                400,
            ),
            (
                AppError::InvalidField {
                    field: "domain".into(),
                    message: "x".into(),
                },
                ErrorCode::InvalidField,
                400,
            ),
            (AppError::Conflict("x".into()), ErrorCode::Conflict, 409),
            (AppError::NoSpace("x".into()), ErrorCode::NoSpace, 507),
            (
                AppError::Unavailable("x".into()),
                ErrorCode::Unavailable,
                503,
            ),
            (AppError::Timeout("x".into()), ErrorCode::Timeout, 504),
            (AppError::Internal("x".into()), ErrorCode::Internal, 500),
        ]
    }

    #[test]
    fn app_error_maps_to_stable_code_and_status() {
        for (err, code, status) in sample_errors() {
            assert_eq!(
                err.error_code(),
                code,
                "{} 的错误码映射不一致",
                err.message()
            );
            assert_eq!(
                err.status().as_u16(),
                status,
                "{} 的状态码变了",
                err.message()
            );
            assert_eq!(err.envelope().error.code, code.as_str());
            assert_eq!(err.envelope().error.status, status);
        }
    }

    #[test]
    fn envelope_has_uniform_shape() {
        let err = AppError::InvalidField {
            field: "domain".into(),
            message: "域名不合法".into(),
        };
        let value = serde_json::to_value(err.envelope()).expect("错误载荷必须可序列化");
        assert_eq!(value["error"]["code"], "shape.field");
        assert_eq!(value["error"]["status"], 400);
        assert_eq!(value["error"]["field"], "domain");
        assert_eq!(value["error"]["retryable"], false);
        assert_eq!(value["error"]["message"], "域名不合法");
    }

    #[test]
    fn dependency_failures_are_retryable() {
        for err in [
            AppError::Unavailable("x".into()),
            AppError::Timeout("x".into()),
        ] {
            assert!(err.envelope().error.retryable);
        }
        for err in [
            AppError::Conflict("x".into()),
            AppError::Internal("x".into()),
        ] {
            assert!(!err.envelope().error.retryable);
        }
    }

    /// 401 / 403 / 503 三类失败必须可区分：CSRF 失败仍是 403（页面行为不变），
    /// 但机器码独立，JSON 客户端能提示「刷新后重试」而不是「无权限」。
    #[test]
    fn csrf_failure_has_dedicated_code() {
        let csrf = AppError::Csrf("CSRF 校验失败，请刷新页面重试".into());
        assert_eq!(csrf.status(), StatusCode::FORBIDDEN);
        assert_eq!(csrf.error_code(), ErrorCode::CsrfInvalid);
        assert_eq!(csrf.envelope().error.code, "auth.csrf_invalid");
        assert_ne!(csrf.envelope().error.code, ErrorCode::Forbidden.as_str());
        assert!(!csrf.envelope().error.retryable);
    }

    #[test]
    fn redirect_is_not_an_error_payload() {
        // 重定向由 HTML 侧处理；这里只锁定「不会产生 3xx 之外的错误语义」。
        let err = AppError::Redirect("/sites".into());
        assert_eq!(err.status(), StatusCode::SEE_OTHER);
        assert_eq!(err.error_code(), ErrorCode::Internal);
    }

    /// 「查不到记录」是业务结果，「查不了库」是依赖故障：两者不能都塌成 500，
    /// 更不能被上层当成「未登录 / 空列表」。
    #[test]
    fn store_errors_split_missing_records_from_dependency_failure() {
        let missing: AppError = skiff_store::StoreError::NotFound("site:demo".into()).into();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert_eq!(missing.error_code(), ErrorCode::NotFound);

        let conflict: AppError = skiff_store::StoreError::Conflict("domain".into()).into();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        assert_eq!(conflict.error_code(), ErrorCode::Conflict);

        let busy: AppError = skiff_store::StoreError::Busy("db".into()).into();
        assert_eq!(busy.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(busy.error_code(), ErrorCode::Unavailable);
        assert_eq!(busy.envelope().error.code, "dependency.unavailable");
        assert!(busy.envelope().error.retryable);

        let decode: AppError = skiff_store::StoreError::Decode("bad row".into()).into();
        assert_eq!(decode.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(decode.error_code(), ErrorCode::Internal);
    }
}
