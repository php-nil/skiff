//! 站点详情页 · 附加域名 tab。

use serde_json::{json, Value};

use super::super::schema_kit::{self as kit, text_line};
use super::*;

pub fn section(data: &SiteDetailData) -> Vec<Value> {
    let site = &data.site;
    let mut body = Vec::new();
    if site.aliases.is_empty() {
        body.push(text_line("暂无附加域名。"));
    } else {
        for alias in &site.aliases {
            let mut line = vec![text_line(alias.as_str())];
            if data.can_manage {
                line.push(site_btn(
                    "删除",
                    json!({
                        "op": "domains",
                        "action": "remove",
                        "alias": alias,
                        "id": site.id,
                        "domain": site.domain
                    }),
                    "danger",
                    Some("text"),
                ));
            }
            body.push(kit::flex(json!({ "gap": 8 }), line));
        }
    }
    if data.can_manage {
        body.push(kit::flex(
            json!({ "gap": 8 }),
            vec![kit::dialog_form_button(kit::DialogForm {
                text: "新增附加域名",
                variant: "flat",
                title: "新增附加域名",
                width: 520,
                submit: site_write_action(json!({
                    "op": "domains",
                    "action": "add",
                    "id": site.id,
                    "domain": site.domain
                })),
                fields: vec![kit::required_input("alias", "附加域名", "请输入域名")],
                initial: None,
                submit_text: "添加",
                notice: "附加域名会写入当前站点 vhost；HTTPS 证书仍按主域名配置。",
            })],
        ));
    }
    vec![kit::card("附加域名", body)]
}
