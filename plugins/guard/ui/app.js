// 插件 UI 的 RPC 桥客户端：所有能力调用都经父页面转发（iframe 自身没有凭据）。
// 协议见 crates/web/static/skiff.js::attachPluginBridges。
(function () {
  var nonce = new URLSearchParams(location.search).get("nonce") || "";
  var pending = {};
  var seq = 0;

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
    el("hour-count").textContent = String(state.count || 0);
    el("objects").textContent = String(objects.length);
    el("dry-run").textContent = cfg.dry_run === "true" ? "开" : "关";

    var names = Object.keys(rows);
    if (names.length === 0) {
      el("containers").innerHTML = '<tr><td colspan="3" class="muted">暂无记录</td></tr>';
    } else {
      el("containers").innerHTML = names.map(function (n) {
        var r = rows[n] || {};
        return "<tr><td><code>" + esc(n) + "</code></td><td>" + when(r.last) +
          '</td><td>' + (r.total || 0) + "</td></tr>";
      }).join("");
    }

    if (events.length === 0) {
      el("events").innerHTML = '<tr><td colspan="4" class="muted">暂无事件</td></tr>';
    } else {
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
  el("tick").addEventListener("click", function () {
    say("巡检中…");
    rpc("tick", {}).then(function (r) {
      var parts = ["检查 " + (r.checked || 0) + " 个异常容器"];
      if (r.restarted && r.restarted.length) parts.push("已重启：" + r.restarted.join(", "));
      if (r.unauthorized && r.unauthorized.length) parts.push("未授权跳过：" + r.unauthorized.join(", "));
      if (r.dry_run) parts.push("（演练模式）");
      say(parts.join("；"));
      return refresh();
    }).catch(function (e) {
      say("巡检失败：" + e, true);
    });
  });

  refresh();
})();
