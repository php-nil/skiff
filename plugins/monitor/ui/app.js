// 主机监控 UI：只展示，不写任何配置；数据经父页面 RPC 桥取回。
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

  // ---- 内核 L3 标签 → 内部视图；`skiff.refresh` 供概览槽位定时刷新 ----
  function setView(name) {
    var views = document.querySelectorAll("[data-view]");
    var known = Array.prototype.some.call(views, function (v) {
      return v.getAttribute("data-view") === name;
    });
    views.forEach(function (v) {
      v.hidden = !!(name && known && v.getAttribute("data-view") !== name);
    });
  }
  setView(new URLSearchParams(location.search).get("tab") || "");

  window.addEventListener("message", function (ev) {
    if (ev.source !== parent) return;
    var msg = ev.data;
    if (!msg || msg.nonce !== nonce) return;
    if (msg.type === "skiff.menu" && typeof msg.tab === "string") setView(msg.tab);
    else if (msg.type === "skiff.refresh") refresh();
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
  function pct(v) { return v == null ? "-" : (Number(v).toFixed(1) + "%"); }
  function gb(kb) {
    if (!kb) return "-";
    var v = kb / 1048576;
    return v >= 1 ? v.toFixed(2) + " GiB" : (kb / 1024).toFixed(0) + " MiB";
  }
  function rate(v) {
    if (v == null) return "-";
    var n = Number(v);
    if (n < 1024) return n.toFixed(0) + " B/s";
    if (n < 1048576) return (n / 1024).toFixed(1) + " KiB/s";
    return (n / 1048576).toFixed(2) + " MiB/s";
  }
  function dur(sec) {
    if (!sec) return "-";
    var d = Math.floor(sec / 86400), h = Math.floor((sec % 86400) / 3600), m = Math.floor((sec % 3600) / 60);
    return (d ? d + " 天 " : "") + h + " 时 " + m + " 分";
  }
  function when(ts) { return ts ? new Date(ts * 1000).toLocaleString() : "-"; }

  function card(id, value, bad) {
    var c = el(id);
    c.querySelector("b").textContent = value;
    c.classList.toggle("bad", !!bad);
  }

  function paint(s, cfg) {
    if (!s) return;
    var cpuWarn = Number(cfg.alert_cpu || 90);
    var memWarn = Number(cfg.alert_mem || 90);
    var memPct = s.mem_total_kb ? (s.mem_used_kb / s.mem_total_kb * 100) : null;
    card("c-cpu", pct(s.cpu), s.cpu != null && s.cpu >= cpuWarn);
    card("c-mem", gb(s.mem_used_kb) + " / " + gb(s.mem_total_kb) + " (" + pct(memPct) + ")",
         memPct != null && memPct >= memWarn);
    card("c-load", [s.load1, s.load5, s.load15].map(function (v) {
      return v == null ? "-" : Number(v).toFixed(2);
    }).join(" / "));
    card("c-net", rate(s.net_rx_rate));
    card("c-uptime", dur(s.uptime));
    card("c-cpus", String(s.cpus || "-"));
  }

  function paintHistory(samples) {
    if (!samples || samples.length === 0) {
      el("history").innerHTML = '<tr><td colspan="6" class="muted">暂无历史采样（cron 每 5 分钟生成一次）</td></tr>';
      return;
    }
    el("history").innerHTML = samples.slice().reverse().slice(0, 30).map(function (s) {
      var memPct = s.mem_total_kb ? (s.mem_used_kb / s.mem_total_kb * 100) : null;
      return "<tr><td>" + when(s.t) + "</td><td>" + pct(s.cpu) + "</td><td>" + pct(memPct) +
        "</td><td>" + (s.load1 == null ? "-" : Number(s.load1).toFixed(2)) +
        "</td><td>" + rate(s.net_rx_rate) + "</td><td>" +
        (s.net_tx ? gb(s.net_tx * 1024) : "-") + "</td></tr>";
    }).join("");
  }

  function refresh() {
    el("msg").textContent = "";
    return rpc("snapshot").then(function (r) {
      rpc("config").then(function (cfg) { paint(r.sample, cfg || {}); }).catch(function () { paint(r.sample, {}); });
      return rpc("history").then(function (h) { paintHistory(h.samples); });
    }).catch(function (e) {
      el("msg").textContent = String(e);
    });
  }

  el("refresh").addEventListener("click", refresh);
  refresh();
})();
