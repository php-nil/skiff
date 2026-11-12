/**
 * budding 壳页启动器（Skiff 侧静态资源，非上游产物）。
 *
 * 为什么单独一个 script：全局 CSP 是 `script-src 'self'`，不能内联；而 budding 的
 * CSS 目录靠 `document.currentScript` 推导，所以产物只能用普通 `<script src>` 加载
 * （不能内联、不能 import()）。
 *
 * 链路：GET /ui/v1/session → 成功则 embed 极简 schema；失败显示错误而不是白屏。
 * 未认证由服务端 303 到登录页（页面请求既有行为），这里不做跳转。
 */
(function () {
  'use strict';

  var mount = document.getElementById('skiff-root');
  if (!mount) return;

  var prefix = mount.getAttribute('data-prefix') || '';
  var apiBase = mount.getAttribute('data-api-base') || '/ui/v1';
  // 版本化 CSS 必须显式传：budding 构建期写死的自动推导名是 `budding.css`，
  // 而入库的是带版本号的文件名。
  var cssHref = mount.getAttribute('data-budding-css') || '';

  function showError(message) {
    mount.textContent = '';
    var box = document.createElement('main');
    box.setAttribute('data-skiff-placeholder', '');
    var title = document.createElement('h1');
    title.textContent = '页面加载失败';
    var detail = document.createElement('p');
    detail.textContent = message;
    box.appendChild(title);
    box.appendChild(detail);
    if (prefix) {
      var line = document.createElement('p');
      var link = document.createElement('a');
      link.href = prefix + '/login';
      link.textContent = '前往登录页';
      line.appendChild(link);
      box.appendChild(line);
    }
    mount.appendChild(box);
  }

  function readJson(resp) {
    return resp.json().then(
      function (body) {
        return body;
      },
      function () {
        return null;
      }
    );
  }

  fetch(apiBase + '/session', {
    method: 'GET',
    credentials: 'same-origin',
    headers: { Accept: 'application/json' }
  })
    .then(function (resp) {
      return readJson(resp).then(function (body) {
        return { status: resp.status, body: body };
      });
    })
    .then(function (res) {
      if (res.status === 401) {
        showError('会话已过期，请刷新页面后重新登录。');
        return;
      }
      if (res.status !== 200 || !res.body || !res.body.data) {
        var message =
          res.body && res.body.error && res.body.error.message
            ? res.body.error.message
            : '服务返回状态 ' + res.status;
        showError(message);
        return;
      }
      var session = res.body.data;
      if (!window.BuddingSDK || typeof window.BuddingSDK.embed !== 'function') {
        showError('前端产物未加载（budding 脚本不可用）。');
        return;
      }
      // 极简 schema：只证明链路（页面标题 + 占位卡片），不含任何业务页面。
      var schema = {
        type: 'page',
        props: { title: 'Skiff' },
        body: [
          {
            type: 'card',
            props: { title: '该功能正在适配中' },
            content: {
              body: [
                {
                  type: 'text',
                  props: {
                    text:
                      '页面正在切换到新的前端运行时，当前为占位视图。当前用户：' +
                      session.username +
                      '（' +
                      session.role +
                      '）'
                  }
                }
              ]
            }
          }
        ]
      };
      var options = { routerMode: 'history', routerBase: prefix };
      if (cssHref) options.css = cssHref;
      window.BuddingSDK.embed(mount, schema, options).catch(function (error) {
        showError('初始化失败：' + (error && error.message ? error.message : String(error)));
      });
    })
    .catch(function (error) {
      showError('无法连接服务：' + (error && error.message ? error.message : String(error)));
    });
})();
