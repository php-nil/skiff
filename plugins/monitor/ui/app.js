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

  // ---- 内核 L3 标签 → 内部视图；`skiff.refresh` 供概览槽位定时刷新 ----
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

  // ---- 进程 / 网络视图共用的小工具 ----
  function memKb(kb) {
    if (kb == null) return "-";
    var n = Number(kb);
    if (n >= 1048576) return (n / 1048576).toFixed(2) + " GiB";
    return (n / 1024).toFixed(1) + " MiB";
  }
  function cpuText(v) { return v == null ? "—" : Number(v).toFixed(1) + "%"; }
  function stateText(s) {
    var map = {
      running: "运行", sleeping: "休眠", "disk-sleep": "不可中断", zombie: "僵尸",
      stopped: "停止", tracing: "跟踪", idle: "空闲", dead: "已退出", unknown: "未知"
    };
    return map[s] || s || "-";
  }
  function debounce(fn, ms) {
    var timer = 0;
    return function () {
      var ctx = this, args = arguments;
      if (timer) clearTimeout(timer);
      timer = setTimeout(function () { fn.apply(ctx, args); }, ms);
    };
  }

  function card(id, value, bad) {
    var c = el(id);
    // 视图按文件拆开加载：总览文件没有历史表格，历史文件没有指标卡。
    if (!c) return;
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
    card("c-net-tx", rate(s.net_tx_rate));
    card("c-uptime", dur(s.uptime));
    card("c-cpus", String(s.cpus || "-"));
  }

  function paintHistory(samples) {
    if (!el("history")) return;
    if (!samples || samples.length === 0) {
      el("history").innerHTML = '<tr><td colspan="6" class="muted">暂无历史采样（内核调度器每 5 分钟生成一次）</td></tr>';
      return;
    }
    el("history").innerHTML = samples.slice().reverse().slice(0, 30).map(function (s) {
      var memPct = s.mem_total_kb ? (s.mem_used_kb / s.mem_total_kb * 100) : null;
      return "<tr><td>" + when(s.t) + "</td><td>" + pct(s.cpu) + "</td><td>" + pct(memPct) +
        "</td><td>" + (s.load1 == null ? "-" : Number(s.load1).toFixed(2)) +
        "</td><td>" + rate(s.net_rx_rate) + "</td><td>" +
        rate(s.net_tx_rate) + "</td></tr>";
    }).join("");
  }

  // ---- 进程视图：只读 /proc，服务端完成排序/筛选/分页 ----
  var proc = { page: 1, pageSize: 50, sort: "cpu", order: "desc", q: "", user: "" };

  function loadProcesses() {
    el("msg").textContent = "";
    return rpc("processes", {
      page: proc.page,
      page_size: proc.pageSize,
      sort: proc.sort,
      order: proc.order,
      q: proc.q,
      user: proc.user
    }).then(renderProcesses).catch(function (e) {
      el("msg").textContent = String(e);
    });
  }

  function updateSortMarks() {
    Array.prototype.forEach.call(document.querySelectorAll("th[data-sort]"), function (th) {
      var on = th.getAttribute("data-sort") === proc.sort;
      th.classList.toggle("sorted", on);
      var mark = th.querySelector(".arrow");
      if (on) {
        if (!mark) {
          mark = document.createElement("span");
          mark.className = "arrow";
          th.appendChild(mark);
        }
        mark.textContent = proc.order === "asc" ? "▲" : "▼";
      } else if (mark) {
        mark.remove();
      }
    });
  }

  function renderProcesses(r) {
    var tb = el("process-table");
    if (!tb) return;
    r = r || {};
    var items = r.items || [];
    if (!items.length) {
      tb.innerHTML = '<tr><td colspan="7" class="muted">没有匹配的进程</td></tr>';
    } else {
      tb.innerHTML = items.map(function (p) {
        return "<tr>" +
          '<td class="num">' + esc(p.pid) + "</td>" +
          "<td>" + esc(p.user) + "</td>" +
          '<td class="num">' + cpuText(p.cpu) + "</td>" +
          '<td class="num">' + memKb(p.mem_kb) + "</td>" +
          "<td>" + when(p.started) + "</td>" +
          "<td>" + esc(stateText(p.state)) + "</td>" +
          '<td class="cmd">' + esc(p.command) + "</td></tr>";
      }).join("");
    }
    var total = r.total || 0;
    var pages = Math.max(1, Math.ceil(total / proc.pageSize));
    el("p-info").textContent = "第 " + (r.page || proc.page) + " / " + pages + " 页 · 共 " +
      total + " 个进程" + (r.scanned ? "（扫描 " + r.scanned + " 个 PID）" : "") +
      (r.truncated ? " · 已达扫描上限，结果可能不完整" : "");
    el("p-note").textContent = r.cpu_baseline === false ? (r.note || "") : "";
    el("p-prev").disabled = (r.page || proc.page) <= 1;
    el("p-next").disabled = (r.page || proc.page) >= pages;
    updateSortMarks();
    scheduleHeight();
  }

  function initProcessView() {
    if (!el("process-table")) return;
    var q = el("p-q"), user = el("p-user"), size = el("p-size");
    q.addEventListener("input", debounce(function () {
      proc.q = q.value.trim(); proc.page = 1; loadProcesses();
    }, 300));
    user.addEventListener("input", debounce(function () {
      proc.user = user.value.trim(); proc.page = 1; loadProcesses();
    }, 300));
    size.addEventListener("change", function () {
      proc.pageSize = Number(size.value) || 50; proc.page = 1; loadProcesses();
    });
    el("p-prev").addEventListener("click", function () {
      if (proc.page > 1) { proc.page--; loadProcesses(); }
    });
    el("p-next").addEventListener("click", function () { proc.page++; loadProcesses(); });
    Array.prototype.forEach.call(document.querySelectorAll("th[data-sort]"), function (th) {
      th.addEventListener("click", function () {
        var key = th.getAttribute("data-sort");
        if (proc.sort === key) {
          proc.order = proc.order === "asc" ? "desc" : "asc";
        } else {
          proc.sort = key;
          proc.order = key === "pid" ? "asc" : "desc";
        }
        proc.page = 1;
        loadProcesses();
      });
    });
  }

  // ---- 网络视图：只读 /proc/net/*，服务端完成筛选与分页 ----
  var net = { page: 1, pageSize: 50, protocol: "all", state: "all", q: "" };

  function loadNetwork() {
    el("msg").textContent = "";
    return rpc("network", {
      page: net.page,
      page_size: net.pageSize,
      protocol: net.protocol,
      state: net.state,
      q: net.q
    }).then(renderNetwork).catch(function (e) {
      el("msg").textContent = String(e);
    });
  }

  function renderNetwork(r) {
    var tb = el("network-table");
    if (!tb) return;
    r = r || {};
    var items = r.items || [];
    if (!items.length) {
      tb.innerHTML = '<tr><td colspan="6" class="muted">没有匹配的连接</td></tr>';
    } else {
      tb.innerHTML = items.map(function (c) {
        return "<tr" + (c.listening ? ' class="listen"' : "") + ">" +
          "<td>" + esc(String(c.protocol || "").toUpperCase()) + (c.v6 ? "6" : "") + "</td>" +
          '<td class="mono">' + esc(c.local) + "</td>" +
          '<td class="mono">' + esc(c.remote) + "</td>" +
          '<td class="st">' + esc(c.state) + "</td>" +
          '<td class="num">' + esc(c.uid) + "</td>" +
          '<td class="num mono">' + esc(c.inode) + "</td></tr>";
      }).join("");
    }
    var total = r.total || 0;
    var pages = Math.max(1, Math.ceil(total / net.pageSize));
    el("n-info").textContent = "第 " + (r.page || net.page) + " / " + pages + " 页 · 共 " + total + " 条连接";
    el("n-note").textContent = r.read_error
      ? ("部分 /proc/net 文件读取失败：" + r.read_error)
      : (r.owner_note || "");
    el("n-prev").disabled = (r.page || net.page) <= 1;
    el("n-next").disabled = (r.page || net.page) >= pages;
    scheduleHeight();
  }

  function initNetworkView() {
    if (!el("network-table")) return;
    var proto = el("n-proto"), state = el("n-state"), q = el("n-q"), size = el("n-size");
    proto.addEventListener("change", function () {
      net.protocol = proto.value; net.page = 1; loadNetwork();
    });
    state.addEventListener("change", function () {
      net.state = state.value; net.page = 1; loadNetwork();
    });
    q.addEventListener("input", debounce(function () {
      net.q = q.value.trim(); net.page = 1; loadNetwork();
    }, 300));
    size.addEventListener("change", function () {
      net.pageSize = Number(size.value) || 50; net.page = 1; loadNetwork();
    });
    el("n-prev").addEventListener("click", function () {
      if (net.page > 1) { net.page--; loadNetwork(); }
    });
    el("n-next").addEventListener("click", function () { net.page++; loadNetwork(); });
  }

  function refresh() {
    el("msg").textContent = "";
    if (el("process-table")) return loadProcesses();
    if (el("network-table")) return loadNetwork();
    return rpc("snapshot").then(function (r) {
      rpc("config").then(function (cfg) { paint(r.sample, cfg || {}); }).catch(function () { paint(r.sample, {}); });
      return rpc("history").then(function (h) { paintHistory(h.samples); });
    }).catch(function (e) {
      el("msg").textContent = String(e);
    });
  }

  initProcessView();
  initNetworkView();
  el("refresh").addEventListener("click", refresh);
  refresh();
})();
