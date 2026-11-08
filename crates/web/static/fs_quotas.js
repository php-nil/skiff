/* 配额页交互：列表 / 设置 / 修改 / 取消 / 重新统计。 */
(function () {
  "use strict";

  var page = document.querySelector("[data-quota]");
  if (!page) {
    return;
  }

  var prefix = page.getAttribute("data-prefix") || "";
  var csrf = page.getAttribute("data-csrf") || "";

  var el = {
    alert: page.querySelector("[data-quota-alert]"),
    alertText: page.querySelector("[data-quota-alert-text]"),
    alertClose: page.querySelector("[data-quota-alert-close]"),
    body: page.querySelector("[data-quota-body]"),
    refresh: page.querySelector("[data-quota-refresh]"),
    form: page.querySelector("[data-quota-form]"),
    reset: page.querySelector("[data-quota-reset]"),
    path: page.querySelector("#quota-path"),
    bytes: page.querySelector("#quota-bytes"),
    files: page.querySelector("#quota-files")
  };

  var quotas = [];

  function apiUrl(path) {
    return "/" + prefix + "/files/api" + path;
  }

  function showAlert(kind, text) {
    if (!el.alert) {
      return;
    }
    el.alert.setAttribute("data-kind", kind);
    el.alertText.textContent = text;
    el.alert.hidden = false;
  }

  function hideAlert() {
    if (el.alert) {
      el.alert.hidden = true;
    }
  }

  function fmtBytes(n) {
    n = Number(n || 0);
    if (n < 1024) {
      return n + " B";
    }
    var units = ["KB", "MB", "GB", "TB"];
    var i = -1;
    do {
      n /= 1024;
      i += 1;
    } while (n >= 1024 && i < units.length - 1);
    return n.toFixed(n >= 10 ? 1 : 2) + " " + units[i];
  }

  function fmtNumber(n) {
    return Number(n || 0).toLocaleString("zh-CN");
  }

  function fmtTime(ts) {
    if (!ts) {
      return "仅增量计数";
    }
    var d = new Date(Number(ts) * 1000);
    if (isNaN(d.getTime())) {
      return "—";
    }
    var pad = function (v) {
      return String(v).padStart(2, "0");
    };
    return d.getFullYear() + "-" + pad(d.getMonth() + 1) + "-" + pad(d.getDate()) +
      " " + pad(d.getHours()) + ":" + pad(d.getMinutes());
  }

  function escapeHtml(value) {
    return String(value == null ? "" : value)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#39;");
  }

  function request(path, options) {
    options = options || {};
    options.headers = Object.assign(
      { "Content-Type": "application/json", "x-csrf-token": csrf },
      options.headers || {}
    );
    return fetch(apiUrl(path), options).then(function (res) {
      return res.text().then(function (raw) {
        var data = null;
        try {
          data = raw ? JSON.parse(raw) : null;
        } catch (e) {
          data = null;
        }
        if (!res.ok || !data || data.ok === false) {
          var err = new Error(
            (data && data.error && data.error.message) || ("请求失败：" + res.status)
          );
          err.code = data && data.error ? data.error.code : String(res.status);
          err.status = res.status;
          throw err;
        }
        return data;
      });
    });
  }

  function usageCell(used, max, fmt) {
    if (max == null) {
      return (
        '<div class="quota-usage"><strong>' + fmt(used) + "</strong>" +
        '<span class="quota-muted">未设置上限（仅统计）</span></div>'
      );
    }
    var pct = max > 0 ? (Number(used) / Number(max)) * 100 : 100;
    var cls = pct >= 100 ? "recycle-danger" : pct >= 90 ? "files-warn" : "";
    return (
      '<div class="quota-usage"><strong class="' + cls + '">' + fmt(used) +
      " / " + fmt(max) + "</strong>" +
      '<span class="quota-muted">' + pct.toFixed(1) + "%</span></div>"
    );
  }

  function render() {
    if (!el.body) {
      return;
    }
    if (!quotas.length) {
      el.body.innerHTML =
        '<tr><td colspan="5" class="quota-empty">尚未设置目录配额</td></tr>';
      return;
    }
    el.body.innerHTML = quotas.map(function (q) {
      var hasLimit = q.max_bytes != null || q.max_files != null;
      return (
        '<tr data-root="' + escapeHtml(q.root) + '">' +
        '<td class="quota-root" title="' + escapeHtml(q.root) + '">' +
        escapeHtml(q.root) + "</td>" +
        "<td>" + usageCell(q.used_bytes, q.max_bytes, fmtBytes) + "</td>" +
        "<td>" + usageCell(q.used_files, q.max_files, fmtNumber) + "</td>" +
        "<td>" + fmtTime(q.updated_at) + "</td>" +
        '<td><div class="quota-row-actions">' +
        '<button type="button" class="files-btn" data-act="scan">重新统计</button>' +
        '<button type="button" class="files-btn" data-act="edit">修改</button>' +
        (hasLimit
          ? '<button type="button" class="files-btn files-btn-danger" data-act="cancel">取消配额</button>'
          : "") +
        "</div></td>" +
        "</tr>"
      );
    }).join("");
  }

  function load() {
    if (el.refresh) {
      el.refresh.disabled = true;
    }
    request("quotas", { method: "GET" })
      .then(function (data) {
        quotas = data.quotas || [];
        render();
      })
      .catch(function (err) {
        showAlert("danger", "加载配额失败：" + err.message);
      })
      .finally(function () {
        if (el.refresh) {
          el.refresh.disabled = false;
        }
      });
  }

  function findQuota(root) {
    return quotas.filter(function (q) {
      return q.root === root;
    })[0];
  }

  function resetForm() {
    if (el.path) {
      el.path.value = "";
      el.path.removeAttribute("readonly");
    }
    if (el.bytes) {
      el.bytes.value = "";
    }
    if (el.files) {
      el.files.value = "";
    }
  }

  function fillForm(q) {
    if (!el.path) {
      return;
    }
    el.path.value = q.root;
    el.bytes.value = q.max_bytes == null ? "" : String(q.max_bytes);
    el.files.value = q.max_files == null ? "" : String(q.max_files);
    el.path.focus();
  }

  function save(event) {
    event.preventDefault();
    var path = (el.path && el.path.value || "").trim();
    var maxBytes = (el.bytes && el.bytes.value || "").trim();
    var maxFiles = (el.files && el.files.value || "").trim();
    if (!path) {
      showAlert("warn", "请填写目录树的绝对路径");
      return;
    }
    if (!maxBytes && !maxFiles) {
      showAlert("warn", "容量上限与文件数上限至少设置一项；如需取消请用行内「取消配额」");
      return;
    }
    request("quotas/set", {
      method: "POST",
      body: JSON.stringify({
        path: path,
        max_bytes: maxBytes || null,
        max_files: maxFiles || null
      })
    })
      .then(function (data) {
        showAlert("ok", "已保存配额：" + data.quota.root);
        resetForm();
        load();
      })
      .catch(function (err) {
        showAlert("danger", "保存配额失败：" + err.message);
      });
  }

  function scan(root) {
    window.skiffConfirm(
      "重新统计会递归扫描整棵目录树，期间可能阻塞较久（成本 O(条目数)）。\n\n" + root,
      "开始统计"
    ).then(function (ok) {
      if (!ok) {
        return;
      }
      return request("quotas/scan", {
        method: "POST",
        body: JSON.stringify({ path: root })
      })
        .then(function (data) {
          showAlert("ok", "统计完成：" + data.quota.root + "，已用 " +
            fmtBytes(data.quota.used_bytes) + " / " + fmtNumber(data.quota.used_files) + " 个文件");
          load();
        })
        .catch(function (err) {
          showAlert("danger", "重新统计失败：" + err.message);
        });
    });
  }

  function cancel(root) {
    var q = findQuota(root);
    if (!q) {
      return;
    }
    window.skiffConfirm(
      "取消配额后不再限制该目录树的写入，但已用量统计会保留。\n\n" + root,
      "取消配额"
    ).then(function (ok) {
      if (!ok) {
        return;
      }
      return request("quotas/cancel", {
        method: "POST",
        body: JSON.stringify({ path: root })
      })
        .then(function () {
          showAlert("ok", "已取消配额：" + root);
          load();
        })
        .catch(function (err) {
          showAlert("danger", "取消配额失败：" + err.message);
        });
    });
  }

  if (el.form) {
    el.form.addEventListener("submit", save);
  }
  if (el.reset) {
    el.reset.addEventListener("click", function () {
      resetForm();
      hideAlert();
    });
  }
  if (el.refresh) {
    el.refresh.addEventListener("click", function () {
      hideAlert();
      load();
    });
  }
  if (el.alertClose) {
    el.alertClose.addEventListener("click", hideAlert);
  }
  if (el.body) {
    el.body.addEventListener("click", function (event) {
      var btn = event.target.closest("[data-act]");
      if (!btn) {
        return;
      }
      var row = btn.closest("tr[data-root]");
      if (!row) {
        return;
      }
      var root = row.getAttribute("data-root");
      var act = btn.getAttribute("data-act");
      if (act === "scan") {
        scan(root);
      } else if (act === "edit") {
        var q = findQuota(root);
        if (q) {
          fillForm(q);
        }
      } else if (act === "cancel") {
        cancel(root);
      }
    });
  }

  load();
})();
