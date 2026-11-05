// Skiff 前端交互（无 Node 构建链）：日志 SSE、容器终端、插件 UI 桥。
(function () {
  "use strict";

  // ---- 日志跟随（SSE）：断连由浏览器 EventSource 自身重连；页面卸载即断开 ----
  function attachLogs() {
    var el = document.getElementById("logs");
    if (!el || el.dataset.attached === "1") return;
    var src = el.dataset.src;
    if (!src) return;
    el.dataset.attached = "1";
    var es = new EventSource(src);
    es.onmessage = function (ev) {
      el.textContent += ev.data + "\n";
      if (el.textContent.length > 400000) el.textContent = el.textContent.slice(-200000);
      el.scrollTop = el.scrollHeight;
    };
    es.addEventListener("error", function () {
      el.textContent += "\n[连接中断]\n";
      es.close();
    });
    window.addEventListener("beforeunload", function () { es.close(); });
  }

  // ---- 容器终端：懒加载 xterm，先换一次性票据再连 WS（§6.2）----
  // 票据绑定会话 + 容器、30 秒有效、用一次即废，所以每次打开终端都要重新申请。
  function attachTerminal() {
    var el = document.getElementById("term");
    if (!el || el.dataset.attached === "1") return;
    var wsUrl = el.dataset.ws;
    var ticketUrl = el.dataset.ticket;
    if (!wsUrl || !ticketUrl) return;
    if (typeof window.Terminal === "undefined") {
      el.innerHTML = '<p class="hint">终端组件未就绪（xterm 未加载成功）。</p>';
      return;
    }
    el.dataset.attached = "1";
    var term = new window.Terminal({ fontSize: 12, theme: { background: "#0b0d11" }, cursorBlink: true });
    term.open(el);
    var proto = location.protocol === "https:" ? "wss:" : "ws:";
    term.writeln("\u001b[33m[申请终端票据…]\u001b[0m");

    fetch(ticketUrl, {
      method: "POST",
      headers: { "x-csrf-token": el.dataset.csrf || "" },
      credentials: "same-origin"
    }).then(function (r) {
      if (!r.ok) throw new Error("HTTP " + r.status);
      return r.json();
    }).then(function (data) {
      if (!data || typeof data.ticket !== "string" || !data.ticket) {
        throw new Error("票据响应缺少 ticket");
      }
      var ws = new WebSocket(
        proto + "//" + location.host + wsUrl + "?ticket=" + encodeURIComponent(data.ticket)
      );
      ws.onopen = function () { term.writeln("\u001b[32mconnected\u001b[0m"); };
      ws.onmessage = function (ev) { term.write(ev.data); };
      ws.onclose = function () { term.writeln("\r\n\u001b[33m[disconnected]\u001b[0m"); };
      term.onData(function (d) { if (ws.readyState === 1) ws.send(d); });
      window.addEventListener("beforeunload", function () { ws.close(); });
    }).catch(function (e) {
      term.writeln("\u001b[31m无法获取终端票据：" + String(e) + "\u001b[0m");
    });
  }

  // 插件 id 的语法校验：只允许 `com.x.y` 这类可打印字符，避免把消息里的字符串
  // 直接拼进跳转 URL。
  var PLUGIN_ID_RE = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;

  /// 面板级跳转：插件请父页面切到另一个插件的界面（插件间来回切）。
  ///
  /// 这条消息**不转发**给任何插件进程，只由父页面改浏览器地址；`path` 是给目标
  /// 插件的定位建议（例如「跳到文件管理并打开 /etc/nginx/conf.d」），能否读仍由
  /// 目标插件自己的 `path:` 授权根判定——跳转本身不授予任何能力。
  /// 目标必须在白名单里（详情页渲染的「已启用且有 UI 的插件」）。
  function openFromPlugin(win, nonce, msg, allowed, prefix) {
    var id = msg.id;
    function fail(why) {
      win.postMessage({ type: "skiff.open.result", nonce: nonce, id: id, ok: false, error: why }, "*");
    }
    var target = typeof msg.plugin === "string" ? msg.plugin : "";
    if (!PLUGIN_ID_RE.test(target)) return fail("目标插件 id 非法");
    if (allowed.indexOf(target) < 0) return fail("目标插件未启用或没有界面");
    var open = "";
    if (typeof msg.path === "string" && msg.path) {
      if (msg.path.length > 512) return fail("定位路径过长");
      if (/[\u0000-\u001f\u007f]/.test(msg.path)) return fail("定位路径含控制字符");
      open = msg.path;
    }
    var url = "/" + prefix + "/plugins/" + encodeURIComponent(target);
    if (open) url += "?open=" + encodeURIComponent(open);
    window.location.assign(url);
  }

  // ---- 插件 UI 沙箱桥 ----
  // 父页面校验 event.source === iframe.contentWindow、nonce、消息 schema，
  // 再代发到 /{prefix}/plugins/{id}/rpc（携带 CSRF 头），回包 targetOrigin="*"
  // 但绑定同一 source+nonce，且不携带任何面板凭据。
  function attachPluginBridges() {
    var prefix = (document.body && document.body.dataset.prefix) || "";
    document.querySelectorAll("iframe[data-plugin]").forEach(function (frame) {
      var plugin = frame.dataset.plugin;
      var nonce = frame.dataset.nonce;
      var endpoint = frame.dataset.rpc;
      var csrf = frame.dataset.csrf;
      var allowed = (frame.dataset.plugins || "").split(",").filter(function (s) { return s; });
      if (!plugin || !nonce || !endpoint) return;
      window.addEventListener("message", function (ev) {
        if (ev.source !== frame.contentWindow) return;
        var msg = ev.data;
        if (!msg) return;
        if (msg.type === "skiff.open") {
          if (msg.nonce !== nonce) return;
          openFromPlugin(frame.contentWindow, nonce, msg, allowed, prefix);
          return;
        }
        // 插件上报内容高度：父页面把 iframe 调到该高度，消除「iframe 内部滚动条 +
        // 页面滚动条」双滚动。数值做上下限裁剪；旧插件不发消息则维持 CSS 默认高度。
        if (msg.type === "skiff.height") {
          if (msg.nonce !== nonce) return;
          // 只接受工作台 iframe：固定槽位有 manifest 声明的高度，不允许被内容撑开。
          if (!frame.hasAttribute("data-workbench-frame")) return;
          var height = Number(msg.height);
          if (!isFinite(height) || height < 1) return;
          height = Math.max(320, Math.min(20000, Math.round(height)));
          frame.classList.add("auto-height");
          if (frame.style.height !== height + "px") frame.style.height = height + "px";
          return;
        }
        if (msg.type !== "skiff.rpc" || msg.nonce !== nonce) return;
        if (typeof msg.method !== "string" || msg.method.length > 64) return;
        var id = msg.id;
        fetch(endpoint, {
          method: "POST",
          headers: { "content-type": "application/json", "x-csrf-token": csrf },
          credentials: "same-origin",
          body: JSON.stringify({ method: msg.method, params: msg.params || null })
        }).then(function (r) { return r.json().catch(function () { return { error: { message: "bad json" } }; }); })
          .then(function (data) {
            frame.contentWindow.postMessage(
              { type: "skiff.rpc.result", nonce: nonce, id: id, result: data.result, error: data.error },
              "*"
            );
          })
          .catch(function (e) {
            frame.contentWindow.postMessage(
              { type: "skiff.rpc.result", nonce: nonce, id: id, error: { message: String(e) } },
              "*"
            );
          });
      });
    });
  }

  // ---- 菜单：持久手风琴（无任何悬停展开），键盘可达，窄屏抽屉 ----
  // 一级行 = 标签 + 独立箭头：箭头只管展开/收起；有自身页面的标签是链接（跳转），
  // 无自身页面的标签是按钮（等同展开）。同一时间只展开一个分组；点空白不收起，
  // 只有切到另一个分组或按 Escape 才收起。展开状态写入 sessionStorage，刷新后保持。
  // 上下布局把二级菜单搬进顶栏下方的独立全宽栏（`.subnav-inner`），侧栏/抽屉留在组内。
  // 菜单关闭时带 `hidden`，Tab 不会进入二级项；Escape 收起并把焦点还给箭头按钮。
  var NAV_STORE_KEY = "skiff.nav.open";
  var NAV_NONE = "none";

  function readNavOpen() {
    try { return sessionStorage.getItem(NAV_STORE_KEY) || ""; } catch (e) { return ""; }
  }
  function writeNavOpen(key) {
    try {
      if (key) sessionStorage.setItem(NAV_STORE_KEY, key);
      else sessionStorage.removeItem(NAV_STORE_KEY);
    } catch (e) { /* 隐私模式禁用存储时退化为不记忆 */ }
  }

  function attachNav() {
    if (document.body.dataset.navReady === "1") return;
    document.body.dataset.navReady = "1";

    var groups = Array.prototype.slice.call(document.querySelectorAll("[data-nav-group]"));
    var subnav = document.querySelector("[data-subnav]");
    var subnavInner = subnav ? subnav.querySelector(".subnav-inner") : null;
    var body = document.body;
    var drawer = document.querySelector("[data-menu-toggle]");
    var narrow = window.matchMedia ? window.matchMedia("(max-width: 860px)") : null;

    function isNarrow() { return narrow ? narrow.matches : window.innerWidth <= 860; }
    // bar 模式 = 上下布局且非窄屏：二级走独立通栏；侧栏与抽屉都就地展开。
    function barMode() { return body.dataset.layout !== "left" && !isNarrow(); }

    // 菜单宿主可能在 `.nav-group` 内（侧栏/抽屉），也可能被搬到 `.subnav-inner`
    //（上下布局）。因此按 `data-nav-key` 全局查找，不依赖当前父节点。
    function menuOf(key) {
      var all = document.querySelectorAll("[data-nav-key]");
      for (var i = 0; i < all.length; i++) {
        if (all[i].getAttribute("data-nav-key") === key) return all[i];
      }
      return null;
    }

    function setOpen(group, open) {
      var toggle = group.querySelector("[data-nav-toggle]");
      var menu = menuOf(group.dataset.menuKey || "");
      if (!toggle || !menu) return;
      if (open) {
        menu.removeAttribute("hidden");
        group.classList.add("open");
      } else {
        menu.setAttribute("hidden", "");
        group.classList.remove("open");
      }
      toggle.setAttribute("aria-expanded", open ? "true" : "false");
    }

    function syncSubnav() {
      if (!subnav) return;
      var anyOpen = groups.some(function (g) { return g.classList.contains("open"); });
      var show = barMode() && anyOpen;
      subnav.classList.toggle("open", show);
      subnav.setAttribute("aria-hidden", show ? "false" : "true");
    }

    // key=null 表示全部收起；`NAV_NONE` 让刷新后也保持收起。
    // remember=false 只用于首屏默认展开（不把「默认」写死成用户选择）。
    function openKey(key, remember) {
      var target = null;
      groups.forEach(function (g) {
        var on = !!key && g.dataset.menuKey === key;
        setOpen(g, on);
        if (on) target = g;
      });
      if (remember !== false) writeNavOpen(target ? key : NAV_NONE);
      syncSubnav();
      return target;
    }

    // 上下布局：把当前分组的菜单搬进通栏；侧栏/抽屉则放回分组内。
    function placeMenus() {
      if (!subnavInner) return;
      var inBar = barMode();
      groups.forEach(function (g) {
        var menu = menuOf(g.dataset.menuKey || "");
        if (!menu) return;
        if (inBar) subnavInner.appendChild(menu);
        else g.appendChild(menu);
      });
      syncSubnav();
    }

    function linksOf(group) {
      var menu = menuOf(group.dataset.menuKey || "");
      return menu ? Array.prototype.slice.call(menu.querySelectorAll("a")) : [];
    }

    function focusTop(delta) {
      var tops = Array.prototype.slice.call(
        document.querySelectorAll(".nav > .nav-link, .nav > .nav-group > .nav-row > .nav-label")
      );
      var i = tops.indexOf(document.activeElement);
      if (i < 0) return;
      var next = tops[(i + delta + tops.length) % tops.length];
      if (next) next.focus();
    }

    groups.forEach(function (group) {
      var caret = group.querySelector("[data-nav-toggle]");
      var label = group.querySelector("[data-nav-label]");
      var toggle = function () {
        if (group.classList.contains("open")) openKey(null);
        else openKey(group.dataset.menuKey || "");
      };

      if (caret) {
        caret.addEventListener("click", toggle);
        caret.addEventListener("keydown", function (e) {
          if (e.key === "ArrowDown") {
            e.preventDefault();
            openKey(group.dataset.menuKey || "");
            var first = linksOf(group)[0];
            if (first) first.focus();
          } else if (e.key === "ArrowRight" || e.key === "ArrowLeft") {
            e.preventDefault();
            focusTop(e.key === "ArrowRight" ? 1 : -1);
          }
        });
      }

      // 无自身页面的一级：标签本身就是展开按钮。
      if (label && label.tagName === "BUTTON") label.addEventListener("click", toggle);
      // 有自身页面的一级：标签负责跳转，但先记住该分组，落地后保持展开。
      if (label && label.tagName !== "BUTTON") {
        label.addEventListener("click", function () {
          writeNavOpen(group.dataset.menuKey || "");
        });
      }

      var menu = menuOf(group.dataset.menuKey || "");
      if (menu) {
        menu.addEventListener("keydown", function (e) {
          var links = linksOf(group);
          var i = links.indexOf(document.activeElement);
          if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            e.preventDefault();
            if (!links.length) return;
            var next = links[(i + (e.key === "ArrowDown" ? 1 : -1) + links.length) % links.length];
            if (next) next.focus();
          } else if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
            e.preventDefault();
            setOpen(group, false);
            if (caret) caret.focus();
          }
        });
      }
    });

    function setDrawer(open) {
      if (!drawer) return;
      body.classList.toggle("nav-open", open);
      drawer.setAttribute("aria-expanded", open ? "true" : "false");
    }

    if (drawer) {
      drawer.addEventListener("click", function () {
        setDrawer(!body.classList.contains("nav-open"));
      });
    }

    // 抽屉：点顶栏外部收起；导航分组本身不再因「点空白」收起。
    document.addEventListener("click", function (e) {
      var target = e.target;
      if (!(target instanceof Element)) return;
      if (target.closest("[data-menu-toggle]")) return;
      if (isNarrow() && !target.closest(".topbar") && body.classList.contains("nav-open")) {
        setDrawer(false);
      }
    });

    document.addEventListener("keydown", function (e) {
      if (e.key !== "Escape") return;
      var openGroup = groups.filter(function (g) {
        return g.classList.contains("open");
      })[0];
      var changed = false;
      if (openGroup) {
        openKey(null);
        var caret = openGroup.querySelector("[data-nav-toggle]");
        if (caret) caret.focus();
        changed = true;
      }
      if (body.classList.contains("nav-open")) {
        setDrawer(false);
        if (drawer) drawer.focus();
        changed = true;
      }
      if (changed) e.preventDefault();
    });

    function onViewportChange() {
      if (!isNarrow()) setDrawer(false);
      placeMenus();
    }
    if (narrow && narrow.addEventListener) narrow.addEventListener("change", onViewportChange);
    else window.addEventListener("resize", onViewportChange);

    placeMenus();

    // 初始：优先恢复会话记忆（`none` = 用户显式收起），其次展开「当前页所在分组」。
    var restored = readNavOpen();
    var currentGroup = document.querySelector("[data-nav-group].current");
    var initial = restored === NAV_NONE ? null : (restored || (currentGroup && currentGroup.dataset.menuKey) || null);
    openKey(initial, restored !== "");
  }

  // ---- L3 工作台标签栏：同一个 iframe，tab 参数 + postMessage ----
  // 切换同一 UI 文件时只发消息（不重载 iframe，避免白屏与重复鉴权）；
  // 声明了不同 `ui` 的标签才换 iframe 文件。链接本身可直达，无 JS 也能用。
  function attachWorkbench() {
    var bar = document.querySelector("[data-workbench]");
    var frame = document.querySelector("[data-workbench-frame]");
    if (!bar || !frame) return;
    var links = Array.prototype.slice.call(bar.querySelectorAll("[data-workbench-tab]"));

    // 吸顶偏移：L3 横栏落在顶栏（上下布局含二级通栏）之下。用实测高度写入
    // `--shell-h`：窗口缩放、二级通栏展开/收起、窄屏顶栏换行都会自动跟着变。
    var shell = document.querySelector(".top-shell");
    var root = document.documentElement;
    function syncShell() {
      root.style.setProperty(
        "--shell-h",
        (shell ? Math.round(shell.getBoundingClientRect().height) : 0) + "px"
      );
    }
    syncShell();
    window.addEventListener("resize", syncShell);
    if (shell && typeof ResizeObserver !== "undefined") {
      new ResizeObserver(syncShell).observe(shell);
    }

    function apply(link, push) {
      if (!link) return;
      var tab = link.dataset.tab || "";
      var ui = link.dataset.ui || "";
      var href = link.getAttribute("href") || "";
      if (!tab || !href) return;
      links.forEach(function (a) {
        if (a === link) a.setAttribute("aria-current", "page");
        else a.removeAttribute("aria-current");
        a.classList.toggle("on", a === link);
      });
      if (push) history.pushState({ workbenchTab: tab, href: href }, "", href);
      if (ui && ui !== frame.dataset.ui) {
        frame.dataset.ui = ui;
        // 换 UI 文件等于重新加载：高度先回落到 CSS 默认值，等新页面上报内容高度。
        frame.style.height = "";
        frame.classList.remove("auto-height");
        frame.src = link.dataset.src || frame.src;
        return;
      }
      frame.dataset.tab = tab;
      if (frame.contentWindow) {
        frame.contentWindow.postMessage(
          { type: "skiff.menu", nonce: frame.dataset.nonce, menu: frame.dataset.menu, tab: tab },
          "*"
        );
      }
    }

    bar.addEventListener("click", function (e) {
      var target = e.target;
      if (!(target instanceof Element)) return;
      var link = target.closest("[data-workbench-tab]");
      if (!link) return;
      e.preventDefault();
      apply(link, true);
    });
    window.addEventListener("popstate", function () {
      if (!history.state || !history.state.workbenchTab) return;
      var tab = history.state.workbenchTab;
      apply(links.filter(function (a) { return a.dataset.tab === tab; })[0], false);
    });
  }

  // ---- 固定页槽位：失败降级 + 自动刷新 ----
  // 插件停用/未授权在服务端就不渲染槽位；这里只兜底「启用了但界面没起来」。
  function attachMounts() {
    document.querySelectorAll("[data-mount]").forEach(function (card) {
      var frame = card.querySelector("iframe");
      if (!frame || frame.dataset.attached === "1") return;
      frame.dataset.attached = "1";
      var fail = card.querySelector("[data-mount-fail]");
      var retry = card.querySelector("[data-mount-retry]");
      var timer = 0;

      function showFail() {
        if (fail) fail.removeAttribute("hidden");
      }
      function watch() {
        clearTimeout(timer);
        timer = setTimeout(showFail, 5000);
      }
      frame.addEventListener("load", function () { clearTimeout(timer); });
      frame.addEventListener("error", showFail);
      watch();

      if (retry) {
        retry.addEventListener("click", function () {
          if (fail) fail.setAttribute("hidden", "");
          var src = (frame.getAttribute("src") || "").replace(/[?&]r=\d+/, "");
          frame.src = src + (src.indexOf("?") >= 0 ? "&" : "?") + "r=" + Date.now();
          watch();
        });
      }

      var refresh = parseInt(frame.dataset.refresh || "0", 10);
      if (refresh > 0) {
        setInterval(function () {
          if (frame.contentWindow) {
            frame.contentWindow.postMessage(
              { type: "skiff.refresh", nonce: frame.dataset.nonce },
              "*"
            );
          }
        }, refresh * 1000);
      }
    });
  }

  // ---- 主题：默认浅色；暗色为完整可选项，选择持久化到 localStorage ----
  // 首屏由 `/static/theme-init.js` 在样式表之前落地 `data-theme`；这里负责切换、
  // 按钮可访问性、向插件 iframe 广播，并在 htmx 局部替换 / 前进后退缓存恢复后
  // 按 localStorage 重新落地。键名与 theme-init.js 必须一致。
  var THEME_KEY = "skiff.theme";
  function attachTheme() {
    var root = document.documentElement;

    function frames() {
      return Array.prototype.slice.call(
        document.querySelectorAll("iframe.mount-frame, iframe.plugin-frame")
      );
    }

    function current() {
      return root.getAttribute("data-theme") === "dark" ? "dark" : "light";
    }

    function broadcastTheme(theme) {
      frames().forEach(function (f) {
        if (f.contentWindow) {
          f.contentWindow.postMessage({ type: "skiff.theme", theme: theme }, "*");
        }
      });
    }

    // 按钮的可见文案由 CSS 按主题切换（深色时显示「浅色」）；这里同步语义状态，
    // 保证 aria-label 描述的是「点击后会发生什么」，aria-pressed 表示「当前是否深色」。
    function syncButton(theme) {
      var button = document.querySelector("[data-theme-toggle]");
      if (!button) return;
      var dark = theme === "dark";
      button.setAttribute("aria-pressed", dark ? "true" : "false");
      button.setAttribute("aria-label", dark ? "切换到浅色主题" : "切换到深色主题");
      button.title = dark ? "切换到浅色主题" : "切换到深色主题";
    }

    function apply(theme, persist) {
      var dark = theme === "dark";
      root.setAttribute("data-theme", dark ? "dark" : "light");
      syncButton(dark ? "dark" : "light");
      if (persist) {
        try { localStorage.setItem(THEME_KEY, dark ? "dark" : "light"); } catch (e) { /* ignore */ }
      }
      broadcastTheme(dark ? "dark" : "light");
    }

    function storedTheme() {
      try {
        var stored = localStorage.getItem(THEME_KEY);
        if (stored === "dark" || stored === "light") return stored;
      } catch (e) { /* 隐私模式 */ }
      return null;
    }

    // 以 localStorage 为准重新落地：刷新、htmx 局部替换、bfcache 恢复都走这里。
    // 存储值缺失时才保留首屏已经落地的属性，避免无谓地把暗色刷成浅色。
    function syncFromStorage() {
      var theme = storedTheme() || current();
      if (theme !== current()) {
        apply(theme, false);
      } else {
        syncButton(theme);
        broadcastTheme(theme);
      }
    }

    if (root.dataset.themeReady !== "1") {
      root.dataset.themeReady = "1";

      // 用文档级委托，htmx 替换掉按钮节点后仍然有效。
      document.addEventListener("click", function (ev) {
        var button = ev.target && ev.target.closest ? ev.target.closest("[data-theme-toggle]") : null;
        if (!button) return;
        apply(current() === "dark" ? "light" : "dark", true);
      });

      // iframe 加载完成后补发，避免子页面脚本尚未就绪时收到的消息被丢弃。
      document.addEventListener("load", function (e) {
        var target = e.target;
        if (!target || target.tagName !== "IFRAME") return;
        if (!target.matches("iframe.mount-frame, iframe.plugin-frame")) return;
        if (target.contentWindow) {
          target.contentWindow.postMessage({ type: "skiff.theme", theme: current() }, "*");
        }
      }, true);

      // 所有 iframe 都加载完后再统一补发一次，覆盖「iframe 先加载、父脚本后就绪」的时序。
      window.addEventListener("load", function () { broadcastTheme(current()); });

      // 插件页面主动索取主题（插件侧在脚本执行和 load 时各发一次）。
      window.addEventListener("message", function (ev) {
        var msg = ev.data;
        if (!msg || msg.type !== "skiff.theme.request") return;
        var frame = frames().filter(function (f) { return f.contentWindow === ev.source; })[0];
        if (!frame) return;
        ev.source.postMessage({ type: "skiff.theme", theme: current() }, "*");
      });

      // 前进/后退缓存恢复时 DOM 是旧快照，按当前存储值重新落地并修按钮语义。
      window.addEventListener("pageshow", syncFromStorage);
      // 其他标签页改了主题时跟随（storage 事件只在别的文档触发）。
      window.addEventListener("storage", function (e) {
        if (e.key !== THEME_KEY) return;
        apply(e.newValue === "dark" ? "dark" : "light", false);
      });

      syncFromStorage();
    } else {
      // htmx 局部替换：重新同步按钮语义，并给新插入的 iframe 补发主题。
      syncFromStorage();
    }
  }

  // ---- 统一确认弹窗：表单加 data-confirm 即拦截提交，二次确认后放行 ----
  function attachConfirm() {
    var dialog = document.querySelector("[data-confirm-dialog]");
    if (!dialog) return;
    var message = dialog.querySelector("[data-confirm-message]");
    var ok = dialog.querySelector("[data-confirm-ok]");
    var cancel = dialog.querySelector("[data-confirm-cancel]");
    var pending = null;

    function close() {
      if (typeof dialog.close === "function") dialog.close();
      else dialog.removeAttribute("open");
      pending = null;
    }

    function open(text, run) {
      if (typeof dialog.showModal !== "function") {
        if (window.confirm(text)) run();
        return;
      }
      if (message) message.textContent = text;
      pending = run;
      dialog.showModal();
    }

    document.addEventListener("submit", function (e) {
      var form = e.target;
      if (!form || !form.dataset) return;
      // 确认文案可放在表单上（整表操作），也可放在触发按钮上（同一表单多种动作）。
      var submitter = e.submitter;
      var msg = (submitter && submitter.dataset && submitter.dataset.confirm)
        || form.dataset.confirm;
      if (!msg) return;
      if (form.dataset.confirmed === "1") {
        delete form.dataset.confirmed;
        return;
      }
      e.preventDefault();
      open(msg, function () {
        form.dataset.confirmed = "1";
        if (submitter && typeof form.requestSubmit === "function") {
          form.requestSubmit(submitter);
          return;
        }
        if (submitter && submitter.name) {
          var hidden = document.createElement("input");
          hidden.type = "hidden";
          hidden.name = submitter.name;
          hidden.value = submitter.value;
          form.appendChild(hidden);
        }
        form.submit();
      });
    });

    if (ok) ok.addEventListener("click", function () {
      var run = pending;
      close();
      if (run) run();
    });
    if (cancel) cancel.addEventListener("click", close);
    dialog.addEventListener("close", function () { pending = null; });
  }

  // ---- 网站创建向导：三步（类型 → 分支参数 → 确认），就地校验 ----
  function attachWizard() {
    var wizard = document.querySelector("[data-wizard]");
    if (!wizard) return;
    var panels = Array.prototype.slice.call(wizard.querySelectorAll("[data-wizard-panel]"));
    var dots = Array.prototype.slice.call(wizard.querySelectorAll("[data-wizard-step]"));
    if (!panels.length) return;
    var current = 1;

    function form() { return wizard.querySelector("form"); }
    function field(name) { return form() ? form().querySelector('[name="' + name + '"]') : null; }
    function errorFor(name) { return wizard.querySelector('[data-error-for="' + name + '"]'); }

    function showError(name, text) {
      var el = errorFor(name);
      if (!el) return;
      if (text) { el.textContent = text; el.removeAttribute("hidden"); }
      else { el.textContent = ""; el.setAttribute("hidden", ""); }
    }

    function validDomain(v) {
      return /^[A-Za-z0-9](?:[A-Za-z0-9.-]{0,251}[A-Za-z0-9])?$/.test(v) && !v.includes("..");
    }

    function siteType() {
      var el = form() ? form().querySelector('[name="site_type"]:checked') : null;
      return el ? el.value : "";
    }

    // 第二步按类型只显示对应分支；未选类型时全部隐藏。
    function syncType() {
      var type = siteType();
      var rootWrap = wizard.querySelector('[data-type-only="root"]');
      if (rootWrap) {
        // 静态/运行环境都可能自定义目录；反代不展示目录。
        if (type === "proxy") rootWrap.setAttribute("hidden", "");
        else rootWrap.removeAttribute("hidden");
      }
      wizard.querySelectorAll("[data-type-panel]").forEach(function (p) {
        if (p.dataset.typePanel === type) p.removeAttribute("hidden");
        else p.setAttribute("hidden", "");
        // 未命中的分支禁用输入，避免把另一分支的字段提交上去。
        p.querySelectorAll("input,select,textarea").forEach(function (el) {
          if (p.dataset.typePanel === type) el.removeAttribute("disabled");
          else el.setAttribute("disabled", "");
        });
      });
    }

    function validateStep(step) {
      var ok = true;
      if (step === 1) {
        if (!siteType()) { ok = false; }
        var typeErr = errorFor("site_type");
        if (typeErr) {
          if (!siteType()) { typeErr.textContent = "请选择网站类型"; typeErr.removeAttribute("hidden"); }
          else { typeErr.textContent = ""; typeErr.setAttribute("hidden", ""); }
        }
      }
      if (step === 2) {
        var type = siteType();
        var domain = field("domain");
        var aliases = field("aliases");
        var root = field("root_dir");
        var dv = domain ? domain.value.trim() : "";
        if (!dv) { showError("domain", "请填写主域名"); ok = false; }
        else if (!validDomain(dv)) { showError("domain", "域名只能包含字母、数字、点和中划线，且不能以符号开头或结尾"); ok = false; }
        else showError("domain", "");

        var av = aliases ? aliases.value.trim() : "";
        if (av) {
          var bad = av.split(/[\s,，]+/).filter(function (x) { return x && !validDomain(x); });
          if (bad.length) { showError("aliases", "别名域名格式不正确：" + bad.join("、")); ok = false; }
          else showError("aliases", "");
        } else showError("aliases", "");

        var rv = root ? root.value.trim() : "";
        if (rv) {
          if (!rv.startsWith("/") || rv.includes("..")) { showError("root_dir", "站点目录必须是以 / 开头的绝对路径，且不能包含 .."); ok = false; }
          else if (!/\/sites\/[^\/]+$/.test(rv.replace(/\/+$/, ""))) { showError("root_dir", "站点目录必须是 /www/sites/<域名> 形式的一级目录"); ok = false; }
          else showError("root_dir", "");
        } else showError("root_dir", "");

        if (type === "runtime") {
          var rt = field("runtime_id");
          var selected = rt ? rt.options[rt.selectedIndex] : null;
          if (!rt || !rt.value) {
            showError("runtime_id", "请选择运行环境"); ok = false;
          } else if (selected && selected.dataset.disabled === "1") {
            showError("runtime_id", selected.dataset.reason || "该运行环境尚未就绪");
            ok = false;
          } else showError("runtime_id", "");
        } else if (type === "proxy") {
          var target = field("proxy_target");
          var tv = target ? target.value.trim() : "";
          if (!tv) { showError("proxy_target", "请填写上游地址"); ok = false; }
          else if (!/^http(s)?:\/\/[A-Za-z0-9._-]+(:\d{1,5})?(\/[^\s]*)?$/.test(tv)) {
            showError("proxy_target", "上游地址必须形如 http://127.0.0.1:8080"); ok = false;
          } else if (dv && tv.replace(/^http(s)?:\/\//, "").split(":")[0].toLowerCase() === dv.toLowerCase()) {
            showError("proxy_target", "上游地址不能指向站点自身"); ok = false;
          } else showError("proxy_target", "");
        }
      }
      return ok;
    }

    function summary() {
      var type = siteType();
      var typeText = { runtime: "运行环境", static: "静态网站", proxy: "反向代理" }[type] || "—";
      var domain = field("domain");
      var aliases = field("aliases");
      var root = field("root_dir");
      var rt = field("runtime_id");
      var target = field("proxy_target");
      var ws = form() ? form().querySelector('[name="proxy_websocket"]') : null;
      var map = {
        type: typeText,
        domain: domain ? domain.value.trim() : "",
        aliases: aliases && aliases.value.trim() ? aliases.value.trim() : "无",
        root: type === "proxy" ? "不适用（反向代理不落文档根）" : (root && root.value.trim() ? root.value.trim() : "默认站点目录"),
        runtime: type === "runtime"
          ? (rt && rt.value ? (rt.options[rt.selectedIndex] ? rt.options[rt.selectedIndex].text : rt.value) : "—")
          : (type === "proxy" ? "不适用" : "不适用（静态网站不接 PHP）"),
        target: type === "proxy" && target ? target.value.trim() : "不适用",
        websocket: type === "proxy" ? (ws && ws.checked ? "开启" : "关闭") : "不适用"
      };
      Object.keys(map).forEach(function (k) {
        var el = wizard.querySelector('[data-summary="' + k + '"]');
        if (el) el.textContent = map[k] || "—";
      });
    }

    function show(step) {
      current = step;
      panels.forEach(function (p) {
        var n = parseInt(p.dataset.wizardPanel, 10);
        if (n === step) p.removeAttribute("hidden");
        else p.setAttribute("hidden", "");
      });
      if (step >= 2) syncType();
      dots.forEach(function (d) {
        var n = parseInt(d.dataset.wizardStep, 10);
        d.classList.toggle("on", n === step);
        d.classList.toggle("done", n < step);
        if (n === step) d.setAttribute("aria-current", "step");
        else d.removeAttribute("aria-current");
      });
      if (step === 3) summary();
      var first = panels.filter(function (p) { return parseInt(p.dataset.wizardPanel, 10) === step; })[0];
      if (first) {
        var focusable = first.querySelector("input,select,textarea,button");
        if (focusable) focusable.focus();
      }
    }

    wizard.addEventListener("click", function (e) {
      var target = e.target;
      if (!(target instanceof Element)) return;
      if (target.closest("[data-wizard-next]")) {
        if (validateStep(current)) show(Math.min(current + 1, panels.length));
      } else if (target.closest("[data-wizard-back]")) {
        show(Math.max(current - 1, 1));
      }
    });
    wizard.addEventListener("change", function (e) {
      var target = e.target;
      if (target instanceof HTMLInputElement && target.name === "site_type") syncType();
    });

    var start = parseInt(wizard.dataset.wizardStart || "1", 10);
    show(start >= 1 && start <= panels.length ? start : 1);
  }

  // ---- 服务端提示转为轻量 toast（不阻塞页面） ----
  function attachToasts() {
    document.querySelectorAll(".alert[data-toast]").forEach(function (alert) {
      var toast = document.createElement("div");
      toast.className = "toast show" + (alert.classList.contains("ok") ? "" : " error");
      toast.setAttribute("role", "status");
      toast.textContent = alert.textContent.trim();
      document.body.appendChild(toast);
      alert.parentNode && alert.parentNode.removeChild(alert);
      setTimeout(function () { toast.classList.remove("show"); }, 4200);
      setTimeout(function () { toast.parentNode && toast.parentNode.removeChild(toast); }, 4700);
    });
  }

  // 内联 SVG 精灵兜底：插件 manifest 里的图标名若不在内置集合内（或旧包携带
  // 未知名称），统一回退到中性圆点，避免出现空白占位。
  function ensureIcons() {
    document.querySelectorAll("svg.menu-ico use").forEach(function (use) {
      var href = use.getAttribute("href") || use.getAttribute("xlink:href") || "";
      if (!href || href.charAt(0) !== "#") return;
      if (!document.getElementById(href.slice(1))) use.setAttribute("href", "#i-dot");
    });
  }

  function boot() {
    attachLogs();
    attachTerminal();
    attachPluginBridges();
    attachNav();
    attachWorkbench();
    attachMounts();
    attachTheme();
    attachConfirm();
    ensureIcons();
    attachWizard();
    attachToasts();
  }

  document.addEventListener("DOMContentLoaded", boot);
  document.body.addEventListener("htmx:afterSwap", boot);
})();
