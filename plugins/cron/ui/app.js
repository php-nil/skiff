// 插件 UI 的 RPC 桥客户端 + 跨插件跳转（协议见 crates/web/static/skiff.js）。
(function () {
  "use strict";

  var nonce = new URLSearchParams(location.search).get("nonce") || "";
  // `?open=job:<id>`：其他插件（如 NGINX「新增备份任务」）创建任务后直接定位过来。
  var open = new URLSearchParams(location.search).get("open") || "";
  var focusId = open.indexOf("job:") === 0 ? open.slice(4) : "";

  var pending = {};
  var seq = 0;
  var targets = [];
  var jobs = [];

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

  function openPlugin(plugin, path) {
    return new Promise(function (resolve, reject) {
      if (!nonce) {
        reject(new Error("缺少 nonce：请从面板插件详情页打开"));
        return;
      }
      var id = "open-" + (++seq);
      pending[id] = { resolve: resolve, reject: reject };
      parent.postMessage(
        { type: "skiff.open", nonce: nonce, id: id, plugin: plugin, path: path || "" },
        "*"
      );
    });
  }

  window.addEventListener("message", function (ev) {
    if (ev.source !== parent) return;
    var msg = ev.data;
    if (!msg || msg.type !== "skiff.open.result" || msg.nonce !== nonce) return;
    var p = pending[msg.id];
    if (!p) return;
    delete pending[msg.id];
    if (msg.ok) p.resolve(true);
    else p.reject(new Error(msg.error || "跳转被拒绝"));
  });

  function el(id) { return document.getElementById(id); }
  function esc(s) {
    return String(s == null ? "" : s).replace(/[&<>"']/g, function (c) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c];
    });
  }
  function when(ts) {
    if (!ts) return "-";
    return new Date(Number(ts) * 1000).toLocaleString();
  }
  function say(text, bad) {
    var m = el("msg");
    m.textContent = text;
    m.className = bad ? "bad" : "muted";
  }
  function isPluginId(s) {
    return /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/.test(s);
  }
  // 内核内置任务（builtin: 前缀）由内核维护，插件侧不提供删除入口。
  function isBuiltinTask(id) {
    return String(id || "").indexOf("builtin:") === 0;
  }

  function renderTargets() {
    var sel = el("new-target");
    // 视图按文件拆开加载：列表页没有「新建任务」表单，跳过目标渲染。
    if (!sel) return;
    sel.innerHTML = "";
    if (targets.length === 0) {
      sel.innerHTML = '<option value="">（无可调用目标）</option>';
      return;
    }
    targets.forEach(function (t) {
      var o = document.createElement("option");
      o.value = t.id;
      o.textContent = (t.name || t.id) + "（" + t.id + "）";
      sel.appendChild(o);
    });
    var first = targets[0];
    if (first && first.methods && first.methods.length) {
      el("new-method").value = first.methods[0].method;
    }
  }

  function pickTarget() {
    if (!el("new-target")) return;
    var id = el("new-target").value;
    var t = targets.filter(function (x) { return x.id === id; })[0];
    if (t && t.methods && t.methods.length) {
      el("new-method").value = t.methods[0].method;
      var desc = t.methods[0].description || t.methods[0].label || "";
      el("targets-note").textContent = desc;
    }
  }

  function render() {
    var body = el("jobs");
    if (!body) return;
    if (jobs.length === 0) {
      body.innerHTML = '<tr><td colspan="6" class="muted">还没有任务。可在此新建，或从其他插件的界面一键添加。</td></tr>';
      return;
    }
    body.innerHTML = jobs.map(function (j) {
      var next = j.next_run && j.next_run.text ? j.next_run.text : "（表达式无效）";
      var src = j.source || "ui";
      var srcText = src === "ui" ? "本面板" : src;
      var jump = src !== "ui" && isPluginId(src)
        ? ' <button class="mini" data-jump="' + esc(src) + '">打开</button>'
        : "";
      var last = j.last;
      var lastText = "从未执行";
      var cls = "muted";
      if (last && last.at) {
        lastText = when(last.at) + " · " + (last.result === "ok" ? "成功" : "失败");
        cls = last.result === "ok" ? "ok" : "bad";
        if (last.detail) lastText += "：" + esc(last.detail);
      }
      var toggle = j.enabled
        ? '<button class="mini" data-toggle="' + esc(j.id) + '" data-enabled="0">停用</button>'
        : '<button class="mini" data-toggle="' + esc(j.id) + '" data-enabled="1">启用</button>';
      // 内置任务不可删除：不渲染删除按钮，避免误导用户点击。
      var remove = isBuiltinTask(j.id)
        ? '<span class="muted">内置任务</span>'
        : '<button class="mini danger" data-remove="' + esc(j.id) + '">删除</button>';
      // 内核状态优先于「上次结果」：blocked（插件不可用）不是失败。
      var stateText = "";
      var stateCls = "muted";
      if (j.state === "blocked" || j.lifecycle === "blocked") {
        stateText = "已阻塞" + (j.blocked_reason ? "：" + esc(j.blocked_reason) : "");
        stateCls = "warn";
      } else if (j.state === "running") {
        stateText = "执行中";
        stateCls = "warn";
      }
      return '<tr data-id="' + esc(j.id) + '"' + (j.id === focusId ? ' class="hit"' : "") + ">" +
        "<td><strong>" + esc(j.name) + "</strong>" +
          '<div class="muted"><code>' + esc(j.id) + "</code>" +
          (j.key ? " · key " + esc(j.key) : "") +
          (stateText ? ' · <span class="' + stateCls + '">' + stateText + "</span>" : "") +
          "</div></td>" +
        "<td><code>" + esc(j.expr) + "</code><div class=\"muted\">下次 " + esc(next) + "</div></td>" +
        "<td><code>" + esc(j.target) + "</code><div class=\"muted\">" + esc(j.method) + "</div></td>" +
        "<td>" + esc(srcText) + jump + "</td>" +
        '<td class="' + cls + '">' + lastText + "</td>" +
        '<td><button class="mini" data-run="' + esc(j.id) + '">立即执行</button> ' +
          toggle + ' ' + remove + "</td>" +
        "</tr>";
    }).join("");

    if (focusId) {
      var hit = body.querySelector('tr[data-id="' + focusId.replace(/"/g, "") + '"]');
      if (hit && hit.scrollIntoView) hit.scrollIntoView({ block: "center" });
    }
  }

  function load() {
    return rpc("list").then(function (r) {
      jobs = (r && r.jobs) || [];
      render();
    }).catch(function (e) {
      say("读取任务失败：" + e, true);
    });
  }

  function loadTargets() {
    return rpc("targets").then(function (r) {
      targets = (r && r.interfaces) || [];
      renderTargets();
    }).catch(function (e) {
      targets = [];
      renderTargets();
      say("读取可调用目标失败：" + e, true);
    });
  }

  function refreshAll() {
    var tasks = [load()];
    if (el("new-target")) tasks.push(loadTargets());
    return Promise.all(tasks);
  }

  el("refresh").addEventListener("click", function () {
    say("刷新中…");
    refreshAll().then(function () { say("已刷新"); });
  });

  if (el("new-target")) el("new-target").addEventListener("change", pickTarget);

  if (el("create")) el("create").addEventListener("click", function () {
    var name = el("new-name").value.trim();
    var expr = el("new-expr").value.trim() || "0 3 * * *";
    var target = el("new-target").value;
    var method = el("new-method").value.trim();
    var key = el("new-key").value.trim();
    var rawParams = el("new-params").value.trim();
    if (!target || !method) {
      say("请选择目标插件并填写方法名", true);
      return;
    }
    var params = {};
    if (rawParams) {
      try {
        params = JSON.parse(rawParams);
      } catch (e) {
        say("参数不是合法 JSON：" + e, true);
        return;
      }
      if (params === null || typeof params !== "object" || Array.isArray(params)) {
        say("参数必须是 JSON 对象", true);
        return;
      }
    }
    say("保存中…");
    rpc("create", {
      name: name, expr: expr, target: target, method: method, key: key, params: params, enabled: true
    }).then(function (r) {
      var next = r && r.next_run && r.next_run.text ? "，下次 " + r.next_run.text : "";
      say((r && r.created ? "已新增任务 " : "已更新任务 ") + (r.id || "") + next);
      return load();
    }).catch(function (e) {
      say("保存失败：" + e, true);
    });
  });

  if (el("jobs")) el("jobs").addEventListener("click", function (ev) {
    var btn = ev.target.closest ? ev.target.closest("button") : null;
    if (!btn) return;
    var runId = btn.getAttribute("data-run");
    var toggleId = btn.getAttribute("data-toggle");
    var removeId = btn.getAttribute("data-remove");
    var jump = btn.getAttribute("data-jump");

    if (jump) {
      say("正在切到 " + jump + "…");
      openPlugin(jump).catch(function (e) {
        say("跳转失败：" + (e && e.message ? e.message : e), true);
      });
      return;
    }
    if (runId) {
      say("执行中…");
      rpc("run", { id: runId }).then(function () {
        say("任务已执行");
        return load();
      }).catch(function (e) {
        say("执行失败：" + e, true);
        return load();
      });
      return;
    }
    if (toggleId) {
      var enabled = btn.getAttribute("data-enabled") === "1";
      rpc("toggle", { id: toggleId, enabled: enabled }).then(function () {
        say(enabled ? "任务已启用" : "任务已停用");
        return load();
      }).catch(function (e) {
        say("操作失败：" + e, true);
      });
      return;
    }
    if (removeId) {
      if (isBuiltinTask(removeId)) {
        say("内置任务不可删除", true);
        return;
      }
      if (!window.confirm("删除任务 " + removeId + "？")) return;
      rpc("remove", { id: removeId }).then(function () {
        say("任务已删除");
        return load();
      }).catch(function (e) {
        say("删除失败：" + e, true);
      });
    }
  });

  rpc("config").then(function (c) {
    var expr = el("new-expr");
    if (c && c.default_expr && expr && !expr.value) {
      expr.value = c.default_expr;
    }
  }).catch(function () {});
  refreshAll();
})();
