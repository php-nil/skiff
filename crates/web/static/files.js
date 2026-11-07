/* 文件管理页前端：只调用内核 /files/api/*，不做任何本地授权判定。
   保存失败、打开失败都保留当前编辑内容，避免「读了一半再保存」丢数据。 */
(function () {
  "use strict";

  var page = document.querySelector("[data-files]");
  if (!page) return;

  var prefix = page.getAttribute("data-prefix") || "";
  var csrf = page.getAttribute("data-csrf") || "";

  var el = {
    alert: page.querySelector("[data-files-alert]"),
    alertText: page.querySelector("[data-files-alert-text]"),
    alertClose: page.querySelector("[data-files-alert-close]"),
    crumbs: page.querySelector("[data-files-crumbs]"),
    list: page.querySelector("[data-files-list]"),
    rename: page.querySelector("[data-files-rename]"),
    remove: page.querySelector("[data-files-delete]"),
    newDir: page.querySelector("[data-files-new-dir]"),
    newFile: page.querySelector("[data-files-new-file]"),
    refresh: page.querySelector("[data-files-refresh]"),
    root: page.querySelector("[data-files-root]"),
    openPath: page.querySelector("[data-files-open-path]"),
    dirty: page.querySelector("[data-files-dirty]"),
    readonly: page.querySelector("[data-files-readonly]"),
    close: page.querySelector("[data-files-close]"),
    save: page.querySelector("[data-files-save]"),
    textarea: page.querySelector("[data-files-textarea]"),
    status: page.querySelector("[data-files-status]"),
    stats: page.querySelector("[data-files-stats]")
  };

  var state = {
    path: page.getAttribute("data-start") || "/",
    listing: null,
    selected: null,
    file: null,
    text: "",
    saved: "",
    readonly: false,
    busy: false
  };

  var ICON_DIR =
    '<svg class="files-ico" viewBox="0 0 16 16" aria-hidden="true"><path d="M1.5 3.5h4l1.2 1.4h7.8v7.6h-13z" fill="none" stroke="currentColor" stroke-width="1.2"/></svg>';
  var ICON_FILE =
    '<svg class="files-ico" viewBox="0 0 16 16" aria-hidden="true"><path d="M3.5 1.5h5.4l3.6 3.6v9.4h-9z" fill="none" stroke="currentColor" stroke-width="1.2"/><path d="M8.7 1.7v3.6h3.6" fill="none" stroke="currentColor" stroke-width="1.2"/></svg>';

  function joinPath(dir, name) {
    if (!dir || dir === "/") return "/" + name;
    return dir.replace(/\/+$/, "") + "/" + name;
  }

  function parentPath(path) {
    var s = path.replace(/\/+$/, "");
    var i = s.lastIndexOf("/");
    return i <= 0 ? "/" : s.slice(0, i);
  }

  function isDirty() {
    return state.file !== null && !state.readonly && state.text !== state.saved;
  }

  function setStatus(text) {
    el.status.textContent = text;
  }

  function showAlert(message, kind) {
    if (!el.alert) return;
    el.alertText.textContent = message;
    el.alert.setAttribute("data-kind", kind || "info");
    el.alert.hidden = false;
  }

  function clearAlert() {
    if (el.alert) el.alert.hidden = true;
  }

  function errorText(e) {
    var code = (e && e.code) || "files.request_failed";
    var msg = (e && e.message) || "请求失败";
    return code + " · " + msg;
  }

  function api(url, options) {
    options = options || {};
    options.credentials = "same-origin";
    options.headers = options.headers || {};
    if (options.body) options.headers["content-type"] = "application/json";
    return fetch(url, options).then(function (res) {
      return res.text().then(function (raw) {
        var data = null;
        try {
          data = raw ? JSON.parse(raw) : null;
        } catch (e) {
          data = null;
        }
        if (!res.ok || !data || data.ok === false) {
          var body = (data && data.error) || {};
          var err = new Error(body.message || "请求失败（HTTP " + res.status + "）");
          err.code = body.code || "http." + res.status;
          throw err;
        }
        return data;
      });
    });
  }

  function post(url, payload) {
    return api(url, {
      method: "POST",
      headers: { "x-csrf-token": csrf },
      body: JSON.stringify(payload)
    });
  }

  function apiUrl(path) {
    return prefix + "/files/api" + path;
  }

  function formatSize(n) {
    if (n === null || n === undefined) return "";
    if (n < 1024) return n + " B";
    var units = ["B", "KB", "MB", "GB", "TB"];
    var i = 0;
    var v = n;
    while (v >= 1024 && i < units.length - 1) {
      v /= 1024;
      i += 1;
    }
    return (i === 0 ? v : v.toFixed(1)) + " " + units[i];
  }

  function formatTime(ts) {
    if (!ts) return "";
    var d = new Date(ts * 1000);
    if (isNaN(d.getTime())) return "";
    var pad = function (x) {
      return x < 10 ? "0" + x : "" + x;
    };
    return d.getFullYear() + "-" + pad(d.getMonth() + 1) + "-" + pad(d.getDate()) +
      " " + pad(d.getHours()) + ":" + pad(d.getMinutes());
  }

  function renderCrumbs() {
    if (!el.crumbs) return;
    el.crumbs.textContent = "";
    var segments = state.path.split("/").filter(function (s) {
      return s.length > 0;
    });
    var base = state.path.charAt(0) === "/" ? "/" : "";
    var crumb = document.createElement("button");
    crumb.type = "button";
    crumb.className = "files-crumb" + (segments.length === 0 ? " files-crumb-current" : "");
    crumb.textContent = "根目录";
    if (segments.length > 0) {
      crumb.addEventListener("click", function () {
        load("/");
      });
    }
    el.crumbs.appendChild(crumb);

    var acc = base;
    segments.forEach(function (seg, idx) {
      acc = acc === "/" ? "/" + seg : acc + "/" + seg;
      var sep = document.createElement("span");
      sep.className = "files-crumb-sep";
      sep.textContent = "/";
      el.crumbs.appendChild(sep);

      var btn = document.createElement("button");
      btn.type = "button";
      btn.className = "files-crumb" + (idx === segments.length - 1 ? " files-crumb-current" : "");
      btn.textContent = seg;
      if (idx !== segments.length - 1) {
        var target = acc;
        btn.addEventListener("click", function () {
          load(target);
        });
      }
      el.crumbs.appendChild(btn);
    });
  }

  function renderList() {
    if (!el.list) return;
    el.list.textContent = "";
    var listing = state.listing;
    if (!listing) {
      var loading = document.createElement("div");
      loading.className = "files-empty";
      loading.textContent = "加载中…";
      el.list.appendChild(loading);
      return;
    }
    if (listing.entries.length === 0) {
      var empty = document.createElement("div");
      empty.className = "files-empty";
      empty.textContent = "空目录";
      el.list.appendChild(empty);
      return;
    }
    listing.entries.forEach(function (entry) {
      var row = document.createElement("div");
      row.className = "files-row" + (entry.dir ? " is-dir" : "");
      row.setAttribute("role", "button");
      row.setAttribute("tabindex", "0");
      row.title = joinPath(state.path, entry.name);
      if (entry.symlink) row.title += "（符号链接）";
      if (state.selected === entry.name) row.classList.add("selected");

      var icon = document.createElement("span");
      icon.innerHTML = entry.dir ? ICON_DIR : ICON_FILE;
      row.appendChild(icon);

      var name = document.createElement("span");
      name.className = "files-row-name";
      name.textContent = entry.name;
      row.appendChild(name);

      var meta = document.createElement("span");
      meta.className = "files-row-meta";
      var bits = [];
      if (entry.dir) {
        bits.push("目录");
      } else {
        bits.push(formatSize(entry.size));
      }
      if (entry.mtime) bits.push(formatTime(entry.mtime));
      meta.textContent = bits.join(" · ");
      row.appendChild(meta);

      row.addEventListener("click", function () {
        select(entry);
      });
      row.addEventListener("dblclick", function () {
        activate(entry);
      });
      row.addEventListener("keydown", function (ev) {
        if (ev.key === "Enter") {
          ev.preventDefault();
          activate(entry);
        }
      });

      el.list.appendChild(row);
    });

    if (listing.truncated) {
      var more = document.createElement("div");
      more.className = "files-empty";
      more.textContent = "目录条目过多，仅显示前 1000 项";
      el.list.appendChild(more);
    }
  }

  function updateListActions() {
    if (el.rename) el.rename.disabled = state.selected === null || state.busy;
    if (el.remove) el.remove.disabled = state.selected === null || state.busy;
  }

  function select(entry) {
    state.selected = entry ? entry.name : null;
    renderList();
    updateListActions();
  }

  function updateEditor() {
    var open = state.file !== null;
    el.textarea.disabled = !open;
    el.close.disabled = !open || state.busy;
    el.save.disabled = !open || state.busy || state.readonly;
    el.openPath.textContent = state.file || "未打开文件";
    el.dirty.hidden = !isDirty();
    el.readonly.hidden = !(open && state.readonly);
    if (open) {
      var lines = state.text.length === 0 ? 0 : state.text.split("\n").length;
      el.stats.textContent = formatSize(state.text.length) + " · " + lines + " 行";
    } else {
      el.stats.textContent = "";
    }
  }

  function confirmDiscard(message) {
    if (!isDirty()) return true;
    return window.confirm(message || "当前文件有未保存修改，继续操作会丢弃这些修改。是否继续？");
  }

  function load(path, opts) {
    opts = opts || {};
    if (!opts.force && !confirmDiscard("切换目录会丢弃当前文件未保存的修改。是否继续？")) {
      return Promise.resolve();
    }
    state.busy = true;
    updateListActions();
    setStatus("读取目录 " + path + " …");
    return api(apiUrl("/list?path=" + encodeURIComponent(path)))
      .then(function (data) {
        state.listing = data.listing;
        state.path = data.listing.path;
        state.selected = null;
        renderCrumbs();
        renderList();
        updateListActions();
        setStatus("目录已更新 · " + data.listing.entries.length + " 项");
        clearAlert();
      })
      .catch(function (e) {
        state.listing = null;
        renderList();
        showAlert("读取目录失败：" + errorText(e), "danger");
        setStatus("读取失败");
      })
      .then(function () {
        state.busy = false;
        updateListActions();
      });
  }

  function activate(entry) {
    if (entry.dir) {
      load(joinPath(state.path, entry.name));
    } else {
      openPath(joinPath(state.path, entry.name));
    }
  }

  function openPath(path) {
    if (!confirmDiscard("打开另一个文件会丢弃当前未保存的修改。是否继续？")) {
      return Promise.resolve();
    }
    setStatus("打开 " + path + " …");
    return api(apiUrl("/read?path=" + encodeURIComponent(path)))
      .then(function (data) {
        state.file = data.file.path;
        state.readonly = !!data.file.truncated;
        state.text = data.file.text;
        state.saved = data.file.text;
        el.textarea.value = state.text;
        el.textarea.readOnly = state.readonly;
        updateEditor();
        if (state.readonly) {
          showAlert("文件超过 256 KiB，页面只读；请用命令行或下载后处理", "warn");
        } else {
          clearAlert();
        }
        setStatus("已打开 · " + formatSize(data.file.size));
        el.textarea.focus();
      })
      .catch(function (e) {
        // 打开失败保留原编辑内容，不清空 textarea。
        showAlert("打开文件失败：" + errorText(e), "danger");
        setStatus("打开失败");
      });
  }

  function save() {
    if (state.file === null || state.readonly || state.busy) return;
    var payload = { path: state.file, content: state.text };
    state.busy = true;
    updateEditor();
    setStatus("保存中…");
    post(apiUrl("/write"), payload)
      .then(function (data) {
        state.saved = state.text;
        updateEditor();
        showAlert("已保存 · " + formatSize(data.result.size) + " · " + data.result.sha256.slice(0, 12), "ok");
        setStatus("已保存");
      })
      .catch(function (e) {
        // 保存失败不改 textarea：用户可以直接重试，不会丢内容。
        showAlert("保存失败：" + errorText(e), "danger");
        setStatus("保存失败");
      })
      .then(function () {
        state.busy = false;
        updateEditor();
      });
  }

  function closeEditor() {
    if (!confirmDiscard("关闭编辑器会丢弃未保存的修改。是否继续？")) return;
    state.file = null;
    state.text = "";
    state.saved = "";
    state.readonly = false;
    el.textarea.value = "";
    el.textarea.readOnly = false;
    updateEditor();
    setStatus("已关闭编辑器");
  }

  function createDir() {
    var name = window.prompt("新建目录名称");
    if (name === null) return;
    name = name.trim();
    if (!name) return;
    var path = joinPath(state.path, name);
    post(apiUrl("/mkdir"), { path: path })
      .then(function () {
        showAlert("已创建目录 · " + path, "ok");
        return load(state.path, { force: true });
      })
      .catch(function (e) {
        showAlert("新建目录失败：" + errorText(e), "danger");
      });
  }

  function createFile() {
    var name = window.prompt("新建文件名称");
    if (name === null) return;
    name = name.trim();
    if (!name) return;
    var path = joinPath(state.path, name);
    post(apiUrl("/write"), { path: path, content: "" })
      .then(function () {
        showAlert("已创建文件 · " + path, "ok");
        return load(state.path, { force: true }).then(function () {
          return openPath(path);
        });
      })
      .catch(function (e) {
        showAlert("新建文件失败：" + errorText(e), "danger");
      });
  }

  function renameSelected() {
    if (state.selected === null) return;
    var name = window.prompt("重命名为", state.selected);
    if (name === null) return;
    name = name.trim();
    if (!name || name === state.selected) return;
    var from = joinPath(state.path, state.selected);
    var to = joinPath(state.path, name);
    post(apiUrl("/rename"), { from: from, to: to })
      .then(function () {
        showAlert("已重命名 · " + name, "ok");
        if (state.file === from) {
          state.file = to;
          updateEditor();
        }
        return load(state.path, { force: true });
      })
      .catch(function (e) {
        showAlert("重命名失败：" + errorText(e), "danger");
      });
  }

  function removeSelected() {
    if (state.selected === null) return;
    var path = joinPath(state.path, state.selected);
    if (!window.confirm("确定永久删除 " + path + " ？（当前版本不进入回收站）")) return;
    post(apiUrl("/remove"), { path: path })
      .then(function () {
        showAlert("已删除 · " + path, "ok");
        if (state.file && (state.file === path || state.file.indexOf(path + "/") === 0)) {
          state.file = null;
          state.text = "";
          state.saved = "";
          el.textarea.value = "";
          updateEditor();
        }
        return load(state.path, { force: true });
      })
      .catch(function (e) {
        showAlert("删除失败：" + errorText(e), "danger");
      });
  }

  function bind() {
    if (el.refresh) el.refresh.addEventListener("click", function () { load(state.path, { force: true }); });
    if (el.root) el.root.addEventListener("click", function () { load("/"); });
    if (el.newDir) el.newDir.addEventListener("click", createDir);
    if (el.newFile) el.newFile.addEventListener("click", createFile);
    if (el.rename) el.rename.addEventListener("click", renameSelected);
    if (el.remove) el.remove.addEventListener("click", removeSelected);
    if (el.save) el.save.addEventListener("click", save);
    if (el.close) el.close.addEventListener("click", closeEditor);
    if (el.alertClose) el.alertClose.addEventListener("click", clearAlert);
    if (el.textarea) {
      el.textarea.addEventListener("input", function () {
        state.text = el.textarea.value;
        updateEditor();
      });
      el.textarea.addEventListener("keydown", function (ev) {
        if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === "s") {
          ev.preventDefault();
          save();
        }
      });
    }
    window.addEventListener("beforeunload", function (ev) {
      if (!isDirty()) return;
      ev.preventDefault();
      ev.returnValue = "";
    });
  }

  bind();
  updateEditor();
  load(state.path);
})();
