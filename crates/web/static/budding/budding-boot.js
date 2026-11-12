/**
 * budding 壳页启动器（Skiff 侧静态资源，非上游产物）。
 *
 * 为什么单独一个 script：全局 CSP 是 `script-src 'self'`，不能内联；而 budding 的
 * CSS 目录靠 `document.currentScript` 推导，所以产物只能用普通 `<script src>` 加载
 * （不能内联、不能 import()）。
 *
 * 链路：GET /ui/v1/session → 入口根再取 GET /ui/v1/overview 渲染只读概览；
 * 其余页面仍 embed 占位卡片。失败显示错误而不是白屏。
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

  function placeholderSchema(session) {
    return {
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
  }

  function textLine(text) {
    return { type: 'text', props: { text: text } };
  }

  function countCard(title, lines) {
    return {
      type: 'card',
      props: { title: title },
      content: { body: lines.map(textLine) }
    };
  }

  // 只读概览：卡片 + 计数。图表位按 D14–D16 未定，用明确提示占位，不选图表库。
  function overviewSchema(data) {
    var body = [];
    if (data.docker && data.docker.status && data.docker.status.code !== 'available') {
      body.push({
        type: 'alert',
        props: {
          type: 'warning',
          title: 'Docker ' + data.docker.status.label,
          text: data.docker.reason || 'Docker 守护进程当前不可达，容器相关操作会返回 503。'
        }
      });
    }
    body.push({
      type: 'grid',
      props: { columns: 3, gap: 16 },
      content: {
        body: [
          countCard('容器', [
            '运行中 ' + data.containers.running,
            '共 ' + data.containers.total + ' 个'
          ]),
          countCard('网站', [
            '共 ' + data.sites.total + ' 个站点',
            data.sites.abnormal > 0 ? data.sites.abnormal + ' 个异常' : '全部正常'
          ]),
          countCard('插件', [
            '共 ' + data.plugins.total + ' 个',
            '已启用 ' + data.plugins.enabled + ' / 停用 ' + data.plugins.disabled
          ]),
          countCard('运行环境', [
            '共 ' + data.runtimes.total + ' 个',
            '异常 ' + data.runtimes.abnormal + ' / 未安装 ' + data.runtimes.uninstalled
          ])
        ]
      }
    });
    body.push({
      type: 'card',
      props: { title: '趋势图表' },
      content: {
        body: [
          {
            type: 'alert',
            props: {
              type: 'info',
              text: '图表位暂缺：D14–D16 尚未定案，待选定图表组件后补充。'
            }
          }
        ]
      }
    });
    body.push(textLine('采集时间 ' + data.collected_at));
    return {
      type: 'page',
      props: { title: '概览 · Skiff', subtitle: '只读计数，不做实时刷新' },
      body: body
    };
  }

  function embed(schema) {
    var options = { routerMode: 'history', routerBase: prefix };
    if (cssHref) options.css = cssHref;
    window.BuddingSDK.embed(mount, schema, options).catch(function (error) {
      showError('初始化失败：' + (error && error.message ? error.message : String(error)));
    });
  }

  function isRootPath() {
    return location.pathname === prefix || location.pathname === prefix + '/';
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
      if (!isRootPath()) {
        embed(placeholderSchema(session));
        return;
      }
      fetch(apiBase + '/overview', {
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
          if (res.status !== 200 || !res.body || !res.body.data) {
            var message =
              res.body && res.body.error && res.body.error.message
                ? res.body.error.message
                : '服务返回状态 ' + res.status;
            showError(message);
            return;
          }
          embed(overviewSchema(res.body.data));
        })
        .catch(function (error) {
          showError('无法连接服务：' + (error && error.message ? error.message : String(error)));
        });
    })
    .catch(function (error) {
      showError('无法连接服务：' + (error && error.message ? error.message : String(error)));
    });
})();
