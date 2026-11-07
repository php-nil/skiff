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
  // 本插件创建的任务 id 是 `plugin:<本插件>:<key>`；内核 task_view 的 source
  // 报的是任务归属（target 插件），不能用来判断「是不是本面板建的」。
  var SELF_TASK_PREFIX = "plugin:com.skiff.cron:";
  function isOwnTask(id) {
    return String(id || "").indexOf(SELF_TASK_PREFIX) === 0;
  }

  // ---- 调度选择器：新建页与列表页的「修改调度」共用这一份实现 ----
  // 预设 → 表达式、表达式 → 预设都由插件 RPC（skiff-cron）翻译，
  // 页面只负责收集选择；内核仍只存标准 5 段表达式。
  var SCHED_DAY_NAMES = { 0: "周日", 1: "周一", 2: "周二", 3: "周三", 4: "周四", 5: "周五", 6: "周六" };

  function pad2(n) {
    n = Number(n);
    return (n < 10 ? "0" : "") + n;
  }
  function hm(hour, minute) {
    return pad2(hour) + ":" + pad2(minute);
  }
  function parseHm(text) {
    var m = /^(\d{1,2}):(\d{2})$/.exec(String(text || ""));
    if (!m) return null;
    return { hour: Number(m[1]), minute: Number(m[2]) };
  }

  function mountScheduleEditor(host, opts) {
    opts = opts || {};
    var st = { kind: "daily", every: 5, hour: 3, minute: 0, days: [1, 2, 3, 4, 5], day: 1 };
    var current = { valid: true, preset: null, expr: "", human: "", error: "" };
    var seq = 0;
    var timer = 0;

    host.innerHTML =
      '<div class="sched-box">' +
        '<div class="sched-line">' +
          '<select class="sched-kind">' +
            '<option value="minutes">每隔 N 分钟</option>' +
            '<option value="hours">每隔 N 小时</option>' +
            '<option value="daily">每天某时刻</option>' +
            '<option value="weekly">每周某几天</option>' +
            '<option value="monthly">每月某日</option>' +
            '<option value="weekdays">工作日</option>' +
            '<option value="custom">自定义表达式</option>' +
          '</select>' +
          '<span class="sched-fields"></span>' +
        '</div>' +
        '<div class="sched-line">' +
          '<input type="text" class="sched-expr" spellcheck="false" placeholder="0 3 * * *">' +
          '<span class="sched-preview"></span>' +
        '</div>' +
        '<div class="sched-error"></div>' +
        '<div class="muted">5 段：分 时 日 月 周；按服务器本地时区执行，不支持秒级与 @reboot。</div>' +
      '</div>';

    var kindSel = host.querySelector(".sched-kind");
    var fields = host.querySelector(".sched-fields");
    var exprInput = host.querySelector(".sched-expr");
    var preview = host.querySelector(".sched-preview");
    var error = host.querySelector(".sched-error");

    function notify() {
      if (typeof opts.onChange === "function") {
        try { opts.onChange(current); } catch (e) { /* 界面回调失败不影响表单 */ }
      }
    }
    function setError(msg) {
      current.valid = false;
      current.error = msg || "调度不合法";
      error.textContent = current.error;
      preview.textContent = "";
      notify();
    }
    function setPreview(text) {
      preview.textContent = text || "";
    }
    function clearError() {
      error.textContent = "";
    }
    function syncEditable() {
      var custom = st.kind === "custom";
      exprInput.readOnly = !custom;
      exprInput.style.opacity = custom ? "1" : "0.75";
    }

    function buildFields() {
      var html = "";
      if (st.kind === "minutes") {
        html = '<span>每隔</span><input type="number" class="sched-every" min="1" max="59" value="' +
          st.every + '"><span>分钟</span>';
      } else if (st.kind === "hours") {
        html = '<span>每隔</span><input type="number" class="sched-every" min="1" max="23" value="' +
          st.every + '"><span>小时（整点）</span>';
      } else if (st.kind === "weekly") {
        html = '<span class="muted">星期</span>';
        [1, 2, 3, 4, 5, 6, 0].forEach(function (d) {
          html += '<label class="sched-day"><input type="checkbox" class="sched-day-box" value="' + d + '"' +
            (st.days.indexOf(d) >= 0 ? " checked" : "") + "> " + SCHED_DAY_NAMES[d] + "</label>";
        });
        html += '<input type="time" class="sched-time" value="' + hm(st.hour, st.minute) + '">';
      } else if (st.kind === "daily") {
        html = '<span>每天</span><input type="time" class="sched-time" value="' + hm(st.hour, st.minute) + '">';
      } else if (st.kind === "weekdays") {
        html = '<span>每个工作日</span><input type="time" class="sched-time" value="' + hm(st.hour, st.minute) + '">';
      } else if (st.kind === "monthly") {
        html = '<span>每月</span><input type="number" class="sched-daynum" min="1" max="31" value="' +
          st.day + '"><span>日</span><input type="time" class="sched-time" value="' +
          hm(st.hour, st.minute) + '">';
      }
      fields.innerHTML = html;

      var every = fields.querySelector(".sched-every");
      if (every) {
        every.addEventListener("input", function () {
          st.every = Number(every.value);
          schedule();
        });
      }
      var time = fields.querySelector(".sched-time");
      if (time) {
        time.addEventListener("input", function () {
          var t = parseHm(time.value);
          if (t) {
            st.hour = t.hour;
            st.minute = t.minute;
            schedule();
          }
        });
      }
      var daynum = fields.querySelector(".sched-daynum");
      if (daynum) {
        daynum.addEventListener("input", function () {
          st.day = Number(daynum.value);
          schedule();
        });
      }
      Array.prototype.forEach.call(fields.querySelectorAll(".sched-day-box"), function (box) {
        box.addEventListener("change", function () {
          st.days = Array.prototype.filter.call(
            fields.querySelectorAll(".sched-day-box"),
            function (b) { return b.checked; }
          ).map(function (b) { return Number(b.value); });
          schedule();
        });
      });
      syncEditable();
    }

    function readPreset() {
      if (st.kind === "minutes") return { kind: "minutes", every: st.every };
      if (st.kind === "hours") return { kind: "hours", every: st.every };
      if (st.kind === "daily") return { kind: "daily", hour: st.hour, minute: st.minute };
      if (st.kind === "weekly") {
        return { kind: "weekly", days: st.days.slice(), hour: st.hour, minute: st.minute };
      }
      if (st.kind === "monthly") {
        return { kind: "monthly", day: st.day, hour: st.hour, minute: st.minute };
      }
      if (st.kind === "weekdays") return { kind: "weekdays", hour: st.hour, minute: st.minute };
      return null;
    }

    function applyPreset(p) {
      st.kind = p.kind;
      if (p.kind === "minutes" || p.kind === "hours") st.every = Number(p.every);
      if (typeof p.hour === "number") st.hour = p.hour;
      if (typeof p.minute === "number") st.minute = p.minute;
      if (p.kind === "weekly" && Array.isArray(p.days)) st.days = p.days.map(Number);
      if (p.kind === "monthly") st.day = Number(p.day);
      kindSel.value = st.kind;
      buildFields();
    }

    function schedule() {
      var my = ++seq;
      if (timer) clearTimeout(timer);
      timer = setTimeout(function () {
        if (my !== seq) return;
        if (st.kind === "custom") {
          var expr = exprInput.value.trim();
          if (!expr) {
            setError("请填写 cron 表达式");
            return;
          }
          rpc("schedule.describe", { expr: expr }).then(function (r) {
            if (my !== seq) return;
            if (!r.valid) {
              setError(r.error || "表达式无效");
              return;
            }
            current = { valid: true, preset: null, expr: expr, human: r.human || "", error: "" };
            clearError();
            setPreview(r.human || "自定义表达式（按服务器本地时区执行）");
            notify();
          }).catch(function (e) {
            if (my !== seq) return;
            setError("校验失败：" + (e && e.message ? e.message : e));
          });
          return;
        }
        var preset = readPreset();
        rpc("schedule.build", { preset: preset }).then(function (r) {
          if (my !== seq) return;
          if (!r.ok) {
            setError(r.error || "调度参数不合法");
            return;
          }
          exprInput.value = r.expr;
          current = { valid: true, preset: preset, expr: r.expr, human: r.human || "", error: "" };
          clearError();
          setPreview(r.human);
          notify();
        }).catch(function (e) {
          if (my !== seq) return;
          setError("生成表达式失败：" + (e && e.message ? e.message : e));
        });
      }, 120);
    }

    function setExpression(expr) {
      expr = String(expr == null ? "" : expr).trim();
      if (!expr) return;
      var my = ++seq;
      if (timer) clearTimeout(timer);
      exprInput.value = expr;
      rpc("schedule.describe", { expr: expr }).then(function (r) {
        if (my !== seq) return;
        if (!r.valid) {
          st.kind = "custom";
          kindSel.value = "custom";
          buildFields();
          exprInput.value = expr;
          setError(r.error || "表达式无效");
          return;
        }
        if (r.preset) {
          applyPreset(r.preset);
          exprInput.value = expr;
          current = { valid: true, preset: r.preset, expr: expr, human: r.human || "", error: "" };
          clearError();
          setPreview(r.human);
          notify();
        } else {
          // 匹配不到预设：落「自定义」，表达式原样保留，不能丢。
          st.kind = "custom";
          kindSel.value = "custom";
          buildFields();
          exprInput.value = expr;
          current = { valid: true, preset: null, expr: expr, human: r.human || "", error: "" };
          clearError();
          setPreview(r.human || "自定义表达式（按服务器本地时区执行）");
          notify();
        }
      }).catch(function (e) {
        if (my !== seq) return;
        st.kind = "custom";
        kindSel.value = "custom";
        buildFields();
        exprInput.value = expr;
        setError("校验失败：" + (e && e.message ? e.message : e));
      });
    }

    /// 保存前做一次确定性校验：自定义走 describe，预设走 build（由插件翻译）。
    function collect() {
      if (st.kind !== "custom") {
        var preset = readPreset();
        return rpc("schedule.build", { preset: preset }).then(function (r) {
          if (!r.ok) return { valid: false, error: r.error || "调度参数不合法", expr: "" };
          return { valid: true, preset: preset, expr: r.expr, human: r.human || "" };
        });
      }
      var expr = exprInput.value.trim();
      if (!expr) return Promise.resolve({ valid: false, error: "请填写 cron 表达式", expr: "" });
      return rpc("schedule.describe", { expr: expr }).then(function (r) {
        if (!r.valid) return { valid: false, error: r.error || "表达式无效", expr: expr };
        return { valid: true, preset: null, expr: expr, human: r.human || "" };
      });
    }

    kindSel.addEventListener("change", function () {
      st.kind = kindSel.value;
      buildFields();
      // 从预设切到自定义时，把当前表达式带过去，用户可以在原值上改。
      if (st.kind === "custom" && !exprInput.value.trim()) exprInput.value = "0 3 * * *";
      schedule();
    });
    exprInput.addEventListener("input", function () {
      if (st.kind === "custom") schedule();
    });

    if (opts.expr) setExpression(opts.expr);
    else schedule();

    return {
      setExpression: setExpression,
      collect: collect,
      state: function () { return current; }
    };
  }

  var edNew = null;
  var edEdit = null;
  var editingJob = null;
  if (el("sched-new")) edNew = mountScheduleEditor(el("sched-new"), { expr: "0 3 * * *" });

  function findJob(id) {
    return jobs.filter(function (j) { return j.id === id; })[0] || null;
  }

  function openEdit(id) {
    var job = findJob(id);
    if (!job) {
      say("任务已不存在，请刷新", true);
      return;
    }
    var card = el("edit-card");
    if (!card) return;
    editingJob = job;
    card.hidden = false;
    el("edit-title").textContent = job.name + "（" + job.id + "）";
    var msg = el("edit-msg");
    if (msg) { msg.textContent = ""; msg.className = "muted"; }
    if (!edEdit) edEdit = mountScheduleEditor(el("edit-sched"), {});
    edEdit.setExpression(job.expr || "");
    if (card.scrollIntoView) card.scrollIntoView({ block: "nearest" });
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
      // 只给自己的任务提供「改调度」：改动落到同一 key，不碰其他插件/内置任务。
      var edit = isOwnTask(j.id) && !isBuiltinTask(j.id)
        ? '<button class="mini" data-edit="' + esc(j.id) + '">改调度</button> '
        : "";
      var human = j.expr_human ? esc(j.expr_human) + " · " : "自定义表达式 · ";
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
        "<td><code>" + esc(j.expr) + "</code><div class=\"muted\">" + human + "下次 " + esc(next) + "</div></td>" +
        "<td><code>" + esc(j.target) + "</code><div class=\"muted\">" + esc(j.method) + "</div></td>" +
        "<td>" + esc(srcText) + jump + "</td>" +
        '<td class="' + cls + '">' + lastText + "</td>" +
        '<td><button class="mini" data-run="' + esc(j.id) + '">立即执行</button> ' +
          toggle + " " + edit + remove + "</td>" +
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
    edNew.collect().then(function (sched) {
      if (!sched.valid) {
        say("调度无效：" + (sched.error || "请检查调度选项"), true);
        return;
      }
      return rpc("create", {
        name: name,
        expr: sched.expr,
        preset: sched.preset,
        target: target,
        method: method,
        key: key,
        params: params,
        enabled: true
      }).then(function (r) {
        var next = r && r.next_run && r.next_run.text ? "，下次 " + r.next_run.text : "";
        say((r && r.created ? "已新增任务 " : "已更新任务 ") + (r.id || "") + next);
        return load();
      });
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
    var editId = btn.getAttribute("data-edit");
    var jump = btn.getAttribute("data-jump");

    if (editId) {
      openEdit(editId);
      return;
    }
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

  if (el("edit-save")) el("edit-save").addEventListener("click", function () {
    if (!editingJob) return;
    var msg = el("edit-msg");
    if (msg) { msg.textContent = "保存中…"; msg.className = "muted"; }
    edEdit.collect().then(function (sched) {
      if (!sched.valid) {
        if (msg) { msg.textContent = "调度无效：" + (sched.error || "请检查调度选项"); msg.className = "bad"; }
        return;
      }
      return rpc("reschedule", {
        id: editingJob.id,
        expr: sched.expr,
        preset: sched.preset
      }).then(function (r) {
        if (msg) { msg.textContent = "已更新调度：" + ((r && r.expr) || sched.expr); msg.className = "ok"; }
        el("edit-card").hidden = true;
        editingJob = null;
        say("调度已更新");
        return load();
      });
    }).catch(function (e) {
      if (msg) { msg.textContent = "保存失败：" + (e && e.message ? e.message : e); msg.className = "bad"; }
    });
  });

  if (el("edit-cancel")) el("edit-cancel").addEventListener("click", function () {
    el("edit-card").hidden = true;
    editingJob = null;
    say("已取消修改");
  });

  rpc("config").then(function (c) {
    if (edNew && c && c.default_expr) edNew.setExpression(c.default_expr);
  }).catch(function () {});
  refreshAll();
})();
