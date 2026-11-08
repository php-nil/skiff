/* 回收站页交互：列表 / 恢复 / 彻底删除 / 清空。 */
(function () {
  "use strict";

  var page = document.querySelector("[data-recycle]");
  if (!page) {
    return;
  }

  var prefix = page.getAttribute("data-prefix") || "";
  var csrf = page.getAttribute("data-csrf") || "";
  var retention = parseInt(page.getAttribute("data-retention") || "30", 10);

  var el = {
    alert: page.querySelector("[data-recycle-alert]"),
    alertText: page.querySelector("[data-recycle-alert-text]"),
    alertClose: page.querySelector("[data-recycle-alert-close]"),
    summary: page.querySelector("[data-recycle-summary]"),
    body: page.querySelector("[data-recycle-body]"),
    refresh: page.querySelector("[data-recycle-refresh]"),
    clear: page.querySelector("[data-recycle-clear]")
  };

  var items = [];

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

  function fmtTime(ts) {
    if (!ts) {
      return "—";
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

  function kindLabel(kind) {
    return kind === "dir" ? "目录" : kind === "symlink" ? "链接" : "文件";
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

  function render() {
    if (!el.body) {
      return;
    }
    if (!items.length) {
      el.body.innerHTML = '<tr><td colspan="8" class="recycle-empty">回收站为空</td></tr>';
      if (el.summary) {
        el.summary.textContent = "共 0 项 · 保留 " + retention + " 天，到期自动清理";
      }
      if (el.clear) {
        el.clear.disabled = true;
      }
      return;
    }
    var bytes = 0;
    var html = items.map(function (item) {
      bytes += Number(item.size || 0);
      return (
        '<tr data-id="' + escapeAttr(item.id) + '">' +
        '<td class="recycle-path" title="' + escapeAttr(item.original_path) + '">' +
        escapeHtml(item.original_path) + "</td>" +
        "<td>" + kindLabel(item.kind) + "</td>" +
        "<td>" + fmtBytes(item.size) + "</td>" +
        "<td>" + Number(item.files || 0) + "</td>" +
        "<td>" + fmtTime(item.deleted_at) + "</td>" +
        '<td class="recycle-muted">' + escapeHtml(item.actor_type + ":" + item.actor_id) + "</td>" +
        "<td>" + fmtTime(item.expires_at) + "</td>" +
        '<td><div class="recycle-row-actions">' +
        '<button type="button" class="files-btn" data-act="restore">恢复</button>' +
        '<button type="button" class="files-btn files-btn-danger" data-act="purge">彻底删除</button>' +
        "</div></td>" +
        "</tr>"
      );
    }).join("");
    el.body.innerHTML = html;
    if (el.summary) {
      el.summary.textContent =
        "共 " + items.length + " 项 · 占用 " + fmtBytes(bytes) +
        " · 保留 " + retention + " 天，到期由 trash-prune 自动清理";
    }
    if (el.clear) {
      el.clear.disabled = false;
    }
  }

  function escapeHtml(value) {
    return String(value == null ? "" : value)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#39;");
  }

  function escapeAttr(value) {
    return escapeHtml(value);
  }

  function load() {
    setBusy(true);
    request("trash", { method: "GET" })
      .then(function (data) {
        items = data.items || [];
        render();
      })
      .catch(function (err) {
        showAlert("danger", "加载回收站失败：" + err.message);
      })
      .finally(function () {
        setBusy(false);
      });
  }

  function setBusy(busy) {
    if (el.refresh) {
      el.refresh.disabled = busy;
    }
  }

  function findItem(id) {
    return items.filter(function (item) {
      return item.id === id;
    })[0];
  }

  function restore(id, overwrite) {
    return request("trash/restore", {
      method: "POST",
      body: JSON.stringify({ id: id, overwrite: !!overwrite })
    });
  }

  function onRestore(id) {
    var item = findItem(id);
    if (!item) {
      return;
    }
    restore(id, false)
      .then(function () {
        showAlert("ok", "已恢复到 " + item.original_path);
        load();
      })
      .catch(function (err) {
        if (err.status === 409) {
          var replace = window.confirm(
            "恢复失败：原路径已存在同名对象。\n是否覆盖现有对象并恢复？\n\n" +
            item.original_path
          );
          if (!replace) {
            showAlert("warn", "恢复已取消：" + err.message);
            return;
          }
          return restore(id, true).then(function () {
            showAlert("ok", "已覆盖并恢复到 " + item.original_path);
            load();
          });
        }
        showAlert("danger", "恢复失败：" + err.message);
      });
  }

  function onPurge(id) {
    var item = findItem(id);
    if (!item) {
      return;
    }
    var ok = window.confirm(
      "彻底删除后不可恢复，确认永久删除？\n\n" + item.original_path +
      "\n大小：" + fmtBytes(item.size)
    );
    if (!ok) {
      return;
    }
    request("trash/purge", {
      method: "POST",
      body: JSON.stringify({ id: id })
    })
      .then(function () {
        showAlert("ok", "已彻底删除 " + item.original_path);
        load();
      })
      .catch(function (err) {
        showAlert("danger", "彻底删除失败：" + err.message);
      });
  }

  function onClear() {
    if (!items.length) {
      return;
    }
    var bytes = items.reduce(function (sum, item) {
      return sum + Number(item.size || 0);
    }, 0);
    var ok = window.confirm(
      "将永久删除回收站内全部 " + items.length + " 个条目（共 " + fmtBytes(bytes) +
      "）。\n清空后无法恢复，且不会进入任何备份。\n\n确认继续？"
    );
    if (!ok) {
      return;
    }
    request("trash/clear", { method: "POST", body: "{}" })
      .then(function (data) {
        var failed = (data.result && data.result.failed) || [];
        if (failed.length) {
          showAlert("warn", "已清理 " + data.result.removed + " 项，" + failed.length +
            " 项失败：" + failed.join("；"));
        } else {
          showAlert("ok", "已清空回收站，释放 " + fmtBytes(data.result.bytes));
        }
        load();
      })
      .catch(function (err) {
        showAlert("danger", "清空失败：" + err.message);
      });
  }

  if (el.refresh) {
    el.refresh.addEventListener("click", function () {
      hideAlert();
      load();
    });
  }
  if (el.clear) {
    el.clear.addEventListener("click", onClear);
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
      var row = btn.closest("tr[data-id]");
      if (!row) {
        return;
      }
      var id = row.getAttribute("data-id");
      if (btn.getAttribute("data-act") === "restore") {
        onRestore(id);
      } else {
        onPurge(id);
      }
    });
  }

  load();
})();
