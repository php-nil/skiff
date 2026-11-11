// 插件 UI 的 RPC 桥客户端：所有能力调用都经父页面转发（iframe 自身没有凭据）。
// 协议见 crates/web/static/skiff.js::attachPluginBridges。
//
// 交互模型：左侧快捷入口/目录树 + 顶部面包屑/工具栏 + 名称/大小/修改时间的列表，
// 支持多选、右键菜单、行内重命名、新建目录与模态编辑器（行号 + 等宽 + 未保存提示）。
// 插件 UI 的 CSP 是 `default-src 'none'; script-src 'self'; connect-src 'none'`：
// 脚本/样式/图标全部同源随包分发，不能用 CDN，也不能有内联脚本。
(function () {
  "use strict";

  var qs = new URLSearchParams(location.search);
  var nonce = qs.get("nonce") || "";
  // 面板入口前缀取自本页路径（`/<prefix>/plugins/com.skiff.files/main`），
  // 用于「在内核页打开」跳转；不依赖任何额外参数。
  var prefix = location.pathname.split("/")[1] || "";
  // 跨插件跳转的目标位置（由另一个插件经父页面 `skiff.open` 转发过来）：
  // 这里只是「建议打开的路径」，能否读仍由内核按本插件的 path: 授权根判定。
  var openHint = qs.get("open") || "";

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

  // ---- 内容高度上报：父面板据此设置 iframe 高度，避免内外双滚动 ----
  // 文件管理是「填满视口」的应用（列表在内部滚动），高度不随目录条目数无限增长；
  // 报一个稳定的舒适值（480–680），父页面据此收缩/放宽 iframe。
  // 旧面板不消费该消息、独立打开时无父页面时，行为与以前一致。
  function reportHeight() {
    if (!nonce) return;
    var h = Math.min(680, Math.max(480, window.innerHeight || 680));
    try {
      parent.postMessage({ type: "skiff.height", nonce: nonce, height: Math.round(h) }, "*");
    } catch (e) { /* 独立打开 */ }
  }
  reportHeight();
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

  /// 请父页面跳到另一个插件的界面（跨插件切换）。父页面按白名单校验 id，
  /// 拒绝时会回 `skiff.open.result {ok:false}`。
  function openPlugin(plugin, path) {
    return new Promise(function (resolve) {
      var id = "open-" + (++seq);
      pending[id] = { resolve: resolve, reject: resolve };
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
    p.resolve(msg);
  });

  /* ---------------- 工具 ---------------- */
  function $(id) { return document.getElementById(id); }
  function esc(s) {
    return String(s == null ? "" : s).replace(/[&<>"']/g, function (c) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c];
    });
  }
  var KB = 1024, MB = KB * 1024, GB = MB * 1024, LIMIT = 256 * KB;
  function bytes(n) {
    n = Number(n || 0);
    if (n < KB) return n + " B";
    if (n < MB) return (n / KB).toFixed(1) + " KiB";
    if (n < GB) return (n / MB).toFixed(1) + " MiB";
    return (n / GB).toFixed(1) + " GiB";
  }
  function when(ts) {
    if (!ts) return "—";
    var d = new Date(Number(ts) * 1000);
    function p(n) { return (n < 10 ? "0" : "") + n; }
    return d.getFullYear() + "-" + p(d.getMonth() + 1) + "-" + p(d.getDate()) + " " +
      p(d.getHours()) + ":" + p(d.getMinutes());
  }
  function join(dir, name) { return (dir.endsWith("/") ? dir : dir + "/") + name; }
  function dirOf(path) { var i = path.lastIndexOf("/"); return i > 0 ? path.slice(0, i) : "/"; }
  function baseName(path) { var i = path.lastIndexOf("/"); return i >= 0 ? path.slice(i + 1) : path; }
  function badName(name) {
    return !name || name === "." || name === ".." || name.indexOf("/") >= 0 ||
      name.indexOf("\\") >= 0 || name.indexOf("..") >= 0;
  }
  function errText(e) { return String((e && e.message) || e || "未知错误"); }
  function iconFor(item) {
    if (item.dir) return "#i-folder";
    var ext = String(item.name).toLowerCase().split(".").pop();
    if (["js", "ts", "json", "rs", "py", "sh", "php", "yml", "yaml", "html", "css", "conf", "toml"].indexOf(ext) >= 0) return "#i-file-code";
    if (["gz", "tgz", "zip", "tar", "bz2", "xz", "7z"].indexOf(ext) >= 0) return "#i-file-archive";
    if (["md", "txt", "log", "env", "ini"].indexOf(ext) >= 0) return "#i-file-text";
    return "#i-file";
  }
  function b64encode(text) {
    var arr = new TextEncoder().encode(text);
    var bin = "";
    for (var i = 0; i < arr.length; i += 0x8000) {
      bin += String.fromCharCode.apply(null, arr.subarray(i, i + 0x8000));
    }
    return btoa(bin);
  }
  function b64decode(b64) {
    var bin = atob(b64 || "");
    var arr = new Uint8Array(bin.length);
    for (var i = 0; i < bin.length; i++) arr[i] = bin.charCodeAt(i);
    return new TextDecoder("utf-8").decode(arr);
  }
  function toast(text, kind) {
    var box = $("toasts");
    var el = document.createElement("div");
    el.className = "toast " + (kind || "good");
    el.innerHTML = '<svg class="ico"><use href="' + (kind === "bad" ? "#i-warn" : "#i-check") +
      '"/></svg><span>' + esc(text) + "</span>";
    box.appendChild(el);
    setTimeout(function () { el.remove(); }, 3200);
  }
  function message(text, kind) { toast(text, kind === "bad" ? "bad" : "good"); }

  /* ---------------- 语法高亮（随包 vendor） ---------------- */
  var hl = window.hljs || null;
  var EXT_LANG = {
    js: "javascript", mjs: "javascript", cjs: "javascript", jsx: "javascript",
    ts: "typescript", tsx: "typescript",
    json: "json", jsonc: "json", json5: "json",
    md: "markdown", markdown: "markdown", mdx: "markdown",
    sh: "bash", bash: "bash", zsh: "bash", ksh: "bash", fish: "bash",
    py: "python", rs: "rust", go: "go", rb: "ruby", php: "php", pl: "perl", lua: "lua", r: "r",
    c: "c", h: "c", cc: "cpp", cpp: "cpp", hpp: "cpp", cxx: "cpp",
    cs: "csharp", java: "java", kt: "kotlin", swift: "swift",
    css: "css", scss: "scss", less: "less",
    xml: "xml", html: "xml", htm: "xml", svg: "xml", vue: "xml", svelte: "xml", plist: "xml",
    yml: "yaml", yaml: "yaml",
    ini: "ini", toml: "ini", conf: "ini", cfg: "ini", env: "ini", service: "ini",
    properties: "properties", sql: "sql", diff: "diff", patch: "diff",
    nginx: "nginx", apache: "apache",
    ps1: "powershell", psm1: "powershell", psd1: "powershell"
  };
  function detectLang(path) {
    var base = String(path || "").toLowerCase().split("/").pop();
    if (base === "dockerfile" || base === "containerfile") return "dockerfile";
    if (base === "makefile") return "makefile";
    var ext = base.indexOf(".") >= 0 ? base.split(".").pop() : base;
    return EXT_LANG[ext] || "";
  }
  function currentLang() {
    var v = $("lang").value;
    if (v && v !== "auto") return v;
    return detectLang(state.editor ? state.editor.path : "");
  }
  function fillLangOptions() {
    var sel = $("lang");
    var opts = ['<option value="auto">自动（按扩展名）</option>'];
    if (hl && typeof hl.listLanguages === "function") {
      hl.listLanguages().slice().sort().forEach(function (l) {
        opts.push('<option value="' + esc(l) + '">' + esc(l) + "</option>");
      });
    }
    sel.innerHTML = opts.join("");
    sel.value = "auto";
  }

  /* ---------------- 能力面（只有 6 个动词） ---------------- */
  var API = {
    list: function (path) { return rpc("list", path ? { path: path } : {}); },
    read: function (path) { return rpc("read", { path: path }); },
    write: function (path, text) { return rpc("write", { path: path, b64: b64encode(text) }); },
    mkdir: function (path) { return rpc("mkdir", { path: path }); },
    rename: function (from, to) { return rpc("rename", { path: from, to: to }); },
    remove: function (path) { return rpc("remove", { path: path }); }
  };

  /* ---------------- 界面状态 ---------------- */
  var state = {
    cwd: "",
    parent: null,
    roots: [],
    items: [],
    filter: "",
    selection: {},
    anchor: -1,
    active: -1,
    tree: {},
    expanded: {},
    editor: null
  };
  function selectedPaths() { return Object.keys(state.selection); }
  function selectedCount() { return selectedPaths().length; }

  // 站点域名：只对 `path:/www/sites/<域名>/index` 形态的授权根生效。
  function siteOfRoot(root) {
    var m = /^\/www\/sites\/([^\/]+)\/index(?:\/|$)/.exec(root || "");
    return m ? m[1] : "";
  }
  function updateSiteLabel() {
    var el = $("site-label");
    if (!el) return;
    var names = [];
    state.roots.forEach(function (r) {
      var d = siteOfRoot(r);
      if (d && names.indexOf(d) < 0) names.push(d);
    });
    el.textContent = names.length ? names.join("、") : "站点文件";
  }
  function visibleItems() {
    if (!state.filter) return state.items;
    var f = state.filter.toLowerCase();
    return state.items.filter(function (it) { return it.name.toLowerCase().indexOf(f) >= 0; });
  }
  function itemAt(path) {
    for (var i = 0; i < state.items.length; i++) if (join(state.cwd, state.items[i].name) === path) return state.items[i];
    return null;
  }

  /* ---------------- 渲染 ---------------- */
  function sortItems(items) {
    return items.slice().sort(function (a, b) {
      if (!!a.dir !== !!b.dir) return a.dir ? -1 : 1;
      return String(a.name).localeCompare(String(b.name), "zh-Hans-CN");
    });
  }
  function rootOf(path) {
    for (var i = 0; i < state.roots.length; i++) {
      if (path === state.roots[i] || path.indexOf(state.roots[i] + "/") === 0) return state.roots[i];
    }
    return null;
  }
  function updateRoots() {
    $("roots").textContent = state.roots.length ? state.roots.join("、") : "未配置授权根";
  }
  function renderCrumbs() {
    var root = rootOf(state.cwd);
    var parts = [];
    if (root) {
      parts.push({ path: root, label: baseName(root) || root });
      var acc = root;
      state.cwd.slice(root.length).split("/").filter(Boolean).forEach(function (seg) {
        acc = join(acc, seg);
        parts.push({ path: acc, label: seg });
      });
    } else if (state.cwd) {
      parts.push({ path: state.cwd, label: state.cwd });
    }
    $("crumbs").innerHTML = parts.map(function (p, i) {
      var last = i === parts.length - 1;
      return (i ? '<span class="crumb-sep">/</span>' : "") +
        '<button class="crumb' + (last ? " last" : "") + '" data-path="' + esc(p.path) +
        '" title="' + esc(p.path) + '">' +
        (i === 0 ? '<svg class="ico" style="width:14px;height:14px"><use href="#i-folder"/></svg>' : "") +
        esc(p.label) + "</button>";
    }).join("");
  }
  function renderStatus() {
    $("status-dir").textContent = state.cwd || "-";
    var dirs = state.items.filter(function (i) { return i.dir; }).length;
    var files = state.items.length - dirs;
    var text = state.items.length + " 项（" + dirs + " 目录 / " + files + " 文件）";
    if (state.filter) text += "，筛选后 " + visibleItems().length + " 项";
    $("status-count").textContent = text;
    var n = selectedCount();
    $("status-sel").textContent = n ? "已选 " + n + " 项" : "";
    $("rename").disabled = n !== 1;
    $("remove").disabled = n === 0;
    var up = $("up");
    up.disabled = !state.parent;
    up.title = state.parent ? "上级目录（Backspace）→ " + state.parent : "已在授权根，不能继续向上";
  }
  function rowHtml(item, index) {
    var path = join(state.cwd, item.name);
    var sel = !!state.selection[path];
    return '<div class="row' + (item.dir ? " folder" : "") +
      (sel ? " selected" : "") + (index === state.active ? " active" : "") +
      '" role="option" aria-selected="' + (sel ? "true" : "false") + '" data-index="' + index +
      '" data-path="' + esc(path) + '" data-dir="' + (item.dir ? "1" : "0") + '">' +
      '<div class="cell"><svg class="ico"><use href="' + iconFor(item) + '"/></svg>' +
      '<span class="name" title="' + esc(item.name) + '">' + esc(item.name) + "</span></div>" +
      '<div class="cell size">' + (item.dir ? "—" : bytes(item.size)) + "</div>" +
      '<div class="cell time">' + when(item.mtime) + "</div></div>";
  }
  function renderList() {
    var items = visibleItems();
    var list = $("list");
    $("error-state").hidden = true;
    $("empty").hidden = !(state.items.length === 0 && !state.filter);
    if (state.items.length === 0) {
      list.innerHTML = "";
    } else if (items.length === 0) {
      list.innerHTML = '<div class="row loading" style="cursor:default;grid-template-columns:1fr">没有匹配「' +
        esc(state.filter) + '」的条目</div>';
    } else {
      list.innerHTML = items.map(rowHtml).join("");
    }
    renderStatus();
  }
  function renderQuick() {
    $("quick").innerHTML = state.roots.map(function (root) {
      var active = rootOf(state.cwd) === root;
      return '<li><button class="quick-item' + (active ? " active" : "") + '" data-path="' + esc(root) +
        '" title="' + esc(root) + '"><svg class="ico"><use href="#i-folder"/></svg>' +
        '<span style="min-width:0"><span class="name">' + esc(baseName(root) || root) + "</span>" +
        '<span class="path">' + esc(root) + "</span></span></button></li>";
    }).join("");
  }
  function renderTree() {
    function node(path, label) {
      var exp = !!state.expanded[path];
      var kids = state.tree[path] || [];
      var html = '<li><div class="tree-row' + (state.cwd === path ? " active" : "") +
        '" data-path="' + esc(path) + '" data-open="' + (exp ? "1" : "0") + '">' +
        '<button class="caret" data-toggle-path="' + esc(path) + '" aria-expanded="' + (exp ? "true" : "false") +
        '" aria-label="展开或收起"><svg class="ico"><use href="#i-caret"/></svg></button>' +
        '<svg class="ico"><use href="' + (exp ? "#i-folder-open" : "#i-folder") + '"/></svg>' +
        '<span class="name" title="' + esc(path) + '">' + esc(label) + "</span></div>";
      if (exp) {
        html += '<ul class="tree-children">' +
          (kids.length ? kids.map(function (k) { return node(join(path, k.name), k.name); }).join("")
            : '<li class="tree-row" style="cursor:default;color:var(--ui-muted)"><span class="name">（无子目录）</span></li>') +
          "</ul>";
      }
      return html + "</li>";
    }
    $("tree").innerHTML = state.roots.map(function (root) {
      return node(root, baseName(root) || root);
    }).join("");
  }
  function setBanner(text, sub) {
    var b = $("banner");
    if (!text) { b.hidden = true; b.innerHTML = ""; return; }
    b.hidden = false;
    b.innerHTML = '<svg class="ico"><use href="#i-warn"/></svg><div class="body"><b>' + esc(text) +
      "</b>" + (sub ? '<div class="sub">' + esc(sub) + "</div>" : "") + "</div>";
  }
  function showLoading() {
    $("empty").hidden = true;
    $("error-state").hidden = true;
    $("list").innerHTML = '<div class="row loading" style="cursor:default;grid-template-columns:1fr">读取目录中…</div>';
  }
  var lastErrorPath = "";
  // 站点文件插件允许「先装后授权」：没有授权根时给明确指引，而不是空白目录或泛化错误。
  function showNoGrant() {
    lastErrorPath = "";
    state.items = [];
    state.selection = {};
    state.active = -1;
    $("list").innerHTML = "";
    $("empty").hidden = true;
    $("error-state").hidden = false;
    $("error-title").textContent = "尚未配置站点目录";
    $("error-detail").textContent =
      "安装时没有添加 path:/www/sites/<域名>/index。请到「插件中心 → 站点文件 → 详情」" +
      "添加站点内容目录的 path: 授权，然后点「重试」。回收站、配额、上传下载请用内核「工具 → 文件管理」。";
    $("error-grant").hidden = false;
    $("error-back").hidden = true;
    setBanner("未配置授权根", "请先到插件详情添加站点内容目录（path:/www/sites/<域名>/index）");
    renderCrumbs();
    renderStatus();
  }
  function showError(path, err) {
    var msg = errText(err);
    lastErrorPath = path || "";
    state.items = [];
    state.selection = {};
    state.active = -1;
    $("list").innerHTML = "";
    $("empty").hidden = true;
    $("error-grant").hidden = true;
    $("error-back").hidden = false;
    $("error-state").hidden = false;
    $("error-title").textContent = /授权|forbidden|EACCES|denied|403/i.test(msg) ? "无权访问该路径" : "读取目录失败";
    $("error-detail").textContent = "请求：" + (path || "(默认目录)") + " · " + msg +
      "。本插件只能访问安装时勾选的 path: 授权根。";
    setBanner("目录不可访问", msg);
    renderCrumbs();
    renderStatus();
  }

  /* ---------------- 导航 ---------------- */
  var navToken = 0;
  function revealCwd() {
    var root = rootOf(state.cwd);
    if (!root) return Promise.resolve();
    var paths = [root];
    var acc = root;
    state.cwd.slice(root.length).split("/").filter(Boolean).forEach(function (seg) {
      acc = join(acc, seg);
      paths.push(acc);
    });
    var chain = Promise.resolve();
    paths.slice(0, Math.max(1, paths.length - 1)).forEach(function (path) {
      chain = chain.then(function () {
        state.expanded[path] = true;
        if (state.tree[path]) return;
        return API.list(path).then(function (r) {
          state.tree[path] = sortItems(r.items || []).filter(function (i) { return i.dir; });
        }).catch(function () {});
      });
    });
    return chain.then(renderTree);
  }
  function navigate(path) {
    var token = ++navToken;
    setBanner(null);
    showLoading();
    return API.list(path || "").then(function (r) {
      if (token !== navToken) return true;
      state.cwd = r.path || path;
      state.roots = r.roots && r.roots.length ? r.roots : state.roots;
      state.parent = r.parent || (rootOf(state.cwd) === state.cwd ? null : dirOf(state.cwd));
      state.items = sortItems(r.items || []);
      state.filter = "";
      $("filter").value = "";
      state.selection = {};
      state.anchor = state.items.length ? 0 : -1;
      state.active = state.items.length ? 0 : -1;
      updateRoots();
      updateSiteLabel();
      renderCrumbs();
      renderQuick();
      renderList();
      renderTree();
      $("list").focus();
      scrollActive();
      revealCwd();
      return true;
    }).catch(function (e) {
      if (token !== navToken) return true;
      // 首屏/刷新时没有授权根：走专门的「未配置」指引，避免空白页被当成故障。
      if (!state.roots.length && /授权|grant|403|forbidden/i.test(errText(e))) {
        showNoGrant();
        return false;
      }
      showError(path, e);
      return false;
    });
  }
  function scrollActive() {
    var row = $("list").querySelector('.row[data-index="' + state.active + '"]');
    if (row) row.scrollIntoView({ block: "nearest" });
  }
  function goUp() {
    if (!state.parent) {
      message("已在授权根，不能继续向上（授权之外的目录不属于本插件）", "bad");
      return;
    }
    navigate(state.parent);
  }

  /* ---------------- 选择 ---------------- */
  function applySelection() {
    var rows = $("list").querySelectorAll(".row[data-path]");
    Array.prototype.forEach.call(rows, function (row) {
      var sel = !!state.selection[row.getAttribute("data-path")];
      row.classList.toggle("selected", sel);
      row.setAttribute("aria-selected", sel ? "true" : "false");
      row.classList.toggle("active", Number(row.getAttribute("data-index")) === state.active);
    });
    renderStatus();
  }
  function selectIndex(index, opts) {
    var items = visibleItems();
    var it = items[index];
    if (!it) return;
    opts = opts || {};
    var path = join(state.cwd, it.name);
    if (opts.shift && state.anchor >= 0) {
      var a = Math.min(state.anchor, index), b = Math.max(state.anchor, index);
      var sel = {};
      for (var i = a; i <= b; i++) sel[join(state.cwd, items[i].name)] = true;
      state.selection = sel;
      state.active = index;
    } else if (opts.toggle) {
      if (state.selection[path]) delete state.selection[path];
      else state.selection[path] = true;
      state.anchor = index;
      state.active = index;
    } else {
      state.selection = {};
      state.selection[path] = true;
      state.anchor = index;
      state.active = index;
    }
    $("list").focus();
    applySelection();
    scrollActive();
  }
  function clearSelection() {
    state.selection = {};
    state.active = -1;
    applySelection();
  }
  function openPath(path) {
    var item = itemAt(path);
    if (!item) return;
    if (item.dir) navigate(path);
    else openEditor(path);
  }
  function openSelected() {
    var paths = selectedPaths();
    if (!paths.length) return;
    var active = visibleItems()[state.active];
    var path = paths.length === 1 ? paths[0] : (active ? join(state.cwd, active.name) : paths[0]);
    openPath(path);
  }

  /* ---------------- 列表事件 ---------------- */
  $("list").addEventListener("mousedown", function (ev) {
    if (ev.target.closest(".rename-input")) return;
    var row = ev.target.closest(".row[data-path]");
    if (row) {
      selectIndex(Number(row.getAttribute("data-index")), {
        toggle: ev.ctrlKey || ev.metaKey,
        shift: ev.shiftKey
      });
    } else {
      clearSelection();
    }
  });
  $("list").addEventListener("dblclick", function (ev) {
    var row = ev.target.closest(".row[data-path]");
    if (row) openPath(row.getAttribute("data-path"));
  });
  $("list").addEventListener("contextmenu", function (ev) {
    var row = ev.target.closest(".row[data-path]");
    if (!row) return;
    ev.preventDefault();
    var path = row.getAttribute("data-path");
    if (!state.selection[path]) selectIndex(Number(row.getAttribute("data-index")), {});
    showCtx(ev.clientX, ev.clientY, itemAt(path));
  });
  $("list").addEventListener("keydown", function (ev) {
    if (ev.target.closest(".rename-input")) return;
    var items = visibleItems();
    var key = ev.key;
    if ((ev.ctrlKey || ev.metaKey) && (key === "a" || key === "A")) {
      ev.preventDefault();
      state.selection = {};
      items.forEach(function (it) { state.selection[join(state.cwd, it.name)] = true; });
      state.active = items.length - 1;
      applySelection();
      return;
    }
    if (key === "ArrowDown" || key === "ArrowUp" || key === "Home" || key === "End") {
      if (!items.length) return;
      ev.preventDefault();
      var next = state.active;
      if (key === "ArrowDown") next = state.active < 0 ? 0 : Math.min(items.length - 1, state.active + 1);
      if (key === "ArrowUp") next = state.active < 0 ? 0 : Math.max(0, state.active - 1);
      if (key === "Home") next = 0;
      if (key === "End") next = items.length - 1;
      selectIndex(next, { shift: ev.shiftKey });
      return;
    }
    if (key === "Enter") {
      ev.preventDefault();
      openSelected();
      return;
    }
    if (key === "F2") {
      ev.preventDefault();
      if (selectedCount() === 1) startRename();
      else message("重命名一次只能选一个条目", "bad");
      return;
    }
    if (key === "Delete") {
      ev.preventDefault();
      if (selectedCount()) confirmRemove();
      return;
    }
    if (key === "Backspace") {
      ev.preventDefault();
      goUp();
      return;
    }
    if (key === "Escape") {
      hideCtx();
      clearSelection();
      return;
    }
    if (key === " ") {
      ev.preventDefault();
      selectIndex(state.active, { toggle: true });
    }
  });

  /* ---------------- 右键菜单 ---------------- */
  function showCtx(x, y, item) {
    var menu = $("ctx");
    var n = selectedCount();
    menu.querySelector('[data-act="open"]').disabled = n !== 1;
    menu.querySelector('[data-act="rename"]').disabled = n !== 1;
    menu.querySelector('[data-act="remove"]').disabled = n === 0;
    menu.querySelector('[data-act="open"] span').textContent = item && item.dir ? "进入目录" : "编辑文件";
    menu.hidden = false;
    var rect = menu.getBoundingClientRect();
    var left = Math.min(x, window.innerWidth - rect.width - 8);
    var top = Math.min(y, window.innerHeight - rect.height - 8);
    menu.style.left = Math.max(8, left) + "px";
    menu.style.top = Math.max(8, top) + "px";
  }
  function hideCtx() { $("ctx").hidden = true; }
  document.addEventListener("click", function (ev) {
    if (!ev.target.closest("#ctx")) hideCtx();
  });
  document.addEventListener("keydown", function (ev) {
    if (ev.key === "Escape") hideCtx();
  });
  $("ctx").addEventListener("click", function (ev) {
    var btn = ev.target.closest(".ctx-item");
    if (!btn) return;
    var act = btn.getAttribute("data-act");
    hideCtx();
    if (act === "open") openSelected();
    if (act === "rename") startRename();
    if (act === "mkdir") openNewFolder();
    if (act === "refresh") navigate(state.cwd);
    if (act === "remove") confirmRemove();
  });

  /* ---------------- 重命名（行内） ---------------- */
  function rowFor(path) {
    var rows = $("list").querySelectorAll(".row[data-path]");
    var found = null;
    Array.prototype.forEach.call(rows, function (r) {
      if (r.getAttribute("data-path") === path) found = r;
    });
    return found;
  }
  function startRename() {
    var paths = selectedPaths();
    if (paths.length !== 1) {
      message("重命名一次只能选一个条目", "bad");
      return;
    }
    var path = paths[0];
    var row = rowFor(path);
    if (!row || row.querySelector(".rename-input")) return;
    var name = baseName(path);
    var input = document.createElement("input");
    input.type = "text";
    input.className = "rename-input";
    input.value = name;
    input.setAttribute("aria-label", "新名称");
    input.spellcheck = false;
    var span = row.querySelector(".name");
    span.replaceWith(input);
    var dot = name.lastIndexOf(".");
    input.focus();
    input.setSelectionRange(0, dot > 0 ? dot : name.length);

    var done = false;
    function finish(commit) {
      if (done) return;
      done = true;
      var next = input.value.trim();
      if (!commit || next === name) {
        renderList();
        applySelection();
        $("list").focus();
        return;
      }
      if (badName(next)) {
        message("名称非法：不能为空、不能包含 / 或 ..", "bad");
        done = false;
        input.focus();
        return;
      }
      var to = join(dirOf(path), next);
      API.rename(path, to).then(function () {
        message("已重命名为 " + next);
        return navigate(state.cwd).then(function () { selectByPath(to); });
      }).catch(function (e) {
        message("重命名失败：" + errText(e), "bad");
        done = false;
        renderList();
        applySelection();
      });
    }
    input.addEventListener("keydown", function (ev) {
      if (ev.key === "Enter") { ev.preventDefault(); ev.stopPropagation(); finish(true); }
      else if (ev.key === "Escape") { ev.preventDefault(); ev.stopPropagation(); finish(false); }
    });
    input.addEventListener("blur", function () { finish(false); });
    input.addEventListener("mousedown", function (ev) { ev.stopPropagation(); });
    input.addEventListener("dblclick", function (ev) { ev.stopPropagation(); });
  }
  function selectByPath(path) {
    var items = visibleItems();
    for (var i = 0; i < items.length; i++) {
      if (join(state.cwd, items[i].name) === path) {
        state.selection = {};
        state.selection[path] = true;
        state.anchor = i;
        state.active = i;
        applySelection();
        scrollActive();
        $("list").focus();
        return;
      }
    }
  }

  /* ---------------- 新建文件夹 / 跳转 / 确认 ---------------- */
  var newParentPath = "";
  function openNewFolder() {
    newParentPath = state.cwd;
    $("new-parent").textContent = state.cwd || "-";
    $("new-name").value = "";
    $("new-err").textContent = "";
    $("dlg-new").showModal();
    setTimeout(function () { $("new-name").focus(); }, 30);
  }
  function openJump() {
    $("jump-path").value = state.cwd;
    $("jump-err").textContent = "";
    $("dlg-jump").showModal();
    setTimeout(function () {
      $("jump-path").focus();
      $("jump-path").select();
    }, 30);
  }
  var confirmOnOk = null;
  function askConfirm(opts) {
    $("confirm-title").textContent = opts.title || "确认操作";
    $("confirm-text").innerHTML = opts.text || "";
    $("confirm-ok").textContent = opts.okText || "确定";
    $("confirm-ok").className = "btn " + (opts.danger ? "danger" : "primary");
    $("confirm-icon").style.color = opts.danger ? "var(--ui-bad)" : "var(--ui-warn)";
    confirmOnOk = opts.onOk || null;
    // 已有确认框时就复用，重复 showModal 会抛 InvalidStateError 并让本次关闭落空。
    if (!$("dlg-confirm").open) $("dlg-confirm").showModal();
  }
  function confirmRemove() {
    var paths = selectedPaths();
    if (!paths.length) return;
    var single = itemAt(paths[0]);
    var text;
    if (paths.length === 1) {
      text = "将永久删除 " + (single && single.dir ? "目录（含其中全部内容）" : "文件") +
        " <code>" + esc(paths[0]) + "</code>，不进回收站，操作不可恢复。";
    } else {
      text = "将永久删除以下 " + paths.length + " 个条目（目录会连同内容一起删除），不进回收站，不可恢复：<ul>" +
        paths.slice(0, 8).map(function (p) { return "<li><code>" + esc(baseName(p)) + "</code></li>"; }).join("") +
        (paths.length > 8 ? "<li>…</li>" : "") + "</ul>";
    }
    askConfirm({
      title: paths.length === 1 ? "永久删除条目" : "永久删除 " + paths.length + " 个条目",
      text: text,
      okText: "永久删除",
      danger: true,
      onOk: function () {
        var removed = paths.slice();
        var chain = Promise.resolve();
        removed.forEach(function (p) {
          chain = chain.then(function () { return API.remove(p); });
        });
        chain.then(function () {
          if (state.editor && removed.indexOf(state.editor.path) >= 0) closeEditorForce();
          message("已永久删除 " + removed.length + " 项");
          return navigate(state.cwd);
        }).catch(function (e) {
          message("删除失败：" + errText(e), "bad");
          navigate(state.cwd);
        });
      }
    });
  }

  /* ---------------- 编辑器（行号 + 等宽 + 未保存） ---------------- */
  function highlightText(text) {
    var lang = currentLang();
    if (!hl) return esc(text);
    try {
      if (lang && hl.getLanguage(lang)) {
        return hl.highlight(text, { language: lang, ignoreIllegals: true }).value;
      }
      if (text.length > 100000) return esc(text);
      return hl.highlightAuto(text).value;
    } catch (e) {
      return esc(text);
    }
  }
  function paintEditor() {
    var ta = $("code-area");
    var text = ta.value;
    var lines = text.split("\n").length;
    var gutter = $("gutter");
    if (gutter.childElementCount !== lines) {
      var html = "";
      for (var i = 1; i <= lines; i++) html += "<div>" + i + "</div>";
      gutter.innerHTML = html;
    }
    $("hl-code").innerHTML = highlightText(text) + (text === "" || text.endsWith("\n") ? "\n" : "");
    $("ed-stat").textContent = lines + " 行 · " + text.length + " 字符";
    syncEditorScroll();
    updateDirty();
  }
  function syncEditorScroll() {
    var ta = $("code-area");
    $("gutter").style.transform = "translateY(-" + ta.scrollTop + "px)";
    $("code-hl").scrollTop = ta.scrollTop;
    $("code-hl").scrollLeft = ta.scrollLeft;
  }
  function updateDirty() {
    var ed = state.editor;
    if (!ed) return;
    var dirty = !ed.readOnly && $("code-area").value !== ed.orig;
    ed.dirty = dirty;
    $("ed-dirty").hidden = !dirty;
    $("ed-save").disabled = !!ed.readOnly;
  }
  function openEditor(path) {
    API.read(path).then(function (r) {
      var text = b64decode(r.b64);
      var readOnly = !r.eof;
      state.editor = {
        path: r.path || path,
        name: baseName(r.path || path),
        orig: text,
        readOnly: readOnly,
        size: Number(r.size || 0),
        dirty: false
      };
      $("ed-name").textContent = state.editor.name;
      $("ed-path").textContent = state.editor.path;
      $("ed-path").title = state.editor.path;
      $("code-area").value = text;
      $("code-area").readOnly = readOnly;
      $("ed-save").disabled = readOnly;
      var banner = $("ed-banner");
      if (readOnly) {
        banner.hidden = false;
        banner.innerHTML = '<svg class="ico"><use href="#i-warn"/></svg><div class="body"><b>文件 ' +
          bytes(state.editor.size) + '，超过单次 256 KiB 读取上限</b><div class="sub">只显示第一页，内容只读；大文件请用 SSH 或系统工具处理。</div></div>';
        $("ed-limits").textContent = "超过 256 KiB：只读";
      } else {
        banner.hidden = true;
        banner.innerHTML = "";
        $("ed-limits").textContent = "";
      }
      syncLangFromPath(state.editor.path);
      paintEditor();
      if (!$("dlg-editor").open) $("dlg-editor").showModal();
      $("code-area").focus();
      $("code-area").setSelectionRange(0, 0);
      $("code-area").scrollTop = 0;
      syncEditorScroll();
    }).catch(function (e) {
      message("打开失败：" + errText(e), "bad");
    });
  }
  function closeEditorForce() {
    state.editor = null;
    var dlg = $("dlg-editor");
    if (dlg.open) dlg.close();
    // 关闭后把焦点还给列表，方向键/Enter 能接着用。
    $("list").focus();
  }
  function requestCloseEditor() {
    var ed = state.editor;
    if (ed && ed.dirty && !ed.readOnly) {
      askConfirm({
        title: "放弃未保存的修改？",
        text: "<code>" + esc(ed.name) + "</code> 有未保存的修改，关闭后这些内容会丢失。",
        okText: "放弃修改",
        danger: true,
        onOk: closeEditorForce
      });
      return;
    }
    closeEditorForce();
  }
  function syncLangFromPath(path) {
    var guess = detectLang(path);
    $("lang").value = guess || "auto";
    if (!$("lang").value) $("lang").value = "auto";
  }
  function saveEditor() {
    var ed = state.editor;
    if (!ed) return;
    if (ed.readOnly) {
      message("超过 256 KiB，当前为只读，不能保存", "bad");
      return;
    }
    var text = $("code-area").value;
    API.write(ed.path, text).then(function (r) {
      ed.orig = text;
      ed.size = Number(r.size || 0);
      updateDirty();
      message("已保存 " + ed.name);
      return API.list(state.cwd).then(function (res) {
        state.items = sortItems(res.items || []);
        renderList();
        applySelection();
      });
    }).catch(function (e) {
      message("保存失败：" + errText(e), "bad");
    });
  }

  var paintTimer = 0;
  $("code-area").addEventListener("input", function () {
    updateDirty();
    if (paintTimer) clearTimeout(paintTimer);
    paintTimer = setTimeout(paintEditor, 120);
  });
  $("code-area").addEventListener("scroll", syncEditorScroll);
  $("code-area").addEventListener("keydown", function (ev) {
    if ((ev.ctrlKey || ev.metaKey) && (ev.key === "s" || ev.key === "S")) {
      ev.preventDefault();
      saveEditor();
      return;
    }
    if (ev.key === "Tab" && !ev.ctrlKey && !ev.altKey) {
      var ta = $("code-area");
      if (ta.readOnly) return;
      ev.preventDefault();
      ta.setRangeText("  ", ta.selectionStart, ta.selectionEnd, "end");
      updateDirty();
      paintEditor();
    }
  });
  $("lang").addEventListener("change", paintEditor);

  /* ---------------- 事件接线 ---------------- */
  $("up").addEventListener("click", goUp);
  $("refresh").addEventListener("click", function () { navigate(state.cwd); });
  $("mkdir").addEventListener("click", openNewFolder);
  $("empty-mkdir").addEventListener("click", openNewFolder);
  $("rename").addEventListener("click", startRename);
  $("remove").addEventListener("click", confirmRemove);
  $("jump").addEventListener("click", openJump);
  $("filter").addEventListener("input", function () {
    state.filter = this.value.trim();
    state.active = visibleItems().length ? 0 : -1;
    renderList();
  });
  $("crumbs").addEventListener("click", function (ev) {
    var crumb = ev.target.closest(".crumb[data-path]");
    if (crumb) navigate(crumb.getAttribute("data-path"));
  });
  $("quick").addEventListener("click", function (ev) {
    var item = ev.target.closest(".quick-item[data-path]");
    if (item) navigate(item.getAttribute("data-path"));
  });
  $("tree").addEventListener("click", function (ev) {
    var row = ev.target.closest(".tree-row[data-path]");
    if (!row) return;
    var path = row.getAttribute("data-path");
    if (ev.target.closest(".caret")) {
      ev.stopPropagation();
      toggleTree(path);
      return;
    }
    navigate(path);
  });
  function toggleTree(path) {
    if (state.expanded[path]) {
      delete state.expanded[path];
      renderTree();
      return;
    }
    API.list(path).then(function (r) {
      state.tree[path] = sortItems(r.items || []).filter(function (i) { return i.dir; });
      state.expanded[path] = true;
      renderTree();
    }).catch(function (e) {
      message("展开失败：" + errText(e), "bad");
    });
  }
  $("error-back").addEventListener("click", function () {
    if (state.roots.length) navigate(state.roots[0]);
  });
  var kernelLink = $("kernel-link");
  if (kernelLink) kernelLink.href = "/" + prefix + "/files";
  var errorGrant = $("error-grant");
  if (errorGrant) errorGrant.href = "/" + prefix + "/plugins/com.skiff.files";
  $("error-retry").addEventListener("click", function () { navigate(lastErrorPath); });

  Array.prototype.forEach.call(document.querySelectorAll("[data-close]"), function (btn) {
    btn.addEventListener("click", function () { btn.closest("dialog").close(); });
  });
  $("new-form").addEventListener("submit", function (ev) {
    ev.preventDefault();
    var name = $("new-name").value.trim();
    if (badName(name)) {
      $("new-err").textContent = "名称非法：不能为空、不能包含 / 或 ..";
      return;
    }
    var path = join(newParentPath, name);
    API.mkdir(path).then(function () {
      $("dlg-new").close();
      message("已创建目录 " + name);
      return navigate(newParentPath).then(function () { selectByPath(path); });
    }).catch(function (e) {
      $("new-err").textContent = errText(e);
    });
  });
  $("jump-form").addEventListener("submit", function (ev) {
    ev.preventDefault();
    var path = $("jump-path").value.trim();
    if (!path) {
      $("jump-err").textContent = "请输入路径";
      return;
    }
    $("dlg-jump").close();
    navigate(path);
  });
  $("confirm-cancel").addEventListener("click", function () {
    confirmOnOk = null;
    $("dlg-confirm").close();
  });
  $("confirm-ok").addEventListener("click", function () {
    var cb = confirmOnOk;
    confirmOnOk = null;
    $("dlg-confirm").close();
    if (cb) cb();
  });
  $("ed-close").addEventListener("click", requestCloseEditor);
  $("ed-save").addEventListener("click", saveEditor);
  // 点遮罩关闭（事件目标是 dialog 自身）：有未保存内容时先走确认，不静默丢内容。
  $("dlg-editor").addEventListener("click", function (ev) {
    if (ev.target === this) requestCloseEditor();
  });
  $("dlg-editor").addEventListener("cancel", function (ev) {
    ev.preventDefault();
    requestCloseEditor();
  });
  window.addEventListener("resize", hideCtx);

  /* ---------------- 启动 ---------------- */
  fillLangOptions();
  $("up").disabled = true;
  navigate(openHint).then(function (ok) {
    if (ok || !openHint) return;
    // 跨插件跳转可能给的是文件路径：先打开父目录，再进编辑器。
    var parent = dirOf(openHint);
    return navigate(parent).then(function () {
      openEditor(openHint);
    });
  });
})();
