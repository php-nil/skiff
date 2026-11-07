// com.skiff.ftp 的共享脚本。协议见 crates/web/static/skiff.js::attachPluginBridges。
// CSP 为 `default-src 'none'; script-src 'self'; connect-src 'none'`：脚本全部
// 同源随包分发，无内联脚本、无外链。
(function () {
  "use strict";

  var qs = new URLSearchParams(location.search);
  var nonce = qs.get("nonce") || "";
  var pending = {};
  var seq = 0;

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

  function reportHeight() {
    if (!nonce) return;
    var h = Math.max(480, document.documentElement.scrollHeight || 0);
    try {
      parent.postMessage({ type: "skiff.height", nonce: nonce, height: Math.round(Math.min(900, h)) }, "*");
    } catch (e) { /* 独立打开 */ }
  }
  window.addEventListener("load", reportHeight);
  window.addEventListener("resize", reportHeight);

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
    if (msg.error) p.reject(new Error(msg.error.message || msg.error));
    else p.resolve(msg.result);
  });

  function $(id) { return document.getElementById(id); }
  function esc(s) {
    return String(s == null ? "" : s).replace(/[&<>"']/g, function (c) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c];
    });
  }
  function toast(msg, bad) {
    var el = $("toast");
    if (!el) return;
    el.textContent = msg;
    el.className = "toast show" + (bad ? " bad" : "");
    setTimeout(function () { el.className = "toast"; }, 4000);
  }
  function busy(btn, on, label) {
    if (!btn) return;
    if (on) {
      btn.dataset.label = btn.textContent;
      btn.disabled = true;
      btn.textContent = label || "处理中…";
    } else {
      btn.disabled = false;
      if (btn.dataset.label) btn.textContent = btn.dataset.label;
    }
  }
  function unwrap(v) { return v && v.result !== undefined ? v.result : v; }

  function maskHome(h) {
    return String(h || "").replace(/\/\.\/$/, "");
  }

  /* ---------------- 账号页 ---------------- */
  var FTPSites = null;
  var FTPStatus = null;

  function shortSite(s) {
    return s.domain || "";
  }

  function renderStatusLine(st) {
    var box = $("status-line");
    if (!box || !st) return;
    var live = st.installed && st.running;
    box.innerHTML = live
      ? '<span class="pill ok">服务运行中</span> <span class="muted">pure-ftpd ' + esc(st.version || "") +
        ' · 被动端口 ' + esc(st.passive_port_start) + '–' + esc(st.passive_port_end) +
        ' · TLS ' + esc(st.tls_mode) + ' · 登录限制 ' + (st.login_guard ? "fail2ban" : "未启用") + '</span>'
      : '<span class="pill bad">服务未运行</span> <span class="muted">创建账号前需要先安装并启用 pure-ftpd。</span>';
    var installBtn = $("btn-install-inline");
    if (installBtn) installBtn.style.display = st.installed && st.running ? "none" : "";
  }

  function siteOptions() {
    var sel = $("site");
    if (!sel) return;
    var sites = (FTPSites && FTPSites.sites) || [];
    var keep = sel.value;
    sel.innerHTML = "";
    var ok = 0;
    sites.forEach(function (s) {
      if (!s.supported) return;
      var opt = document.createElement("option");
      opt.value = s.domain;
      opt.textContent = s.domain + (s.php ? "（PHP）" : "（静态）");
      sel.appendChild(opt);
      ok++;
    });
    if (!ok) {
      var opt = document.createElement("option");
      opt.value = "";
      opt.textContent = "没有可用的站点";
      sel.appendChild(opt);
    }
    if (keep) sel.value = keep;
    var hints = sites.filter(function (s) { return !s.supported; }).map(function (s) {
      return s.domain + "：" + (s.reason || "不支持");
    });
    var note = $("unsupported");
    if (note) note.textContent = hints.length ? "未开放：" + hints.join("；") : "";
  }

  function accountRow(a) {
    return '<tr><td><strong>' + esc(a.site || "（无法归属）") + '</strong></td>' +
      '<td class="mono">' + esc(a.login) + '</td>' +
      '<td class="mono">' + esc(a.uid) + ':' + esc(a.gid) + '</td>' +
      '<td class="mono small">' + esc(maskHome(a.home)) + '</td>' +
      '<td class="right"><button class="sm" data-pw="' + esc(a.login) + '">重置密码</button> ' +
      '<button class="sm danger" data-del="' + esc(a.login) + '">删除</button></td></tr>';
  }

  function renderAccounts(list) {
    var body = $("accounts");
    if (!body) return;
    var rows = (list && list.accounts) || [];
    body.innerHTML = rows.length
      ? rows.map(accountRow).join("")
      : '<tr><td colspan="5" class="empty">还没有 FTP 账号。先在上方为某个站点创建一个。</td></tr>';
    body.querySelectorAll("[data-del]").forEach(function (b) {
      b.onclick = function () { removeAccount(b.getAttribute("data-del")); };
    });
    body.querySelectorAll("[data-pw]").forEach(function (b) {
      b.onclick = function () { resetPassword(b.getAttribute("data-pw")); };
    });
  }

  function refreshAccounts() {
    var btn = $("btn-refresh");
    busy(btn, true);
    Promise.all([
      rpc("status"),
      rpc("sites"),
      rpc("accounts"),
    ]).then(function (res) {
      FTPStatus = unwrap(res[0]);
      FTPSites = unwrap(res[1]);
      renderStatusLine(FTPStatus);
      siteOptions();
      renderAccounts(unwrap(res[2]));
    }).catch(function (e) {
      toast(e.message || String(e), true);
    }).finally(function () {
      busy(btn, false);
      reportHeight();
    });
  }

  function installInline() {
    var btn = $("btn-install-inline");
    busy(btn, true, "安装中…");
    rpc("install").then(function (r) {
      FTPStatus = unwrap(r);
      renderStatusLine(FTPStatus);
      toast("FTP 服务已安装并启动");
    }).catch(function (e) {
      toast(e.message || String(e), true);
    }).finally(function () {
      busy(btn, false);
      refreshAccounts();
    });
  }

  function createAccount(ev) {
    ev.preventDefault();
    var btn = $("btn-create");
    var site = $("site").value;
    var account = $("account").value.trim();
    var password = $("password").value;
    var confirm = $("password2").value;
    if (!site || !account || !password) return toast("站点、账号与密码必须填写", true);
    if (password !== confirm) return toast("两次输入的密码不一致", true);
    busy(btn, true, "创建中…");
    rpc("create", { site: site, account: account, password: password }).then(function (r) {
      var a = unwrap(r);
      toast("已创建 " + (a.login || account) + "（uid/gid " + a.uid + ":" + a.gid + "）");
      $("account").value = "";
      $("password").value = "";
      $("password2").value = "";
      refreshAccounts();
    }).catch(function (e) {
      toast(e.message || String(e), true);
    }).finally(function () {
      busy(btn, false);
    });
  }

  function resetPassword(login) {
    var pw = window.prompt("为 " + login + " 设置新密码（8–128 字节）");
    if (!pw) return;
    rpc("password", { login: login, password: pw }).then(function () {
      toast("密码已更新：" + login);
    }).catch(function (e) {
      toast(e.message || String(e), true);
    });
  }

  function removeAccount(login) {
    if (!window.confirm("删除 FTP 账号 " + login + "？已上传的文件会保留。")) return;
    rpc("delete", { login: login }).then(function () {
      toast("已删除账号：" + login);
      refreshAccounts();
    }).catch(function (e) {
      toast(e.message || String(e), true);
    });
  }

  /* ---------------- 运行状态页 ---------------- */
  function renderRuntime(st) {
    if (!st) return;
    var grid = $("runtime-grid");
    if (!grid) return;
    grid.innerHTML = [
      ["安装状态", st.installed ? "已安装" : "未安装"],
      ["服务状态", st.running ? "运行中" : "未运行"],
      ["版本", st.version || "—"],
      ["被动端口", st.passive_port_start + "–" + st.passive_port_end],
      ["TLS 模式", st.tls_mode],
      ["登录尝试限制", st.login_guard ? "fail2ban：3 次失败 / 10 分钟" : "未启用"],
      ["账号数量", String(st.accounts)],
      ["用户库", st.passwd_file],
    ].map(function (kv) {
      return '<div class="kv"><span>' + esc(kv[0]) + '</span><strong class="mono">' + esc(kv[1]) + '</strong></div>';
    }).join("");
    var installBtn = $("btn-install");
    if (installBtn) installBtn.style.display = st.installed ? "none" : "";
    var restartBtn = $("btn-restart");
    if (restartBtn) restartBtn.style.display = st.installed ? "" : "none";
  }

  function loadRuntime() {
    var btn = $("btn-refresh-runtime");
    busy(btn, true);
    rpc("status").then(function (r) {
      renderRuntime(unwrap(r));
    }).catch(function (e) {
      toast(e.message || String(e), true);
    }).finally(function () {
      busy(btn, false);
      reportHeight();
    });
  }

  function install() {
    var btn = $("btn-install");
    busy(btn, true, "安装中…");
    rpc("install").then(function (r) {
      renderRuntime(unwrap(r));
      toast("FTP 服务已安装并启动");
    }).catch(function (e) {
      toast(e.message || String(e), true);
    }).finally(function () {
      busy(btn, false);
    });
  }

  function restart() {
    var btn = $("btn-restart");
    busy(btn, true, "重启中…");
    rpc("restart").then(function () {
      toast("pure-ftpd 已重启");
      loadRuntime();
    }).catch(function (e) {
      toast(e.message || String(e), true);
    }).finally(function () {
      busy(btn, false);
    });
  }

  function uninstall() {
    var accounts = $("rm-accounts").checked;
    var pkg = $("rm-package").checked;
    if (!window.confirm("停止并禁用 pure-ftpd？" + (accounts ? "本插件创建的 FTP 账号会被删除，文件保留。" : "账号会保留。") + (pkg ? "并卸载 pure-ftpd 软件包。" : ""))) {
      return;
    }
    var btn = $("btn-uninstall");
    busy(btn, true, "处理中…");
    rpc("uninstall", { remove_accounts: accounts, remove_package: pkg }).then(function () {
      toast("FTP 服务已停用" + (pkg ? "并卸载" : ""));
      loadRuntime();
    }).catch(function (e) {
      toast(e.message || String(e), true);
    }).finally(function () {
      busy(btn, false);
    });
  }

  document.addEventListener("DOMContentLoaded", function () {
    var view = document.body.getAttribute("data-view");
    if (view === "accounts") {
      $("btn-refresh").onclick = refreshAccounts;
      $("btn-install-inline").onclick = installInline;
      $("create-form").onsubmit = createAccount;
      refreshAccounts();
    } else if (view === "runtime") {
      $("btn-refresh-runtime").onclick = loadRuntime;
      $("btn-install").onclick = install;
      $("btn-restart").onclick = restart;
      $("btn-uninstall").onclick = uninstall;
      loadRuntime();
    }
  });
})();
