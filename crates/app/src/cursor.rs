//! 列表游标编解码（SDUI D10）。
//!
//! 游标是「上一页最后一条的排序键」＋「产生它的筛选条件指纹」，由服务端用 HMAC
//! 签名后 base64url 编码。安全性来自签名而不是编码：载荷可以 base64url 解开看到，
//! 但**无法伪造**，也改不了任何一位：
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

/// 游标长度上限。审计/作业游标不到 200 字符，但文件列表的排序键含文件名
/// （单段最长 255 字节），两列排序键编码后约 540 字符；取 1024 留出余量，
/// 同时仍拒绝超长输入，避免解码大对象。
pub const CURSOR_MAX_LEN: usize = 1024;

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
    /// 筛选指纹的 SHA-256。存摘要而不是原文：长度恒定（64 字符），
    /// 游标大小不随搜索词/路径等筛选值增长，也不把筛选原文带进 URL 与日志。
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
            filter: filter_digest(filter_fp),
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
        if payload.filter != filter_digest(filter_fp) {
            return Err(CursorError::FilterMismatch);
        }
        Ok(payload.sort)
    }

    fn sign(&self, body: &str) -> String {
        hmac_sha256_hex(&self.secret, &format!("cursor:{}", self.scope), body)
    }
}

/// 筛选指纹入载荷前先摘要：把可变长输入压成固定 64 字符，保证游标大小有上界。
fn filter_digest(filter_fp: &str) -> String {
    skiff_identity::sha256_hex(filter_fp)
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

    /// 载荷是 base64url 编码（不是加密）：这里只确认排序键与筛选原文不会以字面量
    /// 出现在 URL 或日志里；防伪由签名负责，见下面的篡改用例。
    #[test]
    fn cursor_payload_is_encoded_not_plaintext() {
        let cur = codec().encode(&keys(), "action=login");
        assert!(!cur.contains("1730000000"));
        assert!(!cur.contains("action=login"));
    }

    /// 文件列表的排序键含 255 字节文件名：服务端签出的游标必须仍能自己解开，
    /// 否则翻页会在含长文件名的页上直接报「游标格式非法」。
    #[test]
    fn long_file_name_key_roundtrips_within_limit() {
        let c = codec();
        let name = "名".repeat(85);
        assert_eq!(name.len(), 255, "用例本身要覆盖最长的单段文件名");
        let keys = vec!["0".to_string(), name];
        let filter = "path=/www/sites/example.com/index&sort=name";
        let cur = c.encode(&keys, filter);
        assert!(
            cur.len() <= CURSOR_MAX_LEN,
            "游标 {} 字符超过上限 {CURSOR_MAX_LEN}",
            cur.len()
        );
        assert_eq!(c.decode(&cur, filter).unwrap(), keys);
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
