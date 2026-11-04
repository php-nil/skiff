// 概览页「系统状态」槽位：只读展示，无数据时给明确空态，不渲染空白卡。
(function () {
  var nonce = new URLSearchParams(location.search).get("nonce") || "";
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

  function rpc(method) {
    return new Promise(function (resolve, reject) {
      if (!nonce) {
        reject(new Error("缺少 nonce：请从面板打开"));
        return;
      }
      var id = String(++seq);
      pending[id] = { resolve: resolve, reject: reject };
      parent.postMessage({ type: "skiff.rpc", nonce: nonce, id: id, method: method, params: null }, "*");
    });
  }

  window.addEventListener("message", function (ev) {
    if (ev.source !== parent) return;
    var msg = ev.data;
    if (!msg || msg.nonce !== nonce) return;
    if (msg.type === "skiff.rpc.result") {
      var p = pending[msg.id];
      if (!p) return;
      delete pending[msg.id];
      if (msg.error) p.reject(msg.error.message || msg.error);
      else p.resolve(msg.result);
    } else if (msg.type === "skiff.refresh") {
      refresh();
    }
  });

  function el(id) { return document.getElementById(id); }
  function pct(v) { return v == null ? "-" : (Number(v).toFixed(0) + "%"); }
  function dur(sec) {
    if (!sec) return "-";
    var d = Math.floor(sec / 86400), h = Math.floor((sec % 86400) / 3600);
    return (d ? d + "天" : "") + h + "时";
  }
  function empty(text) {
    el("metrics").hidden = true;
    el("empty").hidden = false;
    el("empty").textContent = text;
  }
  function paint(s) {
    el("cpu").textContent = pct(s.cpu);
    el("mem").textContent = s.mem_total_kb ? pct(s.mem_used_kb / s.mem_total_kb * 100) : "-";
    el("load").textContent = s.load1 == null ? "-" : Number(s.load1).toFixed(2);
    el("uptime").textContent = dur(s.uptime);
    el("metrics").hidden = false;
    el("empty").hidden = true;
  }

  function refresh() {
    return rpc("snapshot").then(function (r) {
      if (!r || !r.sample) {
        empty("暂无采样数据（插件进程未启动或 /proc 读取失败）");
        return;
      }
      paint(r.sample);
    }).catch(function (e) {
      empty("暂无采样数据：" + String(e && e.message ? e.message : e));
    });
  }

  refresh();
})();
