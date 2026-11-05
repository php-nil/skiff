// 概览槽位卡片：验证 mount iframe 也能拿到 nonce、走 RPC、跟随主题。
(function () {
  var params = new URLSearchParams(location.search);
  var nonce = params.get("nonce") || "";
  var seq = 0;
  var pending = {};

  window.addEventListener("message", function (ev) {
    if (ev.source !== parent) return;
    var msg = ev.data;
    if (msg && msg.type === "skiff.theme" && (msg.theme === "light" || msg.theme === "dark")) {
      document.documentElement.setAttribute("data-theme", msg.theme);
    }
    if (!msg || msg.type !== "skiff.rpc.result" || msg.nonce !== nonce) return;
    var p = pending[msg.id];
    if (!p) return;
    delete pending[msg.id];
    if (msg.error) p.reject(new Error(msg.error.message || String(msg.error)));
    else p.resolve(msg.result);
  });

  function requestTheme() {
    try { parent.postMessage({ type: "skiff.theme.request" }, "*"); } catch (e) { /* 独立打开 */ }
  }
  requestTheme();
  window.addEventListener("load", requestTheme);

  function rpc(method) {
    return new Promise(function (resolve, reject) {
      if (!nonce) { reject(new Error("缺少 nonce")); return; }
      var id = String(++seq);
      pending[id] = { resolve: resolve, reject: reject };
      parent.postMessage({ type: "skiff.rpc", nonce: nonce, id: id, method: method, params: null }, "*");
    });
  }

  var state = document.getElementById("state");
  var btn = document.getElementById("mount-check");
  if (btn) {
    btn.addEventListener("click", function () {
      state.className = "";
      state.textContent = "调用 ping …";
      rpc("ping").then(function (r) {
        state.className = "ok";
        state.textContent = "槽位 RPC 可用 · " + (r && r.id) + " @" + (r && r.at);
      }).catch(function (e) {
        state.className = "bad";
        state.textContent = "槽位 RPC 失败：" + String(e);
      });
    });
  }
})();
