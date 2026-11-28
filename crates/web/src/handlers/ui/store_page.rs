//! 商店页 Schema：目录快照、分区条目与内核升级通道。
//!
//! 安装确认页不在服务端：它依赖上一步 `POST /store/install` 返回的一次性 token，
//! 只存在于浏览器内存（见 09-JSON_PAGES.md 的例外表）。

use super::schema_kit::{self as kit, flag, text, TableOpts, API_BASE};
use serde_json::{json, Value};

pub fn store_schema(data: &Value) -> Value {
    let mut body = Vec::new();
    let state_code = text(data, "/state/code");
    if !state_code.is_empty() && state_code != "fresh" {
        let message = text(data, "/message");
        body.push(kit::alert(
            if state_code == "rejected" {
                "error"
            } else {
                "warning"
            },
            &format!("目录状态：{}", text(data, "/state/label")),
            if message.is_empty() {
                "目录当前不是最新状态，展示缓存或空列表。"
            } else {
                message
            },
        ));
    }
    if flag(data, "/allow_unsigned") {
        body.push(kit::alert(
            "warning",
            "",
            "当前允许未签名渠道（开发模式），生产环境应导入固定公钥。",
        ));
    }

    body.push(kit::detail_lines(
        "目录信息",
        vec![
            format!(
                "渠道：{} · seq：{}",
                text(data, "/channel"),
                data.pointer("/seq")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into())
            ),
            format!("拉取时间：{}", {
                let v = text(data, "/fetched_at");
                if v.is_empty() {
                    "—"
                } else {
                    v
                }
            }),
            format!("来源：{}", {
                let v = text(data, "/source");
                if v.is_empty() {
                    "—"
                } else {
                    v
                }
            }),
            format!(
                "条目总数：{}",
                data.pointer("/total")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "0".into())
            ),
            format!(
                "公钥已配置：{}",
                if flag(data, "/key_configured") {
                    "是"
                } else {
                    "否"
                }
            ),
        ],
    ));

    let can_install = flag(data, "/can_install");
    if let Some(sections) = data.get("sections").and_then(Value::as_array) {
        for section in sections {
            let items = section
                .get("items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut table_body = Vec::new();
            if items.is_empty() {
                table_body.push(kit::text_line(format!(
                    "{}暂无条目",
                    text(section, "/title")
                )));
            } else {
                let mut table = kit::table(
                    json!([
                        { "key": "name", "label": "名称" },
                        { "key": "version", "label": "版本" },
                        { "key": "source", "label": "来源" },
                        { "key": "compatibility", "label": "兼容性" },
                        { "key": "updated_at", "label": "更新时间" },
                        { "key": "note", "label": "说明" },
                        { "key": "install_note", "label": "安装提示" }
                    ]),
                    json!(items),
                    "name",
                    TableOpts::default(),
                );
                // 只有「插件」分区带安装入口：目录里其它分区是镜像/内核条目。
                if can_install && text(section, "/title") == "插件" {
                    table["props"]["rowActions"] = json!([kit::action_button(
                        "安装 / 升级",
                        vec![json!({ "type": "skiffStoreInstall" })],
                        "tonal"
                    )]);
                }
                table_body.push(table);
            }
            let mut card = kit::card(text(section, "/title"), table_body);
            let hint = text(section, "/hint");
            if !hint.is_empty() {
                card["props"]["subtitle"] = json!(hint);
            }
            body.push(card);
        }
    }

    let upgrade = data.get("upgrade").cloned().unwrap_or(Value::Null);
    let mut upgrade_lines = vec![
        format!(
            "当前版本：{}（{}）",
            text(&upgrade, "/current"),
            text(&upgrade, "/arch")
        ),
        format!("目录可用版本：{}", {
            let v = text(&upgrade, "/available");
            if v.is_empty() {
                "无"
            } else {
                v
            }
        }),
        format!("渠道：{}", text(&upgrade, "/channel")),
        format!("最低来源版本：{}", {
            let v = text(&upgrade, "/min_from");
            if v.is_empty() {
                "—"
            } else {
                v
            }
        }),
    ];
    let changelog = text(&upgrade, "/changelog_url");
    if !changelog.is_empty() {
        upgrade_lines.push(format!("更新说明：{changelog}"));
    }
    let note = text(&upgrade, "/note");
    if !note.is_empty() {
        upgrade_lines.push(note.to_string());
    }
    body.push(kit::detail_lines("内核升级通道", upgrade_lines));
    let mut actions = Vec::new();
    if flag(&upgrade, "/can_download") && !text(&upgrade, "/available").is_empty() {
        actions.push(kit::action_button(
            &format!("下载升级包 {}", text(&upgrade, "/available")),
            vec![json!({ "type": "skiffUpgradeDownload",
                   "props": { "version": text(&upgrade, "/available") } })],
            "tonal",
        ));
    }

    let _ = API_BASE;
    kit::page_with_actions("商店", "目录快照与插件安装", actions, body)
}
