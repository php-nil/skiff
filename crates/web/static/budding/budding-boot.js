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

  /// 把当前地址拆成入口前缀之后的路径段（已解码）。
  function routeParts() {
    var path = location.pathname;
    if (path === prefix || path === prefix + '/') return [];
    if (path.indexOf(prefix + '/') !== 0) return ['__outside'];
    return path
      .slice(prefix.length + 1)
      .split('/')
      .filter(function (s) {
        return s !== '';
      })
      .map(function (s) {
        try {
          return decodeURIComponent(s);
        } catch (e) {
          return s;
        }
      });
  }

  function listQuery(extra) {
    var query = '?limit=50';
    return extra ? query + '&' + extra : query;
  }

  function noticeCard(notice) {
    return {
      type: 'alert',
      props: { type: 'info', text: notice }
    };
  }

  function statusTable(columns, items, rowKey) {
    return {
      type: 'table',
      props: {
        items: items,
        columns: columns,
        rowKey: rowKey,
        pagination: false,
        density: 'compact',
        emptyText: '暂无数据'
      }
    };
  }

  // 站点列表：列取接口字段，行点击进入 budding 详情。
  function sitesSchema(data) {
    var body = [];
    if (data.next_cursor) {
      body.push(
        noticeCard(
          '共 ' +
            data.total +
            ' 个站点，当前显示前 ' +
            data.items.length +
            ' 条；翻页交互待后续批次接入。'
        )
      );
    }
    var table = statusTable(
      [
        { key: 'domain', label: '域名' },
        { key: 'type_label', label: '类型' },
        { key: 'endpoint', label: '接入' },
        { key: 'status', label: '状态', text: '${status.label}' },
        { key: 'https', label: 'HTTPS', text: '${https.label}' },
        { key: 'runtime_name', label: '运行环境' },
        { key: 'created_at', label: '创建时间', width: 180 }
      ],
      data.items,
      'domain'
    );
    table.onEvent = {
      rowClick: {
        actions: [
          {
            type: 'navigate',
            props: { url: prefix + '/sites/${domain}' }
          }
        ]
      }
    };
    body.push(table);
    body.push({
      type: 'link',
      props: {
        text: '新建站点（旧版表单，待迁移）',
        href: prefix + '/m/core/sites/list?new=1'
      }
    });
    return {
      type: 'page',
      props: { title: '网站 · Skiff', subtitle: '只读列表；写操作仍走旧版页面' },
      body: body
    };
  }

  function siteDetailSchema(data) {
    var rows = [
      ['类型', data.type_label],
      ['状态', data.status.label],
      ['HTTPS', data.https.label],
      ['接入方式', data.endpoint || '—'],
      ['运行环境', data.runtime_label || '—'],
      ['站点目录', data.root_dir],
      ['文档根', data.index_dir],
      ['附加域名', data.aliases && data.aliases.length ? data.aliases.join('、') : '—'],
      ['创建时间', data.created_at]
    ];
    if (data.runtime_shared_hint) rows.push(['共享提示', data.runtime_shared_hint]);
    var body = [];
    if (data.runtime_down && data.runtime_alert) {
      body.push({
        type: 'alert',
        props: { type: 'warning', title: '运行环境不可用', text: data.runtime_alert }
      });
    }
    body.push({
      type: 'card',
      props: { title: '站点信息' },
      content: { body: rows.map(function (row) {
        return {
          type: 'flex',
          props: { gap: 12 },
          content: {
            body: [
              { type: 'text', props: { text: row[0], weight: 'medium' } },
              { type: 'text', props: { text: row[1] } }
            ]
          }
        };
      }) }
    });
    body.push({
      type: 'card',
      props: { title: '证书' },
      content: {
        body: [
          textLine(data.ssl.cert_exists ? '已检测到证书文件' : '未检测到证书文件'),
          textLine('证书路径：' + (data.ssl.cert_path || '—')),
          textLine('主体：' + (data.ssl.subject || '—')),
          textLine('签发者：' + (data.ssl.issuer || '—')),
          textLine('到期：' + (data.ssl.expires || '—')),
          {
            type: 'alert',
            props: {
              type: 'info',
              text: '配置、域名、SSL 与目录管理仍走旧版详情页，待写操作批次迁移。'
            }
          },
          {
            type: 'link',
            props: {
              text: '打开旧版详情（配置 / 域名 / SSL / 目录）',
              href:
                prefix +
                '/m/core/sites/list/' +
                encodeURIComponent(data.domain)
            }
          }
        ]
      }
    });
    return {
      type: 'page',
      props: { title: data.domain + ' · 网站', subtitle: '只读概览' },
      body: body
    };
  }

  function runtimesSchema(data) {
    var body = [];
    if (data.next_cursor) {
      body.push(
        noticeCard(
          '共 ' +
            data.total +
            ' 个运行环境，当前显示前 ' +
            data.items.length +
            ' 条；翻页交互待后续批次接入。'
        )
      );
    }
    body.push(
      statusTable(
        [
          { key: 'kind', label: '类型', text: '${kind.label}' },
          { key: 'name', label: '实例' },
          { key: 'version_display', label: '版本' },
          { key: 'version_note', label: '说明' },
          { key: 'state', label: '状态', text: '${state.label}' },
          { key: 'site_count', label: '站点数', type: 'number' },
          { key: 'endpoint_short', label: '接入' }
        ],
        data.items,
        'id'
      )
    );
    body.push({
      type: 'alert',
      props: {
        type: 'info',
        text: '安装、配置、扩展与日志仍走旧版运行环境页，待写操作批次迁移。'
      }
    });
    body.push({
      type: 'link',
      props: { text: '打开旧版运行环境管理', href: prefix + '/m/core/sites/env' }
    });
    return {
      type: 'page',
      props: { title: '运行环境 · Skiff', subtitle: '只读列表；安装状态来自实时容器探测' },
      body: body
    };
  }

  function embedWithError(schema) {
    if (!window.BuddingSDK || typeof window.BuddingSDK.embed !== 'function') {
      showError('前端产物未加载（budding 脚本不可用）。');
      return;
    }
    embed(schema);
  }

  function getJson(path, onOk) {
    fetch(apiBase + path, {
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
        onOk(res.body.data);
      })
      .catch(function (error) {
        showError('无法连接服务：' + (error && error.message ? error.message : String(error)));
      });
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
      var parts = routeParts();
      if (parts.length === 0) {
        getJson('/overview', function (data) {
          embedWithError(overviewSchema(data));
        });
        return;
      }
      if (parts[0] === 'sites' && parts.length === 1) {
        getJson('/sites' + listQuery(), function (data) {
          embedWithError(sitesSchema(data));
        });
        return;
      }
      if (parts[0] === 'sites' && parts.length === 2) {
        getJson('/sites/' + encodeURIComponent(parts[1]), function (data) {
          embedWithError(siteDetailSchema(data));
        });
        return;
      }
      if (parts[0] === 'runtimes' && parts.length === 1) {
        getJson('/runtimes' + listQuery(), function (data) {
          embedWithError(runtimesSchema(data));
        });
        return;
      }
      embed(placeholderSchema(session));
    })
    .catch(function (error) {
      showError('无法连接服务：' + (error && error.message ? error.message : String(error)));
    });
})();
