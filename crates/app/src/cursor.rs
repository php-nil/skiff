//! 列表游标编解码（SDUI D10）。
//!
//! 游标是「上一页最后一条的排序键」＋「产生它的筛选条件指纹」，由服务端用 HMAC
//! 签名后 base64url 编码。客户端既读不出结构也改不了内容：
//!
//! - 改任意一个字符 → 签名不匹配（`Tampered`）；
//! - 换一套筛选条件继续翻页 → 指纹不匹配（`FilterMismatch`）；
//! - 拿另一个资源/另一把密钥签发的游标 → 同样按签名失败拒绝。
//!
//! 这里只提供编解码，不重写任何现有列表查询；排序键与筛选字段由各资源契约声明。

use base64::Engine;
use serde::{Deserialize, Serialize};
use skiff_identity::hmac_sha256_hex;

const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::URL_SAFE_NO_PAD;

/// 载荷结构版本：字段布局变化时递增，旧游标按非法拒绝而不是猜读。
const CURSOR_VERSION: u8 = 1;
/// HMAC-SHA256 十六进制签名长度。
const SIG_LEN: usize = 64;

/// 游标长度上限：正常游标不到 200 字符，超长输入直接拒绝，避免解码大对象。
pub const CURSOR_MAX_LEN: usize = 512;

/// 游标被拒绝的原因。文案面向调用方直接回 400，不做二次解释。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorError {
    /// 不是 `payload.signature` 形式、base64/JSON 非法、版本未知或超长。
    Malformed,
    /// 签名不匹配：被篡改、换了密钥，或来自另一个资源。
    Tampered,
    /// 签名有效，但筛选指纹与当前请求不一致（换了筛选条件还沿用旧游标）。
    FilterMismatch,
}

impl CursorError {
    pub fn message(self) -> &'static str {
        match self {
            CursorError::Malformed => "游标格式非法",
            CursorError::Tampered => "游标校验失败，请从第一页重新开始",
            CursorError::FilterMismatch => "游标与当前筛选条件不匹配，请从第一页重新开始",
        }
    }
}

impl std::fmt::Display for CursorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for CursorError {}

#[derive(Debug, Serialize, Deserialize)]
struct Payload {
    v: u8,
    /// 排序键，按索引列顺序；数字以十进制字符串保存，避免 JSON 数字精度问题。
    sort: Vec<String>,
    /// 筛选指纹（由调用方按声明过的筛选字段计算）。
    filter: String,
}

/// 某个资源的游标编解码器。`scope` 做域分隔：审计游标不能拿到作业列表用。
#[derive(Debug, Clone)]
pub struct CursorCodec {
    secret: String,
    scope: String,
}

impl CursorCodec {
    pub fn new(secret: impl Into<String>, scope: impl Into<String>) -> Self {
        Self {
            secret: secret.into(),
            scope: scope.into(),
        }
    }

    /// 编码：`sort` 为排序键（按索引列顺序），`filter_fp` 为筛选指纹。
    pub fn encode(&self, sort: &[String], filter_fp: &str) -> String {
        let payload = Payload {
            v: CURSOR_VERSION,
            sort: sort.to_vec(),
            filter: filter_fp.to_string(),
        };
        let json = serde_json::to_vec(&payload).expect("游标载荷是纯字符串结构，序列化不会失败");
        let body = B64.encode(json);
        let sig = self.sign(&body);
        format!("{body}.{sig}")
    }

    /// 解码并校验。先验签名再比对指纹：被改过的载荷报 `Tampered`，
    /// 而不是把篡改后的筛选指纹当成「条件不匹配」放过去。
    pub fn decode(&self, cursor: &str, filter_fp: &str) -> Result<Vec<String>, CursorError> {
        if cursor.len() > CURSOR_MAX_LEN {
            return Err(CursorError::Malformed);
        }
        let (body, sig) = cursor.split_once('.').ok_or(CursorError::Malformed)?;
        if body.is_empty() || sig.len() != SIG_LEN || !sig.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(CursorError::Malformed);
        }
        if !skiff_identity::token_matches(&self.sign(body), sig) {
            return Err(CursorError::Tampered);
        }
        let raw = B64.decode(body).map_err(|_| CursorError::Malformed)?;
        let payload: Payload = serde_json::from_slice(&raw).map_err(|_| CursorError::Malformed)?;
        if payload.v != CURSOR_VERSION {
            return Err(CursorError::Malformed);
        }
        if payload.filter != filter_fp {
            return Err(CursorError::FilterMismatch);
        }
        Ok(payload.sort)
    }

    fn sign(&self, body: &str) -> String {
        hmac_sha256_hex(&self.secret, &format!("cursor:{}", self.scope), body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codec() -> CursorCodec {
        CursorCodec::new("secret", "audit")
    }

    fn keys() -> Vec<String> {
        vec!["1730000000".into(), "42".into()]
    }

    #[test]
    fn roundtrip_preserves_sort_keys() {
        let c = codec();
        let cur = c.encode(&keys(), "action=login");
        assert_eq!(c.decode(&cur, "action=login").unwrap(), keys());
    }

    /// 游标对调用方不透明：不应出现明文排序键。
    #[test]
    fn cursor_does_not_leak_plaintext_keys() {
        let cur = codec().encode(&keys(), "action=login");
        assert!(!cur.contains("1730000000"));
        assert!(!cur.contains("action=login"));
    }

    #[test]
    fn filter_fingerprint_must_match() {
        let c = codec();
        let cur = c.encode(&keys(), "action=login");
        assert_eq!(
            c.decode(&cur, "action=delete"),
            Err(CursorError::FilterMismatch)
        );
    }

    /// 改签名最后一个字符：结构仍合法，必须报篡改。
    #[test]
    fn tampered_signature_is_rejected() {
        let c = codec();
        let cur = c.encode(&keys(), "f");
        let (body, sig) = cur.split_once('.').unwrap();
        let last = if sig.ends_with('0') { '1' } else { '0' };
        let forged = format!("{body}.{}{last}", &sig[..sig.len() - 1]);
        assert_eq!(c.decode(&forged, "f"), Err(CursorError::Tampered));
    }

    /// 改载荷（换一个 base64 字符）：签名先失败，不会被解码后误判成条件不匹配。
    #[test]
    fn tampered_payload_is_rejected() {
        let c = codec();
        let cur = c.encode(&keys(), "f");
        let (body, sig) = cur.split_once('.').unwrap();
        let mut chars: Vec<char> = body.chars().collect();
        chars[0] = if chars[0] == 'A' { 'B' } else { 'A' };
        let forged: String = chars.into_iter().collect();
        assert_eq!(
            c.decode(&format!("{forged}.{sig}"), "f"),
            Err(CursorError::Tampered)
        );
    }

    /// 换密钥或换资源（scope）都不通用。
    #[test]
    fn cursor_is_bound_to_secret_and_scope() {
        let cur = codec().encode(&keys(), "f");
        assert_eq!(
            CursorCodec::new("other", "audit").decode(&cur, "f"),
            Err(CursorError::Tampered)
        );
        assert_eq!(
            CursorCodec::new("secret", "jobs").decode(&cur, "f"),
            Err(CursorError::Tampered)
        );
    }

    #[test]
    fn malformed_cursors_are_rejected() {
        let c = codec();
        for bad in ["", ".", "abc", "abc.def", "a.b.c", "!!!.abc"] {
            assert_eq!(c.decode(bad, "f"), Err(CursorError::Malformed), "{bad}");
        }
        let long = "a".repeat(CURSOR_MAX_LEN + 1);
        assert_eq!(c.decode(&long, "f"), Err(CursorError::Malformed));
    }

    /// 签名合法但版本未知：按非法拒绝，避免旧游标被按新结构误读。
    #[test]
    fn unknown_version_is_rejected() {
        let c = codec();
        let payload = Payload {
            v: CURSOR_VERSION + 1,
            sort: keys(),
            filter: "f".into(),
        };
        let body = B64.encode(serde_json::to_vec(&payload).unwrap());
        let sig = c.sign(&body);
        assert_eq!(
            c.decode(&format!("{body}.{sig}"), "f"),
            Err(CursorError::Malformed)
        );
    }
}
