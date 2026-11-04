// 插件 UI 的 RPC 桥客户端：所有能力调用都经父页面转发（iframe 自身没有凭据）。
// 协议见 crates/web/static/skiff.js::attachPluginBridges。
//
// 语法高亮用随包分发的 highlight.js（vendor/），不走外网：插件 UI 的 CSP 是
// `script-src 'self'; connect-src 'none'`，CDN 与内联脚本都加载不了。
(function () {
  "use strict";

  var qs = new URLSearchParams(location.search);
  var nonce = qs.get("nonce") || "";
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

  var cwd = "";
  var roots = [];
  var selected = null;   // { path, dir }
  var editingPath = "";  // 正在编辑的文件
  var readOnly = false;  // 文件超过单页上限时只读

  var hl = window.hljs || null;

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
    if (n < 1024 * 1024 * 1024) return (n / 1024 / 1024).toFixed(1) + " MiB";
    return (n / 1024 / 1024 / 1024).toFixed(1) + " GiB";
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
  function joinPath(dir, name) {
    return (dir.endsWith("/") ? dir : dir + "/") + name;
  }
  function dirOf(path) {
    var i = path.lastIndexOf("/");
    return i > 0 ? path.slice(0, i) : "/";
  }
  function baseName(path) {
    var i = path.lastIndexOf("/");
    return i >= 0 ? path.slice(i + 1) : path;
  }
  function badName(name) {
    return !name || name.indexOf("/") >= 0 || name.indexOf("..") >= 0;
  }

  // ---- base64 ⇄ UTF-8 文本（浏览器没有直接 API，手写两段） ----
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

  // ---- 语法高亮 ----
  // 扩展名 → highlight.js 语言名。找不到映射时退到 highlightAuto（限长度），
  // 再不行就是纯文本。`.toml` 没有独立语法，用 ini（键值表形态足够接近）。
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

  /// 下拉框当前选的语言；`auto` 时按文件名推断，推不出来交给 highlightAuto。
  function currentLang() {
    var v = el("lang").value;
    if (v && v !== "auto") return v;
    return detectLang(editingPath);
  }

  function fillLangOptions() {
    var sel = el("lang");
    var opts = ['<option value="auto">自动（按扩展名）</option>'];
    if (hl && typeof hl.listLanguages === "function") {
      hl.listLanguages().slice().sort().forEach(function (l) {
        opts.push('<option value="' + esc(l) + '">' + esc(l) + "</option>");
      });
    }
    sel.innerHTML = opts.join("");
    sel.value = "auto";
  }

  var paintTimer = 0;
  function schedulePaint() {
    if (paintTimer) return;
    paintTimer = setTimeout(function () { paintTimer = 0; paint(); }, 120);
  }

  function paint() {
    var code = el("hl-code");
    var text = el("content").value;
    if (!code) return;
    if (!hl) { code.textContent = text; return; }
    var lang = currentLang();
    var out;
    try {
      if (lang && hl.getLanguage(lang)) {
        out = hl.highlight(text, { language: lang, ignoreIllegals: true }).value;
      } else if (text.length > 100000) {
        // 自动识别要试遍所有语法，大文件太贵：超过这个量级就不猜了。
        out = esc(text);
      } else {
        out = hl.highlightAuto(text).value;
      }
    } catch (e) {
      out = esc(text);
    }
    // 末尾换行在 <pre> 里不占一行，补一个才能让两层高度对齐。
    code.innerHTML = out + (text.endsWith("\n") ? "\n" : "");
    syncScroll();
  }

  function syncScroll() {
    var ta = el("content"), pre = el("hl");
    pre.scrollTop = ta.scrollTop;
    pre.scrollLeft = ta.scrollLeft;
  }

  function setText(text) {
    el("content").value = text;
    paint();
  }

  function syncLangFromPath(path) {
    var guess = detectLang(path);
    el("lang").value = guess || "auto";
    paint();
  }

  el("content").addEventListener("scroll", syncScroll);
  el("content").addEventListener("input", schedulePaint);
  el("lang").addEventListener("change", paint);
  // Tab 在代码里应是缩进而不是移焦：插入两个空格，保持两层同步。
  el("content").addEventListener("keydown", function (ev) {
    if (ev.key !== "Tab" || el("content").readOnly || ev.ctrlKey || ev.altKey) return;
    ev.preventDefault();
    var ta = el("content");
    var start = ta.selectionStart;
    ta.setRangeText("  ", start, ta.selectionEnd, "end");
    schedulePaint();
  });

  function setSelected(entry) {
    selected = entry;
    el("selected").textContent = entry ? entry.path : "（未选中）";
    Array.prototype.forEach.call(document.querySelectorAll("#items tr[data-path]"), function (tr) {
      tr.className = entry && tr.getAttribute("data-path") === entry.path ? "sel" : "";
    });
  }

  function render(list) {
    cwd = list.path || "";
    roots = list.roots || [];
    el("cwd").textContent = cwd || "-";
    el("roots").textContent = roots.length ? roots.join("、") : "无";
    el("path-input").value = cwd;

    var items = list.items || [];
    if (items.length === 0) {
      el("items").innerHTML = '<tr><td colspan="5" class="muted">空目录</td></tr>';
      return;
    }
    el("items").innerHTML = items.map(function (it) {
      var path = joinPath(cwd, it.name);
      var isDir = !!it.dir;
      return '<tr data-path="' + esc(path) + '" data-dir="' + (isDir ? "1" : "0") + '">' +
        "<td>" + (isDir ? "📁 " : "📄 ") + esc(it.name) + "</td>" +
        "<td>" + (isDir ? "目录" : "文件") + "</td>" +
        "<td>" + (isDir ? "-" : bytes(it.size)) + "</td>" +
        "<td>" + when(it.mtime) + "</td>" +
        '<td><button class="mini" data-act="' + (isDir ? "open-dir" : "open-file") + '">' +
        (isDir ? "进入" : "编辑") + "</button></td></tr>";
    }).join("");
    setSelected(selected && items.some(function (it) { return joinPath(cwd, it.name) === selected.path; }) ? selected : null);
  }

  function load(path) {
    var params = path ? { path: path } : {};
    return rpc("list", params).then(function (r) {
      render(r || {});
    }).catch(function (e) {
      say("读取目录失败：" + e, true);
    });
  }

  /// 打开「目录或文件」：跨插件跳转给的是一个位置，先当目录试，失败再当文件读。
  function openLocation(path) {
    return rpc("list", { path: path }).then(function (r) {
      render(r || {});
      return true;
    }).catch(function () {
      return openFile(path).then(function () { return true; });
    });
  }

  function openFile(path) {
    return rpc("read", { path: path }).then(function (r) {
      editingPath = r.path;
      readOnly = !r.eof;
      el("editing").textContent = r.path + (readOnly ? "（超过 256KiB，仅显示第一页，只读）" : "");
      el("content").readOnly = readOnly;
      setText(b64decode(r.b64));
      syncLangFromPath(r.path);
      say("已打开 " + baseName(r.path) + "（" + bytes(r.size) + "）" + (readOnly ? "，只读" : ""));
    }).catch(function (e) {
      say("打开 " + path + " 失败：" + e, true);
    });
  }

  el("items").addEventListener("click", function (ev) {
    var tr = ev.target.closest ? ev.target.closest("tr[data-path]") : null;
    if (!tr) return;
    var path = tr.getAttribute("data-path");
    var isDir = tr.getAttribute("data-dir") === "1";
    if (ev.target.closest("button")) {
      if (isDir) {
        setSelected(null);
        return load(path);
      }
      setSelected({ path: path, dir: false });
      el("path-input").value = cwd;
      var name = baseName(path);
      el("entry-name").value = name;
      return openFile(path);
    }
    setSelected({ path: path, dir: isDir });
    el("entry-name").value = baseName(path);
  });

  el("up").addEventListener("click", function () {
    rpc("list", { path: cwd }).then(function (r) {
      if (!r || !r.parent) {
        say("已在授权根，不能继续向上（授权之外的目录不属于本插件）");
        return;
      }
      return load(r.parent);
    }).catch(function (e) {
      say("返回上级失败：" + e, true);
    });
  });

  el("go").addEventListener("click", function () {
    var p = el("path-input").value.trim();
    if (!p) {
      say("请填写目录路径", true);
      return;
    }
    load(p);
  });

  el("refresh").addEventListener("click", function () { load(cwd); });

  el("mkfile").addEventListener("click", function () {
    var name = el("entry-name").value.trim();
    if (badName(name)) {
      say("文件名非法（不能为空、不能含 / 或 ..）", true);
      return;
    }
    var path = joinPath(cwd, name);
    rpc("write", { path: path, text: "" }).then(function () {
      return load(cwd).then(function () { say("已新建文件 " + path); });
    }).catch(function (e) {
      say("新建文件失败：" + e, true);
    });
  });

  el("mkdir").addEventListener("click", function () {
    var name = el("entry-name").value.trim();
    if (badName(name)) {
      say("目录名非法（不能为空、不能含 / 或 ..）", true);
      return;
    }
    var path = joinPath(cwd, name);
    rpc("mkdir", { path: path }).then(function () {
      return load(cwd).then(function () { say("已新建目录 " + path); });
    }).catch(function (e) {
      say("新建目录失败：" + e, true);
    });
  });

  el("rename").addEventListener("click", function () {
    if (!selected) {
      say("请先在列表里选中一个条目", true);
      return;
    }
    var name = el("entry-name").value.trim();
    if (badName(name)) {
      say("新名称非法（不能为空、不能含 / 或 ..）", true);
      return;
    }
    var to = joinPath(dirOf(selected.path), name);
    rpc("rename", { path: selected.path, to: to }).then(function () {
      setSelected(null);
      return load(cwd).then(function () { say("已改名为 " + to); });
    }).catch(function (e) {
      say("改名失败：" + e, true);
    });
  });

  el("remove").addEventListener("click", function () {
    if (!selected) {
      say("请先在列表里选中一个条目", true);
      return;
    }
    var what = selected.dir ? "目录（含其中所有内容）" : "文件";
    if (!window.confirm("删除" + what + " " + selected.path + "？此操作不可恢复。")) return;
    var path = selected.path;
    rpc("remove", { path: path }).then(function () {
      if (editingPath === path) {
        editingPath = "";
        el("editing").textContent = "（未打开文件）";
        setText("");
      }
      setSelected(null);
      return load(cwd).then(function () { say("已删除 " + path); });
    }).catch(function (e) {
      say("删除失败：" + e, true);
    });
  });

  el("save").addEventListener("click", function () {
    if (!editingPath) {
      say("请先打开一个文件", true);
      return;
    }
    if (readOnly) {
      say("该文件超过 256KiB，本插件只显示第一页，请勿在此保存", true);
      return;
    }
    var text = el("content").value;
    rpc("write", { path: editingPath, b64: b64encode(text) }).then(function (r) {
      say("已保存 " + editingPath + "（" + bytes(r.size) + "）");
      rpc("list", { path: cwd }).then(render).catch(function () {});
    }).catch(function (e) {
      say("保存失败：" + e, true);
    });
  });

  el("close").addEventListener("click", function () {
    editingPath = "";
    readOnly = false;
    el("content").readOnly = false;
    setText("");
    el("editing").textContent = "（未打开文件）";
  });

  // 供其他插件/用户脚本调用：跳到别的插件界面（例如监控 → 文件管理）。
  window.skiffOpenPlugin = function (plugin, path) {
    return openPlugin(plugin, path).then(function (r) {
      if (!r || !r.ok) say("跳转被拒绝：" + ((r && r.error) || "目标插件未启用或没有界面"), true);
      return r;
    });
  };

  fillLangOptions();

  rpc("config").then(function (r) {
    roots = (r && r.roots) || [];
    var start = openHint || (r && r.start_dir) || "";
    if (roots.length === 0) {
      el("roots").textContent = "无";
      el("items").innerHTML = '<tr><td colspan="5" class="bad">未授权任何路径：请在安装确认页勾选 path: 根后重装本插件</td></tr>';
      return;
    }
    if (openHint) {
      say("来自其他插件：定位到 " + openHint);
      openLocation(openHint).catch(function (e) {
        say("无法定位到 " + openHint + "：" + e + "（本插件未授权该路径时属正常）", true);
      });
      return;
    }
    load(start);
  }).catch(function (e) {
    say("读取插件配置失败：" + e, true);
  });
})();
