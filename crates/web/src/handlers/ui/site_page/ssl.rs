//! 站点详情页 · SSL / HTTPS tab。

use serde_json::{json, Value};

use super::super::schema_kit::{self as kit, or_dash, text_line};
use super::*;

pub fn section(data: &SiteDetailData) -> Vec<Value> {
    let site = &data.site;
    let ssl = &data.ssl;
    let mut body = vec![
        text_line(if ssl.cert_exists {
            "已检测到证书文件"
        } else {
            "未检测到证书文件"
        }),
        text_line(format!("证书路径：{}", or_dash(&ssl.cert_path))),
        text_line(format!("主体：{}", or_dash(&ssl.subject))),
        text_line(format!("签发者：{}", or_dash(&ssl.issuer))),
        text_line(format!("到期：{}", or_dash(&ssl.expires))),
    ];
    if data.can_manage {
        let upload_action = site_write_action(
            json!({ "op": "ssl", "action": "upload", "id": site.id, "domain": site.domain }),
        );
        body.push(kit::flex(
            json!({ "gap": 8, "wrap": true }),
            vec![
                site_btn(
                    "生成自签证书",
                    json!({ "op": "ssl", "action": "generate", "id": site.id, "domain": site.domain }),
                    "primary",
                    None,
                ),
                site_btn(
                    if ssl.force_https {
                        "关闭强制 HTTPS"
                    } else {
                        "开启强制 HTTPS"
                    },
                    json!({ "op": "ssl", "action": "force", "enabled": !ssl.force_https, "id": site.id, "domain": site.domain }),
                    "secondary",
                    None,
                ),
                site_btn(
                    "关闭 HTTPS",
                    json!({ "op": "ssl", "action": "disable", "id": site.id, "domain": site.domain }),
                    "warning",
                    None,
                ),
                site_btn(
                    "删除证书",
                    json!({ "op": "ssl", "action": "delete_cert", "id": site.id, "domain": site.domain }),
                    "danger",
                    Some("text"),
                ),
                json!({
                    "type": "button",
                    "props": { "text": "上传已有证书（PEM）", "variant": "tonal" },
                    "action": {
                        "type": "dialog",
                        "props": {
                            "title": "上传已有证书（PEM）",
                            "width": 620
                        },
                        "content": {
                            "body": [
                                kit::alert(
                                    "info",
                                    "",
                                    "上传后将替换现有证书并自动启用 HTTPS；格式错误的 PEM 会被拒绝。"
                                ),
                                kit::form_body(upload_action,
                                    vec![
                                        json!({
                                            "type": "textarea",
                                            "props": {
                                                "name": "cert_pem",
                                                "label": "证书 PEM",
                                                "rows": 8,
                                                "placeholder": "-----BEGIN CERTIFICATE-----"
                                            }
                                        }),
                                        json!({
                                            "type": "textarea",
                                            "props": {
                                                "name": "key_pem",
                                                "label": "私钥 PEM（保存为 0600）",
                                                "rows": 8,
                                                "placeholder": "-----BEGIN PRIVATE KEY-----"
                                            }
                                        })
                                    ],
                                    None,
                                )
                            ],
                            "footer": [
                                {
                                    "type": "button",
                                    "props": { "text": "取消", "variant": "outlined" },
                                    "action": { "type": "close" }
                                },
                                {
                                    "type": "button",
                                    "props": { "text": "上传并启用", "color": "primary" },
                                    "action": { "type": "submit" }
                                }
                            ]
                        }
                    }
                })
            ],
        ));
    }
    vec![kit::card("证书 / HTTPS", body)]
}
