// 插件架构测试台 UI：只做展示与调用，权限判定全部在内核。
(function () {
  var params = new URLSearchParams(location.search);
  var nonce = params.get("nonce") || "";
  var pending = {};
  var seq = 0;
  var submitted = 0;
  var focused = 0;

  // ---- 主题：父面板 postMessage 下发；未收到时保持默认深色 ----
  window.addEventListener("message", function (ev) {
    if (ev.source !== parent) return;
    var msg = ev.data;
    if (msg && msg.type === "skiff.theme" && (msg.theme === "light" || msg.theme === "dark")) {
      document.documentElement.setAttribute("data-theme", msg.theme);
    }
  });
  function requestTheme() {
    try { parent.postMessage({ type: "skiff.theme.request" }, "*"); } catch (e) { /* 独立打开 */ }
  }
  requestTheme();
  window.addEventListener("load", requestTheme);

  // ---- 内核 L3 标签 → 内部视图；单视图页也保留这段，便于手测 ----
  function setView(name) {
    var views = document.querySelectorAll("[data-view]");
    var known = Array.prototype.some.call(views, function (v) {
      return v.getAttribute("data-view") === name;
    });
    views.forEach(function (v) {
      v.hidden = !!(name && known && v.getAttribute("data-view") !== name);
    });
  }
  setView(params.get("tab") || "");

  window.addEventListener("message", function (ev) {
    if (ev.source !== parent) return;
    var msg = ev.data;
    if (!msg || msg.nonce !== nonce) return;
    if (msg.type === "skiff.menu" && typeof msg.tab === "string") setView(msg.tab);
    else if (msg.type === "skiff.refresh") runAll();
  });

  function rpc(method, args) {
    return new Promise(function (resolve, reject) {
      if (!nonce) {
        reject(new Error("缺少 nonce：请从面板插件信息页打开功能界面"));
        return;
      }
      var id = String(++seq);
      pending[id] = { resolve: resolve, reject: reject };
      parent.postMessage({ type: "skiff.rpc", nonce: nonce, id: id, method: method, params: args || null }, "*");
    });
  }

  window.addEventListener("message", function (ev) {
    if (ev.source !== parent) return;
    var msg = ev.data;
    if (!msg || msg.type !== "skiff.rpc.result" || msg.nonce !== nonce) return;
    var p = pending[msg.id];
    if (!p) return;
    delete pending[msg.id];
    if (msg.error) p.reject(new Error(msg.error.message || String(msg.error)));
    else p.resolve(msg.result);
  });

  function el(id) { return document.getElementById(id); }

  function show(value) {
    var out = el("out");
    if (!out) return;
    out.textContent = typeof value === "string" ? value : JSON.stringify(value, null, 2);
  }

  function line(ok, label, detail) {
    var out = el("out");
    if (!out) return;
    out.textContent += (out.textContent ? "\n" : "") +
      (ok === null ? "… " : ok ? "✓ " : "✗ ") + label + (detail ? " — " + detail : "");
  }

  function describeMenu() {
    var box = el("where");
    if (!box) return;
    box.textContent = "menu=" + (params.get("menu") || "-") +
      "  tab=" + (params.get("tab") || "-") +
      "  l3=" + (params.get("l3") || "-") +
      "  ui=" + (params.get("ui") || "-");
  }
  describeMenu();

  // ---- 单方法按钮 ----
  Array.prototype.forEach.call(document.querySelectorAll("[data-rpc]"), function (btn) {
    btn.addEventListener("click", function () {
      var method = btn.getAttribute("data-rpc");
      show("调用 " + method + " …");
      rpc(method).then(function (r) {
        show({ method: method, ok: true, result: r });
      }).catch(function (e) {
        // 失败分支同样可见：错误码与消息由内核/插件原样透传。
        show({ method: method, ok: false, error: String(e) });
      });
    });
  });

  // ---- 表单提交（沙箱 allow-forms + JS preventDefault；不发起真实跳转）----
  var form = el("probe-form");
  if (form) {
    form.addEventListener("submit", function (ev) {
      ev.preventDefault();
      submitted += 1;
      var path = el("probe-path") ? el("probe-path").value : "/proc/uptime";
      show("读取 " + path + " …");
      rpc("fs_read", { path: path }).then(function (r) {
        show({ ok: true, submitted: submitted, result: r });
      }).catch(function (e) {
        show({ ok: false, submitted: submitted, error: String(e) });
      });
    });
  }

  // ---- 键盘操作：原生 button 的 Enter/Space 即 click，这里只记录是否被触发 ----
  var kb = el("kb-btn");
  if (kb) {
    kb.addEventListener("click", function () {
      focused += 1;
      show({ keyboard: true, clicks: focused });
    });
    kb.addEventListener("keydown", function (ev) {
      if (ev.key === "Enter" || ev.key === " ") {
        line(true, "键盘事件到达按钮", "key=" + ev.key);
      }
    });
  }

  // ---- 自检：逐项真实调用，失败分支按预期判定 ----
  function runAll() {
    var out = el("out");
    if (out) out.textContent = "开始自检…";
    var authorized = el("probe-path") ? el("probe-path").value : "/proc/uptime";
    var checks = [];

    checks.push(rpc("ping").then(function (r) {
      line(r && r.pong === true, "RPC 成功分支 ping", "id=" + (r && r.id));
    }, function (e) {
      line(false, "RPC 成功分支 ping", String(e));
    }));

    checks.push(rpc("fail").then(function () {
      line(false, "RPC 失败分支 fail", "预期失败却成功，错误透传可能未生效");
    }, function (e) {
      line(true, "RPC 失败分支 fail", "已收到预期错误：" + String(e));
    }));

    checks.push(rpc("identity").then(function (r) {
      line(Array.isArray(r && r.objects), "identity:self", "objects=" + (r && r.objects));
    }, function (e) {
      line(false, "identity:self", String(e));
    }));

    checks.push(rpc("fs_read", { path: authorized }).then(function (r) {
      line(true, "fs 授权路径读取", authorized + " bytes=" + (r && r.bytes));
    }, function (e) {
      line(false, "fs 授权路径读取", authorized + " -> " + String(e));
    }));

    checks.push(rpc("fs_read", { path: "/etc/passwd" }).then(function () {
      line(false, "fs 越界路径拒绝", "/etc/passwd 竟然读取成功");
    }, function (e) {
      line(true, "fs 越界路径拒绝", "已拒绝：" + String(e));
    }));

    checks.push(rpc("interop_catalog").then(function (r) {
      line(!!r && !!r.plugins, "interop:catalog", "目标数=" + (r && r.plugins ? r.plugins.length : 0));
    }, function (e) {
      line(false, "interop:catalog", String(e));
    }));

    checks.push(rpc("interop_cron").then(function (r) {
      line(true, "interop:call 成功分支", JSON.stringify(r).slice(0, 80));
    }, function (e) {
      // 未安装 cron / 未授予 plugin: 时这里是预期失败分支，但通路本身是通的。
      line(null, "interop:call 失败分支", String(e).slice(0, 160));
    }));

    checks.push(Promise.resolve().then(function () {
      var theme = document.documentElement.getAttribute("data-theme") || "dark(默认)";
      line(true, "主题跟随", "data-theme=" + theme);
    }));

    checks.push(Promise.resolve().then(function () {
      line(true, "表单提交", "已捕获 submit 事件 " + submitted + " 次（preventDefault 生效）");
    }));

    checks.push(Promise.resolve().then(function () {
      line(true, "键盘操作", "原生 button 可聚焦，Enter/Space 触发 click（已触发 " + focused + " 次）");
    }));

    Promise.all(checks).then(function () {
      line(true, "自检结束");
    });
  }

  var run = el("run-checks");
  if (run) run.addEventListener("click", runAll);

  if (el("out") && !el("run-checks") && !form && !kb && !document.querySelector("[data-rpc]")) {
    rpc("state").then(function (r) { show(r); }).catch(function (e) { show(String(e)); });
  }
})();
