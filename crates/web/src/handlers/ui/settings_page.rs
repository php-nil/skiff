//! 设置页族：安全 / 菜单 / 界面 / Docker 四页。

use super::schema_kit::{self as kit, flag, text, TableOpts, API_BASE};
use serde_json::{json, Value};

/// 只有一句说明的卡片（不带标题）。
fn notice(text: impl Into<String>) -> Value {
    json!({ "type": "card", "content": { "body": [kit::text_line(text)] } })
}

/// 安全设置：账号、口令、TOTP、面板访问与信任公钥。
pub fn security_schema(data: &Value, username: &str, role: &str) -> Value {
    let mut body = vec![json!({
        "type": "card",
        "props": { "title": "当前账号" },
        "content": { "body": [
            kit::flex(json!({"gap": 12}), vec![
                json!({ "type": "text",
                       "props": { "text": format!("{username}（{role}）"), "weight": "medium" } }),
                json!({ "type": "button",
                       "props": { "text": "退出登录", "variant": "outlined" },
                       "onEvent": { "click": { "actions": [ { "type": "skiffLogout" } ] } } })
            ]),
            kit::small_text("退出后需要重新输入口令；启用两步验证时还要动态码。")
        ] }
    })];

    let flash_error = text(data, "/flash/error");
    let flash_notice = text(data, "/flash/notice");
    if !flash_error.is_empty() {
        body.push(kit::alert("error", "", flash_error));
    }
    if !flash_notice.is_empty() {
        body.push(kit::alert("success", "", flash_notice));
    }

    let must_change = flag(data, "/account/must_change_password");
    if must_change {
        body.push(kit::alert(
            "warning",
            "首次登录必须修改口令",
            "修改口令后其他页面才恢复访问；请在下方「修改口令」表单完成修改。",
        ));
    }
    if flag(data, "/panel/initial_admin_pending") && !must_change {
        body.push(kit::alert(
            "info",
            "",
            "建议尽快修改初始口令：当前口令仍保存在数据目录的 initial-admin.txt。",
        ));
    }
    if flag(data, "/panel/plaintext_behind_proxy") {
        body.push(kit::alert(
            "warning",
            "",
            "当前为「可信反代终止 TLS」形态：面板只监听明文，请确保只有 trusted_proxies 中的反代能直连本端口。",
        ));
    }
    let listen_addr = text(data, "/panel/listen_addr");
    if text(data, "/panel/tls_mode") == "off" && listen_addr != "127.0.0.1" && listen_addr != "::1"
    {
        body.push(kit::alert(
            "warning",
            "",
            "已显式关闭 TLS 并监听非 loopback 地址：管理面为明文 HTTP，仅限内网或反向代理之后使用。",
        ));
    }

    let mut account_lines = vec![
        format!(
            "两步验证（TOTP）：{}",
            if flag(data, "/account/totp_enabled") {
                "已启用"
            } else {
                "未启用"
            }
        ),
        "修改口令成功后当前会话会轮换，无需重新登录。".to_string(),
    ];
    if must_change {
        account_lines.push("当前账号仍在使用首启口令，请尽快修改。".to_string());
    }
    body.push(kit::detail_lines("账户安全", account_lines));

    let password_field = |name: &str, label: &str, autocomplete: &str, message: &str| {
        json!({
            "type": "input-text",
            "props": {
                "name": name, "label": label, "type": "password",
                "autocomplete": autocomplete, "required": true,
                "rules": { "required": true, "messages": { "required": message } }
            }
        })
    };
    body.push(json!({
        "type": "form",
        "onEvent": { "submit": [ { "type": "skiffPasswordChange" } ] },
        "props": {
            "submitMode": "event",
            "actionsAlign": "end"
        },
        "body": [
            password_field("old_password", "当前口令", "current-password", "请输入当前口令"),
            password_field("new_password", "新口令", "new-password", "请输入新口令"),
            password_field("confirm_password", "确认新口令", "new-password", "请再次输入新口令")
        ],
        "content": {
            "actions": [
                { "type": "button", "props": { "text": "修改口令", "color": "primary" },
                  "action": { "type": "submit" } }
            ]
        }
    }));

    let totp_enabled = flag(data, "/account/totp_enabled");
    let pending_secret = text(data, "/account/totp_pending_secret");
    if !totp_enabled && !pending_secret.is_empty() {
        body.push(kit::detail_lines(
            "两步验证（TOTP）",
            vec![
                format!("密钥：{pending_secret}"),
                format!("otpauth URI：{}", text(data, "/account/totp_pending_uri")),
                "在验证器中录入后输入当前动态码完成启用。".to_string(),
            ],
        ));
        body.push(json!({
            "type": "form",
            "onEvent": { "submit": [ { "type": "skiffTotpEnable" } ] },
            "props": {
                "submitMode": "event",
                "actionsAlign": "end"
            },
            "body": [
                {
                    "type": "input-text",
                    "props": {
                        "name": "code", "label": "动态码（6 位）", "required": true,
                        "rules": { "required": true, "messages": { "required": "请输入动态码" } }
                    }
                }
            ],
            "content": {
                "actions": [
                    { "type": "button", "props": { "text": "完成启用", "color": "primary" },
                      "action": { "type": "submit" } }
                ]
            }
        }));
    } else if !totp_enabled {
        body.push(notice(
            "生成密钥后，用验证器 App 录入密钥或 otpauth URI，再输入动态码完成启用。",
        ));
        body.push(kit::flex(
            json!({ "gap": 8 }),
            vec![kit::action_button(
                "生成 TOTP 密钥",
                vec![json!({ "type": "skiffTotpSetup" })],
                "tonal",
            )],
        ));
    }

    let mut panel_lines = vec![
        format!(
            "当前监听：{}:{}",
            text(data, "/panel/listen_addr"),
            data.pointer("/panel/listen_port")
                .and_then(Value::as_i64)
                .unwrap_or(0)
        ),
        format!("入口前缀：/{}", text(data, "/panel/entry_prefix")),
        format!(
            "TLS：{}",
            match text(data, "/panel/tls_mode") {
                "on" => "已启用（本进程终止 TLS）",
                "off" => "已关闭（明文 HTTP）",
                _ => "自动推断",
            }
        ),
    ];
    let can_admin = flag(data, "/can_admin");
    if !can_admin {
        panel_lines.push("只有管理员可以修改监听端口、入口前缀与 TLS 配置。".to_string());
        body.push(kit::detail_lines("后台端口 / 安全入口 / TLS", panel_lines));
    } else {
        let initial = json!({
            "listen_addr": text(data, "/panel/listen_addr"),
            "listen_port": data.pointer("/panel/listen_port")
                .and_then(Value::as_i64).unwrap_or(0).to_string(),
            "entry_prefix": text(data, "/panel/entry_prefix"),
            "tls_mode": text(data, "/panel/tls_mode"),
            "tls_cert": text(data, "/panel/tls_cert"),
            "tls_key": text(data, "/panel/tls_key")
        });
        let mut form = kit::form(
            json!({ "type": "skiffSettingsSave" }),
            vec![
                kit::text_input("listen_addr", "监听地址（IPv4 / IPv6 字面量）"),
                kit::text_input("listen_port", "监听端口（1–65535）"),
                kit::text_input("entry_prefix", "安全入口前缀（6–32 位，字母/数字/-/_）"),
                {
                    let mut node = kit::text_input("tls_mode", "TLS 模式");
                    node["props"]["hint"] = json!("auto / on / off");
                    node
                },
                kit::text_input("tls_cert", "TLS 证书路径（PEM）"),
                kit::text_input("tls_key", "TLS 私钥路径（PEM）"),
            ],
            "保存（重启后生效）",
            Some(initial),
        );
        let fields = form["body"].as_array_mut().expect("form body");
        for field in fields.iter_mut() {
            field["props"]["required"] = json!(true);
        }
        form["props"]["successMessage"] = json!("已保存");
        body.push(form);
        body.push(notice(format!(
            "端口 / 前缀 / TLS 会写入 {}，必须重启 skiff 服务后才生效；重启前当前入口仍然可用。",
            text(data, "/panel/config_path")
        )));
    }

    if can_admin {
        let keys = data
            .get("keys")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if keys.is_empty() {
            body.push(kit::detail_lines(
                "信任公钥（TOFU）",
                vec!["信任库为空：未签名包默认拒装".to_string()],
            ));
        } else {
            body.push(kit::detail_lines(
                "信任公钥（TOFU）",
                vec![format!(
                    "共 {} 个；撤销后该公钥签名的包将无法安装/升级。",
                    keys.len()
                )],
            ));
            let rows: Vec<Value> = keys
                .iter()
                .map(|k| {
                    json!({
                        "key_id": text(k, "/key_id"),
                        "label": text(k, "/label"),
                        "fingerprint_short": text(k, "/fingerprint_short")
                    })
                })
                .collect();
            let mut table = kit::table(
                json!([
                    { "key": "fingerprint_short", "label": "指纹" },
                    { "key": "label", "label": "备注" }
                ]),
                json!(rows),
                "key_id",
                TableOpts::default(),
            );
            table["props"]["rowActions"] = json!([kit::row_button(
                "撤销",
                vec![json!({ "type": "skiffKeyRemove", "props": { "key_id": "${key_id}" } })],
                "danger"
            )]);
            body.push(table);
        }
        body.push(kit::flex(
            json!({ "gap": 8 }),
            vec![kit::dialog_form_button(kit::DialogForm {
                text: "导入信任公钥",
                variant: "tonal",
                title: "导入信任公钥",
                width: 560,
                submit: json!({ "type": "skiffKeyImport" }),
                fields: vec![
                    kit::required_input("public_key", "公钥（64 位十六进制）", "请输入公钥"),
                    kit::text_input("label", "备注（可选）"),
                ],
                initial: None,
                submit_text: "导入公钥",
                notice: "公钥为 Ed25519 的 64 位十六进制字符串，请与发布者公布值逐位核对。",
            })],
        ));
    }

    kit::page("设置", "口令、两步验证与面板访问", body)
}

/// 菜单设置：按账号保存顺序、显隐与固定。
pub fn menu_schema(data: &Value) -> Value {
    let mut body = vec![kit::alert(
        "info",
        "",
        "顺序、隐藏与固定只影响当前账号；换账号或换设备不共享。插件升级新增的菜单会自动插到相邻位置。",
    )];

    let menu_button = |text: &str, props: Value, active: bool| {
        json!({
            "type": "button",
            "props": {
                "text": text,
                "variant": if active { "tonal" } else { "text" },
                "color": "primary"
            },
            "onEvent": { "click": { "actions": [ { "type": "skiffMenuWrite", "props": props } ] } }
        })
    };

    if let Some(groups) = data.get("groups").and_then(Value::as_array) {
        for group in groups {
            let rows: Vec<Value> = group
                .get("items")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .map(|item| {
                            let mut flags: Vec<&str> = Vec::new();
                            if flag(item, "/plugin") {
                                flags.push("插件");
                            }
                            if flag(item, "/hidden") {
                                flags.push("已隐藏");
                            }
                            if flag(item, "/pinned") {
                                flags.push("已固定");
                            }
                            json!({
                                "key": text(item, "/key"),
                                "label": text(item, "/label"),
                                "state": if flags.is_empty() { "—".to_string() } else { flags.join(" / ") },
                                "toggle_action": if flag(item, "/hidden") { "show" } else { "hide" },
                                "pin_action": if flag(item, "/pinned") { "unpin" } else { "pin" }
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            if rows.is_empty() {
                continue;
            }
            let mut table = kit::table(
                json!([
                    { "key": "label", "label": "菜单" },
                    { "key": "state", "label": "状态" }
                ]),
                json!(rows),
                "key",
                TableOpts::default(),
            );
            table["props"]["rowActions"] = json!([
                menu_button("↑", json!({ "action": "up", "key": "${key}" }), false),
                menu_button("↓", json!({ "action": "down", "key": "${key}" }), false),
                menu_button(
                    "显隐",
                    json!({ "action": "${toggle_action}", "key": "${key}" }),
                    false
                ),
                menu_button(
                    "固定",
                    json!({ "action": "${pin_action}", "key": "${key}" }),
                    false
                )
            ]);
            body.push(kit::card(text(group, "/label"), vec![table]));
        }
    }

    body.push(kit::flex(
        json!({ "gap": 8 }),
        vec![menu_button("恢复默认", json!({ "action": "reset" }), false)],
    ));
    body.push(notice("恢复默认会清除本账号的顺序、隐藏与固定偏好。"));
    kit::page("菜单设置", "按账号保存显隐、顺序与固定", body)
}

/// 壳主题预设：整套配色（主色 / 页面底色 / 菜单底色）一次套用。
///
/// 三套配色对应品牌定稿的三个方向，第一套与 `skiff_app::theme` 的缺省值一致：
/// - 深潜：深墨绿导航 + 暖白内容 + 青绿主色（缺省）；
/// - 石墨：纯白导航 + 中性底 + 靛紫主色；
/// - 灯塔：近黑底 + 青柠主色（整体暗色）。
///
/// 主色与菜单底色的对比度必须 ≥ 4.5（服务端仍会校正，这里先卡住预设本身），
/// 页面底色只做极轻的色相偏移，避免整页发灰或发黄。
const UI_THEME_PRESETS: [(&str, &str, &str, &str, &str, bool); 3] = [
    (
        "深潜",
        "深墨绿导航 + 青绿主色（默认）",
        "#17a98e",
        "#f4f6f5",
        "#0c2422",
        true,
    ),
    (
        "石墨",
        "纯白导航 + 靛紫主色",
        "#5a4ae3",
        "#f5f5f7",
        "#ffffff",
        true,
    ),
    (
        "灯塔",
        "近黑底 + 青柠主色",
        "#b8f04b",
        "#080a09",
        "#0b0e0c",
        true,
    ),
];

/// 界面设置：预设 + 当前生效色板 + 自定义配色表单。
pub fn ui_schema(data: &Value) -> Value {
    if !flag(data, "/can_admin") {
        return kit::page(
            "界面设置",
            "壳主题与菜单配色（管理员可改）",
            vec![kit::detail_lines(
                "当前主题",
                vec![
                    format!("主色：{}", text(data, "/primary")),
                    format!(
                        "菜单底色：{}{}",
                        text(data, "/menu"),
                        if flag(data, "/menu_custom") {
                            "（自定义）"
                        } else {
                            "（随页面底色自动）"
                        }
                    ),
                    format!("页面底色：{}", text(data, "/page")),
                    format!(
                        "文字色：菜单 {} · 页面 {} · 主色按钮 {}",
                        text(data, "/menu_text"),
                        text(data, "/page_text"),
                        text(data, "/on_primary")
                    ),
                    "只有管理员可以修改主题。".to_string(),
                ],
            )],
        );
    }

    let mut body = vec![kit::alert(
        "info",
        "",
        "主色作用于菜单、页签、按钮与图标激活态；菜单底色由侧栏、顶栏与页签条共用，文字自动反色；页面底色决定整页明暗。主色与菜单底色对比度不足 4.5:1 时自动校正。",
    )];
    if flag(data, "/adjusted") {
        body.push(kit::alert(
            "warning",
            "",
            format!(
                "当前主色已按菜单底色校正为 {}，以保证激活态可读。",
                text(data, "/primary")
            ),
        ));
    }

    let presets: Vec<Value> = UI_THEME_PRESETS
        .iter()
        .map(|(name, desc, primary, page, menu, menu_custom)| {
            preset_card(name, desc, primary, page, menu, *menu_custom)
        })
        .collect();
    body.push(kit::card(
        "预设方案",
        vec![
            kit::text_line("每套预设包含主色、页面底色与菜单底色，点「套用」一次保存；套用后可在下方逐项微调。"),
            json!({ "type": "grid", "props": { "columns": 2, "gap": 12 }, "content": { "body": presets } }),
        ],
    ));

    body.push(kit::card(
        "当前生效",
        vec![
            swatch(
                "主色",
                text(data, "/primary"),
                &format!("按钮文字 {}", text(data, "/on_primary")),
            ),
            swatch(
                "菜单底色",
                text(data, "/menu"),
                &format!(
                    "菜单文字 {}{}",
                    text(data, "/menu_text"),
                    if flag(data, "/menu_custom") {
                        " · 自定义"
                    } else {
                        " · 自动"
                    }
                ),
            ),
            swatch(
                "页面底色",
                text(data, "/page"),
                &format!("页面文字 {}", text(data, "/page_text")),
            ),
        ],
    ));

    let color_rules = |message: &str| {
        json!({
            "pattern": "^#[0-9a-fA-F]{6}$",
            "messages": { "pattern": message }
        })
    };
    let color_field = |name: &str, label: &str, hint: &str, message: &str| {
        let mut node = kit::text_input(name, label);
        node["props"]["required"] = json!(true);
        node["props"]["rules"] = color_rules(message);
        if !hint.is_empty() {
            node["props"]["hint"] = json!(hint);
        }
        node
    };
    let initial = json!({
        "primary": text(data, "/primary"),
        "menu": text(data, "/menu"),
        "page": text(data, "/page"),
        "menu_custom": flag(data, "/menu_custom")
    });
    body.push(kit::write_card(
        "自定义配色",
        "保存后当前页自动刷新；其他已登录会话在下次加载页面时使用新配色。",
        kit::form(
            json!({ "type": "skiffUiThemeWrite" }),
            vec![
                color_field("primary", "主色（#rrggbb）", "", "请输入主色"),
                color_field(
                    "menu",
                    "菜单底色（#rrggbb）",
                    "侧栏、顶栏、页签条共用；关闭「自定义」时按页面底色自动取深/浅。",
                    "请输入菜单底色",
                ),
                json!({
                    "type": "switch",
                    "props": {
                        "name": "menu_custom",
                        "label": "自定义菜单底色",
                        "hint": "关闭时自动推导；开启后使用上面的颜色。"
                    }
                }),
                color_field("page", "页面底色（#rrggbb）", "", "请输入页面底色"),
            ],
            "保存配色",
            Some(initial),
        ),
    ));

    kit::page(
        "界面设置",
        "壳主题：预设 + 主色 / 菜单底色 / 页面底色",
        body,
    )
}

fn preset_card(
    name: &str,
    desc: &str,
    primary: &str,
    page: &str,
    menu: &str,
    menu_custom: bool,
) -> Value {
    let has_menu = menu_custom && !menu.is_empty();
    let chip = |color: &str| {
        json!({
            "type": "chip",
            "className": "skiff-swatch",
            "props": { "text": " ", "color": color, "variant": "flat", "size": "small" }
        })
    };
    json!({
        "type": "card",
        "props": { "title": name, "variant": "outlined" },
        "content": { "body": [
            kit::small_text(desc),
            kit::flex(json!({"gap": 6}), vec![
                chip(primary),
                chip(page),
                chip(if has_menu { menu } else { "#ffffff" }),
                kit::small_text(format!(
                    "主色 {} · 菜单 {}",
                    primary,
                    if has_menu { menu } else { "自动" }
                ))
            ]),
            json!({
                "type": "button",
                "props": {
                    "text": format!("套用「{name}」"),
                    "color": primary,
                    "variant": "flat",
                    "size": "small"
                },
                "action": {
                    "type": "skiffUiPreset",
                    "props": {
                        "primary": primary,
                        "page": page,
                        "menu": menu,
                        "menu_custom": menu_custom
                    }
                }
            })
        ] }
    })
}

fn swatch(label: &str, value: &str, extra: &str) -> Value {
    let mut body = vec![
        json!({ "type": "chip", "props": { "text": " ", "color": value, "variant": "flat", "size": "small" } }),
        json!({ "type": "text", "props": { "text": label, "weight": "medium" } }),
        kit::small_text(value),
    ];
    if !extra.is_empty() {
        body.push(kit::small_text(extra));
    }
    json!({
        "type": "flex",
        "props": { "gap": 8, "align": "center", "wrap": true },
        "content": { "body": body }
    })
}

/// Docker 设置：镜像加速源、镜像前缀与运行环境镜像标签。
pub fn docker_schema(data: &Value, images: &Value) -> Value {
    let mut body = vec![kit::alert(
        "info",
        "",
        "镜像加速源写入宿主 /etc/docker/daemon.json 的 registry-mirrors，用于加速所有镜像拉取。写入前会备份原文件；JSON 非法或 Docker 重启失败会回滚；检测到运行中容器时只落盘、不自动重启。",
    )];
    if !flag(data, "/can_admin") {
        let or_unset = |value: &str| {
            if value.is_empty() {
                "未设置".to_string()
            } else {
                value.to_string()
            }
        };
        let image_or_default = |value: &str| {
            if value.is_empty() {
                "默认".to_string()
            } else {
                value.to_string()
            }
        };
        body.push(kit::detail_lines(
            "镜像拉取参数",
            vec![
                format!("镜像加速源：{}", or_unset(text(data, "/registry_mirror"))),
                format!("镜像前缀：{}", or_unset(text(data, "/image_prefix"))),
                format!(
                    "Web 服务镜像标签：{}",
                    image_or_default(text(images, "/nginx_image"))
                ),
                format!(
                    "PHP 基础镜像标签：{}",
                    image_or_default(text(images, "/php_image"))
                ),
                format!(
                    "版本清单地址（固定路径，只读）：{}（同目录下按类型取 php.json / node.json / rust.json）",
                    or_unset(text(data, "/runtime_index_url"))
                ),
                "只有管理员可以修改 Docker 设置。".to_string(),
            ],
        ));
        return kit::page("Docker 设置", "镜像加速与拉取参数（管理员可改）", body);
    }

    let mut mirror = kit::text_input("registry_mirror", "镜像加速地址");
    mirror["props"]["hint"] = json!("http:// 或 https:// 开头；多个用逗号或换行分隔");
    let mut prefix = kit::text_input("image_prefix", "镜像前缀（可选）");
    prefix["props"]["hint"] =
        json!("拉取站点运行环境镜像（nginx / PHP）时拼在镜像名前；不填则直接使用原镜像名");
    body.push(kit::write_card(
        "镜像拉取参数",
        "只写宿主配置，不改动已有容器。",
        kit::form(
            json!({ "type": "skiffDockerSettingsWrite" }),
            vec![mirror, prefix],
            "保存",
            Some(json!({
                "registry_mirror": text(data, "/registry_mirror"),
                "image_prefix": text(data, "/image_prefix")
            })),
        ),
    ));

    let mut nginx = kit::text_input("nginx_image", "Web 服务镜像标签");
    nginx["props"]["hint"] =
        json!("OpenResty 基础镜像；留空使用内置默认值。完整镜像 = 镜像前缀 + 此 tag");
    let mut php = kit::text_input("php_image", "PHP 基础镜像标签");
    php["props"]["hint"] = json!("仅影响之后新建或重建的 PHP 实例；完整镜像 = 镜像前缀 + 此 tag");
    body.push(kit::write_card(
        "运行环境镜像标签",
        "与上方「镜像前缀」共同决定拉取的完整镜像名；留空使用内置默认值。",
        kit::form(
            json!({ "type": "skiffRuntimeImages" }),
            vec![nginx, php],
            "保存镜像参数",
            Some(json!({
                "nginx_image": text(images, "/nginx_image"),
                "php_image": text(images, "/php_image")
            })),
        ),
    ));

    kit::page("Docker 设置", "镜像拉取参数", body)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 预设必须自身达标：主色在菜单底色上 ≥ 4.5:1，页面底色与菜单底色不能同色，
    /// 否则「套用预设」会被服务端静默校正成另一个颜色，色卡与结果对不上。
    #[test]
    fn theme_presets_hit_contrast_and_are_named() {
        for (name, desc, primary, page, menu, menu_custom) in UI_THEME_PRESETS {
            assert!(!name.is_empty() && !desc.is_empty(), "预设必须有名称与说明");
            assert!(menu_custom, "预设都要显式给菜单底色，不能靠自动推导");
            let ratio = skiff_app::theme::contrast(primary, menu);
            assert!(
                ratio >= skiff_app::theme::MIN_CONTRAST,
                "预设「{name}」主色 {primary} 在菜单 {menu} 上对比度不足：{ratio:.2}"
            );
            assert_ne!(page, menu, "预设「{name}」页面底色与菜单底色不能相同");
        }
    }

    /// 第一套预设必须与缺省主题一致：否则用户点「恢复默认」与点「深潜」得到两种结果。
    #[test]
    fn first_preset_matches_default_theme() {
        let (_, _, primary, page, menu, _) = UI_THEME_PRESETS[0];
        assert_eq!(primary, skiff_app::theme::DEFAULT_PRIMARY);
        assert_eq!(page, skiff_app::theme::DEFAULT_PAGE);
        assert_eq!(menu, skiff_app::theme::DEFAULT_MENU);
    }
}
