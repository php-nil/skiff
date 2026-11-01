// 插件 UI 的 RPC 桥客户端：所有能力调用都经父页面转发（iframe 自身没有凭据）。
// 协议见 crates/web/static/skiff.js::attachPluginBridges。
(function () {
  var nonce = new URLSearchParams(location.search).get("nonce") || "";
  var pending = {};
  var seq = 0;
  var current = "";
  var backupExpr = "0 3 * * *";
  var lastJobId = "";

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

  // ---- 跨插件跳转（面板级消息 skiff.open）----
  // 父页面按白名单校验目标插件，只改浏览器地址；定位路径能否读，由目标插件
  // 自己的 `path:` 授权根判定（跳转本身不授予任何能力）。
  function openPlugin(plugin, path) {
    return new Promise(function (resolve, reject) {
      if (!nonce) {
        reject(new Error("缺少 nonce：请从面板插件详情页打开"));
        return;
      }
      var id = "open-" + (++seq);
      pending[id] = { resolve: resolve, reject: reject };
      parent.postMessage({
        type: "skiff.open", nonce: nonce, id: id, plugin: plugin, path: path || ""
      }, "*");
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
  function bytes(n) {
    n = Number(n || 0);
    if (n < 1024) return n + " B";
    if (n < 1024 * 1024) return (n / 1024).toFixed(1) + " KiB";
    return (n / 1024 / 1024).toFixed(1) + " MiB";
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

  var TEMPLATE = [
    "server {",
    "    listen 80;",
    "    server_name example.com;",
    "    root /var/www/example.com;",
    "    index index.html;",
    "",
    "    location / {",
    "        try_files $uri $uri/ =404;",
    "    }",
    "}",
    ""
  ].join("\n");

  function render(list) {
    el("dir").textContent = list.dir || "-";
    var c = list.container;
    el("container").textContent = c ? (c.name + "（" + (c.state || "未知") + "）") : "未找到或未授权";
    var sites = list.sites || [];
    if (sites.length === 0) {
      el("sites").innerHTML = '<tr><td colspan="4" class="muted">该目录下没有 .conf 站点文件</td></tr>';
      return;
    }
    el("sites").innerHTML = sites.map(function (s) {
      return "<tr><td><code>" + esc(s.name) + "</code></td><td>" + bytes(s.size) +
        '</td><td>' + when(s.mtime) + '</td><td><button class="mini" data-open="' +
        esc(s.name) + '">编辑</button></td></tr>';
    }).join("");
  }

  function load() {
    rpc("sites").then(function (r) {
      render(r || {});
      if (!current && el("content").value === "") el("content").value = TEMPLATE;
    }).catch(function (e) {
      say("读取站点列表失败：" + e, true);
    });
  }

  function openSite(name) {
    rpc("read", { name: name }).then(function (r) {
      current = r.name;
      el("editing").textContent = current;
      el("to-name").value = "";
      el("content").value = r.content || "";
      say("已载入 " + current + "（" + bytes(r.size) + "）");
    }).catch(function (e) {
      say("读取 " + name + " 失败：" + e, true);
    });
  }

  function afterWrite(text) {
    say(text);
    return load();
  }

  el("sites").addEventListener("click", function (ev) {
    var btn = ev.target.closest ? ev.target.closest("button[data-open]") : null;
    if (!btn) return;
    openSite(btn.getAttribute("data-open"));
  });

  el("refresh").addEventListener("click", load);

  // 插件间切换：把当前站点配置目录交给「文件管理」，由它定位打开。
  // 目标插件未启用/未装时父页面会拒绝，这里只做提示。
  el("open-files").addEventListener("click", function () {
    var dir = el("dir").textContent || "";
    if (!dir || dir === "-") {
      say("还没取到站点配置目录，请先点「刷新」", true);
      return;
    }
    say("正在切到文件管理：" + dir + "…");
    openPlugin("com.skiff.files", dir).catch(function (e) {
      say("跳转失败：" + (e && e.message ? e.message : e), true);
    });
  });

  el("backup").addEventListener("click", function () {
    say("备份中…");
    rpc("backup").then(function (r) {
      var parts = ["备份 " + (r.saved || []).length + " 个文件"];
      if (r.dir) parts.push("目录 " + r.dir);
      if (r.pruned && r.pruned.length) parts.push("清理旧备份 " + r.pruned.length + " 份");
      if (r.skipped && r.skipped.length) parts.push("跳过 " + r.skipped.length + " 个（见插件状态）");
      say(parts.join("；"));
    }).catch(function (e) {
      say("备份失败：" + e, true);
    });
  });

  // 一键把备份写进「计划任务」：走 interop（`jobs.create`），按 source+key 幂等。
  el("schedule-backup").addEventListener("click", function () {
    var expr = window.prompt("备份任务的 cron 表达式（5 段，本地时区）", backupExpr);
    if (expr === null) return;
    expr = expr.trim();
    if (!expr) {
      say("表达式不能为空", true);
      return;
    }
    say("正在写入计划任务…");
    rpc("scheduleBackup", { expr: expr }).then(function (r) {
      lastJobId = r && r.job_id ? String(r.job_id) : "";
      var next = r && r.next_run && r.next_run.text ? "，下次 " + r.next_run.text : "";
      say((r && r.created ? "已新增" : "已更新") + "备份任务（" + (r.expr || expr) + "）" + next);
      el("open-cron").disabled = !lastJobId;
    }).catch(function (e) {
      say("新增备份任务失败：" + (e && e.message ? e.message : e), true);
    });
  });

  // 跳到计划任务并高亮刚写入的那条（`?open=job:<id>`）。
  el("open-cron").addEventListener("click", function () {
    say("正在切到计划任务…");
    openPlugin("com.skiff.cron", lastJobId ? "job:" + lastJobId : "").catch(function (e) {
      say("跳转失败：" + (e && e.message ? e.message : e), true);
    });
  });

  el("reload").addEventListener("click", function () {
    say("重启容器中…");
    rpc("reload").then(function (r) {
      say("容器 " + r.container + " 已重启（原状态 " + (r.state || "未知") + "）");
      return load();
    }).catch(function (e) {
      say("重载失败：" + e, true);
    });
  });

  el("save").addEventListener("click", function () {
    var name = current || el("new-name").value.trim();
    if (!name) {
      say("请先选中站点或填写站点名", true);
      return;
    }
    rpc("save", { name: name, content: el("content").value }).then(function (r) {
      current = r.name;
      el("editing").textContent = current;
      return afterWrite("已保存 " + r.name + "（" + bytes(r.size) + "）；点「重载容器」生效");
    }).catch(function (e) {
      say("保存失败：" + e, true);
    });
  });

  el("create").addEventListener("click", function () {
    var name = el("new-name").value.trim();
    if (!name) {
      say("请填写站点名", true);
      return;
    }
    rpc("create", { name: name, content: el("content").value || TEMPLATE }).then(function (r) {
      current = r.name;
      el("editing").textContent = current;
      el("new-name").value = "";
      return afterWrite("已新建 " + r.name + "；请编辑后点「重载容器」");
    }).catch(function (e) {
      say("新建失败：" + e, true);
    });
  });

  el("rename").addEventListener("click", function () {
    var to = el("to-name").value.trim();
    if (!current) {
      say("请先选中站点", true);
      return;
    }
    if (!to) {
      say("请填写新站点名", true);
      return;
    }
    rpc("rename", { name: current, to: to }).then(function (r) {
      current = r.to;
      el("editing").textContent = current;
      el("to-name").value = "";
      return afterWrite("已改名为 " + r.to + "；点「重载容器」生效");
    }).catch(function (e) {
      say("改名失败：" + e, true);
    });
  });

  el("remove").addEventListener("click", function () {
    if (!current) {
      say("请先选中站点", true);
      return;
    }
    if (!window.confirm("删除站点 " + current + ".conf？删除前建议先点「立即备份」——本操作不会自动备份。")) return;
    rpc("remove", { name: current }).then(function (r) {
      current = "";
      el("editing").textContent = "（未选中）";
      el("content").value = "";
      return afterWrite("已删除 " + r.name + ".conf；点「重载容器」生效");
    }).catch(function (e) {
      say("删除失败：" + e, true);
    });
  });

  rpc("config").then(function (c) {
    if (c && c.backup_expr) backupExpr = String(c.backup_expr);
  }).catch(function () {});
  load();
})();
