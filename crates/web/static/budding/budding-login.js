/**
 * budding 登录页启动器（Skiff 侧静态资源，非上游产物）。
 *
 * 职责只有三件：取登录 CSRF（`GET /ui/v1/auth/login`）、用 budding 渲染登录表单、
 * 把提交转给 `POST /ui/v1/auth/login`。认证判定与会话 Cookie 签发全在服务端，
 * 这里不做任何密码/会话逻辑。
 *
 * 失败提示沿用服务端返回的中文（`error.message`）：写入 sessionStorage 后重载同一
 * 登录页，由本脚本读回并渲染，避免把错误文案放进 URL（可被外部构造）。
 *
 * 口令框直接用 `input-text` 的原生 `type: 'password'` 与 `autocomplete`（budding 1.8.0
 * 重打包起支持），不再在渲染后改写 DOM，因此首帧即掩码。
 */
(function () {
  'use strict';

  var mount = document.getElementById('skiff-login');
  if (!mount) return;

  var prefix = mount.getAttribute('data-prefix') || '';
  var apiBase = mount.getAttribute('data-api-base') || '/ui/v1';
  var cssHref = mount.getAttribute('data-budding-css') || '';
  var next = mount.getAttribute('data-next') || '';
  var username = mount.getAttribute('data-username') || '';
  var storedError = '';

  try {
    storedError = sessionStorage.getItem('skiff.login.error') || '';
    sessionStorage.removeItem('skiff.login.error');
    var storedUser = sessionStorage.getItem('skiff.login.username') || '';
    sessionStorage.removeItem('skiff.login.username');
    if (storedUser) username = storedUser;
  } catch (e) {
    /* 隐私模式下 sessionStorage 不可用：错误提示降级为不显示 */
  }

  function showFatal(message) {
    mount.textContent = '';
    var box = document.createElement('main');
    box.setAttribute('data-skiff-placeholder', '');
    var title = document.createElement('h1');
    title.textContent = '登录页加载失败';
    var detail = document.createElement('p');
    detail.textContent = message;
    box.appendChild(title);
    box.appendChild(detail);
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

  function reloadWithError(message, name) {
    try {
      sessionStorage.setItem('skiff.login.error', message);
      if (name) sessionStorage.setItem('skiff.login.username', name);
    } catch (e) {
      /* 存不下时至少不要静默：仍重载页面，错误可能丢失 */
    }
    window.location.replace(window.location.pathname + window.location.search);
  }

  function loginSchema(config) {
    var body = [];
    if (storedError) {
      body.push({
        type: 'alert',
        props: { type: 'error', text: storedError, variant: 'tonal' }
      });
    }
    body.push({
      type: 'form',
      onEvent: { submit: [{ type: 'skiffLogin' }] },
      props: {
        // 真正的提交在 onEvent.submit 的自定义动作里；这里给一个不发送的 api，
        // 只为避开“表单未配置 api”的告警分支（sendOn: false 不会发请求）。
        api: { method: 'POST', url: apiBase + '/auth/login', sendOn: false },
        successMessage: '登录成功',
        actionsAlign: 'end',
        initialValues: { username: username, otp: '' }
      },
      body: [
        {
          type: 'input-text',
          props: {
            name: 'username',
            label: '用户名',
            autocomplete: 'username',
            required: true,
            rules: { required: true, messages: { required: '请输入用户名' } }
          }
        },
        {
          type: 'input-text',
          props: {
            name: 'password',
            label: '口令',
            type: 'password',
            autocomplete: 'current-password',
            required: true,
            rules: { required: true, messages: { required: '请输入口令' } }
          }
        },
        {
          type: 'input-text',
          props: {
            name: 'otp',
            label: '动态码（启用 TOTP 后必填）',
            hint: '未启用 TOTP 可留空',
            autocomplete: 'one-time-code'
          }
        }
      ],
      content: {
        actions: [
          {
            type: 'button',
            props: { text: '登录', color: 'primary', block: true },
            action: { type: 'submit' }
          }
        ]
      }
    });

    return {
      type: 'page',
      props: { title: '登录 · Skiff', maxWidth: 460 },
      body: [
        {
          type: 'card',
          props: { title: '登录 · Skiff', subtitle: '轻舟 · 极简服务器管理面板' },
          content: { body: body }
        }
      ]
    };
  }

  function submitLogin(values, config) {
    var payload = {
      username: String((values && values.username) || ''),
      password: String((values && values.password) || ''),
      otp: String((values && values.otp) || ''),
      csrf: config.csrf_token,
      next: next
    };
    fetch(apiBase + '/auth/login', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
      body: JSON.stringify(payload)
    })
      .then(function (resp) {
        return readJson(resp).then(function (body) {
          return { status: resp.status, body: body };
        });
      })
      .then(function (res) {
        if (res.status === 200 && res.body && res.body.data && res.body.data.redirect_to) {
          window.location.assign(res.body.data.redirect_to);
          return;
        }
        var message =
          res.body && res.body.error && res.body.error.message
            ? res.body.error.message
            : '登录失败（状态 ' + res.status + '）';
        reloadWithError(message, payload.username);
      })
      .catch(function (error) {
        reloadWithError(
          '无法连接服务：' + (error && error.message ? error.message : String(error)),
          payload.username
        );
      });
  }

  fetch(apiBase + '/auth/login', {
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
      if (res.status !== 200 || !res.body || !res.body.data || !res.body.data.csrf_token) {
        var message =
          res.body && res.body.error && res.body.error.message
            ? res.body.error.message
            : '服务返回状态 ' + res.status;
        showFatal(message);
        return;
      }
      if (!window.BuddingSDK || typeof window.BuddingSDK.embed !== 'function') {
        showFatal('前端产物未加载（budding 脚本不可用）。请检查 /' + prefix + '/static/budding/ 下的静态资源。');
        return;
      }
      var config = res.body.data;
      var options = {
        actions: {
          skiffLogin: function (args, ctx) {
            var values = null;
            try {
              values = ctx.scope.get('values');
            } catch (e) {
              values = null;
            }
            submitLogin(values || {}, config);
          }
        }
      };
      if (cssHref) options.css = cssHref;
      window.BuddingSDK.embed(mount, loginSchema(config), options).catch(function (error) {
        showFatal('初始化失败：' + (error && error.message ? error.message : String(error)));
      });
    })
    .catch(function (error) {
      showFatal('无法连接服务：' + (error && error.message ? error.message : String(error)));
    });
})();
