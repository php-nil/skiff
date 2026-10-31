// Skiff 前端交互（无 Node 构建链）：日志 SSE、容器终端、插件 UI 桥。
(function () {
  "use strict";

  // ---- 日志跟随（SSE）：断连由浏览器 EventSource 自身重连；页面卸载即断开 ----
  function attachLogs() {
    var el = document.getElementById("logs");
    if (!el || el.dataset.attached === "1") return;
    var src = el.dataset.src;
    if (!src) return;
    el.dataset.attached = "1";
    var es = new EventSource(src);
    es.onmessage = function (ev) {
      el.textContent += ev.data + "\n";
      if (el.textContent.length > 400000) el.textContent = el.textContent.slice(-200000);
      el.scrollTop = el.scrollHeight;
    };
    es.addEventListener("error", function () {
      el.textContent += "\n[连接中断]\n";
      es.close();
    });
    window.addEventListener("beforeunload", function () { es.close(); });
  }

  // ---- 容器终端：懒加载 xterm，先换一次性票据再连 WS（§6.2）----
  // 票据绑定会话 + 容器、30 秒有效、用一次即废，所以每次打开终端都要重新申请。
  function attachTerminal() {
    var el = document.getElementById("term");
    if (!el || el.dataset.attached === "1") return;
    var wsUrl = el.dataset.ws;
    var ticketUrl = el.dataset.ticket;
    if (!wsUrl || !ticketUrl) return;
    if (typeof window.Terminal === "undefined") {
      el.innerHTML = '<p class="hint">终端组件未就绪（xterm 未加载成功）。</p>';
      return;
    }
    el.dataset.attached = "1";
    var term = new window.Terminal({ fontSize: 12, theme: { background: "#0b0d11" }, cursorBlink: true });
    term.open(el);
    var proto = location.protocol === "https:" ? "wss:" : "ws:";
    term.writeln("\u001b[33m[申请终端票据…]\u001b[0m");

    fetch(ticketUrl, {
      method: "POST",
      headers: { "x-csrf-token": el.dataset.csrf || "" },
      credentials: "same-origin"
    }).then(function (r) {
      if (!r.ok) throw new Error("HTTP " + r.status);
      return r.json();
    }).then(function (data) {
      if (!data || typeof data.ticket !== "string" || !data.ticket) {
        throw new Error("票据响应缺少 ticket");
      }
      var ws = new WebSocket(
        proto + "//" + location.host + wsUrl + "?ticket=" + encodeURIComponent(data.ticket)
      );
      ws.onopen = function () { term.writeln("\u001b[32mconnected\u001b[0m"); };
      ws.onmessage = function (ev) { term.write(ev.data); };
      ws.onclose = function () { term.writeln("\r\n\u001b[33m[disconnected]\u001b[0m"); };
      term.onData(function (d) { if (ws.readyState === 1) ws.send(d); });
      window.addEventListener("beforeunload", function () { ws.close(); });
    }).catch(function (e) {
      term.writeln("\u001b[31m无法获取终端票据：" + String(e) + "\u001b[0m");
    });
  }

  // ---- 插件 UI 沙箱桥 ----
  // 父页面校验 event.source === iframe.contentWindow、nonce、消息 schema，
  // 再代发到 /{prefix}/plugins/{id}/rpc（携带 CSRF 头），回包 targetOrigin="*"
  // 但绑定同一 source+nonce，且不携带任何面板凭据。
  function attachPluginBridges() {
    document.querySelectorAll("iframe[data-plugin]").forEach(function (frame) {
      var plugin = frame.dataset.plugin;
      var nonce = frame.dataset.nonce;
      var endpoint = frame.dataset.rpc;
      var csrf = frame.dataset.csrf;
      if (!plugin || !nonce || !endpoint) return;
      window.addEventListener("message", function (ev) {
        if (ev.source !== frame.contentWindow) return;
        var msg = ev.data;
        if (!msg || msg.type !== "skiff.rpc" || msg.nonce !== nonce) return;
        if (typeof msg.method !== "string" || msg.method.length > 64) return;
        var id = msg.id;
        fetch(endpoint, {
          method: "POST",
          headers: { "content-type": "application/json", "x-csrf-token": csrf },
          credentials: "same-origin",
          body: JSON.stringify({ method: msg.method, params: msg.params || null })
        }).then(function (r) { return r.json().catch(function () { return { error: { message: "bad json" } }; }); })
          .then(function (data) {
            frame.contentWindow.postMessage(
              { type: "skiff.rpc.result", nonce: nonce, id: id, result: data.result, error: data.error },
              "*"
            );
          })
          .catch(function (e) {
            frame.contentWindow.postMessage(
              { type: "skiff.rpc.result", nonce: nonce, id: id, error: { message: String(e) } },
              "*"
            );
          });
      });
    });
  }

  function boot() {
    attachLogs();
    attachTerminal();
    attachPluginBridges();
  }

  document.addEventListener("DOMContentLoaded", boot);
  document.body.addEventListener("htmx:afterSwap", boot);
})();
