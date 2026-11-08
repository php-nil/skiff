// 插件 UI 的 RPC 桥客户端：所有能力调用都经父页面转发（iframe 自身没有凭据）。
// 协议见 crates/web/static/skiff.js::attachPluginBridges。
(function () {
  var nonce = new URLSearchParams(location.search).get("nonce") || "";
  var pending = {};
  var seq = 0;

  // ---- 主题：面板父页面经 postMessage 下发；未收到时保持本插件默认深色 ----
  window.addEventListener("message", function (ev) {
    if (ev.source !== parent) return;
    var msg = ev.data;
    if (msg && msg.type === "skiff.theme" && (msg.theme === "light" || msg.theme === "dark")) {
      document.documentElement.setAttribute("data-theme", msg.theme);
    }
  });
  // 主动向面板索取一次主题：iframe 可能比父页面脚本先就绪，只靠广播会漏。
  function requestTheme() {
    try { parent.postMessage({ type: "skiff.theme.request" }, "*"); } catch (e) { /* 独立打开时无父页面 */ }
  }
  requestTheme();
  window.addEventListener("load", requestTheme);

  // ---- 内容高度上报：父面板据此把 iframe 调到内容高度，避免内外双滚动 ----
  // 未注入面板（独立打开）或旧面板不消费该消息时，行为与以前一致。
  var heightTimer = 0;
  function reportHeight() {
    if (!nonce) return;
    var body = document.body;
    var h = body ? Math.max(body.scrollHeight, body.offsetHeight) : 0;
    if (h < 1) return;
    try {
      parent.postMessage({ type: "skiff.height", nonce: nonce, height: Math.ceil(h) }, "*");
    } catch (e) { /* 独立打开 */ }
  }
  function scheduleHeight() {
    if (heightTimer) clearTimeout(heightTimer);
    heightTimer = setTimeout(reportHeight, 60);
  }
  window.addEventListener("load", reportHeight);
  window.addEventListener("resize", scheduleHeight);
  if (typeof ResizeObserver !== "undefined") {
    new ResizeObserver(scheduleHeight).observe(document.body);
  }

  // ---- 内核 L3 标签 → 内部视图（同一 iframe，切换不重载）----
  function setView(name) {
    var views = document.querySelectorAll("[data-view]");
    var known = Array.prototype.some.call(views, function (v) {
      return v.getAttribute("data-view") === name;
    });
    views.forEach(function (v) {
      v.hidden = !!(name && known && v.getAttribute("data-view") !== name);
    });
    scheduleHeight();
  }
  setView(new URLSearchParams(location.search).get("tab") || "");
  window.addEventListener("message", function (ev) {
    if (ev.source !== parent) return;
    var msg = ev.data;
    if (!msg || msg.type !== "skiff.menu" || msg.nonce !== nonce) return;
    setView(typeof msg.tab === "string" ? msg.tab : "");
  });

  function rpc(method, params) {
    return new Promise(function (resolve, reject) {
      if (!nonce) {
        reject(new Error("缺少 nonce：请从面板插件详情页打开"));
        return;
      }
      var id = String(++seq);
      pending[id] = { resolve: resolve, reject: reject };
      parent.postMessage({ type: "skiff.rpc", nonce: nonce, id: id, method: method, params: params || null }, "*");
    });
  }

  window.addEventListener("message", function (ev) {
    if (ev.source !== parent) return;
    var msg = ev.data;
    if (!msg || msg.type !== "skiff.rpc.result" || msg.nonce !== nonce) return;
    var p = pending[msg.id];
    if (!p) return;
    delete pending[msg.id];
    if (msg.error) p.reject(msg.error.message || msg.error);
    else p.resolve(msg.result);
  });

  function el(id) { return document.getElementById(id); }
  function esc(s) {
    return String(s == null ? "" : s).replace(/[&<>"']/g, function (c) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c];
    });
  }
  function when(ts) {
    if (!ts) return "-";
    var d = new Date(ts * 1000);
    return d.toLocaleString();
  }

  function render(st) {
    st = st || {};
    var state = st.state || {};
    var cfg = st.config || {};
    var events = state.events || [];
    var rows = state.containers || {};
    var objects = st.objects || [];
    if (el("hour-count")) el("hour-count").textContent = String(state.count || 0);
    if (el("objects")) el("objects").textContent = String(objects.length);
    if (el("dry-run")) el("dry-run").textContent = cfg.dry_run === "true" ? "开" : "关";

    // 视图按文件拆开加载：只有当前视图的节点存在，其余视图跳过。
    var names = Object.keys(rows);
    if (el("containers")) {
      if (names.length === 0) {
        el("containers").innerHTML = '<tr><td colspan="3" class="muted">暂无记录</td></tr>';
      } else {
        el("containers").innerHTML = names.map(function (n) {
          var r = rows[n] || {};
          return "<tr><td><code>" + esc(n) + "</code></td><td>" + when(r.last) +
            '</td><td>' + (r.total || 0) + "</td></tr>";
        }).join("");
      }
    }

    if (el("events") && events.length === 0) {
      el("events").innerHTML = '<tr><td colspan="4" class="muted">暂无事件</td></tr>';
    } else if (el("events")) {
      el("events").innerHTML = events.slice().reverse().map(function (e) {
        var cls = e.result === "restarted" ? "ok" : (e.result === "failed" ? "bad" : "warn");
        return "<tr><td>" + when(e.t) + "</td><td><code>" + esc(e.name) + "</code></td><td class=\"" +
          cls + "\">" + esc(e.result) + "</td><td class=\"muted\">" + esc(e.detail) + "</td></tr>";
      }).join("");
    }
  }

  function say(text, bad) {
    var m = el("msg");
    m.textContent = text;
    m.className = bad ? "bad" : "muted";
  }

  function refresh() {
    rpc("status").then(function (st) {
      render(st);
      say("");
    }).catch(function (e) {
      say("读取状态失败：" + e, true);
    });
  }

  el("refresh").addEventListener("click", refresh);

  // 巡检完全由内核调度器驱动：插件 UI 只读，不再提供「立即巡检」入口。
  refresh();
})();
