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

  var sessionState = null;

  function errorMessage(res) {
    return res.body && res.body.error && res.body.error.message
      ? res.body.error.message
      : '服务返回状态 ' + res.status;
  }

  // 容器启停/重启/删除：走 /ui/v1 同策略、同审计的 JSON 端点。
  function containerAction(args) {
    var id = args.id;
    var action = args.action;
    var name = args.name || id;
    if (action === 'delete' && !window.confirm('确认删除容器 ' + name + '？删除后不可恢复，容器内未持久化的数据会丢失。')) {
      return Promise.resolve();
    }
    return fetch(apiBase + '/containers/' + encodeURIComponent(id) + '/' + action, {
      method: 'POST',
      credentials: 'same-origin',
      headers: {
        Accept: 'application/json',
        'x-csrf-token': (sessionState && sessionState.csrf_token) || ''
      }
    })
      .then(function (resp) {
        return readJson(resp).then(function (body) {
          return { status: resp.status, body: body };
        });
      })
      .then(function (res) {
        if (res.status !== 200) {
          window.alert(errorMessage(res));
          return;
        }
        if (action === 'delete') location.href = prefix + '/containers';
        else location.reload();
      })
      .catch(function (error) {
        window.alert('操作失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  function embed(schema) {
    var options = { routerMode: 'history', routerBase: prefix };
    if (cssHref) options.css = cssHref;
    options.actions = {
      skiffContainerAction: containerAction,
      skiffComposeAction: composeAction,
      skiffSettingsSave: settingsSave,
      skiffStoreInstall: storeInstall,
      skiffStoreConfirm: storeConfirm
    };
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

  function containersSchema(data) {
    var body = [];
    if (!data.docker_available) {
      body.push({
        type: 'alert',
        props: {
          type: 'warning',
          title: 'Docker 不可用',
          text:
            data.docker_reason ||
            'Docker 守护进程当前不可达，容器列表为空，相关操作会返回 503。'
        }
      });
    }
    if (data.next_cursor) {
      body.push(
        noticeCard(
          '共 ' +
            data.total +
            ' 个容器，当前显示前 ' +
            data.items.length +
            ' 条；翻页交互待后续批次接入。'
        )
      );
    }
    var table = statusTable(
      [
        { key: 'name', label: '名称' },
        { key: 'short_id', label: 'ID' },
        { key: 'image', label: '镜像' },
        { key: 'state', label: '状态', text: '${state.label}' },
        { key: 'status', label: '状态描述' },
        { key: 'project', label: 'Compose 项目' }
      ],
      data.items,
      'id'
    );
    table.onEvent = {
      rowClick: {
        actions: [
          {
            type: 'navigate',
            props: { url: prefix + '/containers/${id}' }
          }
        ]
      }
    };
    body.push(table);
    body.push({
      type: 'link',
      props: { text: '新建容器（旧版表单，待迁移）', href: prefix + '/containers/new' }
    });
    return {
      type: 'page',
      props: {
        title: '容器 · Skiff',
        subtitle: '只读列表；启停/重启/删除在详情页操作'
      },
      body: body
    };
  }

  function buttonNode(text, action, container) {
    return {
      type: 'button',
      props: { text: text, variant: 'tonal' },
      onEvent: {
        click: {
          actions: [
            {
              type: 'skiffContainerAction',
              props: {
                id: container.id,
                name: container.name,
                action: action
              }
            }
          ]
        }
      }
    };
  }

  function detailLines(title, lines) {
    return {
      type: 'card',
      props: { title: title },
      content: { body: lines.map(textLine) }
    };
  }

  function containerDetailSchema(data) {
    var c = data.container;
    var body = [];
    if (data.docker_reason) {
      body.push({
        type: 'alert',
        props: { type: 'warning', title: 'Docker 不可用', text: data.docker_reason }
      });
    }
    if (c.unprotected_kernel) {
      body.push({
        type: 'alert',
        props: {
          type: 'warning',
          text: '该容器是内核托管的存量实例但缺少保护标签，重建 Web 服务或运行环境后才会生效。'
        }
      });
    }
    var actions = [];
    if (data.can_operate) {
      if (c.state.code === 'running') {
        actions.push(buttonNode('重启', 'restart', c));
        actions.push(buttonNode('停止', 'stop', c));
      } else {
        actions.push(buttonNode('启动', 'start', c));
      }
    }
    if (data.can_delete) {
      actions.push(buttonNode('删除', 'delete', c));
    }
    if (actions.length > 0) {
      body.push({ type: 'flex', props: { gap: 8 }, content: { body: actions } });
    }
    body.push(
      detailLines('基本信息', [
        '名称：' + c.name + '（' + c.short_id + '）',
        '镜像：' + data.image,
        '状态：' + c.state.label + ' · ' + c.status,
        '创建时间：' + (data.created_at || '—'),
        '健康检查：' + data.health,
        '启动命令：' + (data.cmd || '—')
      ])
    );
    body.push(
      detailLines(
        '端口',
        data.ports.length === 0
          ? ['未映射端口']
          : data.ports.map(function (p) {
              return (
                (p.host ? p.host + ' → ' : '') + p.container + '/' + p.protocol
              );
            })
      )
    );
    body.push(
      detailLines(
        '挂载',
        data.mounts.length === 0
          ? ['无挂载']
          : data.mounts.map(function (m) {
              return (
                m.source + ' → ' + m.target + (m.read_only ? '（只读）' : '') + ' [' + m.kind + ']'
              );
            })
      )
    );
    body.push(detailLines('网络', data.networks.length === 0 ? ['未接入网络'] : data.networks));
    body.push(
      detailLines(
        '资源限制',
        data.limits_effective.map(function (line) {
          return line.label + '：' + line.value;
        })
      )
    );
    body.push(
      detailLines(
        '标签',
        data.labels.length === 0
          ? ['无标签']
          : data.labels.map(function (l) {
              return l.key + ' = ' + l.value;
            })
      )
    );
    body.push({
      type: 'alert',
      props: {
        type: 'info',
        text: '资源限制编辑、日志与终端仍在旧版详情页；日志查看器待 T3，终端待 T5。'
      }
    });
    body.push({
      type: 'link',
      props: {
        text: '打开旧版详情（资源限制 / 日志 / 终端）',
        href: prefix + '/m/core/docker/containers/' + encodeURIComponent(c.id)
      }
    });
    return {
      type: 'page',
      props: { title: c.name + ' · 容器', subtitle: '只读详情；操作按钮走 /ui/v1' },
      body: body
    };
  }

  function resourceSchema(title, subtitle, columns, data, fallbackText, fallbackHref) {
    var body = [];
    if (!data.docker_available) {
      body.push({
        type: 'alert',
        props: {
          type: 'warning',
          title: 'Docker 不可用',
          text: data.docker_reason || 'Docker 守护进程当前不可达，列表为空。'
        }
      });
    }
    if (data.next_cursor) {
      body.push(
        noticeCard(
          '共 ' +
            data.total +
            ' 条，当前显示前 ' +
            data.items.length +
            ' 条；翻页交互待后续批次接入。'
        )
      );
    }
    body.push(statusTable(columns, data.items, 'id'));
    body.push({
      type: 'alert',
      props: {
        type: 'info',
        text: '创建/拉取/删除等写操作仍走旧版页面，待写操作批次迁移。'
      }
    });
    body.push({ type: 'link', props: { text: fallbackText, href: fallbackHref } });
    return { type: 'page', props: { title: title, subtitle: subtitle }, body: body };
  }

  function imagesSchema(data) {
    return resourceSchema(
      '镜像 · Skiff',
      '只读列表',
      [
        { key: 'name', label: '镜像' },
        { key: 'meta', label: '大小' },
        { key: 'warn', label: '提示' }
      ],
      data,
      '打开旧版镜像页（拉取 / 删除）',
      prefix + '/m/core/docker/images'
    );
  }

  function networksSchema(data) {
    return resourceSchema(
      '网络 · Skiff',
      '只读列表',
      [
        { key: 'name', label: '名称' },
        { key: 'driver', label: '驱动' },
        { key: 'scope', label: '范围' },
        { key: 'subnet', label: '子网' },
        { key: 'gateway', label: '网关' },
        { key: 'container_count', label: '容器' },
        { key: 'internal', label: '内部/外部' }
      ],
      data,
      '打开旧版网络页（创建 / 删除）',
      prefix + '/m/core/docker/networks'
    );
  }

  function volumesSchema(data) {
    return resourceSchema(
      '卷 · Skiff',
      '只读列表',
      [
        { key: 'name', label: '名称' },
        { key: 'meta', label: '驱动与挂载点' },
        { key: 'warn', label: '提示' }
      ],
      data,
      '打开旧版卷页（创建 / 删除）',
      prefix + '/m/core/docker/volumes'
    );
  }

  // compose 项目级操作：启停/重启走逐容器分发，删除走 compose down。
  function composeAction(args) {
    var project = args.project;
    var action = args.action;
    if (
      action === 'down' &&
      !window.confirm('确认清理项目 ' + project + '？内核托管的容器会被删除，卷默认保留。')
    ) {
      return Promise.resolve();
    }
    var url =
      action === 'down'
        ? apiBase + '/compose/down'
        : apiBase + '/compose/' + encodeURIComponent(project) + '/' + action;
    var init = {
      method: 'POST',
      credentials: 'same-origin',
      headers: {
        Accept: 'application/json',
        'x-csrf-token': (sessionState && sessionState.csrf_token) || ''
      }
    };
    if (action === 'down') {
      init.headers['content-type'] = 'application/json';
      init.body = JSON.stringify({ project: project, remove_volumes: false });
    }
    return fetch(url, init)
      .then(function (resp) {
        return readJson(resp).then(function (body) {
          return { status: resp.status, body: body };
        });
      })
      .then(function (res) {
        if (res.status !== 200) {
          window.alert(errorMessage(res));
          return;
        }
        location.reload();
      })
      .catch(function (error) {
        window.alert('操作失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  function composeButton(text, action) {
    return {
      type: 'button',
      props: { text: text, variant: 'tonal' },
      onEvent: {
        click: {
          actions: [
            {
              type: 'skiffComposeAction',
              props: { project: '${name}', action: action }
            }
          ]
        }
      }
    };
  }

  function composeSchema(data) {
    var body = [];
    if (!data.docker_available) {
      body.push({
        type: 'alert',
        props: {
          type: 'warning',
          title: 'Docker 不可用',
          text: data.docker_reason || 'Docker 守护进程当前不可达，项目列表为空。'
        }
      });
    }
    var table = statusTable(
      [
        { key: 'name', label: '项目' },
        { key: 'managed', label: '归属', text: '${managed}' },
        { key: 'services', label: '服务数', type: 'number' },
        { key: 'running', label: '运行中', type: 'number' }
      ],
      data.projects,
      'name'
    );
    table.props.rowActions = [
      composeButton('启动', 'start'),
      composeButton('停止', 'stop'),
      composeButton('重启', 'restart'),
      composeButton('清理', 'down')
    ];
    body.push(table);
    body.push({
      type: 'alert',
      props: {
        type: 'info',
        text: '粘贴/编辑 compose 文本、校验与部署仍在旧版页面；服务日志查看器待 T3。'
      }
    });
    body.push({
      type: 'link',
      props: { text: '打开旧版编排页（编辑 / 校验 / 部署）', href: prefix + '/m/core/docker/compose' }
    });
    return {
      type: 'page',
      props: { title: '编排 · Skiff', subtitle: '项目列表与启停；编辑/部署走旧版页面' },
      body: body
    };
  }

  function tasksSchema(data) {
    var body = [];
    if (data.next_cursor) {
      body.push(
        noticeCard(
          '共 ' + data.total + ' 个任务，当前显示前 ' + data.items.length + ' 条；翻页待接入。'
        )
      );
    }
    var table = statusTable(
      [
        { key: 'name', label: '任务' },
        { key: 'method', label: '方式' },
        { key: 'target', label: '目标' },
        { key: 'expr', label: '调度' },
        { key: 'next_text', label: '下次运行' },
        { key: 'state_label', label: '状态' },
        { key: 'last_text', label: '最近结果' }
      ],
      data.items,
      'id'
    );
    table.onEvent = {
      rowClick: {
        actions: [{ type: 'navigate', props: { url: prefix + '/tasks/${id}' } }]
      }
    };
    body.push(table);
    body.push({
      type: 'alert',
      props: {
        type: 'info',
        text: '启停、立即运行与删除仍在旧版任务页，待写操作批次迁移。'
      }
    });
    body.push({
      type: 'link',
      props: { text: '打开旧版任务页', href: prefix + '/m/core/tasks' }
    });
    return {
      type: 'page',
      props: { title: '定时任务 · Skiff', subtitle: '只读列表' },
      body: body
    };
  }

  function taskDetailSchema(data) {
    var t = data.task;
    var body = [];
    body.push(
      detailLines('任务信息', [
        '名称：' + t.name + '（' + t.id + '）',
        '方式：' + t.method + ' · 目标：' + t.target,
        '调度：' + t.expr,
        '下次运行：' + (t.next_text || '—'),
        '状态：' + t.state_label,
        '最近结果：' + (t.last_text || '—'),
        '来源：' + (t.source || '—') + ' · 内置：' + (t.builtin ? '是' : '否')
      ])
    );
    body.push(detailLines('参数', ['payload = ' + JSON.stringify(data.payload)]));
    body.push({
      type: 'card',
      props: { title: '最近执行（' + data.runs.length + '）' },
      content: {
        body: [
          statusTable(
            [
              { key: 'started_text', label: '开始' },
              { key: 'trigger', label: '触发' },
              { key: 'duration', label: '耗时' },
              { key: 'result', label: '结果' },
              { key: 'detail', label: '说明' }
            ],
            data.runs,
            'started_text'
          )
        ]
      }
    });
    body.push({
      type: 'link',
      props: {
        text: '打开旧版任务详情（启停 / 立即运行 / 删除）',
        href: prefix + '/m/core/tasks/' + encodeURIComponent(t.id)
      }
    });
    return {
      type: 'page',
      props: { title: t.name + ' · 定时任务', subtitle: '只读详情' },
      body: body
    };
  }

  function jobsSchema(data) {
    var body = [];
    var table = statusTable(
      [
        { key: 'id', label: '作业' },
        { key: 'title', label: '标题' },
        { key: 'kind_label', label: '类型' },
        { key: 'target_label', label: '目标' },
        { key: 'status_label', label: '状态' },
        { key: 'duration', label: '耗时' },
        { key: 'created_by', label: '提交人' },
        { key: 'created_at', label: '创建时间' }
      ],
      data.items,
      'id'
    );
    table.onEvent = {
      rowClick: {
        actions: [{ type: 'navigate', props: { url: prefix + '/jobs/${id}' } }]
      }
    };
    body.push(table);
    return {
      type: 'page',
      props: { title: '作业中心 · Skiff', subtitle: '只读列表（最近 200 条）' },
      body: body
    };
  }

  function jobDetailSchema(data) {
    var body = [];
    body.push(
      detailLines('作业信息', [
        '标题：' + data.title + '（' + data.id + '）',
        '类型：' + data.kind_label + ' · 目标：' + data.target_label,
        '状态：' + data.status_label + (data.phase_label ? ' · ' + data.phase_label : ''),
        '进度：' + data.progress + '%' + (data.indeterminate ? '（不确定）' : ''),
        '创建：' + (data.created_at || '—') + ' · 提交人：' + data.created_by,
        '开始：' + (data.started_at || '—'),
        '结束：' + (data.finished_at || '—'),
        '耗时：' + (data.duration || '—') + ' · 退出码：' + data.exit_code,
        '触发：' + (data.trigger || '—'),
        '错误：' + (data.error_class ? data.error_class + ' ' + data.error_detail : '—')
      ])
    );
    body.push(detailLines('参数', ['params = ' + JSON.stringify(data.params)]));
    body.push(detailLines('结果', ['result = ' + JSON.stringify(data.result)]));
    var logLines = (data.logs || '').split('\n').filter(function (line) {
      return line !== '';
    });
    body.push({
      type: 'card',
      props: { title: '日志（首屏尾巴 ' + logLines.length + ' 行）' },
      content: {
        body:
          logLines.length === 0
            ? [textLine('暂无日志')]
            : logLines.map(function (line) {
                return { type: 'text', props: { text: line } };
              })
      }
    });
    body.push({
      type: 'alert',
      props: {
        type: 'info',
        text: '实时日志流（SSE）与取消/重试仍在旧版作业详情；日志查看器待 T3。'
      }
    });
    body.push({
      type: 'link',
      props: {
        text: '打开旧版作业详情（实时日志 / 取消 / 重试）',
        href: prefix + '/m/core/jobs/' + encodeURIComponent(data.id)
      }
    });
    return {
      type: 'page',
      props: { title: data.title + ' · 作业', subtitle: '只读详情' },
      body: body
    };
  }

  // 设置页：只读快照 + 面板表单（监听地址 / 入口前缀 / TLS）。口令与 TOTP 仍在旧版页。
  function settingsSchema(data) {
    var body = [];
    var panel = data.panel;
    if (data.flash && data.flash.error) {
      body.push({ type: 'alert', props: { type: 'error', text: data.flash.error } });
    }
    if (data.flash && data.flash.notice) {
      body.push({ type: 'alert', props: { type: 'success', text: data.flash.notice } });
    }
    if (data.account.must_change_password) {
      body.push({
        type: 'alert',
        props: {
          type: 'warning',
          title: '首次登录必须修改口令',
          text: '修改口令后其他页面才恢复访问；口令表单在旧版设置页。'
        }
      });
    }
    if (panel.initial_admin_pending && !data.account.must_change_password) {
      body.push({
        type: 'alert',
        props: {
          type: 'info',
          text: '建议尽快修改初始口令：当前口令仍保存在数据目录的 initial-admin.txt。'
        }
      });
    }
    if (panel.plaintext_behind_proxy) {
      body.push({
        type: 'alert',
        props: {
          type: 'warning',
          text:
            '当前为「可信反代终止 TLS」形态：面板只监听明文，请确保只有 trusted_proxies 中的反代能直连本端口。'
        }
      });
    }
    if (panel.tls_mode === 'off' && panel.listen_addr !== '127.0.0.1' && panel.listen_addr !== '::1') {
      body.push({
        type: 'alert',
        props: {
          type: 'warning',
          text:
            '已显式关闭 TLS 并监听非 loopback 地址：管理面为明文 HTTP，仅限内网或反向代理之后使用。'
        }
      });
    }
    body.push(
      detailLines('账户安全', [
        '两步验证（TOTP）：' + (data.account.totp_enabled ? '已启用' : '未启用'),
        '口令与 TOTP 表单在旧版设置页，本页只读展示状态。'
      ])
    );
    body.push({
      type: 'link',
      props: {
        text: '打开旧版设置页（修改口令 / TOTP / 信任公钥增删）',
        href: prefix + '/m/core/settings/security'
      }
    });

    var panelLines = [
      '当前监听：' + panel.listen_addr + ':' + panel.listen_port,
      '入口前缀：/' + panel.entry_prefix,
      'TLS：' + (panel.tls_mode === 'on' ? '已启用（本进程终止 TLS）' : panel.tls_mode === 'off' ? '已关闭（明文 HTTP）' : '自动推断')
    ];
    if (!data.can_admin) {
      panelLines.push('只有管理员可以修改监听端口、入口前缀与 TLS 配置。');
      body.push(detailLines('后台端口 / 安全入口 / TLS', panelLines));
    } else {
      body.push({
        type: 'form',
        onEvent: { submit: [{ type: 'skiffSettingsSave' }] },
        props: {
          api: { method: 'POST', url: apiBase + '/settings/panel', sendOn: false },
          successMessage: '已保存',
          actionsAlign: 'end',
          initialValues: {
            listen_addr: panel.listen_addr,
            listen_port: String(panel.listen_port),
            entry_prefix: panel.entry_prefix,
            tls_mode: panel.tls_mode,
            tls_cert: panel.tls_cert,
            tls_key: panel.tls_key
          }
        },
        body: [
          {
            type: 'input-text',
            props: { name: 'listen_addr', label: '监听地址（IPv4 / IPv6 字面量）', required: true }
          },
          {
            type: 'input-text',
            props: { name: 'listen_port', label: '监听端口（1–65535）', required: true }
          },
          {
            type: 'input-text',
            props: {
              name: 'entry_prefix',
              label: '安全入口前缀（6–32 位，字母/数字/-/_）',
              required: true
            }
          },
          {
            type: 'input-text',
            props: { name: 'tls_mode', label: 'TLS 模式', hint: 'auto / on / off' }
          },
          {
            type: 'input-text',
            props: { name: 'tls_cert', label: 'TLS 证书路径（PEM）' }
          },
          {
            type: 'input-text',
            props: { name: 'tls_key', label: 'TLS 私钥路径（PEM）' }
          }
        ],
        content: {
          actions: [
            {
              type: 'button',
              props: { text: '保存（重启后生效）', color: 'primary' },
              action: { type: 'submit' }
            }
          ]
        }
      });
      body.push(
        noticeCard(
          '端口 / 前缀 / TLS 会写入 ' +
            panel.config_path +
            '，必须重启 skiff 服务后才生效；重启前当前入口仍然可用。'
        )
      );
    }

    if (data.can_admin) {
      if (data.keys.length === 0) {
        body.push(detailLines('信任公钥（TOFU）', ['信任库为空：未签名包默认拒装']));
      } else {
        body.push(
          detailLines(
            '信任公钥（TOFU）',
            data.keys.map(function (k) {
              return k.fingerprint_short + '… · ' + (k.label || '（无备注）') + ' · ' + k.key_id;
            })
          )
        );
      }
    }
    return {
      type: 'page',
      props: { title: '设置 · Skiff', subtitle: '口令、安全入口与面板偏好' },
      body: body
    };
  }

  // 面板设置保存：复用旧表单校验/落盘/审计，只把传输层换成 JSON。
  function settingsSave(args, ctx) {
    var values = null;
    try {
      values = ctx.scope.get('values');
    } catch (e) {
      values = null;
    }
    values = values || {};
    var payload = {
      listen_addr: String(values.listen_addr || ''),
      listen_port: String(values.listen_port || ''),
      entry_prefix: String(values.entry_prefix || ''),
      tls_mode: String(values.tls_mode || ''),
      tls_cert: String(values.tls_cert || ''),
      tls_key: String(values.tls_key || '')
    };
    return fetch(apiBase + '/settings/panel', {
      method: 'POST',
      credentials: 'same-origin',
      headers: {
        Accept: 'application/json',
        'Content-Type': 'application/json',
        'x-csrf-token': (sessionState && sessionState.csrf_token) || ''
      },
      body: JSON.stringify(payload)
    })
      .then(function (resp) {
        return readJson(resp).then(function (body) {
          return { status: resp.status, body: body };
        });
      })
      .then(function (res) {
        if (res.status !== 200) {
          window.alert(errorMessage(res));
          return;
        }
        window.alert((res.body && res.body.data && res.body.data.message) || '已保存');
        location.reload();
      })
      .catch(function (error) {
        window.alert('保存失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  // 商店：目录只读快照 + 插件安装（下载验签后二次确认）。
  var pendingInstall = null;

  function storeItemTable(section, canInstall) {
    if (!section.items || section.items.length === 0) {
      return textLine(section.title + '暂无条目');
    }
    var table = statusTable(
      [
        { key: 'name', label: '名称' },
        { key: 'version', label: '版本' },
        { key: 'source', label: '来源' },
        { key: 'compatibility', label: '兼容性' },
        { key: 'updated_at', label: '更新时间' },
        { key: 'note', label: '说明' },
        { key: 'install_note', label: '安装提示' }
      ],
      section.items,
      'name'
    );
    if (canInstall && section.title === '插件') {
      table.props.rowActions = [
        {
          type: 'button',
          props: { text: '安装 / 升级', variant: 'tonal' },
          onEvent: { click: { actions: [{ type: 'skiffStoreInstall' }] } }
        }
      ];
    }
    return table;
  }

  function storeSchema(data) {
    var body = [];
    if (data.state && data.state.code !== 'fresh') {
      body.push({
        type: 'alert',
        props: {
          type: data.state.code === 'rejected' ? 'error' : 'warning',
          title: '目录状态：' + data.state.label,
          text: data.message || '目录当前不是最新状态，展示缓存或空列表。'
        }
      });
    }
    if (data.allow_unsigned) {
      body.push({
        type: 'alert',
        props: {
          type: 'warning',
          text: '当前允许未签名渠道（开发模式），生产环境应导入固定公钥。'
        }
      });
    }
    body.push(
      detailLines('目录信息', [
        '渠道：' + data.channel + ' · seq：' + data.seq,
        '拉取时间：' + (data.fetched_at || '—'),
        '来源：' + (data.source || '—'),
        '条目总数：' + data.total,
        '公钥已配置：' + (data.key_configured ? '是' : '否')
      ])
    );
    (data.sections || []).forEach(function (section) {
      body.push({
        type: 'card',
        props: { title: section.title, subtitle: section.hint },
        content: { body: [storeItemTable(section, data.can_install)] }
      });
    });
    var upgrade = data.upgrade || {};
    var upgradeLines = [
      '当前版本：' + upgrade.current + '（' + upgrade.arch + '）',
      '目录可用版本：' + (upgrade.available || '无'),
      '渠道：' + upgrade.channel,
      '最低来源版本：' + (upgrade.min_from || '—')
    ];
    if (upgrade.changelog_url) upgradeLines.push('更新说明：' + upgrade.changelog_url);
    if (upgrade.note) upgradeLines.push(upgrade.note);
    body.push(detailLines('内核升级通道', upgradeLines));
    body.push({
      type: 'link',
      props: {
        text: '打开旧版商店（升级包下载 / 表单确认页）',
        href: prefix + '/m/core/plugins/store'
      }
    });
    return {
      type: 'page',
      props: { title: '商店 · Skiff', subtitle: '目录快照与插件安装' },
      body: body
    };
  }

  function storeInstall(args) {
    if (!args || args.installable === false) {
      window.alert((args && args.install_note) || '该条目当前不可安装');
      return Promise.resolve();
    }
    var name = args.name;
    // 下载 + 验签是同步阻塞的单次请求，暂无进度事件；用明确提示占位。
    window.alert('正在下载并验签 ' + name + ' ' + (args.version || '') + '，请稍候…');
    return fetch(apiBase + '/store/plugins/' + encodeURIComponent(name) + '/install', {
      method: 'POST',
      credentials: 'same-origin',
      headers: {
        Accept: 'application/json',
        'Content-Type': 'application/json',
        'x-csrf-token': (sessionState && sessionState.csrf_token) || ''
      },
      body: JSON.stringify({ version: args.version || '' })
    })
      .then(function (resp) {
        return readJson(resp).then(function (body) {
          return { status: resp.status, body: body };
        });
      })
      .then(function (res) {
        if (res.status !== 200 || !res.body || !res.body.data) {
          window.alert(errorMessage(res));
          return;
        }
        pendingInstall = res.body.data;
        embedWithError(storeConfirmSchema(res.body.data));
      })
      .catch(function (error) {
        window.alert('安装准备失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  function storeConfirmSchema(data) {
    var body = [];
    body.push(
      detailLines('待安装插件', [
        '名称：' + data.name + '（' + data.id + '）',
        '版本：' + data.version + ' · API ' + data.api,
        '来源：' + data.channel + ' · ' + data.source,
        '签名：' + (data.signed ? '已验证' : '未验证') + ' · 公钥 ' + data.key_id,
        '宿主能力：' + (data.requires.length ? data.requires.join(', ') : '无'),
        '出网：' + (data.egress.length ? data.egress.join(', ') : '无（默认拒绝）')
      ])
    );
    body.push(
      detailLines(
        '申请权限',
        data.permissions.length ? data.permissions : ['无额外动作权限']
      )
    );
    body.push(
      detailLines(
        '对象授权',
        data.resources.length ? data.resources : ['无资源对象']
      )
    );
    body.push({
      type: 'alert',
      props: {
        type: 'info',
        text: '确认即按上述权限安装；暂存包在安装完成或失败后都会清理。'
      }
    });
    body.push({
      type: 'flex',
      props: { gap: 8 },
      content: {
        body: [
          {
            type: 'button',
            props: { text: '确认安装', color: 'primary' },
            onEvent: { click: { actions: [{ type: 'skiffStoreConfirm' }] } }
          },
          {
            type: 'button',
            props: { text: '取消', variant: 'tonal' },
            onEvent: {
              click: { actions: [{ type: 'navigate', props: { url: prefix + '/store' } }] }
            }
          }
        ]
      }
    });
    return {
      type: 'page',
      props: { title: '确认安装 · 商店', subtitle: data.name },
      body: body
    };
  }

  function storeConfirm() {
    if (!pendingInstall) {
      window.alert('待确认的安装已失效，请返回商店重新发起。');
      return Promise.resolve();
    }
    var data = pendingInstall;
    return fetch(apiBase + '/plugins/confirm', {
      method: 'POST',
      credentials: 'same-origin',
      headers: {
        Accept: 'application/json',
        'Content-Type': 'application/json',
        'x-csrf-token': (sessionState && sessionState.csrf_token) || ''
      },
      body: JSON.stringify({
        token: data.token,
        grant: data.permissions,
        resource: data.resources,
        extra_resource: ''
      })
    })
      .then(function (resp) {
        return readJson(resp).then(function (body) {
          return { status: resp.status, body: body };
        });
      })
      .then(function (res) {
        if (res.status !== 200) {
          window.alert(errorMessage(res));
          return;
        }
        pendingInstall = null;
        window.alert('安装完成');
        location.assign((res.body && res.body.data && res.body.data.redirect_to) || prefix + '/plugins');
      })
      .catch(function (error) {
        window.alert('安装失败：' + (error && error.message ? error.message : String(error)));
      });
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
      sessionState = session;
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
      if (parts[0] === 'containers' && parts.length === 1) {
        var containerFilter =
          new URLSearchParams(location.search).get('filter') === 'error' ? 'filter=error' : '';
        getJson('/containers' + listQuery(containerFilter), function (data) {
          embedWithError(containersSchema(data));
        });
        return;
      }
      if (parts[0] === 'containers' && parts.length === 2) {
        getJson('/containers/' + encodeURIComponent(parts[1]), function (data) {
          embedWithError(containerDetailSchema(data));
        });
        return;
      }
      if (parts[0] === 'images' && parts.length === 1) {
        getJson('/images' + listQuery(), function (data) {
          embedWithError(imagesSchema(data));
        });
        return;
      }
      if (parts[0] === 'networks' && parts.length === 1) {
        getJson('/networks' + listQuery(), function (data) {
          embedWithError(networksSchema(data));
        });
        return;
      }
      if (parts[0] === 'volumes' && parts.length === 1) {
        getJson('/volumes' + listQuery(), function (data) {
          embedWithError(volumesSchema(data));
        });
        return;
      }
      if (parts[0] === 'compose' && parts.length === 1) {
        getJson('/compose', function (data) {
          embedWithError(composeSchema(data));
        });
        return;
      }
      if (parts[0] === 'tasks' && parts.length === 1) {
        getJson('/tasks' + listQuery(), function (data) {
          embedWithError(tasksSchema(data));
        });
        return;
      }
      if (parts[0] === 'tasks' && parts.length === 2) {
        getJson('/tasks/' + encodeURIComponent(parts[1]), function (data) {
          embedWithError(taskDetailSchema(data));
        });
        return;
      }
      if (parts[0] === 'jobs' && parts.length === 1) {
        getJson('/jobs' + listQuery(), function (data) {
          embedWithError(jobsSchema(data));
        });
        return;
      }
      if (parts[0] === 'jobs' && parts.length === 2) {
        getJson('/jobs/' + encodeURIComponent(parts[1]), function (data) {
          embedWithError(jobDetailSchema(data));
        });
        return;
      }
      if (parts[0] === 'store') {
        var refresh = new URLSearchParams(location.search).get('refresh') === '1' ? '?refresh=1' : '';
        getJson('/store' + refresh, function (data) {
          embedWithError(storeSchema(data));
        });
        return;
      }
      if (parts[0] === 'settings') {
        getJson('/settings', function (data) {
          embedWithError(settingsSchema(data));
        });
        return;
      }
      if (parts[0] === 'audit') {
        embedWithError({
          type: 'page',
          props: { title: '审计日志 · Skiff', subtitle: '该功能正在适配中' },
          body: [
            {
              type: 'alert',
              props: {
                type: 'warning',
                title: '该功能正在适配中',
                text:
                  '审计检索需要后端提供关键字 + 游标查询（当前只有最近 200 条），按约定不在前端自造分页；旧版页面暂保留完整列表。'
              }
            },
            {
              type: 'link',
              props: { text: '打开旧版审计页（最近 200 条）', href: prefix + '/m/core/settings/audit' }
            }
          ]
        });
        return;
      }
      embed(placeholderSchema(session));
    })
    .catch(function (error) {
      showError('无法连接服务：' + (error && error.message ? error.message : String(error)));
    });
})();
