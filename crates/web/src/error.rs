use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use serde_json::json;

/// UI 侧错误：统一渲染错误页；API 侧用 `ApiError` 包一层输出 JSON。
#[derive(Debug, Clone)]
pub enum AppError {
    NotFound(String),
    Unauthorized(String),
    Forbidden(String),
    BadRequest(String),
    Conflict(String),
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
            AppError::Forbidden(_) => StatusCode::FORBIDDEN,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            AppError::Timeout(_) => StatusCode::GATEWAY_TIMEOUT,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            AppError::Redirect(_) => StatusCode::SEE_OTHER,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            AppError::NotFound(m)
            | AppError::Unauthorized(m)
            | AppError::Forbidden(m)
            | AppError::BadRequest(m)
            | AppError::Conflict(m)
            | AppError::Unavailable(m)
            | AppError::Timeout(m)
            | AppError::Internal(m)
            | AppError::Redirect(m) => m,
        }
    }

    pub fn code(&self) -> u16 {
        self.status().as_u16()
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code(), self.message())
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
        let code = self.0.status();
        (
            code,
            Json(json!({
                "error": {
                    "code": code.as_u16(),
                    "message": self.0.message(),
                }
            })),
        )
            .into_response()
    }
}

impl From<skiff_store::StoreError> for AppError {
    fn from(e: skiff_store::StoreError) -> Self {
        AppError::Internal(e.to_string())
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
