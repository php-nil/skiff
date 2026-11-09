// 防火墙插件 UI：RPC 桥 + 两个功能页（规则 / fail2ban）。
// 协议见 crates/web/static/skiff.js：nonce 由面板下发，所有调用经 postMessage。
(function () {
  "use strict";

  var nonce = new URLSearchParams(location.search).get("nonce") || "";
  var pending = {};
  var seq = 0;

  // ---- 主题与高度：与其它插件页面保持同一套协议 ----
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
    if (msg.error) p.reject(new Error(msg.error.message || String(msg.error)));
    else p.resolve(msg.result);
  });

  // ---- 小工具 ----
  function $(id) { return document.getElementById(id); }
  function esc(s) {
    return String(s == null ? "" : s).replace(/[&<>"']/g, function (c) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c];
    });
  }
  function say(text, kind) {
    var el = $("msg");
    if (!el) return;
    el.textContent = text || "";
    el.className = kind || "";
  }
  function busy(btn, on) {
    if (!btn) return;
    btn.disabled = !!on;
  }
  function badge(el, text, kind) {
    if (!el) return;
    el.textContent = text;
    el.className = "badge" + (kind ? " " + kind : "");
  }
  function openModal(id) {
    var m = $(id);
    if (m) m.classList.add("open");
    scheduleHeight();
  }
  function closeModal(id) {
    var m = $(id);
    if (m) m.classList.remove("open");
    scheduleHeight();
  }
  document.addEventListener("click", function (ev) {
    var t = ev.target;
    if (t && t.hasAttribute && t.hasAttribute("data-close")) {
      var mask = t.closest(".modal-mask");
      if (mask) mask.classList.remove("open");
    }
    if (t && t.classList && t.classList.contains("modal-mask")) {
      t.classList.remove("open");
    }
  });

  // ═════════════════════════ 规则页 ═════════════════════════
  function initRules() {
    function renderStatus(st) {
      badge($("backend"), st.backend ? st.backend : "不可用", st.backend ? "ok" : "bad");
      var grid = $("status-grid");
      var f2b = st.fail2ban || {};
      grid.innerHTML =
        '<div class="row"><span class="muted">nftables</span><span class="badge ' +
        (st.nftables ? "ok" : "bad") + '">' + (st.nftables ? "可用" : "未找到") + "</span></div>" +
        '<div class="row"><span class="muted">iptables</span><span class="badge ' +
        (st.iptables ? "ok" : "bad") + '">' + (st.iptables ? "可用" : "未找到") + "</span></div>" +
        '<div class="row"><span class="muted">fail2ban</span><span class="badge ' +
        (f2b.running ? "ok" : (f2b.installed ? "warn" : "bad")) + '">' +
        (f2b.running ? "运行中" : (f2b.installed ? "已安装未运行" : "未安装")) + "</span></div>";
      var warnings = st.warnings || [];
      if (warnings.length) say(warnings.join("；"), "err");
    }

    function renderRules(res) {
      var items = (res && res.items) || [];
      $("count").textContent = items.length + " 条托管规则";
      var body = $("rules");
      if (!items.length) {
        body.innerHTML = '<tr><td colspan="6" class="empty">还没有托管规则</td></tr>';
      } else {
        body.innerHTML = items.map(function (r) {
          return "<tr>" +
            '<td><span class="badge ' + (r.action === "drop" ? "bad" : "ok") + '">' + esc(r.action) + "</span></td>" +
            "<td>" + esc(r.protocol || "-") + "</td>" +
            "<td class=\"mono\">" + esc(r.port == null ? "-" : r.port) + "</td>" +
            "<td class=\"mono\">" + esc(r.source || "-") + "</td>" +
            '<td class="mono">' + esc(r.id) + "</td>" +
            '<td><button class="link danger" data-del="' + esc(r.id) + '">删除</button></td>' +
            "</tr>";
        }).join("");
      }
      scheduleHeight();
    }

    function refresh() {
      say("");
      return Promise.all([rpc("status"), rpc("rules.list")]).then(function (out) {
        renderStatus(out[0] || {});
        renderRules(out[1] || {});
      }).catch(function (e) {
        say(e.message || String(e), "err");
      });
    }

    document.addEventListener("click", function (ev) {
      var t = ev.target;
      if (!t || !t.getAttribute) return;
      var id = t.getAttribute("data-del");
      if (!id) return;
      if (!confirm("删除规则 " + id + "？")) return;
      busy(t, true);
      rpc("rule.remove", { id: id }).then(function () {
        say("已删除 " + id, "ok");
        return refresh();
      }).catch(function (e) {
        say(e.message || String(e), "err");
        busy(t, false);
      });
    });

    $("btn-allow").addEventListener("click", function () { openModal("allow-mask"); });
    $("btn-rule").addEventListener("click", function () { openModal("rule-mask"); });
    $("refresh").addEventListener("click", refresh);

    $("allow-go").addEventListener("click", function () {
      var port = parseInt($("allow-port").value, 10);
      if (!port || port < 1 || port > 65535) { say("端口需在 1–65535", "err"); return; }
      var btn = $("allow-go");
      busy(btn, true);
      rpc("port.allow", { port: port, protocol: $("allow-proto").value }).then(function () {
        closeModal("allow-mask");
        say("已放行端口 " + port, "ok");
        return refresh();
      }).catch(function (e) {
        say(e.message || String(e), "err");
      }).finally(function () { busy(btn, false); });
    });

    $("rule-go").addEventListener("click", function () {
      var proto = $("rule-proto").value;
      var portText = $("rule-port").value.trim();
      var source = $("rule-source").value.trim();
      var params = { action: $("rule-action").value };
      if (proto !== "all") params.protocol = proto;
      if (portText) params.port = parseInt(portText, 10);
      if (source) params.source = source;
      if (!portText && !source) { say("端口与来源至少填一项", "err"); return; }
      if (proto === "all" && portText) { say("协议不限时不能指定端口", "err"); return; }
      var btn = $("rule-go");
      busy(btn, true);
      rpc("rule.add", params).then(function () {
        closeModal("rule-mask");
        say("规则已添加", "ok");
        return refresh();
      }).catch(function (e) {
        say(e.message || String(e), "err");
      }).finally(function () { busy(btn, false); });
    });

    refresh();
  }

  // ═════════════════════════ fail2ban 页 ═════════════════════════
  function initFail2ban() {
    var jails = [];

    function renderStatus(st) {
      badge($("f2b-state"), st.running ? "运行中" : (st.installed ? "已安装未运行" : "未安装"),
        st.running ? "ok" : (st.installed ? "warn" : "bad"));
      var actions = $("f2b-actions");
      actions.innerHTML = "";
      if (!st.installed) {
        var b = document.createElement("button");
        b.className = "primary";
        b.textContent = "安装 fail2ban";
        b.addEventListener("click", function () {
          busy(b, true);
          rpc("fail2ban.install").then(function (r) {
            say("fail2ban 已安装并启动", "ok");
            renderStatus(r || {});
            return loadJails();
          }).catch(function (e) {
            say(e.message || String(e), "err");
            busy(b, false);
          });
        });
        actions.appendChild(b);
        $("f2b-help").textContent = "安装后默认关闭 sshd jail，只管理本插件写入的 jail 文件。";
      } else {
        $("f2b-help").textContent = "fail2ban 由宿主机 systemd 管理；本页只增删 skiff-*.conf 并触发重启。";
      }
    }

    function renderJails() {
      var body = $("jails");
      if (!jails.length) {
        body.innerHTML = '<tr><td colspan="3" class="empty">没有已启用的 jail</td></tr>';
        scheduleHeight();
        return;
      }
      body.innerHTML = jails.map(function (jail) {
        return '<tr data-jail="' + esc(jail) + '">' +
          '<td class="mono">' + esc(jail) + "</td>" +
          '<td class="mono muted" data-banned>未查看</td>' +
          '<td><button class="link" data-jail-view="' + esc(jail) + '">查看封禁</button> ' +
          '<button class="link danger" data-remove="' + esc(jail) + '">移除文件</button></td>' +
          "</tr>";
      }).join("");
      scheduleHeight();
    }

    function loadJails() {
      return rpc("fail2ban.status").then(function (st) {
        renderStatus(st || {});
        jails = (st && st.jails) || [];
        renderJails();
      }).catch(function (e) {
        say(e.message || String(e), "err");
      });
    }

    function loadBanned(jail, cell) {
      cell.textContent = "加载中…";
      rpc("fail2ban.banned", { jail: jail }).then(function (r) {
        var ips = (r && r.banned) || [];
        if (!ips.length) {
          cell.innerHTML = '<span class="muted">无</span>';
          return;
        }
        cell.innerHTML = ips.map(function (ip) {
          return '<span class="mono">' + esc(ip) + "</span> " +
            '<button class="link" data-unban="' + esc(jail) + '" data-ip="' + esc(ip) + '">解封</button>';
        }).join("<br>");
        scheduleHeight();
      }).catch(function (e) {
        cell.textContent = e.message || String(e);
      });
    }

    document.addEventListener("click", function (ev) {
      var t = ev.target;
      if (!t || !t.getAttribute) return;
      var jailView = t.getAttribute("data-jail-view");
      if (jailView) {
        var row = t.closest("tr");
        var cell = row ? row.querySelector("[data-banned]") : null;
        if (cell) loadBanned(jailView, cell);
        return;
      }
      var unban = t.getAttribute("data-unban");
      if (unban) {
        var ip = t.getAttribute("data-ip");
        if (!confirm("解封 " + ip + "？")) return;
        busy(t, true);
        rpc("fail2ban.unban", { jail: unban, ip: ip }).then(function () {
          say("已解封 " + ip, "ok");
          var row = t.closest("tr");
          var cell = row ? row.querySelector("[data-banned]") : null;
          if (cell) loadBanned(unban, cell);
        }).catch(function (e) {
          say(e.message || String(e), "err");
          busy(t, false);
        });
        return;
      }
      var remove = t.getAttribute("data-remove");
      if (remove) {
        if (!confirm("移除 jail " + remove + " 的 skiff 配置文件？")) return;
        busy(t, true);
        rpc("fail2ban.jail.remove", { jail: remove }).then(function () {
          say("已移除 " + remove, "ok");
          return loadJails();
        }).catch(function (e) {
          say(e.message || String(e), "err");
          busy(t, false);
        });
      }
    });

    $("btn-refresh").addEventListener("click", function () { say(""); loadJails(); });

    $("jail-save").addEventListener("click", function () {
      var params = {
        jail: $("jail-name").value.trim(),
        filter: $("jail-filter").value.trim(),
        backend: $("jail-backend").value,
        maxretry: parseInt($("jail-maxretry").value, 10),
        findtime: parseInt($("jail-findtime").value, 10),
        bantime: parseInt($("jail-bantime").value, 10)
      };
      var port = $("jail-port").value.trim();
      var logpath = $("jail-logpath").value.trim();
      if (port) params.port = port;
      if (logpath) params.logpath = logpath;
      var btn = $("jail-save");
      busy(btn, true);
      rpc("fail2ban.jail.ensure", params).then(function () {
        say("jail 已保存并重启 fail2ban", "ok");
        return loadJails();
      }).catch(function (e) {
        say(e.message || String(e), "err");
      }).finally(function () { busy(btn, false); });
    });

    $("jail-remove").addEventListener("click", function () {
      var jail = $("jail-name").value.trim();
      if (!jail) { say("先填 jail 名称", "err"); return; }
      if (!confirm("移除 jail " + jail + " 的 skiff 配置文件？")) return;
      var btn = $("jail-remove");
      busy(btn, true);
      rpc("fail2ban.jail.remove", { jail: jail }).then(function () {
        say("已移除 " + jail, "ok");
        return loadJails();
      }).catch(function (e) {
        say(e.message || String(e), "err");
      }).finally(function () { busy(btn, false); });
    });

    // 用 config_schema 的默认值预填表单（读不到就保持 HTML 默认值）。
    rpc("config").then(function (cfg) {
      if (!cfg) return;
      if (cfg.default_maxretry) $("jail-maxretry").value = cfg.default_maxretry;
      if (cfg.default_findtime) $("jail-findtime").value = cfg.default_findtime;
      if (cfg.default_bantime) $("jail-bantime").value = cfg.default_bantime;
    }).catch(function () { /* 非致命 */ });

    loadJails();
  }

  var view = document.body.getAttribute("data-view");
  if (view === "fail2ban") initFail2ban();
  else initRules();
})();
