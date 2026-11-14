// 静态资源压缩规则：`build.rs`（通过 include!）与 `assets.rs` 单测共用，
// 避免「已压缩扩展名」列表在两处漂移。
// 已压缩格式再 gzip 只会增加字节，还让客户端多做一次解压。

// lib 侧只在单测断言里使用；build.rs 通过 include! 在编译期使用。
#[allow(dead_code)]
pub(crate) const ALREADY_COMPRESSED: &[&str] =
    &["woff2", "woff", "png", "jpg", "jpeg", "webp", "zip", "gz"];
