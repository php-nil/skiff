/* 内核「文件管理」页前端：左目录树 + 右文件列表。
   只调用内核 `/files/api/*`，授权、原子写、路径锁、回收站全部由 `skiff-fs` 完成。
   前端不做本地权限判定，也不在 Web 层直接拼路径做 IO。
   阶段一覆盖：目录树、面包屑、列表与元数据、新建、上传、下载、重命名、删除（回收站）、编辑。 */
(function () {
  "use strict";

  var page = document.querySelector("[data-files]");
  if (!page) return;

  var prefix = page.getAttribute("data-prefix") || "";
  var csrf = page.getAttribute("data-csrf") || "";
  var chunkSize = parseInt(page.getAttribute("data-chunk"), 10) || 262144;
  var maxEdit = parseInt(page.getAttribute("data-max-edit"), 10) || 262144;

  var el = {
    alert: page.querySelector("[data-files-alert]"),
    alertText: page.querySelector("[data-files-alert-text]"),
    alertClose: page.querySelector("[data-files-alert-close]"),
    body: page.querySelector("[data-files-body]"),
    tree: page.querySelector("[data-files-tree]"),
    treeClose: page.querySelector("[data-files-tree-close]"),
    treeOpenBtn: page.querySelector("[data-files-tree-open-btn]"),
    treeBackdrop: page.querySelector("[data-files-tree-backdrop]"),
    treeRoots: page.querySelector("[data-files-tree-roots]") || page.querySelector(".files-tree-roots"),
    crumbs: page.querySelector("[data-files-crumbs]"),
    search: page.querySelector("[data-files-search]"),
    sort: page.querySelector("[data-files-sort]"),
    order: page.querySelector("[data-files-order]"),
    hidden: page.querySelector("[data-files-hidden]"),
    refresh: page.querySelector("[data-files-refresh]"),
    up: page.querySelector("[data-files-up]"),
    list: page.querySelector("[data-files-list]"),
    selectAll: page.querySelector("[data-files-select-all]"),
    batch: page.querySelector("[data-files-batch]"),
    batchCount: page.querySelector("[data-files-batch-count]"),
    batchClear: page.querySelector("[data-files-batch-clear]"),
    batchDelete: page.querySelector("[data-files-batch-delete]"),
    batchArchive: page.querySelector("[data-files-batch-archive]"),
    batchCopy: page.querySelector("[data-files-batch-copy]"),
    batchMove: page.querySelector("[data-files-batch-move]"),
    chmodDialog: page.querySelector("[data-files-chmod]"),
    chmodPath: page.querySelector("[data-files-chmod-path]"),
    chmodPreset: page.querySelector("[data-files-chmod-preset]"),
    chmodInput: page.querySelector("[data-files-chmod-input]"),
    chmodError: page.querySelector("[data-files-chmod-error]"),
    chmodOk: page.querySelector("[data-files-chmod-ok]"),
    chmodCancel: page.querySelector("[data-files-chmod-cancel]"),
    chownDialog: page.querySelector("[data-files-chown]"),
    chownPath: page.querySelector("[data-files-chown-path]"),
    chownUid: page.querySelector("[data-files-chown-uid]"),
    chownGid: page.querySelector("[data-files-chown-gid]"),
    chownWarn: page.querySelector("[data-files-chown-warn]"),
    chownError: page.querySelector("[data-files-chown-error]"),
    chownOk: page.querySelector("[data-files-chown-ok]"),
    chownCancel: page.querySelector("[data-files-chown-cancel]"),
    archiveDialog: page.querySelector("[data-files-archive-dialog]"),
    archiveDir: page.querySelector("[data-files-archive-dir]"),
    archiveSources: page.querySelector("[data-files-archive-sources]"),
    archiveName: page.querySelector("[data-files-archive-name]"),
    archiveFormat: page.querySelector("[data-files-archive-format]"),
    archiveOverwrite: page.querySelector("[data-files-archive-overwrite]"),
    archiveError: page.querySelector("[data-files-archive-error]"),
    archiveOk: page.querySelector("[data-files-archive-ok]"),
    archiveCancel: page.querySelector("[data-files-archive-cancel]"),
    extractDialog: page.querySelector("[data-files-extract-dialog]"),
    extractArchive: page.querySelector("[data-files-extract-archive]"),
    extractDest: page.querySelector("[data-files-extract-dest]"),
    extractPick: page.querySelector("[data-files-extract-pick]"),
    extractOverwrite: page.querySelector("[data-files-extract-overwrite]"),
    extractError: page.querySelector("[data-files-extract-error]"),
    extractOk: page.querySelector("[data-files-extract-ok]"),
    extractCancel: page.querySelector("[data-files-extract-cancel]"),
    pickDialog: page.querySelector("[data-files-pick]"),
    pickTitle: page.querySelector("[data-files-pick-title]"),
    pickCrumbs: page.querySelector("[data-files-pick-crumbs]"),
    pickUp: page.querySelector("[data-files-pick-up]"),
    pickList: page.querySelector("[data-files-pick-list]"),
    pickCurrent: page.querySelector("[data-files-pick-current]"),
    pickOk: page.querySelector("[data-files-pick-ok]"),
    pickCancel: page.querySelector("[data-files-pick-cancel]"),
    empty: page.querySelector("[data-files-empty]"),
    emptyTitle: page.querySelector("[data-files-empty-title]"),
    emptyHint: page.querySelector("[data-files-empty-hint]"),
    emptyActions: page.querySelector(".files-empty .actions"),
    emptyUpload: page.querySelector("[data-files-empty-upload]"),
    emptyNewDir: page.querySelector("[data-files-empty-new-dir]"),
    status: page.querySelector("[data-files-status]"),
    summary: page.querySelector("[data-files-summary]"),
    nameDialog: page.querySelector("[data-files-name-dialog]"),
    nameForm: page.querySelector("[data-files-name-form]"),
    nameTitle: page.querySelector("[data-files-name-title]"),
    nameLabel: page.querySelector("[data-files-name-label]"),
    nameInput: page.querySelector("[data-files-name-input]"),
    nameHint: page.querySelector("[data-files-name-hint]"),
    nameError: page.querySelector("[data-files-name-error]"),
    nameCancel: page.querySelector("[data-files-name-cancel]"),
    uploadOpen: page.querySelector("[data-files-upload-open]"),
    uploadDialog: page.querySelector("[data-files-upload-dialog]"),
    uploadDir: page.querySelector("[data-files-upload-dir]"),
    uploadInput: page.querySelector("[data-files-upload-input]"),
    uploadLimit: page.querySelector("[data-files-upload-limit]"),
    uploadList: page.querySelector("[data-files-upload-list]"),
    uploadError: page.querySelector("[data-files-upload-error]"),
    uploadClose: page.querySelector("[data-files-upload-close]"),
    uploadStart: page.querySelector("[data-files-upload-start]"),
    editor: page.querySelector("[data-files-editor]"),
    editorPath: page.querySelector("[data-files-editor-path]"),
    editorDirty: page.querySelector("[data-files-editor-dirty]"),
    editorReadonly: page.querySelector("[data-files-editor-readonly]"),
    editorText: page.querySelector("[data-files-editor-text]"),
    editorSave: page.querySelector("[data-files-editor-save]"),
    editorCancel: page.querySelector("[data-files-editor-cancel]"),
    editorX: page.querySelector("[data-files-editor-x]"),
    editorStatus: page.querySelector("[data-files-editor-status]"),
    editorStats: page.querySelector("[data-files-editor-stats]"),
    confirm: page.querySelector("[data-files-confirm]"),
    confirmTitle: page.querySelector("[data-files-confirm-title]"),
    confirmMessage: page.querySelector("[data-files-confirm-message]"),
    confirmOk: page.querySelector("[data-files-confirm-ok]"),
    confirmCancel: page.querySelector("[data-files-confirm-cancel]")
  };

  var state = {
    path: page.getAttribute("data-start") || "/",
    entries: [],
    truncated: false,
    selected: Object.create(null),
    sortKey: "name",
    sortDir: 1,
    showHidden: false,
    filter: "",
    loading: false,
    menu: null,
    editorPath: null,
    editorOriginal: "",
    editorReadonly: false,
    editing: false,
    uploadBusy: false,
    uploadAbort: false
  };

  var ICON_DIR = '<svg class="files-ico" viewBox="0 0 24 24" aria-hidden="true"><use href="#i-folder"></use></svg>';
  var ICON_FILE = '<svg class="files-ico" viewBox="0 0 24 24" aria-hidden="true"><use href="#i-file"></use></svg>';
  var ICON_ARCHIVE = '<svg class="files-ico" viewBox="0 0 24 24" aria-hidden="true"><use href="#i-archive"></use></svg>';

  // ---- 基础工具 ---------------------------------------------------------

  function apiUrl(path) {
    return "/" + prefix + "/files/api" + path;
  }

  function joinPath(dir, name) {
    if (!dir || dir === "/") return "/" + name;
    return dir.replace(/\/+$/, "") + "/" + name;
  }

  function parentPath(path) {
    var s = path.replace(/\/+$/, "");
    var i = s.lastIndexOf("/");
    return i <= 0 ? "/" : s.slice(0, i);
  }

  function baseName(path) {
    var s = path.replace(/\/+$/, "");
    var i = s.lastIndexOf("/");
    return i < 0 ? s : s.slice(i + 1);
  }

  function isUnder(path, root) {
    if (path === root) return true;
    var r = root.replace(/\/+$/, "");
    return path.indexOf(r + "/") === 0;
  }

  function parseJSON(raw) {
    try {
      return raw ? JSON.parse(raw) : null;
    } catch (e) {
      return null;
    }
  }

  function requestError(res, data) {
    var body = (data && data.error) || {};
    var err = new Error(body.message || "请求失败（HTTP " + res.status + "）");
    err.code = body.code || "http." + res.status;
    return err;
  }

  function api(path, options) {
    options = options || {};
    options.credentials = "same-origin";
    options.headers = options.headers || {};
    if (options.json !== undefined) {
      options.headers["content-type"] = "application/json";
      options.body = JSON.stringify(options.json);
      delete options.json;
    }
    return fetch(apiUrl(path), options).then(function (res) {
      return res.text().then(function (raw) {
        var data = parseJSON(raw);
        if (!res.ok || !data || data.ok === false) throw requestError(res, data);
        return data;
      });
    });
  }

  function post(path, payload) {
    return api(path, {
      method: "POST",
      headers: { "x-csrf-token": csrf },
      json: payload || {}
    });
  }

  function errorText(e) {
    var code = (e && e.code) || "files.request_failed";
    var msg = (e && e.message) || "请求失败";
    return code + " · " + msg;
  }

  function formatSize(n) {
    if (n === null || n === undefined) return "—";
    if (n < 1024) return n + " B";
    var units = ["B", "KB", "MB", "GB", "TB"];
    var i = 0;
    var v = n;
    while (v >= 1024 && i < units.length - 1) {
      v /= 1024;
      i += 1;
    }
    return (v >= 10 || i === 0 ? Math.round(v) : v.toFixed(1)) + " " + units[i];
  }

  function formatMode(mode) {
    if (mode === null || mode === undefined) return "—";
    return ("000" + (mode & 0o7777).toString(8)).slice(-4);
  }

  function formatTime(ts) {
    if (!ts) return "—";
    var d = new Date(ts * 1000);
    if (isNaN(d.getTime())) return "—";
    var pad = function (x) { return x < 10 ? "0" + x : "" + x; };
    return d.getFullYear() + "-" + pad(d.getMonth() + 1) + "-" + pad(d.getDate()) +
      " " + pad(d.getHours()) + ":" + pad(d.getMinutes());
  }

  function archiveName(name) {
    var n = name.toLowerCase();
    return /\.(zip|tar|tgz|tar\.gz|tar\.bz2|tar\.xz|7z|rar|gz)$/.test(n);
  }

  function fileIcon(name) {
    return archiveName(name) ? ICON_ARCHIVE : ICON_FILE;
  }

  function forceVisible(node, flag) {
    if (!node) return;
    if (flag) node.removeAttribute("hidden");
    else node.setAttribute("hidden", "");
  }

  function showAlert(message, kind) {
    if (!el.alert) return;
    el.alertText.textContent = message;
    el.alert.className = "files-alert" + (kind ? " " + kind : "");
    el.alert.hidden = false;
  }

  function clearAlert() { if (el.alert) el.alert.hidden = true; }

  function setStatus(text) { if (el.status) el.status.textContent = text; }

  // ---- 通用确认 / 名称输入弹窗 ------------------------------------------

  var confirmResolve = null;
  var nameResolve = null;

  function confirmAction(opts) {
    opts = opts || {};
    return new Promise(function (resolve) {
      confirmResolve = resolve;
      el.confirmTitle.textContent = opts.title || "请确认操作";
      el.confirmMessage.textContent = opts.message || "";
      el.confirmOk.textContent = opts.okText || "确认执行";
      el.confirmOk.className = "btn " + (opts.plain ? "primary" : "danger");
      if (typeof el.confirm.showModal === "function") el.confirm.showModal();
      else el.confirm.setAttribute("open", "");
    });
  }

  function settleConfirm(value) {
    var resolve = confirmResolve;
    confirmResolve = null;
    if (el.confirm.open && typeof el.confirm.close === "function") el.confirm.close();
    else el.confirm.removeAttribute("open");
    if (resolve) resolve(value);
  }

  el.confirmOk.addEventListener("click", function () { settleConfirm(true); });
  el.confirmCancel.addEventListener("click", function () { settleConfirm(false); });
  el.confirm.addEventListener("cancel", function (e) {
    e.preventDefault();
    settleConfirm(false);
  });

  function promptName(opts) {
    opts = opts || {};
    return new Promise(function (resolve) {
      nameResolve = resolve;
      el.nameTitle.textContent = opts.title || "新建";
      el.nameLabel.textContent = opts.label || "名称";
      el.nameHint.textContent = opts.hint || "";
      el.nameError.hidden = true;
      el.nameInput.value = opts.value || "";
      el.nameInput.placeholder = opts.placeholder || "";
      if (typeof el.nameDialog.showModal === "function") el.nameDialog.showModal();
      else el.nameDialog.setAttribute("open", "");
      setTimeout(function () { el.nameInput.focus(); el.nameInput.select(); }, 0);
    });
  }

  function settleName(value) {
    var resolve = nameResolve;
    nameResolve = null;
    if (el.nameDialog.open && typeof el.nameDialog.close === "function") el.nameDialog.close();
    else el.nameDialog.removeAttribute("open");
    if (resolve) resolve(value);
  }

  function validName(name) {
    if (!name) return "名称不能为空";
    if (name.indexOf("/") >= 0) return "名称不能包含 /";
    if (name === "." || name === "..") return "名称不能是 . 或 ..";
    if (name.length > 255) return "名称不能超过 255 个字符";
    return null;
  }

  el.nameForm.addEventListener("submit", function (e) {
    e.preventDefault();
    var value = el.nameInput.value.trim();
    var bad = validName(value);
    if (bad) {
      el.nameError.textContent = bad;
      el.nameError.hidden = false;
      el.nameInput.focus();
      return;
    }
    settleName(value);
  });
  el.nameCancel.addEventListener("click", function () { settleName(null); });
  el.nameDialog.addEventListener("cancel", function (e) {
    e.preventDefault();
    settleName(null);
  });

  // ---- 目录树 -----------------------------------------------------------

  function treeLabel(node) {
    return node.querySelector(":scope > .files-tree-row > .files-tree-label");
  }

  function treeToggle(node) {
    return node.querySelector(":scope > .files-tree-row > .files-tree-toggle");
  }

  function treeChildren(node) {
    return node.querySelector(":scope > .files-tree-children");
  }

  function markTreeActive(path) {
    Array.prototype.forEach.call(page.querySelectorAll(".files-tree-label.on"), function (btn) {
      btn.classList.remove("on");
    });
    Array.prototype.forEach.call(page.querySelectorAll(".files-tree-node"), function (node) {
      if (node.getAttribute("data-path") === path) {
        var label = treeLabel(node);
        if (label) label.classList.add("on");
      }
    });
  }

  function setTreeOpen(node, open) {
    node.classList.toggle("open", open);
    var toggle = treeToggle(node);
    if (toggle) toggle.setAttribute("aria-expanded", open ? "true" : "false");
    var children = treeChildren(node);
    if (children) forceVisible(children, open);
  }

  function buildTreeChildren(node, dirs) {
    var ul = treeChildren(node);
    if (!ul) return;
    ul.textContent = "";
    dirs.forEach(function (name) {
      var path = joinPath(node.getAttribute("data-path"), name);
      var li = document.createElement("li");
      li.className = "files-tree-node";
      li.setAttribute("data-path", path);
      li.setAttribute("data-label", name);

      var row = document.createElement("div");
      row.className = "files-tree-row";

      var toggle = document.createElement("button");
      toggle.type = "button";
      toggle.className = "files-tree-toggle";
      toggle.setAttribute("data-files-tree-toggle", "");
      toggle.setAttribute("aria-expanded", "false");
      toggle.setAttribute("aria-label", "展开或收起");
      toggle.disabled = false;
      toggle.innerHTML = '<svg viewBox="0 0 24 24" aria-hidden="true"><use href="#i-chevron"></use></svg>';

      var label = document.createElement("button");
      label.type = "button";
      label.className = "files-tree-label";
      label.setAttribute("data-files-tree-open", "");
      label.title = name;
      label.innerHTML = ICON_DIR;
      var text = document.createElement("span");
      text.className = "files-tree-text";
      text.textContent = name;
      label.appendChild(text);

      var kids = document.createElement("ul");
      kids.className = "files-tree-children";
      kids.setAttribute("hidden", "");

      row.appendChild(toggle);
      row.appendChild(label);
      li.appendChild(row);
      li.appendChild(kids);
      ul.appendChild(li);
    });

    var toggle = treeToggle(node);
    if (toggle) toggle.disabled = dirs.length === 0;
    if (dirs.length === 0) {
      var empty = document.createElement("li");
      empty.className = "files-tree-none";
      empty.textContent = "无子目录";
      ul.appendChild(empty);
    }
  }

  function loadTreeChildren(node) {
    if (node.getAttribute("data-loaded") === "1") return Promise.resolve();
    var ul = treeChildren(node);
    if (ul && ul.children.length === 0) {
      ul.textContent = "";
      var loading = document.createElement("li");
      loading.className = "files-tree-loading";
      loading.textContent = "加载中…";
      ul.appendChild(loading);
      forceVisible(ul, true);
    }
    return api("/tree?path=" + encodeURIComponent(node.getAttribute("data-path")))
      .then(function (data) {
        node.setAttribute("data-loaded", "1");
        if (ul) ul.textContent = "";
        buildTreeChildren(node, (data.dirs || []).slice().sort(function (a, b) {
          return a.localeCompare(b, "zh");
        }));
      })
      .catch(function (e) {
        if (ul) {
          ul.textContent = "";
          var fail = document.createElement("li");
          fail.className = "files-tree-none";
          fail.textContent = "展开失败：" + (e.message || "未知错误");
          ul.appendChild(fail);
        }
      });
  }

  function toggleTreeNode(node) {
    var open = node.classList.contains("open");
    if (open) {
      setTreeOpen(node, false);
      return Promise.resolve();
    }
    setTreeOpen(node, true);
    return loadTreeChildren(node);
  }

  function expandToNode(node, target) {
    setTreeOpen(node, true);
    if (node.getAttribute("data-path") === target) return Promise.resolve();
    return loadTreeChildren(node).then(function () {
      var next = null;
      Array.prototype.forEach.call(
        node.querySelectorAll(":scope > .files-tree-children > .files-tree-node"),
        function (child) {
          if (!next && isUnder(target, child.getAttribute("data-path"))) next = child;
        }
      );
      if (next) return expandToNode(next, target);
      return null;
    });
  }

  function revealInTree(path) {
    var roots = page.querySelectorAll(".files-tree-roots > .files-tree-node");
    var chosen = null;
    Array.prototype.forEach.call(roots, function (root) {
      if (!chosen && isUnder(path, root.getAttribute("data-path"))) chosen = root;
    });
    if (!chosen) return Promise.resolve();
    return expandToNode(chosen, path).catch(function () { return null; });
  }

  function closeTreeDrawer() {
    page.classList.remove("files-tree-open");
    forceVisible(el.treeBackdrop, false);
    if (el.tree) el.tree.removeAttribute("aria-modal");
  }

  function openTreeDrawer() {
    page.classList.add("files-tree-open");
    forceVisible(el.treeBackdrop, true);
    if (el.tree) el.tree.setAttribute("aria-modal", "true");
  }

  el.treeRoots.addEventListener("click", function (e) {
    var toggle = e.target.closest("[data-files-tree-toggle]");
    var open = e.target.closest("[data-files-tree-open]");
    var node = e.target.closest(".files-tree-node");
    if (!node) return;
    if (toggle) {
      e.preventDefault();
      if (!toggle.disabled) toggleTreeNode(node);
      return;
    }
    if (open) {
      e.preventDefault();
      load(node.getAttribute("data-path"));
      closeTreeDrawer();
    }
  });

  // 服务端渲染的根节点默认允许展开（是否真有子目录在首次展开后判定）。
  Array.prototype.forEach.call(
    page.querySelectorAll(".files-tree-roots > .files-tree-node"),
    function (node) {
      var toggle = treeToggle(node);
      if (toggle) toggle.disabled = false;
    }
  );

  if (el.treeClose) el.treeClose.addEventListener("click", closeTreeDrawer);
  if (el.treeBackdrop) el.treeBackdrop.addEventListener("click", closeTreeDrawer);
  if (el.treeOpenBtn) el.treeOpenBtn.addEventListener("click", openTreeDrawer);

  // ---- 面包屑 -----------------------------------------------------------

  function renderCrumbs() {
    el.crumbs.textContent = "";
    var segments = state.path.split("/").filter(function (s) { return s.length > 0; });
    var acc = "";

    var rootBtn = document.createElement("button");
    rootBtn.type = "button";
    rootBtn.className = "files-crumb" + (segments.length === 0 ? " on" : "");
    rootBtn.textContent = "/";
    rootBtn.addEventListener("click", function () { load("/"); });
    el.crumbs.appendChild(rootBtn);

    segments.forEach(function (seg, idx) {
      acc += "/" + seg;
      var sep = document.createElement("span");
      sep.className = "files-crumb-sep";
      sep.textContent = "";
      el.crumbs.appendChild(sep);

      var btn = document.createElement("button");
      btn.type = "button";
      btn.className = "files-crumb" + (idx === segments.length - 1 ? " on" : "");
      btn.textContent = seg;
      var target = acc;
      if (idx !== segments.length - 1) {
        btn.addEventListener("click", function () { load(target); });
      }
      el.crumbs.appendChild(btn);
    });
  }

  // ---- 排序 / 过滤 / 渲染 ----------------------------------------------

  function visibleEntries() {
    var list = state.entries.filter(function (e) {
      if (!state.showHidden && e.name.charAt(0) === ".") return false;
      if (state.filter && e.name.toLowerCase().indexOf(state.filter) < 0) return false;
      return true;
    });
    list.sort(function (a, b) {
      if (a.dir !== b.dir) return a.dir ? -1 : 1;
      var c = 0;
      if (state.sortKey === "size") c = (a.size || 0) - (b.size || 0);
      else if (state.sortKey === "mtime") c = (a.mtime || 0) - (b.mtime || 0);
      else c = a.name.localeCompare(b.name, "zh");
      if (c === 0) c = a.name.localeCompare(b.name, "zh");
      return state.sortDir * c;
    });
    return list;
  }

  function selectedCount() {
    return Object.keys(state.selected).length;
  }

  function updateBatchBar() {
    var count = selectedCount();
    if (el.batch) forceVisible(el.batch, count > 0);
    if (el.batchCount) el.batchCount.textContent = "已选 " + count + " 项";
    Array.prototype.forEach.call(el.list.querySelectorAll("tr[data-path]"), function (tr) {
      tr.classList.toggle("is-selected", !!state.selected[tr.getAttribute("data-path")]);
    });
    updateSelectAll();
  }

  function updateSelectAll() {
    if (!el.selectAll) return;
    var visible = visibleEntries();
    var chosen = visible.filter(function (e) { return state.selected[e.path]; }).length;
    el.selectAll.checked = visible.length > 0 && chosen === visible.length;
    el.selectAll.indeterminate = chosen > 0 && chosen < visible.length;
  }

  function updateSummary(list) {
    var dirs = list.filter(function (e) { return e.dir; }).length;
    var files = list.length - dirs;
    var text = "共 " + list.length + " 项 · 目录 " + dirs + " · 文件 " + files;
    if (state.filter) text += " · 过滤：" + state.filter;
    if (state.truncated) text += " · 条目超过单页上限，仅显示前 1000 条";
    if (el.summary) el.summary.textContent = text;
  }

  function renderEmpty(visible) {
    if (!el.empty) return;
    if (visible.length > 0) {
      forceVisible(el.empty, false);
      return;
    }
    forceVisible(el.empty, true);
    if (state.entries.length === 0) {
      el.emptyTitle.textContent = "目录为空";
      el.emptyHint.textContent = "可以新建文件或目录，也可以上传文件。";
      forceVisible(el.emptyActions, true);
    } else if (state.filter) {
      el.emptyTitle.textContent = "没有匹配项";
      el.emptyHint.textContent = "当前目录没有名称包含「" + state.filter + "」的条目。";
      forceVisible(el.emptyActions, false);
    } else {
      el.emptyTitle.textContent = "没有可显示的条目";
      el.emptyHint.textContent = "当前目录只有隐藏项，勾选「显示隐藏」即可查看。";
      forceVisible(el.emptyActions, false);
    }
  }

  function openMenu(btn, entry) {
    closeMenu();
    var menu = document.createElement("div");
    menu.className = "files-menu";
    if (entry.dir) {
      addMenuItem(menu, "打开", function () { load(entry.path); });
    } else {
      addMenuItem(menu, "编辑", function () { openEditor(entry); });
      addMenuItem(menu, "下载", function () { downloadPath(entry.path); });
    }
    addMenuItem(menu, "重命名", function () { renameEntry(entry); });
    addMenuItem(menu, "权限", function () { openChmod(entry); });
    addMenuItem(menu, "属主", function () { openChown(entry); });
    addMenuItem(menu, "压缩", function () { openArchive([entry]); });
    if (!entry.dir && extractable(entry.name)) {
      addMenuItem(menu, "解压", function () { openExtract(entry); });
    }
    addMenuItem(menu, "复制到…", function () { copyOrMove([entry], false); });
    addMenuItem(menu, "移动到…", function () { copyOrMove([entry], true); });
    addMenuItem(menu, "删除", function () { removeEntries([entry]); }, true);

    menu.style.position = "fixed";
    menu.style.visibility = "hidden";
    page.appendChild(menu);
    var rect = btn.getBoundingClientRect();
    menu.style.right = Math.max(8, window.innerWidth - rect.right) + "px";
    var top = rect.bottom + 4;
    if (top + menu.offsetHeight > window.innerHeight - 8) {
      top = Math.max(8, rect.top - menu.offsetHeight - 4);
    }
    menu.style.top = top + "px";
    menu.style.visibility = "visible";
    btn.setAttribute("aria-expanded", "true");
    state.menu = { menu: menu, btn: btn };
  }

  function addMenuItem(menu, label, handler, danger) {
    var item = document.createElement("button");
    item.type = "button";
    item.textContent = label;
    if (danger) item.className = "danger";
    item.addEventListener("click", function (e) {
      e.stopPropagation();
      closeMenu();
      handler();
    });
    menu.appendChild(item);
  }

  function closeMenu() {
    if (!state.menu) return;
    if (state.menu.menu.parentNode) state.menu.menu.parentNode.removeChild(state.menu.menu);
    state.menu.btn.setAttribute("aria-expanded", "false");
    state.menu = null;
  }

  document.addEventListener("click", function (e) {
    if (!state.menu) return;
    if (e.target.closest(".files-menu") || e.target.closest("[data-files-more]")) return;
    closeMenu();
  });
  document.addEventListener("keydown", function (e) {
    if (e.key === "Escape") closeMenu();
  });
  window.addEventListener("resize", closeMenu);
  window.addEventListener("scroll", closeMenu, true);

  function renderList() {
    var visible = visibleEntries();
    el.list.textContent = "";
    var frag = document.createDocumentFragment();

    visible.forEach(function (entry) {
      var tr = document.createElement("tr");
      tr.setAttribute("data-path", entry.path);
      if (state.selected[entry.path]) tr.classList.add("is-selected");

      var checkCell = document.createElement("td");
      var check = document.createElement("input");
      check.type = "checkbox";
      check.setAttribute("data-files-select", "");
      check.checked = !!state.selected[entry.path];
      check.setAttribute("aria-label", "选择 " + entry.name);
      check.addEventListener("change", function () {
        if (check.checked) state.selected[entry.path] = true;
        else delete state.selected[entry.path];
        tr.classList.toggle("is-selected", check.checked);
        updateBatchBar();
      });
      checkCell.appendChild(check);

      var nameCell = document.createElement("td");
      var nameWrap = document.createElement("div");
      nameWrap.className = "files-name-cell";
      nameWrap.innerHTML = entry.dir ? ICON_DIR : fileIcon(entry.name);
      var nameBtn = document.createElement("button");
      nameBtn.type = "button";
      nameBtn.className = entry.dir ? "files-name-btn" : "files-name-btn";
      nameBtn.title = entry.name;
      nameBtn.textContent = entry.name;
      nameBtn.addEventListener("click", function () {
        if (entry.dir) load(entry.path);
        else openEditor(entry);
      });
      nameWrap.appendChild(nameBtn);
      if (entry.symlink) {
        var dot = document.createElement("span");
        dot.className = "files-hint-dot";
        dot.textContent = "→ " + (entry.link_target || "符号链接");
        nameWrap.appendChild(dot);
      }
      nameCell.appendChild(nameWrap);

      var sizeCell = document.createElement("td");
      sizeCell.className = "num nowrap";
      sizeCell.textContent = entry.dir ? "—" : formatSize(entry.size);

      var modeCell = document.createElement("td");
      modeCell.className = "mono nowrap";
      modeCell.textContent = formatMode(entry.mode);

      var ownerCell = document.createElement("td");
      ownerCell.className = "mono nowrap";
      ownerCell.textContent = entry.owner || "—";

      var timeCell = document.createElement("td");
      timeCell.className = "nowrap";
      timeCell.textContent = formatTime(entry.mtime);

      var actionCell = document.createElement("td");
      actionCell.className = "files-row-actions";
      var more = document.createElement("button");
      more.type = "button";
      more.className = "files-more";
      more.setAttribute("data-files-more", "");
      more.setAttribute("aria-expanded", "false");
      more.textContent = "更多";
      more.addEventListener("click", function (e) {
        e.stopPropagation();
        if (state.menu && state.menu.btn === more) closeMenu();
        else openMenu(more, entry);
      });
      actionCell.appendChild(more);

      tr.appendChild(checkCell);
      tr.appendChild(nameCell);
      tr.appendChild(sizeCell);
      tr.appendChild(modeCell);
      tr.appendChild(ownerCell);
      tr.appendChild(timeCell);
      tr.appendChild(actionCell);
      frag.appendChild(tr);
    });

    el.list.appendChild(frag);
    updateSummary(visible);
    updateSelectAll();
    renderEmpty(visible);
    updateBatchBar();
  }

  // ---- 加载目录 ---------------------------------------------------------

  function load(path) {
    state.loading = true;
    closeMenu();
    setStatus("正在加载 " + path + "…");
    return api("/list?path=" + encodeURIComponent(path))
      .then(function (data) {
        state.path = data.listing.path;
        var base = state.path;
        state.entries = (data.listing.entries || []).map(function (e) {
          e.path = joinPath(base, e.name);
          return e;
        });
        state.truncated = !!data.listing.truncated;
        state.selected = Object.create(null);
        state.loading = false;
        renderCrumbs();
        renderList();
        setStatus("已加载 " + state.entries.length + " 项");
        markTreeActive(state.path);
        revealInTree(state.path).then(function () { markTreeActive(state.path); });
      })
      .catch(function (e) {
        state.loading = false;
        setStatus("加载失败");
        showAlert(errorText(e), "error");
        if (state.entries.length === 0) {
          el.list.textContent = "";
          renderEmpty([]);
          if (el.emptyTitle) el.emptyTitle.textContent = "加载失败";
          if (el.emptyHint) el.emptyHint.textContent = e.message || "请稍后重试。";
          forceVisible(el.emptyActions, false);
        }
      });
  }

  // ---- 新建 / 重命名 / 删除 ---------------------------------------------

  function newFile() {
    promptName({
      title: "新建文件",
      label: "文件名",
      hint: "在 " + state.path + " 下创建空文件，创建后可直接编辑。",
      placeholder: "例如 index.html"
    }).then(function (name) {
      if (!name) return;
      var target = joinPath(state.path, name);
      setStatus("正在创建 " + target + "…");
      return post("/write", { path: target, content: "" }).then(function () {
        return load(state.path).then(function () { openEditorByPath(target); });
      });
    }).catch(function (e) { showAlert(errorText(e), "error"); });
  }

  function newDir() {
    promptName({
      title: "新建目录",
      label: "目录名",
      hint: "在 " + state.path + " 下创建目录。",
      placeholder: "例如 uploads"
    }).then(function (name) {
      if (!name) return;
      var target = joinPath(state.path, name);
      setStatus("正在创建 " + target + "…");
      return post("/mkdir", { path: target }).then(function () {
        return load(state.path);
      });
    }).catch(function (e) { showAlert(errorText(e), "error"); });
  }

  function renameEntry(entry) {
    promptName({
      title: "重命名",
      label: "新名称",
      value: entry.name,
      hint: "重命名不会改变所在目录；目标同名条目会被拒绝。"
    }).then(function (name) {
      if (!name || name === entry.name) return;
      var target = joinPath(parentPath(entry.path), name);
      setStatus("正在重命名…");
      return post("/rename", { from: entry.path, to: target }).then(function () {
        return load(state.path);
      });
    }).catch(function (e) { showAlert(errorText(e), "error"); });
  }

  function removeEntries(items) {
    var names = items.map(function (e) { return e.name; });
    var one = items.length === 1;
    return confirmAction({
      title: one ? "删除「" + names[0] + "」" : "删除 " + items.length + " 项",
      message: one
        ? "将移入回收站，可在「回收站」恢复。"
        : "将把这 " + items.length + " 项移入回收站，可在「回收站」恢复：\n" + names.slice(0, 8).join("、") +
          (names.length > 8 ? " 等" : ""),
      okText: "移入回收站"
    }).then(function (ok) {
      if (!ok) return;
      var failed = [];
      setStatus("正在删除…");
      return items.reduce(function (chain, item) {
        return chain.then(function () {
          return post("/remove", { path: item.path }).catch(function (e) { failed.push(item.name + "：" + e.message); });
        });
      }, Promise.resolve()).then(function () {
        state.selected = Object.create(null);
        return load(state.path).then(function () {
          if (failed.length > 0) showAlert("部分删除失败：" + failed.join("；"), "error");
          else showAlert("已移入回收站：" + names.join("、"), "ok");
        });
      });
    });
  }

  // ---- 上传 -------------------------------------------------------------

  var uploadItems = [];

  function resetUpload() {
    uploadItems = [];
    el.uploadList.textContent = "";
    el.uploadError.hidden = true;
    el.uploadInput.value = "";
    el.uploadStart.disabled = false;
    el.uploadStart.textContent = "开始上传";
  }

  function openUpload() {
    if (!state.path) return;
    resetUpload();
    el.uploadDir.textContent = state.path;
    el.uploadLimit.textContent = "分片 " + Math.round(chunkSize / 1024) + " KiB · 支持多选；同名文件会提示是否覆盖。";
    if (typeof el.uploadDialog.showModal === "function") el.uploadDialog.showModal();
    else el.uploadDialog.setAttribute("open", "");
  }

  function addUploadItems(files) {
    Array.prototype.forEach.call(files, function (file) {
      var row = document.createElement("div");
      row.className = "files-upload-item";
      var top = document.createElement("div");
      top.className = "files-upload-item-top";
      var name = document.createElement("span");
      name.className = "name";
      name.title = file.name;
      name.textContent = file.name;
      var st = document.createElement("span");
      st.className = "state";
      st.textContent = formatSize(file.size) + " · 待上传";
      top.appendChild(name);
      top.appendChild(st);
      var bar = document.createElement("div");
      bar.className = "files-progress";
      var fill = document.createElement("i");
      bar.appendChild(fill);
      row.appendChild(top);
      row.appendChild(bar);
      el.uploadList.appendChild(row);
      uploadItems.push({ file: file, row: row, state: st, fill: fill });
    });
  }

  el.uploadInput.addEventListener("change", function () {
    addUploadItems(el.uploadInput.files || []);
    el.uploadInput.value = "";
  });

  function uploadChunk(path, offset, blob) {
    return fetch(apiUrl("/upload?path=" + encodeURIComponent(path) + "&offset=" + offset), {
      method: "POST",
      credentials: "same-origin",
      headers: { "x-csrf-token": csrf, "content-type": "application/octet-stream" },
      body: blob
    }).then(function (res) {
      return res.text().then(function (raw) {
        var data = parseJSON(raw);
        if (!res.ok || !data || data.ok === false) throw requestError(res, data);
        return data;
      });
    });
  }

  function uploadFile(item) {
    var total = item.file.size;
    if (total > 0) item.fill.style.width = "0%";
    return new Promise(function (resolve, reject) {
      var offset = 0;
      var target = joinPath(state.path, item.file.name);
      var paint = function () {
        var pct = total > 0 ? Math.min(100, Math.floor((offset / total) * 100)) : 100;
        item.fill.style.width = pct + "%";
        item.state.textContent = total > 0
          ? pct + "% · " + formatSize(offset) + " / " + formatSize(total)
          : "写入空文件…";
      };
      var step = function () {
        if (state.uploadAbort) {
          item.state.textContent = "已取消";
          reject(new Error("上传已取消"));
          return;
        }
        if (offset >= total) {
          item.state.textContent = formatSize(total) + " · 完成";
          item.fill.style.width = "100%";
          resolve(true);
          return;
        }
        var end = Math.min(offset + chunkSize, total);
        uploadChunk(target, offset, item.file.slice(offset, end)).then(function () {
          offset = end;
          paint();
          step();
        }).catch(function (e) {
          item.row.classList.add("error");
          item.state.textContent = "失败 · " + (e.message || "上传失败");
          reject(e);
        });
      };
      step();
    });
  }

  function startUpload() {
    if (state.uploadBusy || uploadItems.length === 0) {
      if (uploadItems.length === 0) {
        el.uploadError.textContent = "请先选择要上传的文件。";
        el.uploadError.hidden = false;
      }
      return;
    }
    state.uploadBusy = true;
    state.uploadAbort = false;
    el.uploadStart.disabled = true;
    el.uploadStart.textContent = "上传中…";

    var conflicts = uploadItems.filter(function (item) {
      return state.entries.some(function (e) { return !e.dir && e.name === item.file.name; });
    });
    var ask = conflicts.length === 0 ? Promise.resolve(true) : confirmAction({
      title: "覆盖同名文件",
      message: "当前目录已存在 " + conflicts.length + " 个同名文件。继续上传会用新内容覆盖它们。",
      okText: "覆盖上传",
      plain: true
    });
    ask.then(function (ok) {
      if (!ok) {
        state.uploadBusy = false;
        el.uploadStart.disabled = false;
        el.uploadStart.textContent = "开始上传";
        return;
      }
      var done = 0;
      var failed = 0;
      return uploadItems.reduce(function (chain, item) {
        return chain.then(function () {
          if (state.uploadAbort) return null;
          return uploadFile(item).then(function () {
            done += 1;
          }).catch(function () { failed += 1; });
        });
      }, Promise.resolve()).then(function () {
        state.uploadBusy = false;
        el.uploadStart.disabled = false;
        el.uploadStart.textContent = "开始上传";
        if (state.uploadAbort) return null;
        return load(state.path).then(function () {
          if (failed > 0) {
            el.uploadError.textContent = failed + " 个文件上传失败，详见上方条目标注。";
            el.uploadError.hidden = false;
          } else {
            el.uploadDialog.close();
            showAlert("上传完成：" + done + " 个文件", "ok");
          }
        });
      });
    }).catch(function (e) {
      state.uploadBusy = false;
      el.uploadStart.disabled = false;
      el.uploadStart.textContent = "开始上传";
      el.uploadError.textContent = e.message || "上传失败";
      el.uploadError.hidden = false;
    });
  }

  el.uploadOpen.addEventListener("click", openUpload);
  if (el.emptyUpload) el.emptyUpload.addEventListener("click", openUpload);
  if (el.emptyNewDir) el.emptyNewDir.addEventListener("click", newDir);
  el.uploadClose.addEventListener("click", function () {
    if (state.uploadBusy) {
      confirmAction({ title: "取消上传", message: "当前还有文件在上传，关闭窗口会中断上传。", okText: "中断并关闭" })
        .then(function (ok) {
          if (!ok) return;
          state.uploadAbort = true;
          el.uploadDialog.close();
        });
      return;
    }
    el.uploadDialog.close();
  });
  el.uploadStart.addEventListener("click", startUpload);

  // ---- 下载 -------------------------------------------------------------

  function downloadPath(path) {
    var a = document.createElement("a");
    a.href = apiUrl("/download?path=" + encodeURIComponent(path));
    a.rel = "noopener";
    a.setAttribute("download", "");
    document.body.appendChild(a);
    a.click();
    setTimeout(function () { document.body.removeChild(a); }, 0);
  }

  // ---- 阶段二：权限 / 属主 / 压缩 / 解压 / 复制移动 --------------------

  function openDialog(d) {
    if (!d) return;
    if (typeof d.showModal === "function") {
      if (!d.open) d.showModal();
    } else {
      d.setAttribute("open", "");
    }
  }

  function closeDialog(d) {
    if (!d) return;
    if (typeof d.close === "function" && d.open) d.close();
    else d.removeAttribute("open");
  }

  function setFieldError(node, message) {
    if (!node) return;
    if (message) {
      node.textContent = message;
      forceVisible(node, true);
    } else {
      node.textContent = "";
      forceVisible(node, false);
    }
  }

  function parseModeInput(raw) {
    var s = String(raw || "").trim().toLowerCase();
    if (s.indexOf("0o") === 0) s = s.slice(2);
    if (!/^[0-7]{1,4}$/.test(s)) return null;
    return parseInt(s, 8);
  }

  var chmodTarget = null;

  function openChmod(entry) {
    chmodTarget = entry;
    el.chmodPath.textContent = entry.path;
    el.chmodInput.value =
      entry.mode === null || entry.mode === undefined ? "" : formatMode(entry.mode);
    el.chmodPreset.value = "";
    setFieldError(el.chmodError, "");
    openDialog(el.chmodDialog);
    el.chmodInput.focus();
    el.chmodInput.select();
  }

  function submitChmod() {
    if (!chmodTarget) return;
    var mode = parseModeInput(el.chmodInput.value);
    if (mode === null) {
      setFieldError(el.chmodError, "请输入八进制权限位，例如 0755、2775");
      return;
    }
    if (mode & 0o4000) {
      setFieldError(el.chmodError, "不允许设置 setuid（4000）：共享目录请用 setgid（2775）");
      return;
    }
    el.chmodOk.disabled = true;
    post("/chmod", { path: chmodTarget.path, mode: mode })
      .then(function () {
        setFieldError(el.chmodError, "");
        closeDialog(el.chmodDialog);
        chmodTarget = null;
        setStatus("权限已更新");
        load(state.path);
      })
      .catch(function (e) {
        setFieldError(el.chmodError, e.message || "修改权限失败");
      })
      .then(function () { el.chmodOk.disabled = false; });
  }

  var chownTarget = null;

  function parseOwner(owner) {
    var m = /^(\d+):(\d+)$/.exec(String(owner || ""));
    return m ? { uid: m[1], gid: m[2] } : null;
  }

  function openChown(entry) {
    chownTarget = entry;
    el.chownPath.textContent = entry.path;
    var parsed = parseOwner(entry.owner);
    el.chownUid.value = parsed ? parsed.uid : "";
    el.chownGid.value = parsed ? parsed.gid : "";
    forceVisible(el.chownWarn, !!(parsed && parsed.uid === "0"));
    setFieldError(el.chownError, "");
    openDialog(el.chownDialog);
    el.chownUid.focus();
  }

  function submitChown() {
    if (!chownTarget) return;
    var uid = parseInt(el.chownUid.value, 10);
    var gid = parseInt(el.chownGid.value, 10);
    if (!isFinite(uid) || uid < 0 || !isFinite(gid) || gid < 0) {
      setFieldError(el.chownError, "UID / GID 必须是非负整数");
      return;
    }
    el.chownOk.disabled = true;
    post("/chown", { path: chownTarget.path, uid: uid, gid: gid })
      .then(function (data) {
        var result = (data && data.result) || {};
        closeDialog(el.chownDialog);
        chownTarget = null;
        if (result.root_owned) {
          showAlert("属主已改为 root（uid 0），该文件现在由 root 持有。", "ok");
        } else {
          setStatus("属主已更新为 " + (result.owner || uid + ":" + gid));
        }
        load(state.path);
      })
      .catch(function (e) {
        setFieldError(el.chownError, e.message || "修改属主失败");
      })
      .then(function () { el.chownOk.disabled = false; });
  }

  function extractable(name) {
    return /\.(zip|tar\.gz|tgz)$/i.test(String(name || ""));
  }

  function archiveDefaultName(items, format) {
    var base;
    if (items.length === 1) {
      base = String(items[0].name || "archive").replace(/\.(zip|tar\.gz|tgz)$/i, "");
    } else {
      base = "archive";
    }
    return base + (format === "tar.gz" ? ".tar.gz" : ".zip");
  }

  var archiveItems = [];

  function openArchive(items) {
    if (!items.length) return;
    archiveItems = items.slice();
    el.archiveDir.textContent = state.path;
    el.archiveSources.textContent =
      "共 " + items.length + " 项：" + items.map(function (e) { return e.name; }).join("、");
    el.archiveFormat.value = "zip";
    el.archiveName.value = archiveDefaultName(items, "zip");
    el.archiveOverwrite.checked = false;
    setFieldError(el.archiveError, "");
    openDialog(el.archiveDialog);
    el.archiveName.focus();
    el.archiveName.select();
  }

  function submitArchive() {
    if (!archiveItems.length) return;
    var name = el.archiveName.value.trim();
    var format = el.archiveFormat.value;
    if (!name || name === "." || name === ".." || name.indexOf("/") >= 0) {
      setFieldError(el.archiveError, "压缩包名称不能为空，且不能包含 /");
      return;
    }
    var expected = format === "tar.gz" ? /\.tar\.gz$/i : /\.zip$/i;
    if (!expected.test(name)) {
      name += format === "tar.gz" ? ".tar.gz" : ".zip";
      el.archiveName.value = name;
    }
    var dest = joinPath(state.path, name);
    el.archiveOk.disabled = true;
    post("/archive", {
      sources: archiveItems.map(function (e) { return e.path; }),
      dest: dest,
      format: format,
      overwrite: !!el.archiveOverwrite.checked
    })
      .then(function (data) {
        var result = (data && data.result) || {};
        closeDialog(el.archiveDialog);
        archiveItems = [];
        state.selected = Object.create(null);
        setStatus("已压缩到 " + (result.path || dest) + "（" + formatSize(result.bytes || 0) + "）");
        load(state.path);
      })
      .catch(function (e) {
        setFieldError(el.archiveError, e.message || "压缩失败");
      })
      .then(function () { el.archiveOk.disabled = false; });
  }

  var extractTarget = null;

  function openExtract(entry) {
    extractTarget = entry;
    el.extractArchive.textContent = entry.path;
    el.extractDest.value = state.path;
    el.extractOverwrite.checked = false;
    setFieldError(el.extractError, "");
    openDialog(el.extractDialog);
  }

  function submitExtract() {
    if (!extractTarget) return;
    var dest = el.extractDest.value.trim();
    if (!dest) {
      setFieldError(el.extractError, "请填写解压目标目录");
      return;
    }
    el.extractOk.disabled = true;
    post("/extract", {
      archive: extractTarget.path,
      dest: dest,
      overwrite: !!el.extractOverwrite.checked
    })
      .then(function (data) {
        var result = (data && data.result) || {};
        closeDialog(el.extractDialog);
        extractTarget = null;
        var extra = "";
        if (result.skipped_symlinks) extra += "，跳过 " + result.skipped_symlinks + " 个符号链接";
        if (result.skipped_special) extra += "，跳过 " + result.skipped_special + " 个特殊文件";
        showAlert(
          "已解压 " + (result.files || 0) + " 个文件到 " + (result.dest || dest) + extra + "。",
          extra ? "error" : "ok"
        );
        load(state.path);
      })
      .catch(function (e) {
        setFieldError(el.extractError, e.message || "解压失败");
      })
      .then(function () { el.extractOk.disabled = false; });
  }

  function copyOrMove(items, move) {
    if (!items.length) return;
    pickDirectory(state.path, move ? "选择移动目标目录" : "选择复制目标目录").then(function (targetDir) {
      if (!targetDir) return;
      var errors = [];
      var chain = Promise.resolve();
      items.forEach(function (item) {
        chain = chain.then(function () {
          var to = joinPath(targetDir, item.name);
          if (move && item.path === to) return;
          var payload = move
            ? { from: item.path, to: to }
            : { from: item.path, to: to, overwrite: false };
          return post(move ? "/rename" : "/copy", payload).catch(function (e) {
            errors.push(item.name + "：" + (e.message || "失败"));
          });
        });
      });
      return chain.then(function () {
        if (errors.length) {
          showAlert((move ? "移动" : "复制") + "部分失败：\n" + errors.join("\n"), "error");
        } else {
          showAlert((move ? "已移动 " : "已复制 ") + items.length + " 项到 " + targetDir, "ok");
        }
        state.selected = Object.create(null);
        load(state.path);
      });
    });
  }

  // ---- 目标目录选择器 ---------------------------------------------------

  var pickState = { path: "", resolve: null };

  function pickDirectory(start, title) {
    return new Promise(function (resolve) {
      if (!el.pickDialog) {
        resolve(null);
        return;
      }
      pickState.resolve = resolve;
      el.pickTitle.textContent = title || "选择目标目录";
      openDialog(el.pickDialog);
      browsePick(start || state.path);
    });
  }

  function finishPick(value) {
    closeDialog(el.pickDialog);
    var resolve = pickState.resolve;
    pickState.resolve = null;
    if (resolve) resolve(value);
  }

  function renderPickCrumbs(path) {
    el.pickCrumbs.textContent = "";
    var segments = path.split("/").filter(function (s) { return s.length > 0; });
    var rootBtn = document.createElement("button");
    rootBtn.type = "button";
    rootBtn.className = "files-crumb";
    rootBtn.textContent = "/";
    rootBtn.addEventListener("click", function () { browsePick("/"); });
    el.pickCrumbs.appendChild(rootBtn);
    var acc = "";
    segments.forEach(function (seg, idx) {
      acc += "/" + seg;
      var sep = document.createElement("span");
      sep.className = "files-crumb-sep";
      el.pickCrumbs.appendChild(sep);
      var btn = document.createElement("button");
      btn.type = "button";
      btn.className = "files-crumb" + (idx === segments.length - 1 ? " on" : "");
      btn.textContent = seg;
      var target = acc;
      if (idx !== segments.length - 1) {
        btn.addEventListener("click", function () { browsePick(target); });
      }
      el.pickCrumbs.appendChild(btn);
    });
  }

  function browsePick(path) {
    pickState.path = path;
    el.pickCurrent.textContent = path;
    renderPickCrumbs(path);
    el.pickList.textContent = "";
    var loading = document.createElement("li");
    loading.className = "files-pick-empty";
    loading.textContent = "正在加载目录…";
    el.pickList.appendChild(loading);
    api("/list?path=" + encodeURIComponent(path))
      .then(function (data) {
        var dirs = (data.listing.entries || []).filter(function (e) { return e.dir; });
        dirs.sort(function (a, b) { return a.name.localeCompare(b.name, "zh"); });
        el.pickList.textContent = "";
        if (!dirs.length) {
          var empty = document.createElement("li");
          empty.className = "files-pick-empty";
          empty.textContent = "该目录没有子目录，可直接选择当前目录";
          el.pickList.appendChild(empty);
          return;
        }
        dirs.forEach(function (entry) {
          var li = document.createElement("li");
          var btn = document.createElement("button");
          btn.type = "button";
          btn.className = "files-pick-item";
          btn.innerHTML = ICON_DIR;
          var name = document.createElement("span");
          name.textContent = entry.name;
          name.title = entry.name;
          btn.appendChild(name);
          btn.addEventListener("click", function () { browsePick(joinPath(path, entry.name)); });
          li.appendChild(btn);
          el.pickList.appendChild(li);
        });
      })
      .catch(function (e) {
        el.pickList.textContent = "";
        var li = document.createElement("li");
        li.className = "files-pick-empty";
        li.textContent = "加载失败：" + (e.message || "未知错误");
        el.pickList.appendChild(li);
      });
  }

  // ---- 编辑器（可关闭、保存失败保留内容）---------------------------------

  function editorDirty() {
    return state.editing && !state.editorReadonly && el.editorText.value !== state.editorOriginal;
  }

  function updateEditorState() {
    var dirty = editorDirty();
    forceVisible(el.editorDirty, dirty);
    el.editorSave.disabled = state.editorReadonly || !state.editing;
    var size = new Blob([el.editorText.value]).size;
    el.editorStats.textContent = size + " 字节" + (size > maxEdit ? " · 超过编辑上限" : "");
    if (dirty && size > maxEdit) el.editorSave.disabled = true;
  }

  function openEditorByPath(path) {
    setStatus("正在打开 " + path + "…");
    return api("/read?path=" + encodeURIComponent(path)).then(function (data) {
      var file = data.file;
      state.editing = true;
      state.editorPath = path;
      state.editorOriginal = file.text;
      state.editorReadonly = !!file.truncated;
      el.editorText.value = file.text;
      el.editorText.disabled = state.editorReadonly;
      el.editorPath.textContent = path;
      el.editorPath.title = path;
      el.editorReadonly.textContent = "只读 · 超过 " + Math.round(maxEdit / 1024) + " KiB";
      forceVisible(el.editorReadonly, state.editorReadonly);
      el.editorStatus.textContent = state.editorReadonly
        ? "文件超过编辑上限，仅可查看"
        : "就绪";
      el.editorStatus.style.color = "";
      forceVisible(el.editorDirty, false);
      updateEditorState();
      if (typeof el.editor.showModal === "function") el.editor.showModal();
      else el.editor.setAttribute("open", "");
      el.editorText.focus();
    }).catch(function (e) {
      setStatus("打开失败");
      showAlert("无法编辑：" + (e.message || "打开失败"), "error");
    });
  }

  function openEditor(entry) {
    return openEditorByPath(entry.path);
  }

  function closeEditor() {
    if (typeof el.editor.close === "function" && el.editor.open) el.editor.close();
    else el.editor.removeAttribute("open");
    state.editing = false;
    state.editorPath = null;
    state.editorOriginal = "";
    state.editorReadonly = false;
    el.editorText.value = "";
    el.editorText.disabled = true;
    el.editorSave.disabled = true;
    forceVisible(el.editorDirty, false);
    forceVisible(el.editorReadonly, false);
  }

  function requestCloseEditor() {
    if (!state.editing) {
      closeEditor();
      return Promise.resolve();
    }
    if (!editorDirty()) {
      closeEditor();
      return Promise.resolve();
    }
    return confirmAction({
      title: "关闭编辑器",
      message: "有未保存的修改，关闭后这些修改会丢失。",
      okText: "丢弃并关闭"
    }).then(function (ok) {
      if (ok) closeEditor();
    });
  }

  function saveEditor() {
    if (!state.editing || state.editorReadonly) return Promise.resolve();
    var content = el.editorText.value;
    if (new Blob([content]).size > maxEdit) {
      el.editorStatus.textContent = "保存失败：内容超过 " + Math.round(maxEdit / 1024) + " KiB 编辑上限";
      el.editorStatus.style.color = "var(--danger)";
      return Promise.resolve();
    }
    el.editorSave.disabled = true;
    el.editorStatus.style.color = "";
    el.editorStatus.textContent = "正在保存…";
    return post("/write", { path: state.editorPath, content: content }).then(function () {
      state.editorOriginal = content;
      forceVisible(el.editorDirty, false);
      el.editorStatus.textContent = "已保存 · " + formatSize(new Blob([content]).size);
      el.editorSave.disabled = false;
      updateEditorState();
      if (state.path === parentPath(state.editorPath)) load(state.path);
    }).catch(function (e) {
      // 保存失败保留当前内容，便于重试或复制出去。
      el.editorStatus.textContent = "保存失败：" + (e.message || "未知错误") + "（内容未丢失）";
      el.editorStatus.style.color = "var(--danger)";
      el.editorSave.disabled = false;
    });
  }

  el.editorText.addEventListener("input", function () {
    if (!state.editing) return;
    updateEditorState();
    if (state.editorReadonly) return;
    el.editorStatus.textContent = editorDirty() ? "有未保存的修改" : "已保存";
    el.editorStatus.style.color = "";
  });

  el.editorText.addEventListener("keydown", function (e) {
    if ((e.ctrlKey || e.metaKey) && (e.key === "s" || e.key === "S")) {
      e.preventDefault();
      saveEditor();
    }
    if (e.key === "Tab") {
      e.preventDefault();
      var start = el.editorText.selectionStart;
      var end = el.editorText.selectionEnd;
      var value = el.editorText.value;
      el.editorText.value = value.slice(0, start) + "  " + value.slice(end);
      el.editorText.selectionStart = el.editorText.selectionEnd = start + 2;
      updateEditorState();
    }
  });

  el.editorSave.addEventListener("click", saveEditor);
  el.editorCancel.addEventListener("click", requestCloseEditor);
  el.editorX.addEventListener("click", requestCloseEditor);
  el.editor.addEventListener("cancel", function (e) {
    // Esc 同样走「未保存先确认」，避免误关丢内容。
    e.preventDefault();
    requestCloseEditor();
  });

  // ---- 事件绑定 ---------------------------------------------------------

  el.refresh.addEventListener("click", function () { load(state.path); });
  el.up.addEventListener("click", function () { load(parentPath(state.path)); });

  el.search.addEventListener("input", function () {
    state.filter = el.search.value.trim().toLowerCase();
    closeMenu();
    renderList();
  });

  el.sort.addEventListener("change", function () {
    state.sortKey = el.sort.value;
    renderList();
  });

  el.order.addEventListener("click", function () {
    state.sortDir = -state.sortDir;
    el.order.textContent = state.sortDir > 0 ? "升序 ↑" : "降序 ↓";
    renderList();
  });

  el.hidden.addEventListener("change", function () {
    state.showHidden = el.hidden.checked;
    renderList();
  });

  el.selectAll.addEventListener("change", function () {
    var visible = visibleEntries();
    if (el.selectAll.checked) {
      visible.forEach(function (e) { state.selected[e.path] = true; });
    } else {
      visible.forEach(function (e) { delete state.selected[e.path]; });
    }
    renderList();
  });

  if (el.batchClear) el.batchClear.addEventListener("click", function () {
    state.selected = Object.create(null);
    renderList();
  });
  if (el.batchDelete) el.batchDelete.addEventListener("click", function () {
    var items = state.entries.filter(function (e) { return state.selected[e.path]; });
    if (items.length === 0) return;
    removeEntries(items);
  });
  if (el.batchArchive) el.batchArchive.addEventListener("click", function () {
    var items = state.entries.filter(function (e) { return state.selected[e.path]; });
    if (items.length) openArchive(items);
  });
  if (el.batchCopy) el.batchCopy.addEventListener("click", function () {
    var items = state.entries.filter(function (e) { return state.selected[e.path]; });
    if (items.length) copyOrMove(items, false);
  });
  if (el.batchMove) el.batchMove.addEventListener("click", function () {
    var items = state.entries.filter(function (e) { return state.selected[e.path]; });
    if (items.length) copyOrMove(items, true);
  });

  // 权限弹窗
  if (el.chmodPreset) el.chmodPreset.addEventListener("change", function () {
    if (el.chmodPreset.value) el.chmodInput.value = el.chmodPreset.value;
  });
  if (el.chmodOk) el.chmodOk.addEventListener("click", submitChmod);
  if (el.chmodCancel) el.chmodCancel.addEventListener("click", function () {
    chmodTarget = null;
    setFieldError(el.chmodError, "");
    closeDialog(el.chmodDialog);
  });
  if (el.chmodDialog) el.chmodDialog.addEventListener("cancel", function (e) {
    e.preventDefault();
    chmodTarget = null;
    setFieldError(el.chmodError, "");
    closeDialog(el.chmodDialog);
  });

  // 属主弹窗：uid 0 常显提示
  if (el.chownUid) el.chownUid.addEventListener("input", function () {
    forceVisible(el.chownWarn, el.chownUid.value.trim() === "0");
  });
  if (el.chownOk) el.chownOk.addEventListener("click", submitChown);
  if (el.chownCancel) el.chownCancel.addEventListener("click", function () {
    chownTarget = null;
    setFieldError(el.chownError, "");
    closeDialog(el.chownDialog);
  });
  if (el.chownDialog) el.chownDialog.addEventListener("cancel", function (e) {
    e.preventDefault();
    chownTarget = null;
    setFieldError(el.chownError, "");
    closeDialog(el.chownDialog);
  });

  // 压缩弹窗
  if (el.archiveFormat) el.archiveFormat.addEventListener("change", function () {
    var format = el.archiveFormat.value;
    var name = el.archiveName.value.trim();
    if (archiveItems.length === 1 && /\.(zip|tar\.gz|tgz)$/i.test(name)) {
      el.archiveName.value = name.replace(/\.(zip|tar\.gz|tgz)$/i, "") +
        (format === "tar.gz" ? ".tar.gz" : ".zip");
    } else if (name) {
      el.archiveName.value = archiveDefaultName(archiveItems, format);
    }
  });
  if (el.archiveOk) el.archiveOk.addEventListener("click", submitArchive);
  if (el.archiveCancel) el.archiveCancel.addEventListener("click", function () {
    archiveItems = [];
    setFieldError(el.archiveError, "");
    closeDialog(el.archiveDialog);
  });
  if (el.archiveDialog) el.archiveDialog.addEventListener("cancel", function (e) {
    e.preventDefault();
    archiveItems = [];
    setFieldError(el.archiveError, "");
    closeDialog(el.archiveDialog);
  });

  // 解压弹窗
  if (el.extractPick) el.extractPick.addEventListener("click", function () {
    pickDirectory(el.extractDest.value.trim() || state.path, "选择解压目标目录").then(function (dir) {
      if (dir) el.extractDest.value = dir;
    });
  });
  if (el.extractOk) el.extractOk.addEventListener("click", submitExtract);
  if (el.extractCancel) el.extractCancel.addEventListener("click", function () {
    extractTarget = null;
    setFieldError(el.extractError, "");
    closeDialog(el.extractDialog);
  });
  if (el.extractDialog) el.extractDialog.addEventListener("cancel", function (e) {
    e.preventDefault();
    extractTarget = null;
    setFieldError(el.extractError, "");
    closeDialog(el.extractDialog);
  });

  // 目标目录选择器
  if (el.pickUp) el.pickUp.addEventListener("click", function () {
    browsePick(parentPath(pickState.path || state.path));
  });
  if (el.pickOk) el.pickOk.addEventListener("click", function () {
    finishPick(pickState.path || state.path);
  });
  if (el.pickCancel) el.pickCancel.addEventListener("click", function () { finishPick(null); });
  if (el.pickDialog) el.pickDialog.addEventListener("cancel", function (e) {
    e.preventDefault();
    finishPick(null);
  });

  el.alertClose.addEventListener("click", clearAlert);

  // 首屏：如果 start 是目录则直接加载；否则退回其父目录。
  load(state.path);
})();
