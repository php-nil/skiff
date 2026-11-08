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
  var defaultExpr = "0 3 * * *";

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
    if (!m) return;
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

  // ---- 调度选择器：新建与编辑共用这一份实现 ----
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
  function defaultSchedState() {
    return { kind: "daily", every: 5, hour: 3, minute: 0, days: [1, 2, 3, 4, 5], day: 1 };
  }

  function mountScheduleEditor(host, opts) {
    opts = opts || {};
    var st = defaultSchedState();
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

    /// 重置为新建默认值；带 expr 时按编辑对象回填。
    function reset(expr) {
      seq++;
      if (timer) clearTimeout(timer);
      st = defaultSchedState();
      current = { valid: true, preset: null, expr: "", human: "", error: "" };
      kindSel.value = "daily";
      exprInput.value = "";
      preview.textContent = "";
      clearError();
      buildFields();
      if (expr) setExpression(expr);
      else schedule();
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
      reset: reset,
      collect: collect,
      state: function () { return current; }
    };
  }

  // ---- 统一确认弹窗（删除等危险操作共用）----
  var confirmDlg = null;
  var confirmResolve = null;
  function ensureConfirm() {
    if (confirmDlg) return confirmDlg;
    var host = el("editor-host");
    if (!host) return null;
    host.insertAdjacentHTML("beforeend",
      '<dialog class="modal" id="confirm-dialog" data-confirm-dialog style="width:min(480px, calc(100vw - 24px))">' +
        '<div class="modal-head"><h2 id="confirm-title">请确认</h2></div>' +
        '<div class="modal-body"><p id="confirm-text" class="muted"></p></div>' +
        '<div class="modal-foot">' +
          '<span class="msg"></span>' +
          '<button id="confirm-cancel">取消</button>' +
          '<button id="confirm-ok" class="danger">确认</button>' +
        '</div>' +
      '</dialog>');
    confirmDlg = el("confirm-dialog");
    el("confirm-cancel").addEventListener("click", function () { closeConfirm(false); });
    el("confirm-ok").addEventListener("click", function () { closeConfirm(true); });
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
    el("confirm-title").textContent = title || "请确认";
    el("confirm-text").textContent = text || "";
    el("confirm-ok").textContent = okLabel || "确认";
    try { dlg.showModal(); } catch (e) { dlg.setAttribute("open", ""); }
    return new Promise(function (resolve) { confirmResolve = resolve; });
  }

  // ---- 新建/编辑弹窗 ----
  var editorHost = el("editor-host");
  var editorDlg = null;
  var schedEditor = null;
  var editingJob = null; // null = 新建
  var focusAfterOpen = false;

  function buildEditor() {
    if (!editorHost) return null;
    if (editorDlg) return editorDlg;
    editorHost.insertAdjacentHTML("beforeend",
      '<dialog class="modal" id="job-dialog">' +
        '<div class="modal-head">' +
          '<h2 id="je-title">新建任务</h2>' +
          '<button class="x" id="je-close" aria-label="关闭">×</button>' +
        '</div>' +
        '<div class="modal-body">' +
          '<div class="form-section">' +
            '<h4>基本信息</h4>' +
            '<label class="field"><span>任务名称</span><input type="text" id="je-name" maxlength="64" placeholder="例如：网站配置每日备份"></label>' +
            '<div class="field-grid" id="je-create-identity">' +
              '<label class="field"><span>类型（目标插件）</span><select id="je-target"></select></label>' +
              '<label class="field"><span>方法</span><select id="je-method"></select></label>' +
            '</div>' +
            '<label class="field" id="je-method-custom" hidden><span>自定义方法名</span><input type="text" id="je-method-text" placeholder="例如 backup"></label>' +
            '<div class="field-grid" id="je-edit-identity" hidden>' +
              '<label class="field"><span>类型（目标插件 / 方法）</span><input type="text" id="je-edit-target" readonly></label>' +
              '<label class="field"><span>任务 ID</span><input type="text" id="je-edit-id" readonly></label>' +
            '</div>' +
            '<label class="field"><span>目标（幂等 key，可空）</span><input type="text" id="je-key" maxlength="64" placeholder="留空则由内核生成"></label>' +
            '<label class="field"><span>调用参数（JSON 对象，可空）</span><textarea id="je-params" placeholder="{&quot;name&quot;:&quot;example.com&quot;}"></textarea></label>' +
            '<p class="hint" id="je-targets-note">可调度目标来自内核目录；看不到目标说明尚未授予 <code>plugin:</code> 授权。</p>' +
          '</div>' +
          '<div class="form-section">' +
            '<h4>调度</h4>' +
            '<div id="je-sched"></div>' +
          '</div>' +
          '<div class="form-section">' +
            '<h4>高级</h4>' +
            '<div class="field-grid">' +
              '<label class="field"><span>超时（秒，1–600）</span><input type="number" id="je-timeout" min="1" max="600" value="30"></label>' +
              '<label class="field"><span>重试次数（0–10）</span><input type="number" id="je-retry" min="0" max="10" value="0"></label>' +
            '</div>' +
            '<div class="field-grid" style="margin-top:8px">' +
              '<label class="field"><span>重试退避（秒，1–3600）</span><input type="number" id="je-backoff" min="1" max="3600" value="30"></label>' +
              '<label class="field"><span>并发策略</span><select id="je-concurrency">' +
                '<option value="skip">跳过重叠</option>' +
                '<option value="queue_one">排队一次</option>' +
              '</select></label>' +
            '</div>' +
            '<div class="checkline" style="margin-top:8px">' +
              '<input type="checkbox" id="je-enabled" checked><span>启用任务</span>' +
            '</div>' +
            '<p class="hint">重试与并发随本次保存写入；内核任务视图未单独回传这两项，未改动时按默认值（0 次 / 跳过重叠）保存。</p>' +
          '</div>' +
          '<div id="je-error" class="bad" style="min-height:18px"></div>' +
        '</div>' +
        '<div class="modal-foot">' +
          '<span class="msg" id="je-foot-msg"></span>' +
          '<button id="je-cancel">取消</button>' +
          '<button id="je-save" class="primary">保存</button>' +
        '</div>' +
      '</dialog>');
    editorDlg = el("job-dialog");
    schedEditor = mountScheduleEditor(el("je-sched"), {});

    el("je-close").addEventListener("click", closeEditor);
    el("je-cancel").addEventListener("click", closeEditor);
    editorDlg.addEventListener("cancel", function (ev) {
      ev.preventDefault();
      closeEditor();
    });

    el("je-target").addEventListener("change", function () {
      el("je-method-custom").hidden = true;
      fillMethods(el("je-target").value);
    });
    el("je-method").addEventListener("change", function () {
      var custom = el("je-method").value === "__custom";
      el("je-method-custom").hidden = !custom;
      if (custom) el("je-method-text").focus();
    });

    el("je-save").addEventListener("click", saveEditor);
    return editorDlg;
  }

  function fillTargets() {
    var sel = el("je-target");
    if (!sel) return;
    sel.innerHTML = "";
    if (targets.length === 0) {
      sel.innerHTML = '<option value="">（无可调用目标）</option>';
      el("je-method").innerHTML = '<option value="">（无方法）</option>';
      return;
    }
    targets.forEach(function (t) {
      var o = document.createElement("option");
      o.value = t.id;
      o.textContent = (t.name || t.id) + "（" + t.id + "）";
      sel.appendChild(o);
    });
    fillMethods(targets[0].id);
  }

  function fillMethods(pluginId) {
    var t = targets.filter(function (x) { return x.id === pluginId; })[0];
    var sel = el("je-method");
    sel.innerHTML = "";
    if (!t || !t.methods || t.methods.length === 0) {
      sel.innerHTML = '<option value="">（无方法）</option>';
      el("je-targets-note").textContent = "该目标没有可调用方法。";
      return;
    }
    t.methods.forEach(function (m) {
      var o = document.createElement("option");
      o.value = m.method;
      o.textContent = (m.label || m.description || m.method) + "（" + m.method + "）";
      sel.appendChild(o);
    });
    var custom = document.createElement("option");
    custom.value = "__custom";
    custom.textContent = "自定义方法名…";
    sel.appendChild(custom);
    el("je-targets-note").textContent = t.methods[0].description || t.methods[0].label || "";
  }

  function clearEditorError() {
    el("je-error").textContent = "";
    el("je-foot-msg").textContent = "";
    el("je-foot-msg").className = "msg";
  }
  function editorError(text) {
    el("je-error").textContent = text;
  }
  function footMessage(text, bad) {
    var m = el("je-foot-msg");
    m.textContent = text;
    m.className = "msg " + (bad ? "bad" : "muted");
  }

  function openEditor(job) {
    var dlg = buildEditor();
    if (!dlg) return;
    editingJob = job || null;
    clearEditorError();
    el("je-title").textContent = editingJob ? "编辑任务" : "新建任务";

    var createBlock = el("je-create-identity");
    var editBlock = el("je-edit-identity");
    createBlock.hidden = !!editingJob;
    editBlock.hidden = !editingJob;
    el("je-method-custom").hidden = true;

    if (editingJob) {
      el("je-name").value = editingJob.name || "";
      el("je-edit-target").value = (editingJob.target || "") + (editingJob.method ? " / " + editingJob.method : "");
      el("je-edit-id").value = editingJob.id || "";
      el("je-key").value = editingJob.key || "";
      el("je-key").readOnly = true;
      el("je-params").value = JSON.stringify(editingJob.payload && editingJob.payload.params || {}, null, 2);
      el("je-timeout").value = String(editingJob.timeout_secs || 30);
      el("je-retry").value = "0";
      el("je-backoff").value = "30";
      el("je-concurrency").value = "skip";
      el("je-enabled").checked = !!editingJob.enabled;
      schedEditor.reset(editingJob.expr || "");
      footMessage("修改后保存，任务 ID 与目标不会变化");
    } else {
      fillTargets();
      el("je-name").value = "";
      el("je-key").value = "";
      el("je-key").readOnly = false;
      el("je-params").value = "";
      el("je-timeout").value = "30";
      el("je-retry").value = "0";
      el("je-backoff").value = "30";
      el("je-concurrency").value = "skip";
      el("je-enabled").checked = true;
      schedEditor.reset(defaultExpr);
      footMessage("保存后写入内核调度器");
    }
    try { dlg.showModal(); } catch (e) { dlg.setAttribute("open", ""); }
    setTimeout(function () {
      if (el("je-name")) el("je-name").focus();
      scheduleHeight();
    }, 0);
  }

  function closeEditor() {
    if (!editorDlg) return;
    try { editorDlg.close(); } catch (e) { editorDlg.removeAttribute("open"); }
    editingJob = null;
    scheduleHeight();
  }

  function intField(id, min, max, label) {
    var raw = el(id).value.trim();
    var n = Number(raw);
    if (!raw || !isFinite(n) || Math.floor(n) !== n) throw new Error(label + "必须是整数");
    if (n < min || n > max) throw new Error(label + "需在 " + min + "–" + max + " 之间");
    return n;
  }

  function saveEditor() {
    if (!editorDlg) return;
    clearEditorError();
    var name = el("je-name").value.trim();
    var target;
    var method;
    var key = "";
    if (editingJob) {
      target = editingJob.target;
      method = editingJob.method;
      key = editingJob.key || "";
    } else {
      target = el("je-target").value;
      method = el("je-method").value === "__custom"
        ? el("je-method-text").value.trim()
        : el("je-method").value;
      key = el("je-key").value.trim();
      if (!target) { editorError("请选择目标插件"); return; }
      if (!method) { editorError("请填写方法名"); return; }
      if (!/^[A-Za-z0-9._-]{0,64}$/.test(key)) {
        editorError("幂等 key 只允许字母 / 数字 / . / - / _，最长 64");
        return;
      }
    }
    if (!name) { editorError("任务名称不能为空"); return; }
    if (name.length > 64) { editorError("任务名称不超过 64 字符"); return; }

    var params = {};
    var rawParams = el("je-params").value.trim();
    if (rawParams) {
      try {
        params = JSON.parse(rawParams);
      } catch (e) {
        editorError("调用参数不是合法 JSON：" + e.message);
        return;
      }
      if (params === null || typeof params !== "object" || Array.isArray(params)) {
        editorError("调用参数必须是 JSON 对象");
        return;
      }
    }

    var timeout, retry, backoff;
    try {
      timeout = intField("je-timeout", 1, 600, "超时");
      retry = intField("je-retry", 0, 10, "重试次数");
      backoff = intField("je-backoff", 1, 3600, "重试退避");
    } catch (e) {
      editorError(e.message);
      return;
    }
    var enabled = !!el("je-enabled").checked;
    var concurrency = el("je-concurrency").value;

    var saveBtn = el("je-save");
    saveBtn.disabled = true;
    footMessage("保存中…");
    schedEditor.collect().then(function (sched) {
      if (!sched || !sched.valid) {
        editorError("调度无效：" + ((sched && sched.error) || "请检查调度选项"));
        footMessage("保存失败", true);
        return;
      }
      var payload = {
        name: name,
        expr: sched.expr,
        preset: sched.preset,
        params: params,
        enabled: enabled,
        timeout_secs: timeout,
        retry_max: retry,
        backoff_secs: backoff,
        concurrency: concurrency,
      };
      var call;
      if (editingJob) {
        payload.id = editingJob.id;
        call = rpc("update", payload);
      } else {
        payload.target = target;
        payload.method = method;
        payload.key = key;
        call = rpc("create", payload);
      }
      return call.then(function (r) {
        var next = r && r.next_run && r.next_run.text ? "，下次 " + r.next_run.text : "";
        say((r && r.created ? "已新增任务 " : "已保存任务 ") + ((r && r.id) || "") + next);
        closeEditor();
        return load();
      });
    }).catch(function (e) {
      var text = e && e.message ? e.message : String(e);
      editorError("保存失败：" + text);
      footMessage("保存失败，已保留填写内容", true);
      say("保存失败：" + text, true);
    }).then(function () {
      saveBtn.disabled = false;
    });
  }

  // ---- 列表 ----
  function findJob(id) {
    return jobs.filter(function (j) { return j.id === id; })[0] || null;
  }

  function matchFilter(j, q, flt) {
    if (flt === "enabled" && !j.enabled) return false;
    if (flt === "disabled" && j.enabled) return false;
    if (flt === "builtin" && !isBuiltinTask(j.id)) return false;
    if (flt === "own" && !isOwnTask(j.id)) return false;
    if (flt === "other" && (isOwnTask(j.id) || isBuiltinTask(j.id))) return false;
    if (!q) return true;
    var hay = [j.name, j.id, j.target, j.method, j.source].join(" ").toLowerCase();
    return hay.indexOf(q) >= 0;
  }

  function render() {
    var body = el("jobs");
    if (!body) return;
    var q = (el("q") && el("q").value || "").trim().toLowerCase();
    var flt = el("flt") ? el("flt").value : "all";
    var rows = jobs.filter(function (j) { return matchFilter(j, q, flt); });
    if (jobs.length === 0) {
      body.innerHTML = '<tr><td colspan="6" class="empty">还没有任务。点击右上角「新建任务」创建，或从其他插件的界面一键添加。</td></tr>';
      return;
    }
    if (rows.length === 0) {
      body.innerHTML = '<tr><td colspan="6" class="empty">没有匹配的任务，试试调整搜索或筛选。</td></tr>';
      return;
    }
    body.innerHTML = rows.map(function (j) {
      var next = j.next_run && j.next_run.text ? j.next_run.text : "（表达式无效）";
      var src = j.source || "ui";
      var srcText = src === "ui" ? "本面板" : src;
      var jump = src !== "ui" && isPluginId(src)
        ? ' <button class="mini" data-jump="' + esc(src) + '">打开</button>'
        : "";
      var last = j.last;
      var lastText = "从未执行";
      var cls = "muted";
      var lastTitle = "";
      if (last && last.at) {
        lastText = when(last.at) + " · " + (last.result === "ok" ? "成功" : "失败");
        cls = last.result === "ok" ? "ok" : "bad";
        if (last.detail) lastTitle = last.detail;
      }
      var toggle = j.enabled
        ? '<button class="mini" data-toggle="' + esc(j.id) + '" data-enabled="0">停用</button>'
        : '<button class="mini" data-toggle="' + esc(j.id) + '" data-enabled="1">启用</button>';
      // 内置任务不可删除：不渲染删除按钮，保留启停。
      var remove = isBuiltinTask(j.id)
        ? '<span class="muted">内置</span>'
        : '<button class="mini danger" data-remove="' + esc(j.id) + '">删除</button>';
      // 只给自己的任务提供编辑：内核对 task.update 不改变任务身份。
      var edit = isOwnTask(j.id)
        ? '<button class="mini" data-edit="' + esc(j.id) + '">编辑</button> '
        : "";
      var human = j.expr_human ? esc(j.expr_human) + " · " : "自定义表达式 · ";
      var stateText = "";
      var stateCls = "muted";
      if (j.state === "blocked" || j.lifecycle === "blocked") {
        stateText = "已阻塞" + (j.blocked_reason ? "：" + esc(j.blocked_reason) : "");
        stateCls = "warn";
      } else if (j.state === "running") {
        stateText = "执行中";
        stateCls = "warn";
      }
      var timeout = j.timeout_secs && j.timeout_secs !== 30 ? " · 超时 " + j.timeout_secs + "s" : "";
      return '<tr data-id="' + esc(j.id) + '"' + (j.id === focusId ? ' class="hit"' : "") + ">" +
        '<td class="cut" title="' + esc(j.name + " · " + j.id + (j.key ? " · key " + j.key : "")) + '">' +
          "<strong>" + esc(j.name) + "</strong>" +
          '<div class="muted">' + esc(j.id) + (j.key ? " · key " + esc(j.key) : "") + timeout +
          (stateText ? ' · <span class="' + stateCls + '">' + stateText + "</span>" : "") +
          "</div></td>" +
        '<td class="cut" title="' + esc((j.expr || "") + " " + (j.expr_human || "")) + '">' +
          "<code>" + esc(j.expr) + "</code>" +
          '<div class="muted">' + esc(human + "下次 " + next) + "</div></td>" +
        '<td class="cut" title="' + esc((j.target || "") + " / " + (j.method || "")) + '">' +
          "<code>" + esc(j.target) + "</code>" +
          '<div class="muted">' + esc(j.method) + "</div></td>" +
        '<td class="cut" title="' + esc(srcText) + '">' + esc(srcText) + jump + "</td>" +
        '<td class="cut ' + cls + '" title="' + esc(lastTitle) + '">' + esc(lastText) + "</td>" +
        '<td class="actions">' +
          '<button class="mini" data-run="' + esc(j.id) + '">执行</button> ' +
          toggle + " " + edit + remove +
        "</td>" +
        "</tr>";
    }).join("");

    if (focusId) {
      var hitRows = body.querySelectorAll("tr[data-id]");
      for (var i = 0; i < hitRows.length; i++) {
        if (hitRows[i].getAttribute("data-id") === focusId) {
          if (hitRows[i].scrollIntoView) hitRows[i].scrollIntoView({ block: "center" });
          break;
        }
      }
    }
  }

  function load() {
    return rpc("list").then(function (r) {
      jobs = (r && r.jobs) || [];
      render();
      scheduleHeight();
    }).catch(function (e) {
      say("读取任务失败：" + e, true);
    });
  }

  function loadTargets() {
    return rpc("targets").then(function (r) {
      targets = (r && r.interfaces) || [];
    }).catch(function (e) {
      targets = [];
      say("读取可调用目标失败：" + e, true);
    });
  }

  function refreshAll() {
    return Promise.all([load(), loadTargets(), loadConfig()]);
  }

  function loadConfig() {
    return rpc("config").then(function (c) {
      if (c && c.default_expr) defaultExpr = c.default_expr;
    }).catch(function () { /* 配置读取失败不影响默认值 */ });
  }

  // ---- 事件接线 ----
  var isList = !!el("jobs");
  if (isList) {
    if (el("q")) el("q").addEventListener("input", render);
    if (el("flt")) el("flt").addEventListener("change", render);
    if (el("refresh")) el("refresh").addEventListener("click", function () {
      say("刷新中…");
      refreshAll().then(function () { say("已刷新"); });
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
        var job = findJob(editId);
        if (!job) { say("任务已不存在，请刷新", true); return; }
        openEditor(job);
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
          say("内置任务不可删除（只能停用）", true);
          return;
        }
        var jobName = (findJob(removeId) || {}).name || removeId;
        confirmAction("删除任务", "确认删除任务「" + jobName + "」？该任务会立即停止调度，无法恢复。", "删除").then(function (ok) {
          if (!ok) return;
          rpc("remove", { id: removeId }).then(function () {
            say("任务已删除");
            return load();
          }).catch(function (e) {
            say("删除失败：" + e, true);
          });
        });
      }
    });
  }

  if (el("btn-new")) el("btn-new").addEventListener("click", function () {
    openEditor(null);
  });

  // 列表页首次打开也加载目标，保证「新建任务」弹窗即开即用。
  refreshAll().then(function () {
    // 新建任务页：进页即弹窗；标题与菜单项保持一致。
    if (!isList) openEditor(null);
  });
})();
