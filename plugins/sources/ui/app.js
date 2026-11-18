// 软件源页：RPC 桥 + 两个源表格 + 测速/应用。
// 协议见 crates/web/static/skiff.js：只经 postMessage，不持凭据、不发同源请求。
(function () {
  "use strict";

  var nonce = new URLSearchParams(location.search).get("nonce") || "";
  var pending = {};
  var seq = 0;

  var state = {
    apk: [],
    registry: [],
    results: {},   // base -> 最近一次测速结果（内存里，刷新即失，符合「只代表此刻」）
    busy: false
  };

  // ---- 主题 ----
  window.addEventListener("message", function (ev) {
    if (ev.source !== parent) return;
    var m = ev.data;
    if (m && m.type === "skiff.theme" && (m.theme === "light" || m.theme === "dark")) {
      document.documentElement.setAttribute("data-theme", m.theme);
    }
  });
  function requestTheme() {
    try { parent.postMessage({ type: "skiff.theme.request" }, "*"); } catch (e) { /* 独立打开 */ }
  }
  requestTheme();
  window.addEventListener("load", requestTheme);

  // ---- 高度上报：父面板据此调 iframe 高度，避免内外双滚动 ----
  var heightTimer = 0;
  function reportHeight() {
    if (!nonce) return;
    var b = document.body;
    var h = b ? Math.max(b.scrollHeight, b.offsetHeight) : 0;
    if (h < 1) return;
    try { parent.postMessage({ type: "skiff.height", nonce: nonce, height: Math.ceil(h) }, "*"); } catch (e) { /* 独立打开 */ }
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

  // ---- RPC ----
  function rpc(method, params) {
    return new Promise(function (resolve, reject) {
      if (!nonce) {
        reject(new Error("缺少 nonce：请从面板插件详情页打开"));
        return;
      }
      var id = String(++seq);
      pending[id] = { resolve: resolve, reject: reject };
      parent.postMessage(
        { type: "skiff.rpc", nonce: nonce, id: id, method: method, params: params || null },
        "*"
      );
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
  function say(text, cls) {
    var m = el("msg");
    m.className = cls || "muted";
    m.textContent = text || "";
    scheduleHeight();
  }

  // ---- 渲染 ----
  function fmt(r) {
    if (!r) return '<span class="muted">未测</span>';
    if (!r.ok) {
      return '<span class="bad">不可用</span><div class="addr">' + esc(r.error || "") + "</div>";
    }
    var kb = r.bytes_per_sec / 1024;
    var main = kb >= 1024 ? (kb / 1024).toFixed(1) + " MB/s" : kb.toFixed(0) + " KB/s";
    // 采样字节数与耗时必须一起显示：否则「3 MB / 4 ms」这种缓存命中会被当成带宽。
    var detail = (r.bytes / 1048576).toFixed(2) + " MB / " +
                 (r.elapsed_ms / 1000).toFixed(1) + " s";
    return '<span class="ok">' + main + '</span><div class="addr">' + detail + "</div>";
  }

  function rowHtml(kind, item) {
    var base = item.base;
    var action = kind === "apk"
      ? '<button data-act="apply" data-base="' + esc(base) + '">使用</button>'
      : '<button data-act="pick" data-base="' + esc(base) + '">加入</button>';
    var active = item.current
      ? '<span class="ok">使用中</span> '
      : "";
    return "<tr>" +
      "<td>" + esc(item.name) + "</td>" +
      '<td class="addr">' + esc(base) + "</td>" +
      '<td class="num" data-cell="' + esc(base) + '">' + fmt(state.results[base]) + "</td>" +
      '<td>' + active + '<button data-act="probe" data-base="' + esc(base) + '">测速</button> ' + action + "</td>" +
      "</tr>";
  }

  function isCurrent(kind, base) {
    var cur = kind === "apk" ? state.apkCurrent : state.registryCurrent;
    if (!cur) return false;
    if (kind === "apk") {
      // apk_current 是被写进 /etc/apk/repositories 的整行（含 /vX.Y/main），按前缀匹配。
      return cur.indexOf(base.replace(/\/$/, "")) === 0;
    }
    return cur.some(function (m) { return m.replace(/\/$/, "") === base.replace(/\/$/, ""); });
  }

  function render(kind) {
    var items = state[kind];
    var rows = items.map(function (it) {
      return rowHtml(kind, { name: it.name, base: it.base, current: isCurrent(kind, it.base) });
    });
    el(kind + "-rows").innerHTML = rows.join("") ||
      '<tr><td colspan="4" class="muted">无候选</td></tr>';
    scheduleHeight();
  }

  function renderCurrent() {
    el("apk-current").innerHTML = state.apkCurrent
      ? "当前：" + esc(state.apkCurrent) + (state.apkFixed
          ? '（<span class="ok">已固定</span>，不再被自动探测覆盖）'
          : "（安装器缓存，24 小时后会重新探测）")
      : '当前：<span class="muted">跟随镜像默认源</span>';
    el("reg-current").innerHTML = state.registryCurrent
      ? "当前：" + esc(state.registryCurrent.join("，") || "未配置（直连 Docker Hub）")
      : '<span class="warn">读不到 /etc/docker/daemon.json</span>，请先在「设置 → Docker」保存一次再回来';
    if (state.registryCurrent) {
      el("reg-apply").value = state.registryCurrent.join(",");
    }
    scheduleHeight();
  }

  function updateCell(base) {
    var cells = document.querySelectorAll('[data-cell="' + base.replace(/"/g, '\\"') + '"]');
    for (var i = 0; i < cells.length; i++) cells[i].innerHTML = fmt(state.results[base]);
    scheduleHeight();
  }

  // ---- 测速 ----
  function markProbing(kind) {
    state[kind].forEach(function (it) {
      var cells = document.querySelectorAll('[data-cell="' + it.base.replace(/"/g, '\\"') + '"]');
      for (var i = 0; i < cells.length; i++) cells[i].innerHTML = '<span class="muted">测速中…</span>';
    });
    scheduleHeight();
  }

  function probeAll(kind) {
    if (state.busy) return;
    state.busy = true;
    markProbing(kind);
    say("正在测速（每个源最多 8 秒，并行进行）…");
    var bases = state[kind].map(function (it) { return it.base; });
    rpc("probe.all", { kind: kind, bases: bases }).then(function (list) {
      (list || []).forEach(function (r) { state.results[r.base] = r; });
      render(kind);
      var okCount = (list || []).filter(function (r) { return r.ok; }).length;
      say("测速完成：" + okCount + "/" + (list || []).length + " 个源可用", "muted");
    }).catch(function (e) {
      say("测速失败：" + (e.message || e), "bad");
      render(kind);
    }).then(function () {
      state.busy = false;
    });
  }

  function probeOne(kind, base) {
    var cells = document.querySelectorAll('[data-cell="' + base.replace(/"/g, '\\"') + '"]');
    for (var i = 0; i < cells.length; i++) cells[i].innerHTML = '<span class="muted">测速中…</span>';
    rpc("probe", { kind: kind, base: base }).then(function (r) {
      state.results[r.base] = r;
      updateCell(base);
    }).catch(function (e) {
      say("测速失败：" + (e.message || e), "bad");
      updateCell(base);
    });
  }

  function addCustom(kind) {
    var input = el(kind + "-custom");
    var base = (input.value || "").trim().replace(/\/$/, "");
    if (!base) return;
    if (!/^https?:\/\//.test(base)) {
      say("地址必须以 http:// 或 https:// 开头", "bad");
      return;
    }
    if (!state[kind].some(function (it) { return it.base === base; })) {
      state[kind].push({ id: "custom", name: "自定义", base: base });
      input.value = "";
      render(kind);
    }
    probeOne(kind, base);
  }

  // ---- 应用 ----
  function applyApk(base) {
    say("正在写入安装器源缓存…");
    rpc("apply.apk", { base: base }).then(function (r) {
      var lines = String((r && r.repositories) || "").split("\n");
      state.apkCurrent = lines[1] || base;
      state.apkFixed = true;
      render("apk");
      renderCurrent();
      say((r && r.note) || "已写入", "ok");
    }).catch(function (e) {
      say("写入失败：" + (e.message || e), "bad");
    });
  }

  function applyRegistry() {
    var raw = (el("reg-apply").value || "").trim();
    if (!raw) {
      say("请先填写要应用的镜像源（可点行内「加入」）", "bad");
      return;
    }
    say("正在写入 daemon.json 并热重载 Docker…");
    rpc("apply.registry", { mirrors: raw }).then(function (r) {
      state.registryCurrent = raw.split(",").map(function (s) { return s.trim(); })
        .filter(function (s) { return s; });
      render("registry");
      renderCurrent();
      say((r && r.note) || "已生效", "ok");
    }).catch(function (e) {
      say("应用失败：" + (e.message || e), "bad");
    });
  }

  // ---- 事件 ----
  document.addEventListener("click", function (ev) {
    var btn = ev.target.closest ? ev.target.closest("button[data-act]") : null;
    if (!btn) return;
    var act = btn.getAttribute("data-act");
    var base = btn.getAttribute("data-base");
    var kind = btn.closest("section").querySelector("tbody").id === "apk-rows" ? "apk" : "registry";
    if (act === "probe") probeOne(kind, base);
    else if (act === "apply") applyApk(base);
    else if (act === "pick") {
      var box = el("reg-apply");
      var cur = box.value.split(",").map(function (s) { return s.trim(); }).filter(function (s) { return s; });
      if (cur.indexOf(base) < 0) cur.push(base);
      box.value = cur.join(",");
      say("已加入待应用列表，点「应用」后生效");
    }
  });

  el("apk-probe").addEventListener("click", function () { probeAll("apk"); });
  el("reg-probe").addEventListener("click", function () { probeAll("registry"); });
  el("apk-add").addEventListener("click", function () { addCustom("apk"); });
  el("reg-add").addEventListener("click", function () { addCustom("registry"); });
  el("reg-save").addEventListener("click", applyRegistry);

  // ---- 初始化 ----
  rpc("status").then(function (s) {
    state.apk = (s.apk || []).map(function (x) { return { id: x.id, name: x.name, base: x.base }; });
    state.registry = (s.registry || []).map(function (x) { return { id: x.id, name: x.name, base: x.base }; });
    state.apkCurrent = s.apk_current || "";
    state.apkFixed = !!s.apk_fixed;
    state.registryCurrent = s.registry_current || null;
    render("apk");
    render("registry");
    renderCurrent();
    say("");
  }).catch(function (e) {
    say("读取状态失败：" + (e.message || e), "bad");
  });
})();
