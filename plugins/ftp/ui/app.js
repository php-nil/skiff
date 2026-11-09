// com.skiff.ftp 的共享脚本。协议见 crates/web/static/skiff.js::attachPluginBridges。
// CSP 为 `default-src 'none'; script-src 'self'; connect-src 'none'`：脚本全部
// 同源随包分发，无内联脚本、无外链。密码只在内存中传递，不写日志、不落 DOM 列表。
(function () {
  "use strict";

  var qs = new URLSearchParams(location.search);
  var nonce = qs.get("nonce") || "";
  var pending = {};
  var seq = 0;
  var FTPSites = null;
  var FTPStatus = null;
  var FTPConfig = null;
  var FTPAccounts = null;

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
    var h = Math.max(320, document.body ? document.body.scrollHeight : 0);
    try {
      parent.postMessage({ type: "skiff.height", nonce: nonce, height: Math.round(Math.min(1200, h)) }, "*");
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
  function maskHome(h) { return String(h || "").replace(/\/\.\/$/, ""); }

  function tlsText(mode) {
    if (mode === "0" || mode === "off") return "关闭";
    if (mode === "1" || mode === "optional") return "显式 TLS（可选）";
    if (mode === "2" || mode === "required") return "显式 TLS（强制）";
    return String(mode || "—");
  }

  function unknownText(v, label) {
    return (v === null || v === undefined) ? (label || "未知（读取失败）") : String(v);
  }

  function boolText(v, yes, no) {
    if (v === null || v === undefined) return "未知（读取失败）";
    return v ? yes : no;
  }

  function dialogHost() {
    var host = $("dialog-host");
    if (host) return host;
    host = document.createElement("div");
    host.id = "dialog-host";
    document.body.appendChild(host);
    return host;
  }

  // ---- 统一确认弹窗 ----
  var confirmDlg = null;
  var confirmResolve = null;
  function ensureConfirm() {
    if (confirmDlg) return confirmDlg;
    dialogHost().insertAdjacentHTML("beforeend",
      '<dialog class="modal" id="confirm-dialog" data-confirm-dialog style="width:min(480px, calc(100vw - 24px))">' +
        '<div class="modal-head"><h2 id="confirm-title">请确认</h2></div>' +
        '<div class="modal-body"><p id="confirm-text" class="muted" style="margin:0"></p></div>' +
        '<div class="modal-foot"><span class="msg"></span>' +
          '<button id="confirm-cancel">取消</button>' +
          '<button id="confirm-ok" class="danger">确认</button>' +
        '</div>' +
      '</dialog>');
    confirmDlg = $("confirm-dialog");
    $("confirm-cancel").addEventListener("click", function () { closeConfirm(false); });
    $("confirm-ok").addEventListener("click", function () { closeConfirm(true); });
    confirmDlg.addEventListener("cancel", function (ev) {
      ev.preventDefault();
      closeConfirm(false);
    });
    return confirmDlg;
  }
  function closeConfirm(ok) {
    if (!confirmDlg) return;
    try { confirmDlg.close(); } catch (e) { confirmDlg.removeAttribute("open"); }
    var resolve = confirmResolve;
    confirmResolve = null;
    if (resolve) resolve(!!ok);
  }
  function confirmAction(title, text, okLabel) {
    var dlg = ensureConfirm();
    if (!dlg) return Promise.resolve(false);
    $("confirm-title").textContent = title || "请确认";
    $("confirm-text").textContent = text || "";
    $("confirm-ok").textContent = okLabel || "确认";
    try { dlg.showModal(); } catch (e) { dlg.setAttribute("open", ""); }
    return new Promise(function (resolve) { confirmResolve = resolve; });
  }

  // ---- 服务配置弹窗（账号页与运行状态页共用）----
  var cfgDlg = null;
  function ensureConfigDialog() {
    if (cfgDlg) return cfgDlg;
    dialogHost().insertAdjacentHTML("beforeend",
      '<dialog class="modal" id="cfg-dialog">' +
        '<div class="modal-head"><h2>服务配置</h2><button class="x" id="cfg-close" aria-label="关闭">×</button></div>' +
        '<div class="modal-body">' +
          '<div class="form-section">' +
            '<h4>网络与端口</h4>' +
            '<div class="field-grid">' +
              '<label class="field"><span>监听端口</span><input type="number" id="cfg-listen-port" min="1" max="65535"></label>' +
              '<label class="field"><span>TLS 模式</span><select id="cfg-tls">' +
                '<option value="off">关闭（不推荐，仅限内网）</option>' +
                '<option value="optional">显式 TLS（可选，推荐）</option>' +
                '<option value="required">显式 TLS（强制）</option>' +
              '</select></label>' +
            '</div>' +
            '<div class="field-grid">' +
              '<label class="field"><span>被动端口起始</span><input type="number" id="cfg-start" min="1024" max="65535"></label>' +
              '<label class="field"><span>被动端口结束</span><input type="number" id="cfg-end" min="1024" max="65535"></label>' +
            '</div>' +
            '<div class="field-grid">' +
              '<label class="field"><span>最大连接数</span><input type="number" id="cfg-max-clients" min="1" max="1000"></label>' +
              '<label class="field"><span>每 IP 最大连接</span><input type="number" id="cfg-max-per-ip" min="1" max="1000"></label>' +
            '</div>' +
            '<p class="hint">被动端口 1024–65535，起始小于结束，跨度不超过 500；每 IP 上限不能超过总连接数。</p>' +
            '<div class="warn" style="margin-top:8px">监听端口或被动端口变更后，宿主防火墙必须一并放通，否则客户端能登录但列目录/传输会卡住。</div>' +
          '</div>' +
          '<div class="form-section">' +
            '<h4>传输安全</h4>' +
            '<div class="field-grid">' +
              '<label class="field"><span>TLS 证书路径（PEM，可留空）</span><input id="cfg-cert-path" placeholder="/etc/letsencrypt/live/example/fullchain.pem"></label>' +
              '<label class="field"><span>TLS 私钥路径（PEM，可留空）</span><input id="cfg-key-path" placeholder="/etc/letsencrypt/live/example/privkey.pem"></label>' +
            '</div>' +
            '<p class="hint">证书与私钥必须同时填写或同时留空。留空时沿用 /etc/ssl/private/pure-ftpd.pem；首次启用且该文件不存在时由内核生成自签证书。配置了路径但文件缺失/非法会直接报错，不会静默降级。</p>' +
            '<div class="readonly-row"><span>匿名访问</span><span id="cfg-anon">强制关闭（内核固定 NoAnonymous=yes）</span></div>' +
          '</div>' +
          '<div class="form-section">' +
            '<h4>登录失败封禁</h4>' +
            '<p class="hint">fail2ban 已收口到独立的「防火墙」插件，FTP 配置不再安装或维护它。需要登录防护时，请在「运行状态」页的可选防护卡片中开启；未安装防火墙插件时该选项不可用，不影响 FTP 本身。</p>' +
          '</div>' +
          '<div id="cfg-error" class="bad" style="min-height:18px"></div>' +
        '</div>' +
        '<div class="modal-foot"><span class="msg" id="cfg-foot"></span>' +
          '<button id="cfg-cancel">取消</button>' +
          '<button id="cfg-save" class="primary">保存并重载</button>' +
        '</div>' +
      '</dialog>');
    cfgDlg = $("cfg-dialog");
    $("cfg-close").addEventListener("click", closeConfig);
    $("cfg-cancel").addEventListener("click", closeConfig);
    cfgDlg.addEventListener("cancel", function (ev) {
      ev.preventDefault();
      closeConfig();
    });
    $("cfg-save").addEventListener("click", saveConfig);
    return cfgDlg;
  }

  function closeConfig() {
    if (!cfgDlg) return;
    try { cfgDlg.close(); } catch (e) { cfgDlg.removeAttribute("open"); }
    scheduleHeight();
  }

  function openConfig() {
    var dlg = ensureConfigDialog();
    rpc("config").then(function (r) {
      FTPConfig = unwrap(r) || {};
      $("cfg-listen-port").value = String(FTPConfig.listen_port || 21);
      $("cfg-start").value = String(FTPConfig.passive_port_start || 30000);
      $("cfg-end").value = String(FTPConfig.passive_port_end || 30100);
      $("cfg-max-clients").value = String(FTPConfig.max_clients || 30);
      $("cfg-max-per-ip").value = String(FTPConfig.max_per_ip || 3);
      var tls = FTPConfig.tls_mode;
      if (tls === "0") tls = "off";
      if (tls === "1") tls = "optional";
      if (tls === "2") tls = "required";
      $("cfg-tls").value = tls === "required" ? "required" : (tls === "off" ? "off" : "optional");
      $("cfg-cert-path").value = FTPConfig.tls_cert_path || "";
      $("cfg-key-path").value = FTPConfig.tls_key_path || "";
      $("cfg-anon").textContent = "强制关闭（内核固定 NoAnonymous=yes）";
      $("cfg-error").textContent = "";
      $("cfg-foot").textContent = "";
      $("cfg-foot").className = "msg muted";
      try { dlg.showModal(); } catch (e) { dlg.setAttribute("open", ""); }
    }).catch(function (e) {
      toast("读取服务配置失败：" + (e.message || e), true);
    });
  }

  function saveConfig() {
    var port = Number($("cfg-listen-port").value);
    var start = Number($("cfg-start").value);
    var end = Number($("cfg-end").value);
    var maxClients = Number($("cfg-max-clients").value);
    var maxPerIp = Number($("cfg-max-per-ip").value);
    var tls = $("cfg-tls").value;
    var cert = $("cfg-cert-path").value.trim();
    var key = $("cfg-key-path").value.trim();
    var saveBtn = $("cfg-save");
    $("cfg-error").textContent = "";
    function intOk(v) { return isFinite(v) && Math.floor(v) === v; }
    if (!intOk(port) || port < 1 || port > 65535) {
      $("cfg-error").textContent = "监听端口需为 1–65535 的整数";
      return;
    }
    if (!intOk(maxClients) || maxClients < 1 || maxClients > 1000) {
      $("cfg-error").textContent = "最大连接数需为 1–1000 的整数";
      return;
    }
    if (!intOk(maxPerIp) || maxPerIp < 1 || maxPerIp > maxClients) {
      $("cfg-error").textContent = "每 IP 最大连接需为 1–最大连接数之间的整数";
      return;
    }
    if (!intOk(start) || !intOk(end) || start < 1024 || end > 65535 || start >= end || end - start > 500) {
      $("cfg-error").textContent = "被动端口范围非法（需 1024–65535，起始小于结束，跨度≤500）";
      return;
    }
    if ((cert && !key) || (!cert && key)) {
      $("cfg-error").textContent = "TLS 证书与私钥路径必须同时填写或同时留空";
      return;
    }
    busy(saveBtn, true, "保存并重载中…");
    rpc("config.save", {
      listen_port: port,
      passive_port_start: start,
      passive_port_end: end,
      max_clients: maxClients,
      max_per_ip: maxPerIp,
      tls_mode: tls,
      tls_cert_path: cert || null,
      tls_key_path: key || null,
    }).then(function (r) {
      var res = unwrap(r) || {};
      if (res.status) applyStatus(res.status);
      if (res.ok) {
        FTPConfig = FTPConfig || {};
        FTPConfig.listen_port = port;
        FTPConfig.passive_port_start = start;
        FTPConfig.passive_port_end = end;
        FTPConfig.max_clients = maxClients;
        FTPConfig.max_per_ip = maxPerIp;
        FTPConfig.tls_mode = tls;
        FTPConfig.tls_cert_path = cert || null;
        FTPConfig.tls_key_path = key || null;
        toast(res.message || "配置已保存");
        closeConfig();
        refreshPage();
      } else {
        $("cfg-error").textContent = (res.message || "保存失败") + (res.error ? "：" + res.error : "");
        toast(res.message || "保存失败", true);
      }
    }).catch(function (e) {
      $("cfg-error").textContent = "保存失败：" + (e.message || e);
      toast(e.message || String(e), true);
    }).finally(function () {
      busy(saveBtn, false);
    });
  }

  function applyStatus(st) {
    if (!st) return;
    FTPStatus = st;
    renderStatusLine(FTPStatus);
    if (document.body.getAttribute("data-view") === "runtime") renderRuntime(FTPStatus);
  }

  // ---- 新建账号弹窗 ----
  var createDlg = null;
  function ensureCreateDialog() {
    if (createDlg) return createDlg;
    dialogHost().insertAdjacentHTML("beforeend",
      '<dialog class="modal" id="create-dialog">' +
        '<div class="modal-head"><h2>新建 FTP 账号</h2><button class="x" id="cd-close" aria-label="关闭">×</button></div>' +
        '<div class="modal-body">' +
          '<div class="form-section">' +
            '<h4>账号</h4>' +
            '<label class="field"><span>站点</span><select id="cd-site"></select></label>' +
            '<div class="field-grid">' +
              '<label class="field"><span>账号名（1–32 位）</span><input id="cd-account" autocomplete="off" placeholder="deploy"></label>' +
              '<label class="field"><span>根目录（只读）</span><input id="cd-home" readonly></label>' +
            '</div>' +
          '</div>' +
          '<div class="form-section">' +
            '<h4>密码</h4>' +
            '<div class="field-grid">' +
              '<label class="field"><span>密码（8–128 字节）</span><input id="cd-password" type="password" autocomplete="new-password"></label>' +
              '<label class="field"><span>确认密码</span><input id="cd-password2" type="password" autocomplete="new-password"></label>' +
            '</div>' +
            '<p class="hint">密码只在提交时经内核传给 pure-pw，不会回显到列表、弹窗重开或日志。</p>' +
          '</div>' +
          '<div class="form-section">' +
            '<h4>配额 / 速率</h4>' +
            '<div class="readonly-row"><span>配额与速率限制</span><span>当前内核 ftp 账号能力面暂不支持</span></div>' +
            '<p class="hint">为避免账号文件被 pure-pw 的 -y 参数破坏归属，内核固定不启用配额与速率。</p>' +
          '</div>' +
          '<div class="note" style="margin-top:0">' +
            '账号只能看到该站点 <span class="mono">index/</span>；创建时会把既有文件属主/属组对齐到站点运行用户，' +
            '目录改为 2775、文件改为 0664。' +
          '</div>' +
          '<div id="cd-error" class="bad" style="min-height:18px"></div>' +
        '</div>' +
        '<div class="modal-foot"><span class="msg" id="cd-foot"></span>' +
          '<button id="cd-cancel">取消</button>' +
          '<button id="cd-save" class="primary">创建账号</button>' +
        '</div>' +
      '</dialog>');
    createDlg = $("create-dialog");
    $("cd-close").addEventListener("click", closeCreate);
    $("cd-cancel").addEventListener("click", closeCreate);
    createDlg.addEventListener("cancel", function (ev) {
      ev.preventDefault();
      closeCreate();
    });
    $("cd-site").addEventListener("change", updateHomePreview);
    $("cd-save").addEventListener("click", submitCreate);
    return createDlg;
  }

  function clearPasswordFields() {
    ["cd-password", "cd-password2", "pw-password", "pw-password2"].forEach(function (id) {
      var el = $(id);
      if (el) el.value = "";
    });
  }

  function closeCreate() {
    if (!createDlg) return;
    clearPasswordFields();
    try { createDlg.close(); } catch (e) { createDlg.removeAttribute("open"); }
    scheduleHeight();
  }

  function updateHomePreview() {
    var site = $("cd-site").value;
    var sites = (FTPSites && FTPSites.sites) || [];
    var found = sites.filter(function (s) { return s.domain === site; })[0];
    $("cd-home").value = found && found.index_dir ? found.index_dir : "（站点根目录不可用）";
  }

  function openCreate() {
    var dlg = ensureCreateDialog();
    $("cd-error").textContent = "";
    $("cd-foot").textContent = "";
    $("cd-foot").className = "msg";
    $("cd-account").value = "";
    clearPasswordFields();
    var sel = $("cd-site");
    sel.innerHTML = "";
    var sites = (FTPSites && FTPSites.sites) || [];
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
      opt.textContent = "没有可用站点";
      sel.appendChild(opt);
    }
    updateHomePreview();
    try { dlg.showModal(); } catch (e) { dlg.setAttribute("open", ""); }
    setTimeout(function () { $("cd-account").focus(); }, 0);
  }

  function submitCreate() {
    var btn = $("cd-save");
    var site = $("cd-site").value;
    var account = $("cd-account").value.trim();
    var password = $("cd-password").value;
    var confirm = $("cd-password2").value;
    $("cd-error").textContent = "";
    if (!site) { $("cd-error").textContent = "请选择站点"; return; }
    if (!account) { $("cd-error").textContent = "账号名不能为空"; return; }
    if (!password) { $("cd-error").textContent = "密码不能为空"; return; }
    if (password !== confirm) { $("cd-error").textContent = "两次输入的密码不一致"; return; }
    busy(btn, true, "创建中…");
    rpc("create", { site: site, account: account, password: password }).then(function (r) {
      var a = unwrap(r) || {};
      clearPasswordFields();
      closeCreate();
      toast("已创建 " + (a.login || account) + "（uid/gid " + a.uid + ":" + a.gid + "）");
      refreshAccounts();
    }).catch(function (e) {
      $("cd-error").textContent = "创建失败：" + (e.message || e);
      $("cd-foot").textContent = "已保留填写内容";
      $("cd-foot").className = "msg bad";
      toast(e.message || String(e), true);
    }).finally(function () {
      busy(btn, false);
    });
  }

  // ---- 重置密码弹窗 ----
  var pwDlg = null;
  var pwLogin = "";
  function ensurePasswordDialog() {
    if (pwDlg) return pwDlg;
    dialogHost().insertAdjacentHTML("beforeend",
      '<dialog class="modal" id="pw-dialog" style="width:min(480px, calc(100vw - 24px))">' +
        '<div class="modal-head"><h2>重置密码</h2><button class="x" id="pw-close" aria-label="关闭">×</button></div>' +
        '<div class="modal-body">' +
          '<div class="readonly-row"><span>账号</span><span class="mono" id="pw-login"></span></div>' +
          '<div class="field-grid" style="margin-top:10px">' +
            '<label class="field"><span>新密码（8–128 字节）</span><input id="pw-password" type="password" autocomplete="new-password"></label>' +
            '<label class="field"><span>确认新密码</span><input id="pw-password2" type="password" autocomplete="new-password"></label>' +
          '</div>' +
          '<p class="hint">新密码不会回显到列表或日志。</p>' +
          '<div id="pw-error" class="bad" style="min-height:18px"></div>' +
        '</div>' +
        '<div class="modal-foot"><span class="msg" id="pw-foot"></span>' +
          '<button id="pw-cancel">取消</button>' +
          '<button id="pw-save" class="primary">保存密码</button>' +
        '</div>' +
      '</dialog>');
    pwDlg = $("pw-dialog");
    $("pw-close").addEventListener("click", closePassword);
    $("pw-cancel").addEventListener("click", closePassword);
    pwDlg.addEventListener("cancel", function (ev) {
      ev.preventDefault();
      closePassword();
    });
    $("pw-save").addEventListener("click", submitPassword);
    return pwDlg;
  }

  function closePassword() {
    if (!pwDlg) return;
    clearPasswordFields();
    pwLogin = "";
    try { pwDlg.close(); } catch (e) { pwDlg.removeAttribute("open"); }
  }

  function openPassword(login) {
    ensurePasswordDialog();
    pwLogin = login;
    $("pw-login").textContent = login;
    $("pw-error").textContent = "";
    $("pw-foot").textContent = "";
    $("pw-foot").className = "msg";
    clearPasswordFields();
    try { pwDlg.showModal(); } catch (e) { pwDlg.setAttribute("open", ""); }
    setTimeout(function () { $("pw-password").focus(); }, 0);
  }

  function submitPassword() {
    var btn = $("pw-save");
    var password = $("pw-password").value;
    var confirm = $("pw-password2").value;
    $("pw-error").textContent = "";
    if (!password) { $("pw-error").textContent = "新密码不能为空"; return; }
    if (password !== confirm) { $("pw-error").textContent = "两次输入的密码不一致"; return; }
    busy(btn, true, "保存中…");
    rpc("password", { login: pwLogin, password: password }).then(function () {
      var login = pwLogin;
      closePassword();
      toast("密码已更新：" + login);
    }).catch(function (e) {
      $("pw-error").textContent = "保存失败：" + (e.message || e);
      $("pw-foot").textContent = "已保留填写内容";
      $("pw-foot").className = "msg bad";
    }).finally(function () {
      busy(btn, false);
    });
  }

  /* ---------------- 账号页 ---------------- */
  function renderStatusLine(st) {
    var box = $("status-line");
    if (!box || !st) return;
    var live = st.installed && st.running === true;
    box.innerHTML = live
      ? '<span class="pill ok">服务运行中</span> <span class="muted">pure-ftpd ' + esc(st.version || "") +
        ' · 监听 ' + esc(unknownText(st.listen_port)) +
        ' · 被动端口 ' + esc(st.passive_port_start) + '–' + esc(st.passive_port_end) +
        ' · TLS ' + esc(tlsText(st.tls_mode)) + '</span>'
      : '<span class="pill bad">' +
        (st.running === null || st.running === undefined ? "服务状态未知" : "服务未运行") +
        '</span> <span class="muted">创建账号前需要先安装并启用 pure-ftpd。</span>';
    var installBtn = $("btn-install-inline");
    if (installBtn) installBtn.style.display = live ? "none" : "";
  }

  function siteOptionsNote() {
    var sites = (FTPSites && FTPSites.sites) || [];
    var hints = sites.filter(function (s) { return !s.supported; }).map(function (s) {
      return s.domain + "：" + (s.reason || "不支持");
    });
    var note = $("unsupported");
    if (note) note.textContent = hints.length ? "未开放：" + hints.join("；") : "";
  }

  function accountRow(a) {
    var enabled = a.enabled !== false;
    var last = a.last_login
      ? '<span class="mono">' + esc(a.last_login) + '</span>'
      : '不可得 <span class="small muted">（pure-pw 不记录最后登录）</span>';
    return '<tr><td class="cut" title="' + esc(a.site || "") + '"><strong>' + esc(a.site || "（无法归属）") + '</strong></td>' +
      '<td class="cut mono" title="' + esc(a.login) + '">' + esc(a.login) + '</td>' +
      '<td class="mono">' + esc(a.uid) + ':' + esc(a.gid) + '</td>' +
      '<td class="cut mono small" title="' + esc(maskHome(a.home)) + '">' + esc(maskHome(a.home)) + '</td>' +
      '<td><span class="pill ' + (enabled ? "ok" : "bad") + '">' + (enabled ? "启用" : "已禁用") + '</span></td>' +
      '<td class="cut muted" title="最后登录不可得；配额与速率列为后续项">' + last +
      ' <span class="small muted">配额/速率：后续项</span></td>' +
      '<td class="actions">' +
      '<button class="sm" data-toggle="' + esc(a.login) + '" data-enabled="' + (enabled ? "0" : "1") + '">' +
        (enabled ? "禁用" : "启用") + '</button> ' +
      '<button class="sm" data-pw="' + esc(a.login) + '"' + (enabled ? "" : ' disabled title="先启用账号再修改密码"') + '>重置密码</button> ' +
      '<button class="sm danger" data-del="' + esc(a.login) + '">删除</button></td></tr>';
  }

  function renderAccounts(list) {
    var body = $("accounts");
    if (!body) return;
    var rows = (list && list.accounts) || [];
    var q = ($("q") && $("q").value || "").trim().toLowerCase();
    if (q) {
      rows = rows.filter(function (a) {
        return [a.site, a.login, a.home].join(" ").toLowerCase().indexOf(q) >= 0;
      });
    }
    body.innerHTML = rows.length
      ? rows.map(accountRow).join("")
      : '<tr><td colspan="7" class="empty">' +
        ((list && list.accounts && list.accounts.length) ? "没有匹配的账号。" : "还没有 FTP 账号。点击右上角「新建账号」创建。") +
        '</td></tr>';
    body.querySelectorAll("[data-del]").forEach(function (b) {
      b.onclick = function () { removeAccount(b.getAttribute("data-del")); };
    });
    body.querySelectorAll("[data-pw]").forEach(function (b) {
      b.onclick = function () { openPassword(b.getAttribute("data-pw")); };
    });
    body.querySelectorAll("[data-toggle]").forEach(function (b) {
      b.onclick = function () {
        toggleAccount(b.getAttribute("data-toggle"), b.getAttribute("data-enabled") === "1");
      };
    });
  }

  function toggleAccount(login, enabled) {
    function run() {
      return rpc("toggle", { login: login, enabled: enabled }).then(function () {
        toast((enabled ? "已启用账号：" : "已禁用账号：") + login);
        refreshAccounts();
      }).catch(function (e) {
        toast(e.message || String(e), true);
      });
    }
    if (!enabled) {
      confirmAction(
        "禁用 FTP 账号",
        "禁用后 " + login + " 将无法登录，但账号记录与站点目录都会保留，可随时重新启用。",
        "禁用"
      ).then(function (ok) { if (ok) run(); });
    } else {
      run();
    }
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
      siteOptionsNote();
      FTPAccounts = unwrap(res[2]);
      renderAccounts(FTPAccounts);
    }).catch(function (e) {
      toast(e.message || String(e), true);
    }).finally(function () {
      busy(btn, false);
      scheduleHeight();
    });
  }

  function installInline() {
    var btn = $("btn-install-inline");
    busy(btn, true, "安装中…");
    rpc("install").then(function (r) {
      var res = unwrap(r) || {};
      if (res.status) applyStatus(res.status);
      toast("FTP 服务已安装并启动");
    }).catch(function (e) {
      toast(e.message || String(e), true);
    }).finally(function () {
      busy(btn, false);
      refreshAccounts();
    });
  }

  function removeAccount(login) {
    confirmAction("删除 FTP 账号", "确认删除 FTP 账号 " + login + "？该账号立即无法登录，已上传的文件会保留。", "删除").then(function (ok) {
      if (!ok) return;
      rpc("delete", { login: login }).then(function () {
        toast("已删除账号：" + login);
        refreshAccounts();
      }).catch(function (e) {
        toast(e.message || String(e), true);
      });
    });
  }

  /* ---------------- 运行状态页 ---------------- */
  function renderRuntime(st) {
    if (!st) return;
    var grid = $("runtime-grid");
    if (!grid) return;
    var certText = (st.tls_cert_path && st.tls_key_path)
      ? st.tls_cert_path + ' + ' + st.tls_key_path
      : "/etc/ssl/private/pure-ftpd.pem（已有或自签）";
    grid.innerHTML = [
      ["安装状态", st.installed ? "已安装" : "未安装"],
      ["服务状态", boolText(st.running, "运行中", "未运行")],
      ["监听状态", boolText(st.listening, "是", "否")],
      ["监听端口", unknownText(st.listen_port)],
      ["当前连接数", unknownText(st.current_connections)],
      ["版本", st.version || "—"],
      ["被动端口", st.passive_port_start + "–" + st.passive_port_end],
      ["最大连接 / 每 IP", unknownText(st.max_clients) + " / " + unknownText(st.max_per_ip)],
      ["TLS 模式", tlsText(st.tls_mode)],
      ["证书路径", certText],
      ["匿名访问", "强制关闭"],
      ["账号数量", String(st.accounts) + "（启用 " + String(st.accounts_enabled) + " / 禁用 " + String(st.accounts_disabled) + "）"],
      ["用户库", st.passwd_file],
    ].map(function (kv) {
      return '<div class="kv"><span>' + esc(kv[0]) + '</span><strong class="mono">' + esc(kv[1]) + '</strong></div>';
    }).join("");
    var installBtn = $("btn-install");
    if (installBtn) installBtn.style.display = st.installed ? "none" : "";
    var restartBtn = $("btn-restart");
    if (restartBtn) restartBtn.style.display = st.installed ? "" : "none";
    loadProtection();
  }

  // 可选登录防护：状态来自独立防火墙插件；未安装时整张卡片只展示原因，不影响 FTP。
  function renderProtection(p) {
    var box = $("f2b-card");
    if (!box || !p) return;
    if (!p.available) {
      box.innerHTML = '<div class="note" style="margin:0">' + esc(p.reason || "fail2ban 防护不可用") + '</div>';
      scheduleHeight();
      return;
    }
    var state = p.enabled
      ? "已启用"
      : (p.installed ? "未启用（fail2ban 已安装）" : "未启用（fail2ban 未安装）");
    box.innerHTML =
      '<div class="readonly-row"><span>防护状态</span><span class="mono">' + esc(state) + '</span></div>' +
      '<div class="readonly-row"><span>fail2ban 服务</span><span class="mono">' + esc(p.running ? "运行中" : "未运行") + '</span></div>' +
      '<div class="readonly-row"><span>专用 jail</span><span class="mono">' + esc(p.jail) + '</span></div>' +
      '<div class="readonly-row"><span>管理方</span><span class="mono">独立防火墙插件 com.skiff.firewall</span></div>' +
      '<div class="row" style="margin-top:10px">' +
        '<button id="btn-protect-toggle" class="' + (p.enabled ? "danger" : "primary") + '">' +
          (p.enabled ? "关闭登录防护" : "开启登录防护") + '</button>' +
        (p.enabled ? '<button id="btn-protect-banned">查看封禁</button>' : '') +
      '</div>' +
      '<div id="protect-banned" class="mono-list"></div>' +
      '<p class="hint">开启后由防火墙插件写入 pure-ftpd jail（5 次失败 / 10 分钟封禁 1 小时）并重启 fail2ban；关闭只删本 jail，不卸载 fail2ban，也不改动其它 jail。</p>';
    var toggle = $("btn-protect-toggle");
    toggle.addEventListener("click", function () {
      busy(toggle, true, "处理中…");
      rpc(p.enabled ? "protection.disable" : "protection.enable").then(function (r) {
        renderProtection(unwrap(r) || {});
        toast(p.enabled ? "已关闭登录防护" : "已开启登录防护");
      }).catch(function (e) {
        toast(e.message || String(e), true);
        busy(toggle, false);
      });
    });
    var bannedBtn = $("btn-protect-banned");
    if (bannedBtn) bannedBtn.addEventListener("click", function () { loadBanned(p.jail); });
    scheduleHeight();
  }

  function loadProtection() {
    rpc("protection").then(function (r) {
      renderProtection(unwrap(r) || {});
    }).catch(function (e) {
      var box = $("f2b-card");
      if (box) box.innerHTML = '<div class="note" style="margin:0">' + esc(e.message || String(e)) + '</div>';
    });
  }

  function loadBanned(jail) {
    var box = $("protect-banned");
    if (!box) return;
    box.textContent = "加载中…";
    rpc("protection.banned", { jail: jail }).then(function (r) {
      var res = unwrap(r) || {};
      var ips = res.banned || [];
      if (!ips.length) {
        box.innerHTML = '<span class="muted">当前没有封禁 IP</span>';
        return;
      }
      box.innerHTML = ips.map(function (ip) {
        return '<div class="readonly-row"><span class="mono">' + esc(ip) + '</span>' +
          '<button class="link" data-unban="' + esc(ip) + '">解封</button></div>';
      }).join("");
      Array.prototype.forEach.call(box.querySelectorAll("button[data-unban]"), function (btn) {
        btn.addEventListener("click", function () {
          var ip = btn.getAttribute("data-unban");
          busy(btn, true);
          rpc("protection.unban", { jail: jail, ip: ip }).then(function () {
            toast("已解封 " + ip);
            loadBanned(jail);
          }).catch(function (e) { toast(e.message || String(e), true); busy(btn, false); });
        });
      });
      scheduleHeight();
    }).catch(function (e) { box.textContent = e.message || String(e); });
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
      scheduleHeight();
    });
  }

  function install() {
    var btn = $("btn-install");
    busy(btn, true, "安装中…");
    rpc("install").then(function (r) {
      var res = unwrap(r) || {};
      if (res.status) renderRuntime(res.status);
      else renderRuntime(res);
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
    var text = "停止并禁用 pure-ftpd？" +
      (accounts ? "本插件创建的 FTP 账号会被删除，文件保留。" : "账号会保留。") +
      (pkg ? "并卸载 pure-ftpd 软件包。" : "") +
      " fail2ban 由独立防火墙插件管理，回收 FTP 不会卸载它。";
    confirmAction("停止并回收 FTP", text, "停止回收").then(function (ok) {
      if (!ok) return;
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
    });
  }

  function refreshPage() {
    if (document.body.getAttribute("data-view") === "accounts") refreshAccounts();
    else loadRuntime();
  }

  function init() {
    var view = document.body.getAttribute("data-view");
    if (view === "accounts") {
      $("btn-refresh").onclick = refreshAccounts;
      $("btn-install-inline").onclick = installInline;
      $("btn-config").onclick = openConfig;
      $("btn-new").onclick = openCreate;
      if ($("q")) $("q").addEventListener("input", function () {
        renderAccounts(FTPAccounts);
      });
      refreshAccounts();
    } else if (view === "runtime") {
      $("btn-refresh-runtime").onclick = loadRuntime;
      $("btn-install").onclick = install;
      $("btn-restart").onclick = restart;
      $("btn-config").onclick = openConfig;
      $("btn-uninstall").onclick = uninstall;
      loadRuntime();
    }
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
})();
