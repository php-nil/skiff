//! budding 壳与静态产物：boot 脚本契约、页面 JSON 装配、菜单与主题、静态资产缓存与引用可解析性。

use crate::common::*;

/// 浏览器默认探测 `/favicon.ico`：必须 200，且缓存策略可被浏览器复用一天。
/// 页面同时用 `<link rel="icon">` 指向带版本查询的静态 SVG。
#[tokio::test]
async fn favicon_is_served_with_reasonable_cache() {
    let h = harness();
    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/favicon.ico")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let cc = resp
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = body_text(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ct.as_deref(), Some("image/svg+xml"));
    assert!(
        cc.unwrap_or_default().contains("max-age=86400"),
        "favicon 应允许缓存一天"
    );
    assert!(
        body.contains(r#"viewBox="0 0 32 32""#) && body.contains("<svg"),
        "favicon 必须是可缩放矢量图标"
    );

    let resp = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/p/static/favicon.svg?v=2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("public, max-age=31536000, immutable"),
        "带版本查询的静态图标应走 immutable"
    );
}

#[tokio::test]
async fn prefixed_static_assets_are_served() {
    let h = harness();
    let root = h
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/static/skiff.js")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        root.status(),
        StatusCode::NOT_FOUND,
        "根路径不得再兜底静态资源"
    );
    for uri in ["/p/static/skiff.js"] {
        let resp = h
            .app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{uri} 应可匿名获取");
        let ct = resp
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            ct.contains("javascript"),
            "{uri} 的 Content-Type 不对: {ct}"
        );
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("skiff.theme.request"), "{uri} 内容不应为空壳");
    }
}

/// 计划任务：新建任务是列表页右上角的按钮 + 页内弹窗，不是 L3 独立视图。
/// 守住真实约束：有效视图集合只有 list；旧的 new 视图不得复活。
#[test]
fn cron_new_task_is_a_list_button_not_a_separate_view() {
    let m = skiff_pkgfmt::Manifest::parse(include_str!("../../../../plugins/cron/manifest.toml"))
        .expect("cron manifest 必须可解析");
    assert_eq!(m.menu.len(), 1, "cron 只应声明一个 L2 菜单");
    let menu = &m.menu[0];
    assert_eq!(menu.id, "jobs");
    let views = menu.effective_views();
    assert_eq!(
        views,
        vec!["list"],
        "cron 的有效视图集合只能是 [list]，实际 {views:?}"
    );
    assert!(
        !views.contains(&"new"),
        "「新建任务」是按钮，不得再作为独立视图存在"
    );

    let list = include_str!("../../../../plugins/cron/ui/list.html");
    assert!(
        list.contains(r#"id="btn-new""#),
        "列表页右上角必须有「新建任务」按钮"
    );
    assert!(
        !list.contains("<h1"),
        "插件页面不得渲染页面级标题（标题由内核提供）"
    );

    let js = include_str!("../../../../plugins/cron/ui/app.js");
    assert!(
        js.contains(r#"id="job-dialog""#),
        "新建与编辑必须复用同一个页内弹窗"
    );
    assert!(
        !js.contains("new.html"),
        "列表页脚本不得再跳转到独立的「新建任务」页面"
    );
}

/// 导航交互：静态脚本里不得再有悬停展开；单开手风琴、点空白不收起、Escape 回焦箭头。
#[test]
fn nav_js_has_no_hover_and_is_accordion() {
    let js = include_str!("../../static/skiff.js");
    assert!(!js.contains("mouseenter"), "导航不得再监听 hover 展开");
    assert!(!js.contains("mouseleave"), "导航不得再监听 hover 收起");
    assert!(!js.contains("hoverTimer"), "不得保留 320ms 悬停辅助计时器");
    assert!(
        !js.contains("(hover: hover)"),
        "不得保留触摸设备/指针设备的悬停判断"
    );
    assert!(js.contains("function openKey"), "手风琴需要统一的单开函数");
    assert!(
        js.contains("g.dataset.menuKey === key"),
        "展开一个分组前必须先收起其他分组（同一时间只开一个）"
    );
    assert!(
        js.contains("sessionStorage.setItem"),
        "展开状态应在会话内记忆"
    );
    assert!(
        js.contains(r#"menu.setAttribute("hidden", "")"#),
        "关闭面板必须写回 hidden，不能进入 Tab 顺序"
    );
    assert!(
        js.contains("caret.focus()"),
        "Escape 收起后焦点应回到箭头按钮"
    );
    // 点空白不收起：全局 click 只处理窄屏抽屉，不再按 `.nav-group` 关闭导航。
    assert!(
        !js.contains(r#"target.closest("[data-nav-group]")"#),
        "点内容区/空白不得收起导航"
    );
    assert!(
        js.contains(r#"if (e.key !== "Escape") return;"#),
        "只有 Escape 才收起当前分组"
    );
}

/// D2：侧栏 `.nav-menu{display:flex}` 权重高于通用 `[hidden]`，必须有显式收起规则。
#[test]
fn d2_sidebar_collapsed_menu_is_hidden() {
    let css = include_str!("../../static/skiff.css");
    assert!(
        css.contains("body[data-layout=\"left\"] .nav-menu[hidden] { display: none; }"),
        "侧栏布局必须显式覆盖 display:flex，收起后二级菜单才真的不可见"
    );
}

/// D3：插件 iframe 要跟随面板主题；插件缺省深色，旧面板不下发主题时观感不变。
#[test]
fn d3_plugin_ui_adapts_to_theme_message() {
    let css = include_str!("../../static/skiff.css");
    let js = include_str!("../../static/skiff.js");
    assert!(
        css.contains("--plugin-frame: #f6f8fa;") && css.contains("--plugin-frame: #0b0d11;"),
        "父页面 iframe 底衬必须按浅/深主题切换"
    );
    assert!(
        js.contains(r#"postMessage({ type: "skiff.theme", theme: theme }, "*")"#),
        "父页面必须向插件 iframe 广播主题"
    );
    assert!(
        js.contains(r#"iframe.mount-frame, iframe.plugin-frame"#),
        "槽位 iframe 与插件详情 iframe 都要收到主题"
    );
    assert!(
        js.contains(r#"msg.type !== "skiff.theme.request""#),
        "父页面必须响应插件主动索取主题，覆盖 iframe 先加载的时序"
    );
    assert!(
        js.contains(
            r#"window.addEventListener("load", function () { broadcastTheme(current()); });"#
        ),
        "父页面 window.load 后必须再次广播，避免 iframe 先加载时漏掉消息"
    );

    // 插件侧：页面 chrome 用变量，默认深色，收到 skiff.theme 才切浅色。
    for (name, html, js) in [
        (
            "guard-containers",
            include_str!("../../../../plugins/guard/ui/containers.html"),
            include_str!("../../../../plugins/guard/ui/app.js"),
        ),
        (
            "guard-events",
            include_str!("../../../../plugins/guard/ui/events.html"),
            include_str!("../../../../plugins/guard/ui/app.js"),
        ),
        (
            "cron-list",
            include_str!("../../../../plugins/cron/ui/list.html"),
            include_str!("../../../../plugins/cron/ui/app.js"),
        ),
        (
            "monitor-overview",
            include_str!("../../../../plugins/monitor/ui/overview.html"),
            include_str!("../../../../plugins/monitor/ui/app.js"),
        ),
        (
            "monitor-history",
            include_str!("../../../../plugins/monitor/ui/history.html"),
            include_str!("../../../../plugins/monitor/ui/app.js"),
        ),
        (
            "monitor-health",
            include_str!("../../../../plugins/monitor/ui/health.html"),
            include_str!("../../../../plugins/monitor/ui/health.js"),
        ),
        (
            "monitor-network",
            include_str!("../../../../plugins/monitor/ui/network.html"),
            include_str!("../../../../plugins/monitor/ui/app.js"),
        ),
        (
            "monitor-process",
            include_str!("../../../../plugins/monitor/ui/process.html"),
            include_str!("../../../../plugins/monitor/ui/app.js"),
        ),
        (
            "sources-main",
            include_str!("../../../../plugins/sources/ui/main.html"),
            include_str!("../../../../plugins/sources/ui/app.js"),
        ),
        (
            "testkit-main",
            include_str!("../../../../plugins/testkit/ui/main.html"),
            include_str!("../../../../plugins/testkit/ui/app.js"),
        ),
        (
            "testkit-mount",
            include_str!("../../../../plugins/testkit/ui/mount.html"),
            include_str!("../../../../plugins/testkit/ui/mount.js"),
        ),
        (
            "testkit-check",
            include_str!("../../../../plugins/testkit/ui/check.html"),
            include_str!("../../../../plugins/testkit/ui/app.js"),
        ),
        (
            "testkit-group",
            include_str!("../../../../plugins/testkit/ui/group.html"),
            include_str!("../../../../plugins/testkit/ui/app.js"),
        ),
        (
            "testkit-single",
            include_str!("../../../../plugins/testkit/ui/single.html"),
            include_str!("../../../../plugins/testkit/ui/app.js"),
        ),
    ] {
        assert!(
            html.contains(r#"html[data-theme="light"]"#),
            "{name} 必须提供浅色主题变量覆盖"
        );
        assert!(
            html.contains("background: var(--ui-bg)"),
            "{name} 的页面底色必须走变量"
        );
        assert!(
            html.contains("color-scheme: dark;"),
            "{name} 默认必须保持深色，旧面板不下发主题时不破裂"
        );
        assert!(
            js.contains(r#".type === "skiff.theme""#),
            "{name} 必须监听父页面的主题消息"
        );
        assert!(
            js.contains(r#"type: "skiff.theme.request""#),
            "{name} 必须在脚本执行与 load 时向父页面索取主题"
        );
        assert!(
            !html.contains("<h1") && !html.contains("data-plugin-title"),
            "{name} 不得渲染页面级标题（标题统一由内核提供）"
        );
    }

    // 共用 `ui/base.css` 的页面（ftp / firewall）：主题变量与页面底色在样式表里，
    // 断言要落到样式表，否则「页面没有内联变量」会被误判成不合规。
    for (name, pages, css) in [
        (
            "ftp",
            [
                include_str!("../../../../plugins/ftp/ui/accounts.html"),
                include_str!("../../../../plugins/ftp/ui/runtime.html"),
            ],
            include_str!("../../../../plugins/ftp/ui/base.css"),
        ),
        (
            "firewall",
            [
                include_str!("../../../../plugins/firewall/ui/rules.html"),
                include_str!("../../../../plugins/firewall/ui/fail2ban.html"),
            ],
            include_str!("../../../../plugins/firewall/ui/base.css"),
        ),
    ] {
        assert!(
            css.contains(r#"html[data-theme="light"]"#)
                && css.contains("color-scheme: dark;")
                && css.contains("background: var(--ui-bg)"),
            "{name} 的 base.css 必须提供深/浅两套变量且底色走变量"
        );
        for page in pages {
            assert!(
                page.contains(r#"href="base.css""#) && !page.contains("<h1"),
                "{name} 的页面必须引用 base.css，且不渲染页面级标题（标题统一由内核提供）"
            );
        }
    }

    // 工作台插件侧：上报内容高度，父页面才能消除内外双滚动。
    for (name, js) in [
        ("cron", include_str!("../../../../plugins/cron/ui/app.js")),
        ("guard", include_str!("../../../../plugins/guard/ui/app.js")),
        (
            "monitor",
            include_str!("../../../../plugins/monitor/ui/app.js"),
        ),
        ("ftp", include_str!("../../../../plugins/ftp/ui/app.js")),
        (
            "sources",
            include_str!("../../../../plugins/sources/ui/app.js"),
        ),
        (
            "firewall",
            include_str!("../../../../plugins/firewall/ui/app.js"),
        ),
        (
            "testkit",
            include_str!("../../../../plugins/testkit/ui/app.js"),
        ),
    ] {
        assert!(
            js.contains(r#"type: "skiff.height""#) && js.contains("reportHeight"),
            "{name} 必须向父页面上报内容高度"
        );
    }
}

/// 主题一致性：bfcache 恢复后必须按 localStorage 重新 apply，
/// 避免出现「localStorage=light 但 html data-theme=dark」或按钮 aria 未同步。
#[test]
fn d4b_theme_resyncs_after_swap_and_bfcache() {
    let js = include_str!("../../static/skiff.js");
    assert!(
        js.contains("function syncFromStorage"),
        "必须有统一的按存储值重新落地函数"
    );
    assert!(
        js.contains(r#"var theme = storedTheme() || current();"#),
        "重新落地时以 localStorage 为准"
    );
    assert!(
        js.contains(r#"window.addEventListener("pageshow", syncFromStorage)"#),
        "bfcache 前进/后退恢复时必须重新 apply"
    );
    assert!(
        !js.contains("htmx"),
        "htmx 已随旧 SSR 局部刷新端点下线，脚本里不得再留 htmx 分支"
    );
}

/// 没有随包 schema 的旧 HTML 插件视图自动回落到 `?legacy=1`，而不是渲染失败页。
#[test]
fn plugin_view_falls_back_to_legacy_when_schema_missing() {
    let js = include_str!("../../static/budding/budding-boot.js");
    assert!(
        js.contains("legacyUrl.searchParams.set('legacy', '1')")
            && js.contains("location.replace(legacyUrl.pathname + legacyUrl.search)"),
        "schema 404 时必须自动回落旧 SSR 工作台"
    );
}

/// 任务页筛选：关键字必须随页面骨架请求带进服务端，再由表格数据源复用。
#[test]
fn budding_tasks_page_keeps_keyword_filter() {
    // 「关键字 → 骨架请求」的拼装已收进 `routes.rs::page_source`。
    let source = include_str!("../../src/handlers/ui/routes.rs");
    assert!(
        source.contains("api(\"/tasks?limit=1&with_schema=true\")"),
        "任务页骨架必须由服务端产出"
    );
    assert!(
        source.contains("format!(\"&q={}\", enc(s.get(\"q\")))"),
        "页面骨架请求必须带上关键字，否则筛选后的骨架与数据不一致"
    );
    let page = include_str!("../../src/handlers/ui/tasks_page.rs");
    assert!(
        page.contains(r#"json!({ "q": keyword })"#),
        "表格数据源必须复用同一份关键字"
    );
    assert!(page.contains("清除筛选"), "必须有清除筛选入口");
}

/// 插件 UI 小回归：内置任务不渲染删除按钮；累计流量按字节自适应进位。
#[test]
fn plugin_ui_hides_builtin_delete_and_formats_traffic_bytes() {
    let cron = include_str!("../../../../plugins/cron/ui/app.js");
    assert!(
        cron.contains("function isBuiltinTask") && cron.contains("isBuiltinTask(j.id)"),
        "cron 列表必须按 builtin: 前缀隐藏删除按钮"
    );
    assert!(
        !cron.contains("toggle + ' <button class=\"mini danger\" data-remove="),
        "删除按钮不得再无条件渲染"
    );

    let monitor = include_str!("../../../../plugins/monitor/ui/app.js");
    assert!(
        monitor.contains("rate(s.net_rx_rate)") && monitor.contains("rate(s.net_tx_rate)"),
        "历史表入站/出站都必须按速率列显示"
    );
    assert!(
        monitor.contains("card(\"c-net\", rate(s.net_rx_rate))")
            && monitor.contains("card(\"c-net-tx\", rate(s.net_tx_rate))"),
        "概览必须拆入站/出站两张速率卡"
    );
    assert!(
        !monitor.contains("bytes(s.net_tx)"),
        "不得再把累计字节当速率展示"
    );
    assert!(
        !monitor.contains("gb(s.net_tx * 1024)"),
        "不得再把字节当 KiB 换算后拼 GiB"
    );
}

/// UI 语义兜底回归：`hidden` 与未打开的 dialog 必须不可见，
/// 避免页面里的 display 规则把空进度条、空告警、幽灵弹窗顶出来。
#[test]
fn ui_hidden_and_dialog_guards_hold() {
    let css = include_str!("../../static/skiff.css");
    assert!(
        css.contains("[hidden] { display: none !important; }"),
        "缺少 [hidden] 全局兜底，组件 display 会覆盖 hidden"
    );
    assert!(
        css.contains("dialog:not([open]) { display: none !important; }"),
        "缺少 dialog:not([open]) 兜底，关闭的弹窗会被 display:flex 顶出来"
    );
}

/// 列表空态是场景语义，必须原样落到表格 props；不得被共享层丢弃成组件默认的
/// 「暂无数据」。页面骨架已全部由服务端产出，因此契约钉在 `schema_kit::table`
/// 与各页的文案上，不再钉壳脚本（脚本里已没有表格构建器）。
/// 各列表页的文案按 docs/contract/UI_CONTRACT.md §7 的口径（真空列表 / 筛选无结果 /
/// 依赖未就绪）分别给出，这里逐条防回归。
/// 镜像 / 网络 / 卷是 JSON 页面资产，文案断言读资产而不是读脚本。
#[test]
fn budding_boot_status_table_honors_caller_empty_text() {
    // 构件层：调用方给了文案就必须进 props，没给才回落到通用文案。
    let kit = include_str!("../../src/handlers/ui/schema_kit.rs");
    assert!(
        kit.contains(r#"opts.empty_text.unwrap_or_else(|| "暂无数据".to_string())"#),
        "schema_kit::table 必须把调用方空态文案传给表格组件"
    );
    assert!(
        kit.contains(r#""emptyText": empty_text"#),
        "空态文案必须落到表格 props.emptyText"
    );
    // 站点列表已服务端化，文案在 site_page.rs 而非 JS 内联。
    let site_page = include_str!("../../src/handlers/ui/site_page.rs");
    assert!(
        site_page.contains("还没有站点，创建后会生成整站目录、nginx vhost 与 PHP 探针"),
        "站点空态文案在服务端 schema"
    );
    // 容器 / 运行环境 / compose / Web 的空态随页面骨架迁到服务端 schema。
    for (source, text) in [
        // 容器：按 Docker 可用性与筛选状态分三种。
        (
            include_str!("../../src/handlers/ui/containers_page.rs"),
            "Docker 不可用，容器列表为空",
        ),
        (
            include_str!("../../src/handlers/ui/containers_page.rs"),
            "没有异常容器",
        ),
        (
            include_str!("../../src/handlers/ui/containers_page.rs"),
            "还没有容器，可在右上角新建",
        ),
        // 运行环境：真空列表引导创建；配置 / 扩展页签各自的空态。
        (
            include_str!("../../src/handlers/ui/runtimes_page.rs"),
            "还没有运行环境，点击右上角「新建运行环境」安装后即可绑定站点",
        ),
        (
            include_str!("../../src/handlers/ui/runtimes_page.rs"),
            "没有可编辑的配置文件",
        ),
        (
            include_str!("../../src/handlers/ui/runtimes_page.rs"),
            "还没有可用扩展",
        ),
        // compose：真空与 Docker 不可用两种。
        (
            include_str!("../../src/handlers/ui/compose_page.rs"),
            "还没有 compose 项目，可在下方编辑并部署",
        ),
        (
            include_str!("../../src/handlers/ui/compose_page.rs"),
            "Docker 不可用，compose 项目列表为空",
        ),
        // Web 日志查询无结果。
        (
            include_str!("../../src/handlers/ui/web_page.rs"),
            "本次查询没有日志行",
        ),
    ] {
        assert!(source.contains(text), "列表空态文案缺失：{text}");
    }
    // 任务 / 作业 / 审计的空态文案已随页面迁到服务端 schema。
    for (source, text) in [
        (
            include_str!("../../src/handlers/ui/tasks_page.rs"),
            "还没有计划任务，站点详情页可开启每日备份",
        ),
        (
            include_str!("../../src/handlers/ui/tasks_page.rs"),
            "还没有运行记录",
        ),
        (
            include_str!("../../src/handlers/ui/jobs_page.rs"),
            "还没有长耗时任务，安装运行环境等操作会在这里显示进度",
        ),
        (
            include_str!("../../src/handlers/ui/audit_page.rs"),
            "暂无审计记录",
        ),
    ] {
        assert!(source.contains(text), "列表空态文案缺失：{text}");
    }
    // 镜像 / 网络 / 卷：真空列表与依赖未就绪两种，逐页两种都要在。
    for (asset, texts) in [
        (
            include_str!("../../static/budding/pages/docker-images.json"),
            [
                "还没有拉取镜像，拉取后新建容器或运行环境可直接使用",
                "Docker 不可用，镜像列表为空",
            ],
        ),
        (
            include_str!("../../static/budding/pages/docker-networks.json"),
            [
                "还没有自定义网络，需要隔离或固定子网时创建",
                "Docker 不可用，网络列表为空",
            ],
        ),
        (
            include_str!("../../static/budding/pages/docker-volumes.json"),
            [
                "还没有命名卷，创建后可在新建容器时挂载",
                "Docker 不可用，卷列表为空",
            ],
        ),
    ] {
        for text in texts {
            assert!(asset.contains(text), "JSON 页面空态文案缺失：{text}");
        }
    }
}

/// 页面 Schema 迁到 JSON 资产后的接线：路由给资产 URL，资产自带取数与动作。
/// 三件事缺一环就是白屏 / 空表 / 403，所以逐项钉住。
#[test]
fn budding_boot_json_pages_are_wired_and_parse() {
    let js = include_str!("../../static/budding/budding-boot.js");
    // 路由表已由服务端下发：资产 URL 登记处从壳脚本换到 `handlers/ui/routes.rs`。
    let routes_src = include_str!("../../src/handlers/ui/routes.rs");
    for name in [
        "docker-images.json",
        "docker-networks.json",
        "docker-volumes.json",
    ] {
        assert!(
            routes_src.contains(&format!("PageKind::Asset(\"{name}\")")),
            "路由必须指向 JSON 资产：{name}"
        );
    }
    // 已迁出的页面不得再留在脚本里，否则两份实现会各自漂移。
    for kind in ["images", "networks", "volumes"] {
        assert!(
            !js.contains(&format!("kind === '{kind}'")),
            "页面已迁到 JSON，脚本不应再有 kind === '{kind}' 分支"
        );
        assert!(
            !js.contains(&format!("'skiff:{kind}'")),
            "路由不应再引用 skiff:{kind}"
        );
    }
    // JSON 里写 `${prefix}` / `${api}` 依赖全局 context；写请求的 CSRF 靠平台适配器补。
    assert!(
        js.contains("context: { prefix: prefix, api: apiBase }"),
        "必须把入口前缀与 API 基址注入页面根作用域"
    );
    assert!(
        js.contains("if (method !== 'GET' && method !== 'HEAD')"),
        "平台请求适配器必须为写请求补 CSRF 头，否则 JSON 里的 request / form 一律 403"
    );

    for (name, api) in [
        ("docker-images.json", "/ui/v1/images"),
        ("docker-networks.json", "/ui/v1/networks"),
        ("docker-volumes.json", "/ui/v1/volumes"),
    ] {
        let text = match name {
            "docker-images.json" => include_str!("../../static/budding/pages/docker-images.json"),
            "docker-networks.json" => {
                include_str!("../../static/budding/pages/docker-networks.json")
            }
            _ => include_str!("../../static/budding/pages/docker-volumes.json"),
        };
        let page: serde_json::Value = serde_json::from_str(text).unwrap_or_else(|e| {
            panic!("{name} 不是合法 JSON：{e}");
        });
        assert_eq!(page["type"], "page", "{name} 根节点必须是 page");
        assert_eq!(
            page["source"]["url"], api,
            "{name} 必须由页面 source 自己取数（can_admin / docker_available 都在这个响应里）"
        );
        let table = page["body"]
            .as_array()
            .and_then(|b| b.iter().find(|n| n["type"] == "table"))
            .unwrap_or_else(|| panic!("{name} 缺少表格节点"));
        assert!(
            table["id"].as_str().is_some_and(|s| !s.is_empty()),
            "{name} 表格必须声明稳定 id——写操作成功后要靠 invoke reload 刷新"
        );
        assert_eq!(table["source"]["url"], api, "{name} 表格取数地址");
        assert_eq!(
            table["props"]["itemsPath"], "data.items",
            "{name} 表格行路径必须对齐 /ui/v1 信封"
        );
        let actions = table["props"]["rowActions"][0]["onEvent"]["click"]["actions"]
            .as_array()
            .unwrap_or_else(|| panic!("{name} 行操作缺少动作链"));
        let types: Vec<&str> = actions.iter().filter_map(|a| a["type"].as_str()).collect();
        assert_eq!(
            types,
            ["confirm", "request", "invoke"],
            "{name} 删除必须走 确认 → 请求 → 刷新 三步，且只用 budding 内置动作"
        );
    }
}

/// budding 的 `button` 只渲染 `<button type="button">`，不消费 `to`；
/// 页面切换必须用显式动作。同 path 只换 query 的页内跳转（容器「仅异常」）
/// 还要解决 SDK 页面加载键不含 query 的问题：走 `skiffQueryNav` 导航后显式
/// `refreshPage`，并对浏览器前进/后退的同 path 情况做整页刷新兜底。
#[test]
fn budding_boot_buttons_navigate_explicitly() {
    let js = include_str!("../../static/budding/budding-boot.js");
    assert!(
        !js.contains("to: prefix +"),
        "button.props.to 不会触发导航，必须改用 click → 导航动作"
    );
    // query 变化不会改变 SDK 的页面加载键，必须注册并下发 skiffQueryNav。
    assert!(
        js.contains("skiffQueryNav: queryNav,"),
        "skiffQueryNav 必须注册到自定义动作表"
    );
    assert!(
        js.contains("runtime.navigateTo({ path: path, query: query });"),
        "skiffQueryNav 必须经宿主路由更新地址栏（含 query）"
    );
    assert!(
        js.contains("runtime.refreshPage();"),
        "skiffQueryNav 必须在导航后显式刷新页面内容"
    );
    assert!(
        js.contains("window.addEventListener('popstate'"),
        "浏览器前进/后退的同 path query 变化必须有 popstate 兜底"
    );
    // 容器过滤按钮的服务端 schema：两个页内跳转都走 skiffQueryNav。
    let containers_page = include_str!("../../src/handlers/ui/containers_page.rs");
    for needle in [
        "{ \"type\": \"skiffQueryNav\", \"props\": { \"path\": \"${prefix}/docker/containers\", \"query\": query } }",
        "filter_button(\"全部\", filter != \"error\", json!({}))",
        "filter_button(\"仅异常\", filter == \"error\", json!({ \"filter\": \"error\" }))",
    ] {
        assert!(
            containers_page.contains(needle),
            "容器过滤按钮缺少 skiffQueryNav 动作：{needle}"
        );
    }
    assert!(
        !js.contains("url: prefix + '/docker/containers?filter=error'"),
        "只换 query 的跳转不能再用 navigate（只会 pushState，不会重载内容）"
    );
    // 计数与列表共用同一 filter 口径的拼装已收进 `routes.rs::page_source`。
    let routes_src = include_str!("../../src/handlers/ui/routes.rs");
    assert!(
        routes_src.contains("if s.get(\"filter\") == \"error\""),
        "容器计数必须与列表用同一 filter 口径，切换后计数才同步变化"
    );
}

/// 宿主桥与文件写入口的静态守门：平台注入 confirm、确认框走运行时图层、
/// 文件能力面动作与弹窗表单入口都在，且不再依赖原生 prompt。
#[test]
fn budding_boot_host_bridge_and_file_entries_present() {
    let js = include_str!("../../static/budding/budding-boot.js");
    for needle in [
        // 确认框走 budding 内建图层，不再注入 platform.confirm 或自建按钮动作。
        "runtime.confirm(payload)",
        "buddingRuntime()",
        "consumeFlashToast();",
        // 文件能力面入口（后端 /ui/v1/files/* 已补齐）。
        "skiffFileWrite: fileWrite,",
        "skiffFileCopy: fileCopy,",
        "skiffFileChmod: fileChmod,",
        "skiffFileChown: fileChown,",
        "skiffFileArchive: fileArchive,",
        "skiffFileExtract: fileExtract,",
        // 站点详情 Schema 由服务端 UI 端点产出，前端只 fetch 并做运行时转义。
        "function toPageSchema(",
        "prepareServerSchema(schema)",
    ] {
        assert!(js.contains(needle), "宿主桥/文件入口缺失：{needle}");
    }
    // 文件目录行模板已迁到 JSON 资产：解压入口按文件类型显隐的声明在这里。
    let file_row = include_str!("../../static/budding/pages/_file-row.json");
    assert!(
        file_row.contains("\"visibleOn\": \"${item.can_extract}\""),
        "文件行模板缺少解压入口的显隐条件"
    );
    // 站点删除的确认链已随 Schema 迁到服务端（`site_page.rs`），静态守门跟着挪。
    let site_page = include_str!("../../src/handlers/ui/site_page.rs");
    for needle in [
        "\"requireText\": site.domain",
        "\"op\": \"delete\", \"id\": site.id, \"domain\": site.domain",
    ] {
        assert!(
            site_page.contains(needle),
            "服务端站点 Schema 缺少删除确认链：{needle}"
        );
    }
    for banned in [
        "confirm: hostConfirm",
        "skiffConfirmAnswer",
        "skiffSiteDelete",
        "siteDetailSchema",
    ] {
        assert!(!js.contains(banned), "确认框不应再自建图层：{banned}");
    }
    assert!(
        !js.contains("window.prompt("),
        "不再允许 window.prompt；输入型确认必须用内置 confirm.requireText"
    );
    assert!(
        !js.contains("window.alert("),
        "不允许用阻塞 alert 反馈结果；成功/失败统一走 toast"
    );
    assert_eq!(
        js.matches("window.confirm(").count(),
        1,
        "window.confirm 只允许保留在 hostConfirm 的运行时未就绪兜底分支"
    );
    assert!(
        !js.contains("window.confirm('确认删除 ' + display"),
        "Docker 资源删除确认必须走宿主图层"
    );
}

#[test]
fn budding_boot_p1_confirms_and_kill_entry_present() {
    let js = include_str!("../../static/budding/budding-boot.js");
    let containers_page = include_str!("../../src/handlers/ui/containers_page.rs");
    // 容器强制终止入口（后端 action_verb 白名单已支持 kill）；列表骨架在服务端。
    assert!(
        containers_page.contains("container_op(\"强制终止\", \"kill\", c)"),
        "容器详情缺少强制终止入口"
    );
    for needle in [
        // 站点破坏性动作确认（站点仍是宿主动作，B 形待迁）。
        "title: '停止站点'",
        "title: '关闭 HTTPS'",
        "title: '移除附加域名'",
        "title: '批量停止站点'",
        // 设置侧确认。
        "title: '撤销公钥'",
        "title: '恢复默认菜单'",
    ] {
        assert!(js.contains(needle), "P1 确认/入口缺失：{needle}");
    }
    // 容器强制终止与插件卸载已声明化，确认文案随页面迁到服务端 schema。
    let plugins_page = include_str!("../../src/handlers/ui/plugins_page.rs");
    for (source, needle) in [
        (containers_page, "\"title\": \"强制终止容器\""),
        (plugins_page, "\"title\": \"卸载插件\""),
    ] {
        assert!(source.contains(needle), "P1 确认/入口缺失：{needle}");
    }
    // 表单就地数值校验：字段随页面迁到服务端 schema。
    for (source, needle) in [
        (
            include_str!("../../src/handlers/ui/runtimes_page.rs"),
            "\"pattern\": \"^[0-9]{1,5}$\"",
        ),
        (
            include_str!("../../src/handlers/ui/web_page.rs"),
            "\"pattern\": \"^[0-9]+$\"",
        ),
    ] {
        assert!(source.contains(needle), "表单数值校验缺失：{needle}");
    }
}

/// 站点目录入口与归档恢复：两处都容易在改版时被「顺手简化」掉。
///
/// - 产品不提供站点目录浏览页签，详情概览的「站点目录 / 文档根」必须带文件管理链接；
/// - 备份页签的恢复必须走 `SITE_RESTORE` 接口，而不是前端自己调 tar。
#[test]
fn site_page_keeps_directory_links_and_restore_entry() {
    let site_page = include_str!("../../src/handlers/ui/site_page.rs");
    let js = include_str!("../../static/budding/budding-boot.js");
    for needle in [
        "files?path=",
        "\"打开文件管理\"",
        "\"op\": \"restore\"",
        "\"archive\": archive.name",
        "\"归档与恢复\"",
    ] {
        assert!(site_page.contains(needle), "站点详情缺入口：{needle}");
    }
    for needle in ["} else if (op === 'restore') {", "'/backups/restore'"] {
        assert!(js.contains(needle), "skiffSiteWrite 缺恢复分支：{needle}");
    }
}

/// P2 列表行内入口：站点列表不再只有详情入口，容器列表补齐启停/重启/删除。
#[test]
fn budding_boot_p2_row_actions_present() {
    let js = include_str!("../../static/budding/budding-boot.js");
    let site_page = include_str!("../../src/handlers/ui/site_page.rs");
    let containers_page = include_str!("../../src/handlers/ui/containers_page.rs");
    // JS 里注册的自定义动作：站点启停仍保留（B 形待迁）。
    assert!(
        js.contains("skiffSiteToggle: siteToggle"),
        "站点行内启停入口缺失（JS）"
    );
    // 容器行内启停已声明化：状态分支 + 请求 + 表格刷新都在服务端 schema。
    assert!(
        containers_page.contains("\"when\": \"${state.code === 'running'}\""),
        "容器列表缺少启停状态分支"
    );
    assert!(
        containers_page.contains("\"component\": \"containers-table\""),
        "容器列表缺少表格 reload 目标"
    );
    assert!(
        site_page.contains("\"props\": { \"id\": \"${id}\", \"domain\": \"${domain}\", \"status\": \"${status.code}\" }"),
        "站点列表服务端 schema 缺失 skiffSiteToggle props"
    );
}

/// 第一批 A 形写动作已全部声明化：确认 / 请求 / 刷新（或跳转）由服务端 Schema
/// 的动作链表达，壳脚本不再保留对应的命令式写请求。
#[test]
fn budding_boot_a_form_writes_are_declarative() {
    let js = include_str!("../../static/budding/budding-boot.js");
    for banned in [
        "function containerAction(",
        "function containerToggle(",
        "function taskWrite(",
        "function jobWrite(",
        "function trashWrite(",
        "function pluginToggle(",
        "function pluginDelete(",
        "skiffContainerAction:",
        "skiffContainerToggle:",
        "skiffTaskWrite:",
        "skiffJobWrite:",
        "skiffTrashWrite:",
        "skiffPluginToggle:",
        "skiffPluginDelete:",
    ] {
        assert!(!js.contains(banned), "写动作体应已迁出脚本：{banned}");
    }
    // 详情页没有可重载的列表，写操作后统一整页重取。
    assert!(js.contains("skiffReload: reloadPage"), "整页重取动作缺失");

    let containers = include_str!("../../src/handlers/ui/containers_page.rs");
    for needle in [
        "\"type\": \"condition\"",
        "\"type\": \"confirm\"",
        "\"type\": \"request\"",
        "\"component\": \"containers-table\"",
        "\"url\": \"${prefix}/docker/containers\"",
        "dialog_form_button",
        "container_create_fields",
    ] {
        assert!(containers.contains(needle), "容器动作链缺失：{needle}");
    }

    let tasks = include_str!("../../src/handlers/ui/tasks_page.rs");
    for needle in [
        "\"enabled\": \"${!enabled}\"",
        "\"component\": \"tasks-table\"",
        "\"type\": \"navigate\"",
    ] {
        assert!(tasks.contains(needle), "任务动作链缺失：{needle}");
    }

    let jobs = include_str!("../../src/handlers/ui/jobs_page.rs");
    for needle in [
        "\"assign\": \"job\"",
        "\"url\": \"${prefix}/tasks/jobs/${job.data.id}\"",
        "\"title\": \"取消作业\"",
    ] {
        assert!(jobs.contains(needle), "作业动作链缺失：{needle}");
    }

    let plugins = include_str!("../../src/handlers/ui/plugins_page.rs");
    for needle in [
        "/plugins/{enc_id}/toggle",
        "/plugins/{enc_id}/delete",
        "\"delete_data\": delete_data",
    ] {
        assert!(plugins.contains(needle), "插件动作链缺失：{needle}");
    }

    let files = include_str!("../../src/handlers/ui/files_page.rs");
    for needle in [
        "${{api}}/files/trash/{op}",
        "\"overwrite\": true",
        "\"onError\"",
        "\"path\": \"${root}\"",
    ] {
        assert!(files.contains(needle), "回收站动作链缺失：{needle}");
    }
    assert!(!files.contains("skiffTrashWrite"), "回收站不应再有宿主动作");
}

/// B 形字段错误回填是可选能力：默认不声明就保持框架自身的提示；
/// 页面显式写 `onError: [skiffFormErrors]` 时才把服务端 `error.field` 贴回表单。
#[test]
fn budding_boot_form_errors_bridge_is_opt_in() {
    let js = include_str!("../../static/budding/budding-boot.js");
    assert!(
        js.contains("function applyFormErrors("),
        "字段错误回填动作缺失"
    );
    assert!(
        js.contains("skiffFormErrors: applyFormErrors"),
        "字段错误回填动作未注册"
    );
    assert!(
        js.contains("lastApiError") && js.contains("body.error.field"),
        "平台适配器未保留失败信封的字段名"
    );
}

/// 动作注册表键去重：`skiffDockerWrite` 曾与设置保存动作共用同名键，
/// 后注册的实现静默覆盖前一个，导致镜像/卷删除被劫持到 `/settings/docker`。
#[test]
fn budding_boot_action_registry_has_no_duplicate_keys() {
    let js = include_str!("../../static/budding/budding-boot.js");
    let marker = "      actions: {";
    let start = js.find(marker).expect("actions 注册表缺失") + marker.len();
    let bytes = js.as_bytes();
    let mut depth = 1usize;
    let mut end = start;
    for (offset, byte) in bytes[start..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    end = start + offset;
                    break;
                }
            }
            _ => {}
        }
    }
    assert!(depth == 0, "actions 注册表未闭合");
    let block = &js[start..end];

    let mut keys = Vec::new();
    for line in block.lines() {
        let trimmed = line.trim();
        let Some((key, _)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if !key.is_empty()
            && key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        {
            keys.push(key.to_string());
        }
    }
    let mut seen = std::collections::HashSet::new();
    let mut dup = Vec::new();
    for key in &keys {
        if !seen.insert(key.clone()) {
            dup.push(key.clone());
        }
    }
    assert!(dup.is_empty(), "动作注册表存在重复键：{dup:?}");
    assert_eq!(
        keys.iter()
            .filter(|k| k.as_str() == "skiffDockerWrite")
            .count(),
        1,
        "资源动作 skiffDockerWrite 必须唯一注册"
    );
    assert_eq!(
        keys.iter()
            .filter(|k| k.as_str() == "skiffDockerSettingsWrite")
            .count(),
        1,
        "Docker 设置保存动作必须独立命名"
    );
}

/// 资源动作必须映射到资源接口；设置保存只能用于 Docker 设置表单。
/// 镜像 / 网络 / 卷 / 容器新建均已迁到 JSON 页面资产（声明式 `request`），
/// 表单提交改事件模式（`submitMode: "event"`）后，落点从表单 `api.url` 移到
/// 壳脚本的宿主动作里——全部改读各自产出端，不再钉脚本。
#[test]
fn budding_boot_docker_actions_map_to_resource_endpoints() {
    let js = include_str!("../../static/budding/budding-boot.js");
    assert!(
        js.contains("path = '/containers';"),
        "Docker 动作映射缺失：path = '/containers';"
    );
    for (asset, needles) in [
        (
            include_str!("../../static/budding/pages/docker-images.json"),
            vec!["/ui/v1/images/pull", "/ui/v1/images/${id}/remove"],
        ),
        (
            include_str!("../../static/budding/pages/docker-networks.json"),
            vec!["/ui/v1/networks", "/ui/v1/networks/${id}/remove"],
        ),
        (
            include_str!("../../static/budding/pages/docker-volumes.json"),
            vec!["/ui/v1/volumes", "/ui/v1/volumes/${id}/remove"],
        ),
        (
            include_str!("../../src/handlers/ui/containers_page.rs"),
            vec!["{API_BASE}/containers", "skiffDockerWrite"],
        ),
        (
            include_str!("../../src/handlers/ui/settings_page.rs"),
            vec!["\"type\": \"skiffDockerSettingsWrite\""],
        ),
    ] {
        for needle in needles {
            assert!(asset.contains(needle), "JSON 页面动作映射缺失：{needle}");
        }
    }
    // 表单不再自带 `api.url`，Docker 设置保存的落点只剩壳脚本这一处。
    // 2026-11-24 写操作统一走 `writePost`（内部再调 `submitJson`），断言跟到模板层。
    assert!(
        js.contains("writePost('/settings/docker'"),
        "Docker 设置保存必须提交到 /settings/docker"
    );
}

/// 服务端下发的弹窗只需给提交动作打一个 `inLayer` 标记：宿主动作据此决定成功后
/// 关层还是整页刷新。budding 1.8.6 起提交失败不关图层、字段值保留，所以壳层
/// **不再**保存弹窗正文副本、也不再「关掉重开」——那套注册表必须彻底消失。
///
/// budding 1.8.3 起 dialog 的正文/底部在动作顶层 `content`，读 `props.body`
/// 会永远拿不到 body、静默漏标。
#[test]
fn budding_boot_marks_layer_submits_from_content_slot() {
    let js = include_str!("../../static/budding/budding-boot.js");
    let fn_body = js
        .split("function markLayerSubmit(button)")
        .nth(1)
        .and_then(|s| s.split("\n  function ").next())
        .expect("markLayerSubmit 必须存在");
    assert!(
        fn_body.contains("button.action.content"),
        "服务端弹窗必须从 action.content 取插槽"
    );
    assert!(
        !fn_body.contains("layer.body"),
        "不得再从 props.body（layer.body）取 body：该位置在 1.8.3+ 是空的"
    );
    assert!(
        fn_body.contains("inLayer = true"),
        "弹窗内表单的提交动作必须打上 inLayer 标记"
    );
    for gone in [
        "dialogRegistry",
        "function dialogError(",
        "function registerServerDialog(",
        "JSON.parse(JSON.stringify(def.content",
    ] {
        assert!(!js.contains(gone), "克隆重开弹窗的旧机制应已删除：{gone}");
    }

    // 资产里每个 dialog 的提交动作都要带 inLayer，否则成功后不关层。
    let row: serde_json::Value =
        serde_json::from_str(include_str!("../../static/budding/pages/_file-row.json")).unwrap();
    fn walk_forms(node: &serde_json::Value, seen: &mut usize) {
        match node {
            serde_json::Value::Array(items) => items.iter().for_each(|i| walk_forms(i, seen)),
            serde_json::Value::Object(map) => {
                if map.get("type").and_then(|t| t.as_str()) == Some("form") {
                    if let Some(submit) = map
                        .get("onEvent")
                        .and_then(|e| e.get("submit"))
                        .and_then(|s| s.as_array())
                        .and_then(|s| s.first())
                    {
                        assert!(
                            submit
                                .get("props")
                                .and_then(|p| p.get("inLayer"))
                                .is_some_and(|d| d.as_bool() == Some(true)),
                            "弹窗表单的提交动作必须带 props.inLayer：{submit}"
                        );
                        *seen += 1;
                    }
                }
                map.values().for_each(|v| walk_forms(v, seen));
            }
            _ => {}
        }
    }
    let mut forms = 0;
    walk_forms(&row, &mut forms);
    assert_eq!(forms, 6, "文件行模板里的弹窗表单数量变了");
}

/// 作业日志页：游标聚合与骨架都在服务端，脚本不再自建日志页。
#[test]
fn budding_boot_job_logs_page_comes_from_server() {
    let js = include_str!("../../static/budding/budding-boot.js");
    // 页面取数地址由服务端 `/ui/v1/page` 回答，脚本只报「当前路径」。
    assert!(
        js.contains("function fetchPageSchema("),
        "页面分派必须走服务端取数地址"
    );
    for banned in ["function loadJobLogs(", "function jobLogsSchema("] {
        assert!(!js.contains(banned), "作业日志构建器已迁出：{banned}");
    }
    // 产出端：路由表登记隐藏路由，`page_source` 指到日志端点。
    let routes = include_str!("../../src/handlers/ui/routes.rs");
    // 路径与标签在登记表里各占一行（rustfmt 会拆行），分别断言，别锁死排版。
    for needle in ["\"/tasks/jobs/:id/logs\"", "\"作业日志\""] {
        assert!(
            routes.contains(needle),
            "路由表必须登记作业日志隐藏路由：{needle}"
        );
    }
    assert!(
        routes.contains("\"/jobs/{}/logs?with_schema=true\""),
        "page_source 必须把作业日志指到日志端点"
    );
    let page = include_str!("../../src/handlers/ui/jobs_page.rs");
    for needle in [
        "pub fn job_logs_schema(",
        "仅加载前 10 页，可刷新继续",
        "\"type\": \"skiffReload\"",
    ] {
        assert!(page.contains(needle), "作业日志页缺失：{needle}");
    }
    let handler = include_str!("../../src/handlers/ui/jobs.rs");
    for needle in [
        ".route(\"/ui/v1/jobs/{id}/logs\", get(job_logs))",
        "pages >= 10",
    ] {
        assert!(handler.contains(needle), "作业日志端点缺失：{needle}");
    }
}

/// 文件弹窗表单：数值字段必须发数字，失败必须原地回填到表单。
#[test]
fn budding_boot_file_forms_send_typed_values_and_show_errors() {
    let js = include_str!("../../static/budding/budding-boot.js");
    let chown = js
        .split("writePost('/files/chown'")
        .nth(1)
        .expect("fileChown 必须提交 /files/chown");
    let body = &chown[..chown.find("})").unwrap_or(chown.len())];
    assert!(body.contains("uid: uid"), "chown 的 uid 必须发数字变量");
    assert!(body.contains("gid: gid"), "chown 的 gid 必须发数字变量");
    assert!(
        !body.contains("uid: uidRaw"),
        "不得把原始字符串直接发给 u32 字段"
    );
    assert!(js.contains("var uid = Number(uidRaw);"));
    assert!(js.contains("var gid = Number(gidRaw);"));
    assert!(js.contains("'uid 与 gid 必须是非负整数'"));

    // 失败就地回填：写表单错误 + 聚焦首个错误字段，不再关掉重开弹窗。
    // 表单实例按 budding 的目标解析规则两档取：表单族 provide 的祖先表单
    // （`extras.form`）+ 所属图层的登记表（`budding:layer-forms`）。弹窗页脚的
    // 提交按钮不在表单祖先链上，只有登记表能找到它——这一档是主路径，缺了就是
    // 「提交失败什么也不显示」的静默失败。
    let form_of = js
        .split("function formOf(ctx)")
        .nth(1)
        .and_then(|s| s.split("\n  function ").next())
        .expect("formOf 必须存在");
    for needle in ["extras.form", "LAYER_FORMS_KEY", "registry.list()"] {
        assert!(
            form_of.contains(needle),
            "表单实例解析必须覆盖图层登记表：{needle}"
        );
    }
    assert!(
        js.contains("var LAYER_FORMS_KEY = 'budding:layer-forms';"),
        "图层登记表的键必须与 budding 的图层数据袋约定一致"
    );
    let err = js
        .split("function formError(ctx, message, field)")
        .nth(1)
        .and_then(|s| s.split("\n  function ").next())
        .expect("formError 必须存在");
    for needle in ["formOf(ctx)", "setErrors", "focusFirstError", "__form__"] {
        assert!(err.contains(needle), "formError 必须原地回填表单：{needle}");
    }

    for name in [
        "fileWrite",
        "fileRename",
        "fileCopy",
        "fileChmod",
        "fileChown",
        "fileArchive",
        "fileExtract",
    ] {
        let start = js
            .find(&format!("function {name}("))
            .unwrap_or_else(|| panic!("{name} 不存在"));
        let rest = &js[start..];
        let end = rest[1..]
            .find("\n  function ")
            .map(|i| i + 1)
            .unwrap_or(rest.len());
        assert!(
            rest[..end].contains("formError("),
            "{name} 的错误分支必须走 formError（弹窗内可见）"
        );
    }
}

/// 插件菜单的「发现 / 可更新」与商店升级包下载必须留在原生壳页，
/// 不得再退回旧 SSR（旧路由与模板按约定保留，只是产品前端不再链接）。
#[test]
fn budding_boot_keeps_plugin_tabs_and_store_upgrade_native() {
    let js = include_str!("../../static/budding/budding-boot.js");
    // 路由映射由服务端 `/ui/v1/routes` 下发，产出端是 routes.rs 的内置页登记表。
    let routes = include_str!("../../src/handlers/ui/routes.rs");
    for (key, path) in [
        ("core/plugins/discover", "/plugins/discover"),
        ("core/plugins/updates", "/plugins/updates"),
    ] {
        assert!(
            routes.contains(&format!("(\"{key}\", \"{path}\", PageKind::Page)")),
            "{key} 必须映射到原生插件页"
        );
    }
    // 页内标签：随插件页迁到服务端 schema，链接仍指向原生路径。
    let plugins_page = include_str!("../../src/handlers/ui/plugins_page.rs");
    assert!(
        plugins_page.contains("kit::link(\"发现\", \"${prefix}/plugins/discover\")"),
        "页内标签应使用原生路径"
    );
    assert!(
        js.contains("/store/upgrades/' + encodeURIComponent(version) + '/download'"),
        "升级包下载应走 /ui/v1"
    );
}

/// 新增的界面设置与 Web 服务运维入口必须落在既有契约上：
/// 主色由服务端写进壳页内联 `<style>`；Web 动作复用 `/ui/v1/runtimes/web/action/{op}`。
#[test]
fn budding_boot_has_ui_theme_and_web_env_entries() {
    let js = include_str!("../../static/budding/budding-boot.js");
    let spa = include_str!("../../src/handlers/spa.rs");
    // 界面设置：路由登记 + 页面取数地址都在 routes.rs，脚本不再持有映射。
    let routes = include_str!("../../src/handlers/ui/routes.rs");
    assert!(
        routes.contains("(\"core/settings/ui\", \"/settings/ui\", PageKind::Page)"),
        "界面设置必须映射到规范路径"
    );
    assert!(
        routes.contains("\"/settings/ui?with_schema=true\""),
        "界面设置的页面取数地址必须指到 /ui/v1/settings/ui"
    );
    // 主题 CSS 由服务端出：颜色计算只有 `skiff_app::theme` 一处，前端不再重算。
    assert!(
        spa.contains("pub fn theme_css(theme: &skiff_app::theme::Theme) -> String"),
        "壳主题必须由服务端生成 CSS（与 skiff_app::theme 同源）"
    );
    assert!(
        spa.contains(r#"<style id="skiff-shell-theme">{theme_css}</style>"#),
        "壳页必须内联主题样式"
    );
    assert!(
        !js.contains("applyShellTheme"),
        "前端不得再自带主题实现（亮度/对比度算法会与服务端漂移）"
    );
    assert!(
        !js.contains("schema.theme"),
        "budding 1.8.0-r7 根节点未注册 theme，禁止写进 schema 树"
    );
    assert!(
        js.contains("markShellChrome();") && js.contains("classList.add('bud-admin--chrome')"),
        "外壳换底色必须补 bud-admin--chrome 类（schema.theme 被编译期拒绝）"
    );
    assert!(
        js.contains("skiffUiThemeWrite: uiThemeWrite,"),
        "界面设置写动作必须注册"
    );
    // Web 服务运维入口随页面迁到服务端 schema（`web_page.rs`）。
    let web_page = include_str!("../../src/handlers/ui/web_page.rs");
    for needle in [
        "web_button(\"安装 Web 服务\", \"install\", \"primary\", \"flat\")",
        "web_button(\"重启\", \"restart\", \"\", \"tonal\")",
        "web_button(\"重新安装\", \"rebuild\", \"\", \"tonal\")",
        "web_button(\"删除容器\", \"delete\", \"danger\", \"tonal\")",
        "if running { \"stop\" } else { \"start\" }",
        "{ \"type\": \"skiffWebEnv\", \"props\": { \"op\": op } }",
        "查看任务详情",
        "fn install_alert(install: &Value)",
        "fn install_summary(install: &Value)",
        "fn install_steps_table(auto_refresh: bool)",
        "fn install_headline(install: &Value)",
    ] {
        assert!(web_page.contains(needle), "Web 服务操作入口缺失：{needle}");
    }
    // 启停/重启在页头主操作区；完成态收敛成页尾摘要，不再占顶部告警位。
    assert!(
        web_page.contains("kit::page_with_actions(\"Web 服务\", subtitle, actions, body)"),
        "状态页主操作必须走页头 content.actions"
    );
    assert!(
        !web_page.contains("(\"success\", \"安装已完成\")"),
        "完成态不得再用顶部 success 告警（改为页尾摘要）"
    );
    assert!(
        web_page.contains("${{prefix}}/tasks/jobs/{job_id}"),
        "任务详情入口必须指向 /tasks/jobs/<id>"
    );
    assert!(
        web_page.contains(
            "let installed = !container_state.is_empty() && container_state != \"missing\";"
        ),
        "未安装与已安装必须区分，避免出现无意义按钮"
    );
    // 安装进度改由表格 `source.autoRefresh` 轮询：壳脚本不再自己定时改 DOM。
    assert!(
        web_page.contains("table[\"source\"][\"autoRefresh\"] = json!(2000);"),
        "安装步骤表必须自带轮询，否则进度停在打开那一刻"
    );
    // 提交动作仍在壳脚本里（要 POST + 刷新），但目标与刷新时机是契约。
    for needle in [
        "'/runtimes/web/action/' + encodeURIComponent(op)",
        "skiffWebEnv: webEnvAction,",
    ] {
        assert!(js.contains(needle), "Web 服务操作入口缺失：{needle}");
    }
    // 安装/重建点击后不得先弹阻塞提示再刷新，否则旧作业结果会继续可见。
    let action = js
        .split("function webEnvAction(args)")
        .nth(1)
        .expect("webEnvAction 必须存在");
    let compact = action.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        compact
            .contains("if (op === 'install' || op === 'rebuild') { location.reload(); return; }"),
        "安装/重建提交后应立即进入当前作业视图"
    );
}

/// 「设置 → 界面」配色扩展：预设 + 主色 / 菜单底色 / 页面底色。
///
/// budding 1.8.0-r7 没有颜色选择器节点，前端只提交 `#rrggbb` 文本；服务端仍是
/// 唯一裁决（非法 400 不落库），配色经文档级 CSS 变量全站生效。
#[test]
fn budding_boot_ui_theme_presets_and_color_fields() {
    let js = include_str!("../../static/budding/budding-boot.js");
    // 预设与配色表单随「设置 → 界面」页迁到服务端 schema。
    let settings_page = include_str!("../../src/handlers/ui/settings_page.rs");
    for name in ["\"深潜\"", "\"石墨\"", "\"灯塔\""] {
        assert!(settings_page.contains(name), "预设方案缺少名称：{name}");
    }
    assert!(
        settings_page.contains("fn preset_card("),
        "预设方案应渲染为色卡而不是纯文字按钮"
    );
    assert!(
        settings_page.contains("\"className\": \"skiff-swatch\""),
        "预设色点应有描边类名（浅色在白底上可见）"
    );
    for needle in [
        "color_field(\"primary\",",
        "color_field(\n                    \"menu\",",
        "color_field(\"page\",",
        "\"name\": \"menu_custom\"",
        "let color_rules = |message: &str|",
        "\"pattern\": \"^#[0-9a-fA-F]{6}$\"",
    ] {
        assert!(settings_page.contains(needle), "配色扩展缺失：{needle}");
    }
    // 动作注册仍在壳脚本：写操作要 POST 并整页刷新；CSS 变量由服务端注入。
    for needle in [
        "skiffUiPreset: uiPresetApply,",
        "skiffUiThemeWrite: uiThemeWrite,",
    ] {
        assert!(js.contains(needle), "配色扩展缺失：{needle}");
    }
    let spa = include_str!("../../src/handlers/spa.rs");
    for needle in [
        "--bud-admin-chrome-bg:rgb({menu}) !important",
        "--v-theme-background:{page} !important",
    ] {
        assert!(spa.contains(needle), "壳主题变量缺失：{needle}");
    }
    assert!(
        !js.contains("color-picker") && !js.contains("input-color"),
        "1.8.0-r7 没有颜色选择器节点，禁止引用"
    );
    // 关闭自定义时必须留空，避免旧值在下次开启时复活。
    assert!(
        js.contains("menu: menuCustom ? menu : ''"),
        "自定义关闭时不得提交菜单底色"
    );
    assert!(
        js.contains("formError(ctx, invalid + '必须是 #rrggbb 格式"),
        "颜色本地校验不通过时必须在弹窗/页面显示原因"
    );
}

/// 站点 / 运行环境列表的新建改为弹窗；独立深链页必须保持可用。
#[test]
fn budding_boot_create_dialogs_keep_deep_links() {
    let js = include_str!("../../static/budding/budding-boot.js");
    // 深链页路由与独立表单都保留：路由与「页面结构去哪取」都在 `routes.rs`
    // （`DETAIL_ROUTES` 的 `/sites/new` + `page_source` 映射到 `/ui/v1/pages/sites-new`）。
    let routes_src = include_str!("../../src/handlers/ui/routes.rs");
    for needle in [
        "(\"/sites/new\", \"新建站点\", PageKind::Page, \"/sites/list\")",
        "api(\"/pages/sites-new\")",
    ] {
        assert!(
            routes_src.contains(needle),
            "新建站点深链登记缺失：{needle}"
        );
    }
    for needle in [
        // 弹窗提交成功回列表刷新；独立页仍进入详情。
        "op === 'create' && data.id && !args.inLayer",
        "if (args.inLayer) closeTopLayer();",
        "formError(\n            ctx,\n            '操作失败：'",
    ] {
        assert!(js.contains(needle), "新建弹窗改造缺失：{needle}");
    }
    // 运行环境新建弹窗随页面迁到服务端：按钮 → dialog → 表单字段。
    let runtimes_page = include_str!("../../src/handlers/ui/runtimes_page.rs");
    for needle in [
        "kit::dialog_form_button(kit::DialogForm {",
        "text: \"新建运行环境\"",
        "variant: \"flat\"",
        "title: \"新建运行环境\"",
        "width: 640",
        "submit_text: \"创建并安装\"",
        "\"name\": \"kind\"",
        // 版本下拉按类型一份：字段名与显示条件都按目录 key 生成（`version_php` /
        // `version_node` / `version_rust`），加类型不改弹窗代码。语义断言见
        // `runtimes_page` 单测（`php_versions_come_from_registry_index`）。
        "\"name\": format!(\"version_{key}\")",
        "\"visibleOn\": format!(\"${{kind === '{key}'}}\")",
        "\"name\": \"version_custom\"",
        "\"type\": \"skiffRuntimeVersions\"",
        "\"name\": \"apk_mirror\"",
    ] {
        assert!(
            runtimes_page.contains(needle),
            "运行环境新建弹窗缺失：{needle}"
        );
    }
    // budding 1.8.3 契约（upstream compiler action.ts）：dialog 的 body/footer 是动作
    // 顶层 content 插槽，写在 props 里会整页 CompileError。文件行模板逐弹窗校验。
    let row: serde_json::Value =
        serde_json::from_str(include_str!("../../static/budding/pages/_file-row.json")).unwrap();
    fn walk_dialogs(node: &serde_json::Value, found: &mut usize) {
        match node {
            serde_json::Value::Array(items) => {
                for item in items {
                    walk_dialogs(item, found);
                }
            }
            serde_json::Value::Object(map) => {
                if map.get("type").and_then(|t| t.as_str()) == Some("dialog") {
                    *found += 1;
                    assert!(
                        map.contains_key("content"),
                        "dialog 动作必须在顶层写 content：{node}"
                    );
                    if let Some(props) = map.get("props") {
                        assert!(
                            props.get("body").is_none() && props.get("footer").is_none(),
                            "dialog 的 body/footer 不能写进 props：{node}"
                        );
                    }
                }
                for value in map.values() {
                    walk_dialogs(value, found);
                }
            }
            _ => {}
        }
    }
    let mut dialogs = 0;
    walk_dialogs(&row, &mut dialogs);
    assert_eq!(
        dialogs, 6,
        "文件行模板的弹窗数量变了，请复核 content 插槽是否仍合规"
    );
    // 站点列表的新建/批量弹窗随页面骨架迁到服务端。
    let site_page = include_str!("../../src/handlers/ui/site_page.rs");
    for needle in [
        "text: \"批量启停\"",
        "text: \"新建站点\"",
        "initial: Some(json!({ \"site_type\": \"static\" }))",
        "fn site_create_fields(",
        "kit::dialog_form_button(kit::DialogForm {",
        "title: \"新建站点\"",
        "width: 640",
        "submit_text: \"创建站点\"",
    ] {
        assert!(
            site_page.contains(needle),
            "服务端站点列表弹窗缺失：{needle}"
        );
    }
    assert!(!js.contains("sitesSchema"), "站点列表骨架不得再由前端拼接");
    assert!(
        !js.contains("'安装运行环境'"),
        "运行环境列表不应再保留独立安装卡片"
    );
    assert!(
        !js.contains("type: 'navigate', props: { url: prefix + '/sites/new' }"),
        "列表新建不得再跳独立页面"
    );
}

/// 壳页 `admin` 根节点不得再写 1.8.4 已移除的 `content.breadcrumbs` 插槽。
///
/// 1.8.4 把它从字段与插槽表里删掉，旧写法会让整棵壳页 `CompileError`（面板打不开），
/// 关闭面包屑只能走 `props.breadcrumbs=false`。
#[test]
fn shell_schema_disables_breadcrumbs_via_props() {
    let js = include_str!("../../static/budding/budding-boot.js");
    assert!(
        js.contains("breadcrumbs: false"),
        "壳页必须用 props.breadcrumbs=false 关闭面包屑"
    );
    assert!(
        !js.contains("breadcrumbs: []"),
        "content.breadcrumbs 插槽在 budding 1.8.4 已移除，写了会整页编译失败"
    );
}

/// 静态防回归：禁止 boot 向 schema 根写入未注册的顶层属性。
///
/// 白名单来自随包 budding 1.8.0-r7 的编译期属性集合：
/// `type` / `id` / `definitions` / `props` 为特殊键，其余为全局基础属性
/// （该版本未注册任何 `nodeAttributes`，所以 `theme` 之类一律不合法）。
/// 这类错误会让整棵 schema 编译失败、全站白屏，静态断言必须走在装机前面。
#[test]
fn budding_boot_only_writes_registered_schema_root_keys() {
    let js = include_str!("../../static/budding/budding-boot.js");
    let allowed = [
        "type",
        "id",
        "definitions",
        "props",
        "data",
        "visibleOn",
        "hiddenOn",
        "permission",
        "permissions",
        "onEvent",
        "action",
        "to",
        "class",
        "className",
        "style",
        "content",
        "source",
    ];
    let mut rest = js;
    while let Some(pos) = rest.find("schema.") {
        let tail = &rest[pos + "schema.".len()..];
        let key_len = tail
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .count();
        let key = &tail[..key_len];
        let after = tail[key_len..].trim_start();
        if !key.is_empty()
            && after.starts_with('=')
            && !after.starts_with("==")
            && !after.starts_with("=>")
        {
            assert!(
                allowed.contains(&key),
                "schema.{key} 未在 budding 1.8.1 注册，编译会整棵树失败"
            );
        }
        rest = tail;
    }
}

/// 1.8.0-r3 已闭环 T10 / T16 / code Pack：宿主不得再保留旧能力缺失时的绕过。
#[test]
fn budding_boot_drops_pre_r3_workarounds() {
    let js = include_str!("../../static/budding/budding-boot.js");
    // T16：17 个业务语义图标全部有映射，不再只映射 clock/file/dot。
    // 图标映射随路由表迁到服务端 `routes.rs::MENU_ICONS`，断言跟到产出端。
    let routes_src = include_str!("../../src/handlers/ui/routes.rs");
    for (name, glyph) in [
        ("overview", "mdi-view-dashboard"),
        ("globe", "mdi-web"),
        ("docker", "mdi-docker"),
        ("plug", "mdi-puzzle"),
        ("tools", "mdi-wrench"),
        ("settings", "mdi-cog"),
        ("container", "mdi-cube-outline"),
        ("shield", "mdi-shield-outline"),
        ("lock", "mdi-lock-outline"),
        ("network", "mdi-lan"),
        ("user", "mdi-account-outline"),
        ("activity", "mdi-pulse"),
        ("list", "mdi-format-list-bulleted"),
        ("archive", "mdi-archive-outline"),
    ] {
        assert!(
            routes_src.contains(&format!("(\"{name}\", \"{glyph}\")")),
            "菜单图标映射缺失：{name}: '{glyph}'"
        );
    }
    // T13：高亮包已改名 @budding/code，旧 highlight 注册与产物不得残留。
    assert!(
        js.contains("registerPack(\n        'code'"),
        "应注册 code Pack"
    );
    assert!(js.contains("budding-code-"), "code Pack 应走版本化文件名");
    assert!(
        !js.contains("budding-highlight-"),
        "不得再引用旧 highlight 产物"
    );
    assert!(!js.contains("@budding/highlight"), "文案不得再指向旧 Pack");
    // T10：列表走 source 游标分页（构件在 schema_kit，资产里声明的页面直接写 JSON）。
    let kit = include_str!("../../src/handlers/ui/schema_kit.rs");
    assert!(
        kit.contains(r#"props.insert("paging".into(), json!("cursor"))"#),
        "列表应使用游标分页"
    );
    assert!(
        include_str!("../../static/budding/pages/docker-images.json")
            .contains(r#""source": { "url": "/ui/v1/images", "params": {} }"#),
        "镜像列表应有 source 路径"
    );
    assert!(js.contains("'/sites'"), "站点列表应有 source 路径");
    assert!(
        !js.contains("翻页交互待后续批次接入"),
        "翻页待接入提示应删除"
    );
    assert!(!js.contains("翻页待接入"), "翻页待接入提示应删除");
    assert!(!js.contains("旧版详情页"), "容器日志不得再指向旧详情页");
}

/// 1.8.0-r3 的代码渲染器注册名是 `code`、文本字段是 `modelValue`；
/// 可编辑表单字段已改用 `textarea`（避免代码编辑器提交空串），`code` 只作只读查看器；
/// 列表必须渲染宿主给定的全部行（审计页定高内部滚动），游标才与本页末行对齐。
#[test]
fn budding_boot_uses_registered_code_component_and_full_rows() {
    let js = include_str!("../../static/budding/budding-boot.js");
    assert!(
        !js.contains("code-viewer"),
        "宿主不得再使用未注册的 code-viewer 类型"
    );
    assert!(
        !js.contains("props: { code:"),
        "code 组件的文本字段是 modelValue，不是 code"
    );
    // `code` 组件只用于只读查看器（日志 / 配置预览），必须显式传 `modelValue` 绑定文本；
    // 可编辑表单字段（配置编辑器 / 文件编辑）统一走 `textarea`，不得再裸挂 `code`。
    // 页面模块里的只读查看器统一由 `schema_kit::code_block` 产出（日志 / 配置预览），
    // 契约钉在这一个构件上，避免各页各写一份后漂移。
    let kit = include_str!("../../src/handlers/ui/schema_kit.rs");
    assert!(
        kit.contains("pub fn code_block(text: &str, height: i64) -> Value {")
            && kit.contains("\"modelValue\": text")
            && kit.contains("\"readonly\": true"),
        "只读代码块必须由 schema_kit 统一产出并绑定 modelValue"
    );
    let mut viewers = 0;
    for (pos, _) in js.match_indices("type: 'code'") {
        let rest = &js[pos + "type: 'code'".len()..];
        let end = rest.find("type: '").unwrap_or(rest.len());
        let seg = &rest[..end];
        assert!(
            seg.contains("modelValue:"),
            "code 查看器必须绑定 modelValue：{seg}"
        );
        assert!(
            !seg.contains("name: '"),
            "code 不得再作可编辑表单字段：{seg}"
        );
        viewers += 1;
    }
    // 日志/配置查看器已随页面迁到服务端（`schema_kit::code_block`），壳脚本只剩
    // 容器终端一处：数量不再随页面迁移而变，只要求「有一处且绑定 modelValue」。
    assert!(viewers >= 1, "壳脚本至少保留终端查看器：{viewers}");
    // 列表行由宿主一次性给出，不得再被组件默认每页 10 条截断（构件层已固定）。
    assert!(kit.contains(r#""perPage": -1"#), "列表应渲染本页全部行");
    // 审计页骨架已迁到服务端：定高与场景空态文案在 audit_page.rs。
    let audit_page = include_str!("../../src/handlers/ui/audit_page.rs");
    assert!(
        audit_page.contains("height: Some(420)") && audit_page.contains("暂无审计记录"),
        "审计列表应定高并内部滚动，且带场景空态文案"
    );
    // 四个菜单图标使用 1.8.1 子集的原生字形，不再近似兜底或留空。
    // 图标映射随路由表迁到服务端（`routes.rs`），断言跟着转过去。
    let routes_src = include_str!("../../src/handlers/ui/routes.rs");
    assert!(
        routes_src.contains(r#"("folder", "mdi-folder-outline")"#),
        "文件管理应使用原生字形"
    );
    assert!(
        routes_src.contains(r#"("disk", "mdi-harddisk")"#),
        "卷应使用原生字形"
    );
    assert!(
        routes_src.contains(r#"("image", "mdi-image-outline")"#),
        "镜像应使用原生字形"
    );
    assert!(
        routes_src.contains(r#"("hard-drive", "mdi-harddisk")"#),
        "配额管理应使用原生字形"
    );
}

/// 登录态失效走 budding `redirect` 组件（1.8.1 新增）：401 不再只抛一行错误，
/// 宿主整页重挂一个 `redirect` 页面，并把各 JSON 通道的 401 都接到同一出口。
#[test]
fn budding_boot_session_expiry_uses_redirect_component() {
    let js = include_str!("../../static/budding/budding-boot.js");
    assert!(
        js.contains("function showSessionExpired()"),
        "401 必须有统一的登录失效出口"
    );
    // 会话过期简化为硬跳转：立即 window.location.replace 到登录页，
    // 不再用 budding redirect 组件（无延迟、无按钮、不重新 embed）。
    assert!(
        js.contains("loginUrl()"),
        "跳转目标应构造带 next 的登录页 URL"
    );
    assert!(
        js.contains("window.location.replace"),
        "401 应硬跳转，不走 budding redirect 组件"
    );
    assert!(js.contains("/login?next="), "跳转目标应回带 next");
    // 每个 fetch 通道一处：会话引导、读数据、读裸 JSON、写通道、上传、插件 RPC、终端。
    // 写通道原先各写一份（submitJson / postPrefixJson），合并后总数为 8；阈值跟着
    // 实现走只会拦「通道被删掉」，不拦「多接一次」。
    let calls = js.matches("showSessionExpired()").count();
    assert!(
        calls >= 8,
        "401 出口覆盖不足：showSessionExpired 出现 {calls} 次"
    );
    assert!(
        !js.contains("会话已过期，请刷新页面后重新登录。"),
        "旧的一行错误文案应被硬跳转替代"
    );
}

/// 1.8.0-r7 起浮层归属图层根（上游 `docs/plan-layer-stacking.md`）：入库主包必须带
/// `data-bud-layer-root` 与浮层 `attach` 选择器，宿主 boot 则不得再注入层叠补丁。
/// 这条断言的作用是「换包时别把层级契约换丢」——升级到没有该契约的旧包会当场失败。
#[test]
fn budding_bundle_carries_layer_root_attach_contract() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("static/budding");
    let mut bundles: Vec<String> = std::fs::read_dir(&dir)
        .expect("static/budding 必须存在")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| {
            name.starts_with("budding-")
                && name.ends_with(".min.js")
                && !name.starts_with("budding-code-")
                && !name.starts_with("budding-terminal-")
        })
        .collect();
    bundles.sort();
    assert_eq!(
        bundles.len(),
        1,
        "目录里应只有一份主包（升级时删除旧产物，避免 immutable 命中旧缓存）：{bundles:?}"
    );
    let js = std::fs::read_to_string(dir.join(&bundles[0])).expect("主包应可读");
    assert!(
        js.contains("data-bud-layer-root"),
        "主包应声明图层根标识（浮层 attach 目标）：{}",
        bundles[0]
    );
    assert!(
        js.contains("[data-bud-layer-root]"),
        "主包应把浮层 attach 到图层根选择器：{}",
        bundles[0]
    );
    let boot = include_str!("../../static/budding/budding-boot.js");
    for banned in ["v-overlay-container", "data-bud-layer-root"] {
        assert!(
            !boot.contains(banned),
            "宿主不得再为弹窗内浮层打补丁（出现 {banned}），层级归上游契约负责"
        );
    }
}

/// 版本化产物 immutable、稳定别名 no-cache，两者同一份字节 → 同一强 ETag；
/// gzip 变体与 identity 是两个表示（ETag 不同）。
#[tokio::test]
async fn budding_static_assets_follow_d11_cache_contract() {
    let h = harness();
    let ver = skiff_web::assets::BUDDING_VERSION;
    let js_v = format!("/p/static/budding/budding-{ver}.min.js");
    let js_a = "/p/static/budding/budding.min.js".to_string();
    let css_v = format!("/p/static/budding/budding-{ver}.min.css");
    let css_a = "/p/static/budding/budding.css".to_string();

    for (uri, immutable) in [
        (js_v.as_str(), true),
        (css_v.as_str(), true),
        (js_a.as_str(), false),
        (css_a.as_str(), false),
    ] {
        let resp = get_raw(&h.app, uri, &[]).await;
        assert_eq!(resp.status(), StatusCode::OK, "{uri} 应 200");
        let cc = header_str(&resp, "cache-control");
        if immutable {
            assert_eq!(cc, "public, max-age=31536000, immutable", "{uri}");
        } else {
            assert_eq!(cc, "no-cache", "{uri}");
        }
        let etag = header_str(&resp, "etag");
        assert!(
            etag.starts_with('"') && etag.ends_with('"') && etag.len() == 66,
            "{uri} 必须是强 ETag，实际: {etag}"
        );
        let ct = header_str(&resp, "content-type");
        assert!(
            ct.starts_with(if uri.ends_with(".css") {
                "text/css"
            } else {
                "text/javascript"
            }),
            "{uri} Content-Type 不对: {ct}"
        );
    }

    // 别名与版本化指向同一份内嵌字节：ETag 必须相同（不复制第二份文件）。
    let versioned = get_raw(&h.app, js_v.as_str(), &[]).await;
    let alias = get_raw(&h.app, js_a.as_str(), &[]).await;
    let etag = header_str(&versioned, "etag");
    assert_eq!(etag, header_str(&alias, "etag"));

    // If-None-Match 命中 304，ETag/Cache-Control 原样保留。
    let resp = get_raw(&h.app, js_v.as_str(), &[("if-none-match", &etag)]).await;
    assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(header_str(&resp, "etag"), etag);
    assert_eq!(
        header_str(&resp, "cache-control"),
        "public, max-age=31536000, immutable"
    );

    // gzip 变体：有 Content-Encoding，ETag 与 identity 不同，字节确实是 gzip。
    let gz = get_raw(&h.app, js_v.as_str(), &[("accept-encoding", "gzip")]).await;
    assert_eq!(gz.status(), StatusCode::OK);
    assert_eq!(header_str(&gz, "content-encoding"), "gzip");
    let gz_etag = header_str(&gz, "etag");
    assert_ne!(gz_etag, etag, "gzip 与 identity 的 ETag 必须不同");
    let bytes = axum::body::to_bytes(gz.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(bytes.len() > 2 && bytes[0] == 0x1f && bytes[1] == 0x8b);
    let gz_alias = get_raw(&h.app, js_a.as_str(), &[("accept-encoding", "gzip")]).await;
    assert_eq!(gz_etag, header_str(&gz_alias, "etag"));
}

/// 壳页里引用的 JS/CSS（含 data 属性里的 CSS 与启动脚本）都必须真实可 200：
/// 防「引用写错、上线才 404」。
#[tokio::test]
async fn shell_html_asset_references_resolve() {
    let h = harness();
    let token = login(&h.app, "admin", "pw-test-123").await;
    let (status, _, html) = get_ui(
        &h.app,
        "/p/files/deep/link",
        Some(&token),
        Some("text/html"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let mut uris: Vec<String> = attr_values(&html, "src");
    uris.extend(attr_values(&html, "data-budding-css"));
    uris.retain(|u| u.starts_with("/p/static/"));
    assert!(
        uris.len() >= 3,
        "壳页应引用产物 JS、启动脚本与 CSS: {uris:?}"
    );
    for uri in uris {
        let resp = get_raw(&h.app, &uri, &[]).await;
        assert_eq!(resp.status(), StatusCode::OK, "壳页引用必须可取: {uri}");
    }
}

/// 版本取值与刷新入口在壳脚本里：字段名与动作名是前后端契约，静态锁住——否则后端改了
/// 字段、前端悄悄取不到值，页面看起来「选了但装出旧版本」。
#[test]
fn budding_boot_runtime_version_sources() {
    let js = include_str!("../../static/budding/budding-boot.js");
    for needle in [
        "formText(v['version_' + kind]).trim();",
        "skiffRuntimeVersions: runtimeVersionsRefresh,",
        "'/runtimes/versions?refresh=true&kind=' + kind",
        "var kinds = ['php', 'node', 'rust'];",
    ] {
        assert!(js.contains(needle), "运行环境版本来源缺失：{needle}");
    }
}
