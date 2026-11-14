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
  var buddingVersion = mount.getAttribute('data-budding-version') || '';

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

  // 概览指标卡：大号数值 + 一行补充说明，比多行纯文本更容易扫读。
  function metricCard(title, value, suffix, caption, color) {
    var stat = { value: value, label: caption, suffix: suffix };
    if (color) stat.color = color;
    return {
      type: 'card',
      props: { title: title, variant: 'outlined' },
      content: { body: [{ type: 'statistic', props: stat }] }
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
      props: { columns: 2, gap: 16 },
      content: {
        body: [
          metricCard(
            '容器',
            data.containers.running,
            ' 个',
            '运行中 · 共 ' + data.containers.total + ' 个'
          ),
          metricCard(
            '网站',
            data.sites.total,
            ' 个',
            data.sites.abnormal > 0 ? data.sites.abnormal + ' 个异常' : '全部正常',
            data.sites.abnormal > 0 ? 'warning' : 'success'
          ),
          metricCard(
            '插件',
            data.plugins.total,
            ' 个',
            '已启用 ' + data.plugins.enabled + ' · 停用 ' + data.plugins.disabled
          ),
          metricCard(
            '运行环境',
            data.runtimes.total,
            ' 个',
            '异常 ' + data.runtimes.abnormal + ' · 未安装 ' + data.runtimes.uninstalled,
            data.runtimes.abnormal > 0 ? 'warning' : null
          )
        ]
      }
    });
    // `overview.health` 槽位的原生替代：数据来自主机监控插件的 snapshot RPC。
    // 没有提供者时也渲染原因卡片，不让整块静默消失。
    if (data.health) {
      if (data.health.error) {
        body.push({
          type: 'card',
          props: { title: '系统状态' },
          content: {
            body: [
              { type: 'text', props: { text: '暂不可用：' + data.health.error } },
              {
                type: 'text',
                props: {
                  text: '请检查主机监控插件是否已启用、授权根（path:/proc）是否完整，以及当前角色是否具备插件管理权限。'
                }
              }
            ]
          }
        });
      } else {
        if (data.health.cpu_high || data.health.mem_high) {
          body.push({
            type: 'alert',
            props: {
              type: 'warning',
              text:
                '主机资源超过阈值：CPU ' +
                data.health.cpu_text +
                '，内存 ' +
                data.health.mem_text
            }
          });
        }
        body.push({
          type: 'card',
          props: { title: '系统状态' },
          content: {
            body: [
              {
                type: 'grid',
                props: { columns: 3, gap: 12 },
                content: {
                  body: [
                    {
                      type: 'statistic',
                      props: {
                        value: data.health.cpu_text,
                        label: 'CPU',
                        color: data.health.cpu_high ? 'warning' : null
                      }
                    },
                    {
                      type: 'statistic',
                      props: {
                        value: data.health.mem_text,
                        label: '内存',
                        color: data.health.mem_high ? 'warning' : null
                      }
                    },
                    {
                      type: 'statistic',
                      props: { value: data.health.load_text, label: '负载' }
                    }
                  ]
                }
              },
              {
                type: 'text',
                props: {
                  text:
                    '运行时长 ' +
                    data.health.uptime_text +
                    ' · 采集时间 ' +
                    data.health.t_text +
                    '（UTC）'
                }
              }
            ]
          }
        });
      }
    }
    body.push(textLine('采集时间 ' + data.collected_at));
    return {
      type: 'page',
      props: {
        title: '概览 · Skiff',
        subtitle: '只读计数，不做实时刷新；趋势图表待定（D14–D16），当前不展示'
      },
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

  /// 任务行内按钮（rowActions 元素是完整节点，不是描述对象）。
  function taskRowButton(text, props, color) {
    return {
      type: 'button',
      props: { text: text, variant: 'text', color: color || 'primary' },
      onEvent: {
        click: { actions: [{ type: 'skiffTaskWrite', props: props }] }
      }
    };
  }

  /// 定时任务写操作：切换启停、立即运行、删除（内置任务由后端拒绝删除）。
  function taskWrite(args) {
    var op = args.op;
    if (op === 'delete' && !window.confirm('确认删除任务 ' + (args.name || args.id) + '？')) {
      return Promise.resolve();
    }
    var payload = {};
    var path = '/tasks/' + encodeURIComponent(args.id) + '/' + op;
    if (op === 'toggle') {
      var current = args.enabled === true || args.enabled === 'true';
      payload = { enabled: !current };
    }
    return submitJson(path, payload)
      .then(function () {
        location.reload();
      })
      .catch(function (error) {
        window.alert('操作失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  /// 作业写操作：请求取消、重试；重试成功后跳到新作业详情。
  function jobWrite(args) {
    var op = args.op;
    if (op === 'cancel' && !window.confirm('确认请求取消该作业？')) {
      return Promise.resolve();
    }
    return submitJson('/jobs/' + encodeURIComponent(args.id) + '/' + op, {})
      .then(function (data) {
        if (op === 'retry' && data.id) {
          location.href = prefix + '/jobs/' + encodeURIComponent(data.id);
          return;
        }
        if (data.message) window.alert(data.message);
        location.reload();
      })
      .catch(function (error) {
        window.alert('操作失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  /// 回收站写操作：复用 `/{p}/files/trash/*`（CSRF 与动作权限由既有接口统一执行）。
  function trashWrite(args) {
    var op = args.op;
    if (op === 'refresh') {
      location.reload();
      return Promise.resolve();
    }
    var base = prefix + '/files/trash/';
    if (op === 'clear') {
      if (!window.confirm('确认清空回收站？将永久删除全部条目，且不会进入任何备份。')) {
        return Promise.resolve();
      }
      return postPrefixJson(base + 'clear', {})
        .then(function (data) {
          var failed = (data.result && data.result.failed) || [];
          window.alert(
            failed.length
              ? '已清理部分条目，' + failed.length + ' 项失败：' + failed.join('；')
              : '已清空回收站。'
          );
          location.reload();
        })
        .catch(function (error) {
          window.alert('清空失败：' + (error && error.message ? error.message : String(error)));
        });
    }
    if (op === 'purge') {
      if (!window.confirm('确认彻底删除 ' + (args.path || '该条目') + '？不可恢复。')) {
        return Promise.resolve();
      }
      return postPrefixJson(base + 'purge', { id: args.id })
        .then(function () {
          location.reload();
        })
        .catch(function (error) {
          window.alert('彻底删除失败：' + (error && error.message ? error.message : String(error)));
        });
    }
    if (op === 'restore') {
      return postPrefixJson(base + 'restore', { id: args.id, overwrite: false })
        .then(function () {
          location.reload();
        })
        .catch(function (error) {
          var message = error && error.message ? error.message : String(error);
          if (!window.confirm('恢复失败：' + message + '\n\n是否覆盖现有对象后重试？')) {
            window.alert('恢复已取消。');
            return;
          }
          return postPrefixJson(base + 'restore', { id: args.id, overwrite: true })
            .then(function () {
              location.reload();
            })
            .catch(function (retryError) {
              window.alert(
                '恢复失败：' +
                  (retryError && retryError.message ? retryError.message : String(retryError))
              );
            });
        });
    }
    window.alert('未知的回收站操作：' + op);
    return Promise.resolve();
  }

  /// 配额写操作：设置 / 重新统计 / 取消，复用既有 files JSON 接口。
  function quotaWrite(args, ctx) {
    var op = args.op;
    var values = formValues(ctx);
    var path = op === 'set' ? formText(values.path).trim() : args.path;
    var base = prefix + '/files/quotas/';
    if (op === 'set') {
      var bytes = formText(values.max_bytes).trim();
      var files = formText(values.max_files).trim();
      if (!path) {
        window.alert('请填写目录树的绝对路径。');
        return Promise.resolve();
      }
      if (!bytes && !files) {
        window.alert('容量上限与文件数上限至少设置一项；如需取消请用行内「取消配额」。');
        return Promise.resolve();
      }
      return postPrefixJson(base + 'set', {
        path: path,
        max_bytes: bytes ? Number(bytes) : null,
        max_files: files ? Number(files) : null
      })
        .then(function () {
          window.alert('已保存配额：' + path);
          location.reload();
        })
        .catch(function (error) {
          window.alert('保存配额失败：' + (error && error.message ? error.message : String(error)));
        });
    }
    if (op === 'scan') {
      if (!window.confirm('重新统计会递归扫描整棵目录树，可能耗时较久。\n\n' + path)) {
        return Promise.resolve();
      }
      return postPrefixJson(base + 'scan', { path: path })
        .then(function () {
          window.alert('统计完成：' + path);
          location.reload();
        })
        .catch(function (error) {
          window.alert('重新统计失败：' + (error && error.message ? error.message : String(error)));
        });
    }
    if (op === 'cancel') {
      if (!window.confirm('确认取消该目录的配额？\n\n' + path)) {
        return Promise.resolve();
      }
      return postPrefixJson(base + 'cancel', { path: path })
        .then(function () {
          location.reload();
        })
        .catch(function (error) {
          window.alert('取消配额失败：' + (error && error.message ? error.message : String(error)));
        });
    }
    window.alert('未知的配额操作：' + op);
    return Promise.resolve();
  }

  function formText(value) {
    return String(value === undefined || value === null ? '' : value);
  }

  function formLines(value) {
    return formText(value)
      .split(/[\n,]/)
      .map(function (s) {
        return s.trim();
      })
      .filter(function (s) {
        return s !== '';
      });
  }

  /// Docker 写操作统一入口：容器新建/资源限制、镜像拉取/删除、网络与卷增删。
  /// 全部走 `/ui/v1` 的 JSON 端点，复用既有权限矩阵、CSRF 与审计。
  function dockerWrite(args, ctx) {
    var values = formValues(ctx);
    var kind = args.kind;
    var op = args.op;
    if (op === 'remove') {
      var display = args.name || args.id;
      if (!window.confirm('确认删除 ' + display + '？此操作不可撤销。')) {
        return Promise.resolve();
      }
    }
    var path = '';
    var payload = {};
    if (kind === 'container' && op === 'create') {
      path = '/containers';
      payload = {
        image: formText(values.image),
        name: formText(values.name),
        ports: formText(values.ports),
        env: formText(values.env),
        binds: formText(values.binds),
        volumes: formText(values.volumes),
        restart: formText(values.restart),
        cpu_shares: formText(values.cpu_shares),
        cpu_period: formText(values.cpu_period),
        cpu_quota: formText(values.cpu_quota),
        memory_limit_mb: formText(values.memory_limit_mb),
        memory_swap_mb: formText(values.memory_swap_mb),
        pids_limit: formText(values.pids_limit),
        networks: formLines(values.networks)
      };
    } else if (kind === 'container' && op === 'limits') {
      path = '/containers/' + encodeURIComponent(args.id) + '/limits';
      payload = {
        cpu_shares: formText(values.cpu_shares),
        cpu_period: formText(values.cpu_period),
        cpu_quota: formText(values.cpu_quota),
        memory_limit_mb: formText(values.memory_limit_mb),
        memory_swap_mb: formText(values.memory_swap_mb),
        pids_limit: formText(values.pids_limit)
      };
    } else if (kind === 'image' && op === 'pull') {
      path = '/images/pull';
      payload = { image: formText(values.image) };
    } else if (kind === 'image' && op === 'remove') {
      path = '/images/' + encodeURIComponent(args.id) + '/remove';
    } else if (kind === 'network' && op === 'create') {
      path = '/networks';
      payload = { name: formText(values.name) };
    } else if (kind === 'network' && op === 'remove') {
      path = '/networks/' + encodeURIComponent(args.id) + '/remove';
    } else if (kind === 'volume' && op === 'create') {
      path = '/volumes';
      payload = { name: formText(values.name) };
    } else if (kind === 'volume' && op === 'remove') {
      path = '/volumes/' + encodeURIComponent(args.id) + '/remove';
    } else {
      window.alert('未知的 Docker 操作：' + kind + '/' + op);
      return Promise.resolve();
    }
    return submitJson(path, payload)
      .then(function (data) {
        if (kind === 'container' && op === 'create' && data.id) {
          location.href = prefix + '/containers/' + encodeURIComponent(data.id);
          return;
        }
        location.reload();
      })
      .catch(function (error) {
        window.alert('操作失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  /// 站点写操作统一入口：建站、启停、默认、域名、配置、证书、备份、删除、批量。
  /// 权限、CSRF 与审计全部由 `/ui/v1` 复用既有站点命令。
  function siteWrite(args, ctx) {
    var values = formValues(ctx);
    var op = args.op;
    var domain = args.domain || '';
    var path = '';
    var payload = {};
    if (op === 'create') {
      path = '/sites';
      payload = {
        site_type: formText(values.site_type),
        domain: formText(values.domain),
        root_dir: formText(values.root_dir),
        runtime_id: formText(values.runtime_id),
        proxy_target: formText(values.proxy_target),
        proxy_websocket: !!values.proxy_websocket,
        aliases: formLines(values.aliases),
        aliases_bulk: ''
      };
    } else if (op === 'power') {
      path = '/sites/' + encodeURIComponent(domain) + '/' + args.action;
    } else if (op === 'default') {
      path = '/sites/' + encodeURIComponent(domain) + '/default';
      payload = { enabled: !!args.enabled };
    } else if (op === 'domains') {
      path = '/sites/' + encodeURIComponent(domain) + '/domains';
      payload = { action: args.action, alias: args.alias || formText(values.alias) };
    } else if (op === 'config') {
      path = '/sites/' + encodeURIComponent(domain) + '/config';
      payload = { content: formText(values.content) };
    } else if (op === 'ssl') {
      if (args.action === 'delete_cert' && !window.confirm('确认删除站点证书？')) {
        return Promise.resolve();
      }
      path = '/sites/' + encodeURIComponent(domain) + '/ssl';
      payload = {
        action: args.action,
        enabled: !!args.enabled,
        cert_pem: formText(values.cert_pem),
        key_pem: formText(values.key_pem)
      };
    } else if (op === 'backup') {
      path = '/sites/' + encodeURIComponent(domain) + '/backup';
      payload = { enabled: !!args.enabled };
    } else if (op === 'delete') {
      var typed = window.prompt('删除站点不可恢复。请输入域名 ' + domain + ' 以确认：');
      if (typed === null) return Promise.resolve();
      path = '/sites/' + encodeURIComponent(domain) + '/delete';
      payload = { confirm: typed.trim() };
    } else if (op === 'batch') {
      path = '/sites/batch';
      payload = {
        action: args.action || formText(values.action),
        domains: formLines(values.domains)
      };
    } else {
      window.alert('未知的站点操作：' + op);
      return Promise.resolve();
    }
    return submitJson(path, payload)
      .then(function (data) {
        if (op === 'create' && data.domain) {
          location.href = prefix + '/sites/' + encodeURIComponent(data.domain);
          return;
        }
        if (op === 'delete') {
          location.href = prefix + '/sites';
          return;
        }
        location.reload();
      })
      .catch(function (error) {
        window.alert('操作失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  /// 内核内置菜单图标名 → budding 包内图标子集（`Budding Icons`）。
  ///
  /// 1.8.0-r4 的子集含镜像、磁盘、文件夹等业务字形，全部语义名直接映射，
  /// 不再做近似兜底或留空。
  var MENU_ICONS = {
    overview: 'mdi-view-dashboard',
    globe: 'mdi-web',
    docker: 'mdi-docker',
    plug: 'mdi-puzzle',
    tools: 'mdi-wrench',
    settings: 'mdi-cog',
    container: 'mdi-cube-outline',
    shield: 'mdi-shield-outline',
    lock: 'mdi-lock-outline',
    network: 'mdi-lan',
    user: 'mdi-account-outline',
    activity: 'mdi-pulse',
    list: 'mdi-format-list-bulleted',
    archive: 'mdi-archive-outline',
    clock: 'mdi-clock-outline',
    file: 'mdi-file-document',
    dot: 'mdi-circle',
    folder: 'mdi-folder-outline',
    disk: 'mdi-harddisk',
    image: 'mdi-image-outline',
    'hard-drive': 'mdi-harddisk'
  };

  function iconFor(name) {
    return MENU_ICONS[name] || null;
  }

  /// 未登记的语义名直接省略 icon 字段，避免导航渲染出空白图标占位。
  function withIcon(route, name) {
    var icon = iconFor(name);
    if (icon) route.icon = icon;
    return route;
  }

  /// 已迁移到 budding 的菜单项：稳定键 → 路由路径（相对入口前缀）。
  /// 未列出的键保留内核深链 `href`（插件功能页仍是内核 SSR + iframe）。
  var BUILTIN_PAGES = {
    overview: { path: '', schema: 'skiff:overview' },
    'core/sites/list': { path: '/sites', schema: 'skiff:sites' },
    'core/sites/web': { path: '/sites/web', schema: 'skiff:web' },
    'core/sites/env': { path: '/runtimes', schema: 'skiff:runtimes' },
    'core/docker/containers': { path: '/containers', schema: 'skiff:containers' },
    'core/docker/images': { path: '/images', schema: 'skiff:images' },
    'core/docker/networks': { path: '/networks', schema: 'skiff:networks' },
    'core/docker/volumes': { path: '/volumes', schema: 'skiff:volumes' },
    'core/docker/compose': { path: '/compose', schema: 'skiff:compose' },
    'core/plugins/installed': { path: '/plugins', schema: 'skiff:plugins' },
    'core/plugins/discover': { path: '/plugins/discover', schema: 'skiff:plugins' },
    'core/plugins/updates': { path: '/plugins/updates', schema: 'skiff:plugins' },
    'core/plugins/store': { path: '/store', schema: 'skiff:store' },
    'core/tasks': { path: '/tasks', schema: 'skiff:tasks' },
    'core/jobs': { path: '/jobs', schema: 'skiff:jobs' },
    'core/files': { path: '/files', schema: 'skiff:files' },
    'core/settings/security': { path: '/settings', schema: 'skiff:settings' },
    'core/settings/menu': { path: '/settings/menu', schema: 'skiff:menu-settings' },
    'core/docker/settings': { path: '/settings/docker', schema: 'skiff:docker-settings' },
    'core/settings/audit': { path: '/audit', schema: 'skiff:audit' }
  };

  /// 插件菜单键 → `/ui/v1/menu` 下发的视图元数据（id / 默认视图 / 视图列表）。
  var pluginMeta = {};

  /// 一个菜单项 → 一条路由记录。命中已迁移页面时给出 budding 路径与 Schema；
  /// 插件功能页走 `/plugins/<id>/<view>`，由壳页加载随包 schema 原生渲染。
  function menuRoute(key, label, icon, href) {
    if (key.indexOf('plugin/') === 0) {
      var meta = pluginMeta[key];
      if (!meta || !meta.plugin_id || !meta.default_view) return null;
      return withIcon(
        {
          path:
            prefix +
            '/plugins/' +
            encodeURIComponent(meta.plugin_id) +
            '/' +
            encodeURIComponent(meta.default_view),
          title: label,
          schema: 'skiff:plugin-view',
          meta: { activeMenu: prefix + '/plugins' }
        },
        icon
      );
    }
    var page = BUILTIN_PAGES[key];
    if (page) {
      return withIcon(
        {
          path: page.path ? prefix + page.path : prefix,
          title: label,
          schema: page.schema
        },
        icon
      );
    }
    if (!href) return null;
    return withIcon(
      {
        path: prefix + '/__menu-item/' + encodeURIComponent(key),
        href: href,
        target: '_self',
        title: label
      },
      icon
    );
  }

  /// 内核菜单树 → budding 路由表（菜单是路由的投影，不另造第二份菜单）。
  function buildRoutes(menuData) {
    var routes = [];
    (menuData.groups || []).forEach(function (group) {
      (group.items || []).forEach(function (item) {
        if (item.plugin && item.plugin_id && item.views) {
          pluginMeta[item.key] = item;
        }
      });
    });
    (menuData.groups || []).forEach(function (group, index) {
      var order = index * 10;
      if (!group.items || group.items.length === 0) {
        var leaf = menuRoute(group.key, group.label, group.icon, group.href);
        if (leaf) {
          leaf.meta = { order: order };
          routes.push(leaf);
        }
        return;
      }
      var children = [];
      group.items.forEach(function (item) {
        var child = menuRoute(item.key, item.label, item.icon, item.href);
        if (child) children.push(child);
      });
      // 工具空态：保留内核空态文案，指向已原生渲染的插件中心。
      if (children.length === 0 && group.empty) {
        children.push({
          path: prefix + '/__menu-empty/' + encodeURIComponent(group.key),
          title: group.empty.text
        });
        children[0].schema = 'skiff:plugins';
      }
      if (children.length === 0) return;
      // 分组本身不是地址：用入口前缀下的保留段承载菜单层级，子项用绝对路径。
      routes.push(
        withIcon(
          {
            path: prefix + '/__menu/' + encodeURIComponent(group.key),
            title: group.label,
            meta: { order: order },
            children: children
          },
          group.icon
        )
      );
    });
    // 详情页：菜单不出现，但地址栏直达与列表行内跳转都走同一路由。
    routes.push({
      path: prefix + '/sites/new',
      title: '新建站点',
      hidden: true,
      schema: 'skiff:site-new',
      meta: { activeMenu: prefix + '/sites' }
    });
    routes.push({
      path: prefix + '/sites/:domain',
      title: '站点详情',
      hidden: true,
      schema: 'skiff:site-detail',
      meta: { activeMenu: prefix + '/sites' }
    });
    routes.push({
      path: prefix + '/runtimes/:id',
      title: '运行环境详情',
      hidden: true,
      schema: 'skiff:runtime-detail',
      meta: { activeMenu: prefix + '/runtimes' }
    });
    routes.push({
      path: prefix + '/containers/new',
      title: '新建容器',
      hidden: true,
      schema: 'skiff:container-new',
      meta: { activeMenu: prefix + '/containers' }
    });
    routes.push({
      path: prefix + '/containers/:id',
      title: '容器详情',
      hidden: true,
      schema: 'skiff:container-detail',
      meta: { activeMenu: prefix + '/containers' }
    });
    routes.push({
      path: prefix + '/tasks/:id',
      title: '任务详情',
      hidden: true,
      schema: 'skiff:task-detail',
      meta: { activeMenu: prefix + '/tasks' }
    });
    routes.push({
      path: prefix + '/jobs/:id',
      title: '作业详情',
      hidden: true,
      schema: 'skiff:job-detail',
      meta: { activeMenu: prefix + '/jobs' }
    });
    routes.push({
      path: prefix + '/containers/:id/terminal',
      title: '容器终端',
      hidden: true,
      schema: 'skiff:container-terminal',
      meta: { activeMenu: prefix + '/containers' }
    });
    routes.push({
      path: prefix + '/containers/:id/logs',
      title: '容器日志',
      hidden: true,
      schema: 'skiff:container-logs',
      meta: { activeMenu: prefix + '/containers' }
    });
    routes.push({
      path: prefix + '/runtimes/:id/logs',
      title: '运行环境日志',
      hidden: true,
      schema: 'skiff:runtime-logs',
      meta: { activeMenu: prefix + '/runtimes' }
    });
    routes.push({
      path: prefix + '/jobs/:id/logs',
      title: '作业日志',
      hidden: true,
      schema: 'skiff:job-logs',
      meta: { activeMenu: prefix + '/jobs' }
    });
    routes.push({
      path: prefix + '/files/view',
      title: '代码查看器',
      hidden: true,
      schema: 'skiff:file-view',
      meta: { activeMenu: prefix + '/files' }
    });
    routes.push({
      path: prefix + '/files/trash',
      title: '回收站',
      hidden: true,
      schema: 'skiff:files-trash',
      meta: { activeMenu: prefix + '/files' }
    });
    routes.push({
      path: prefix + '/files/quotas',
      title: '配额管理',
      hidden: true,
      schema: 'skiff:files-quotas',
      meta: { activeMenu: prefix + '/files' }
    });
    routes.push({
      path: prefix + '/settings/menu',
      title: '菜单设置',
      hidden: true,
      schema: 'skiff:menu-settings',
      meta: { activeMenu: prefix + '/settings' }
    });
    routes.push({
      path: prefix + '/settings/docker',
      title: 'Docker 设置',
      hidden: true,
      schema: 'skiff:docker-settings',
      meta: { activeMenu: prefix + '/settings' }
    });
    routes.push({
      path: prefix + '/sites/:domain/logs',
      title: '站点日志',
      hidden: true,
      schema: 'skiff:site-logs',
      meta: { activeMenu: prefix + '/sites' }
    });
    routes.push({
      path: prefix + '/plugins/:id',
      title: '插件详情',
      hidden: true,
      schema: 'skiff:plugin-info',
      meta: { activeMenu: prefix + '/plugins' }
    });
    routes.push({
      path: prefix + '/store/confirm',
      title: '安装确认',
      hidden: true,
      schema: 'skiff:store-confirm',
      meta: { activeMenu: prefix + '/store' }
    });
    // 插件功能页：每个可见视图一条路由；页面结构由随包 schema 提供，
    // 壳页只做数据绑定与 RPC 转发。
    Object.keys(pluginMeta).forEach(function (key) {
      var meta = pluginMeta[key];
      (meta.views || []).forEach(function (v) {
        if (!v || !v.view || v.view === meta.default_view) return;
        routes.push({
          path:
            prefix +
            '/plugins/' +
            encodeURIComponent(meta.plugin_id) +
            '/' +
            encodeURIComponent(v.view),
          title: meta.label + ' · ' + v.label,
          hidden: true,
          schema: 'skiff:plugin-view',
          meta: { activeMenu: prefix + '/plugins' }
        });
      });
    });
    return routes;
  }

  /// 壳页 Schema：后台骨架 + 路由出口。菜单与当前页都由内核路由表驱动。
  function shellSchema(session) {
    return {
      type: 'admin',
      props: { title: 'Skiff', brandTitle: 'Skiff' },
      content: {
        actions: [
          {
            type: 'text',
            props: { text: session.username + '（' + session.role + '）', size: 'small' }
          },
          {
            type: 'button',
            props: { text: '退出登录', variant: 'text' },
            onEvent: { click: { actions: [{ type: 'skiffLogout' }] } }
          }
        ],
        body: [{ type: 'outlet' }]
      }
    };
  }

  /// 登出：复用 `/ui/v1/session/logout` 与页面写操作同一个 CSRF 头。
  function logout() {
    return fetch(apiBase + '/session/logout', {
      method: 'POST',
      credentials: 'same-origin',
      headers: {
        Accept: 'application/json',
        'x-csrf-token': (sessionState && sessionState.csrf_token) || ''
      }
    }).then(function (resp) {
      if (!resp.ok) throw new Error('服务返回状态 ' + resp.status);
      window.location.href = prefix + '/login';
    });
  }

  function embed(schema, routes) {
    var options = {
      routerMode: 'history',
      routes: routes,
      documentTitle: 'Skiff · {title}',
      platform: { loadJson: loadJson, request: apiRequest, upload: uploadFile },
      actions: {
        skiffContainerAction: containerAction,
        skiffDockerWrite: dockerWrite,
        skiffQueryNav: queryNav,
        skiffSiteWrite: siteWrite,
        skiffTaskWrite: taskWrite,
        skiffJobWrite: jobWrite,
        skiffTrashWrite: trashWrite,
        skiffQuotaWrite: quotaWrite,
        skiffComposeAction: composeAction,
        skiffComposeEdit: composeEdit,
        skiffSettingsSave: settingsSave,
        skiffMenuWrite: menuWrite,
        skiffDockerWrite: dockerSettingsWrite,
        skiffRuntimeCreate: runtimeCreate,
        skiffRuntimeImages: runtimeImagesWrite,
        skiffRuntimeAction: runtimeAction,
        skiffRuntimeProbe: runtimeProbe,
        skiffRuntimeConfig: runtimeConfigWrite,
        skiffRuntimeNode: runtimeNodeWrite,
        skiffRuntimePort: runtimePortWrite,
        skiffRuntimeFile: runtimeFileWrite,
        skiffRuntimeExt: runtimeExt,
        skiffRuntimePecl: runtimePecl,
        skiffRuntimeClone: runtimeClone,
        skiffRuntimeUnbind: runtimeUnbind,
        skiffWebConfig: webConfigWrite,
        skiffWebParams: webParamsWrite,
        skiffWebLogs: webLogsLoad,
        skiffPasswordChange: passwordChange,
        skiffTotpSetup: totpSetup,
        skiffTotpEnable: totpEnable,
        skiffKeyImport: keyImport,
        skiffKeyRemove: keyRemove,
        skiffFileOpen: fileOpen,
        skiffFileDownload: fileDownload,
        skiffFileMkdir: fileMkdir,
        skiffFileRename: fileRename,
        skiffFileRemove: fileRemove,
        skiffLogSwitch: logSwitch,
        skiffLogFilter: logFilter,
        skiffLogReload: logReload,
        skiffAuditSearch: auditSearch,
        skiffAuditNext: auditNext,
        skiffTerminalReconnect: function () {
          window.location.reload();
        },
        skiffStoreInstall: storeInstall,
        skiffStoreConfirm: storeConfirm,
        skiffUpgradeDownload: storeUpgradeDownload,
        skiffPluginToggle: pluginToggle,
        skiffPluginDelete: pluginDelete,
        skiffPluginRpc: pluginRpcAction,
        skiffPluginView: pluginViewAction,
        skiffLogout: function () {
          return logout().catch(function (error) {
            window.alert('退出失败：' + (error && error.message ? error.message : String(error)));
          });
        }
      }
    };
    if (cssHref) options.css = cssHref;
    registerPacks();
    window.BuddingSDK.embed(mount, schema, options).catch(function (error) {
      showError('初始化失败：' + (error && error.message ? error.message : String(error)));
    });
  }


  /// 独立 Pack：`code` 组件的语法高亮由 @budding/code 提供。
  /// 未注册或加载失败时组件按纯文本工作，不影响只读查看。
  function registerPacks() {
    if (!buddingVersion || !window.BuddingSDK) return;
    if (typeof window.BuddingSDK.registerPack !== 'function') return;
    try {
      window.BuddingSDK.registerPack(
        'code',
        prefix +
          '/static/budding/budding-code-' +
          buddingVersion +
          '.min.js?v=' +
          buddingVersion
      );
      window.BuddingSDK.registerPack(
        'terminal',
        prefix +
          '/static/budding/budding-terminal-' +
          buddingVersion +
          '.min.js?v=' +
          buddingVersion
      );
    } catch (e) {
      /* 注册失败只影响高亮/终端，其余页面仍可用 */
    }
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

  function noticeCard(notice) {
    return {
      type: 'alert',
      props: { type: 'info', text: notice }
    };
  }

  /// 列表统一走表格节点；`options` 目前只承载 `height`（定高滚动）与
  /// `emptyText`（按场景的空态文案）。新增字段必须在这里显式落到 props，
  /// 否则调用方传了也会被丢弃。
  function statusTable(columns, items, rowKey, options) {
    options = options || {};
    var props = {
      items: items,
      columns: columns,
      rowKey: rowKey,
      pagination: false,
      // 列表行由宿主一次性给出（本页 / 已累计），不能再被组件默认每页 10 条截断；
      // -1 表示「全部渲染」，游标分页的下一页游标才会与本页实际展示的最后一行对齐。
      perPage: -1,
      density: 'compact',
      // 空态文案属于场景语义（Docker 不可用 / 仅异常 / 首次使用），
      // 由调用方下发；未提供时才回落到通用文案。
      emptyText: options.emptyText || '暂无数据'
    };
    if (options.height) {
      // 定高 + 内部滚动：避免长列表把整页撑开，同时保证本页所有行都可达。
      props.virtual = true;
      props.height = options.height;
    }
    return {
      type: 'table',
      props: props
    };
  }

  function listTable(columns, path, rowKey, params, options) {
    var table = statusTable(columns, [], rowKey, options);
    table.source = { url: apiBase + path, params: params || {} };
    table.props.paging = 'cursor';
    table.props.cursorParam = 'cursor';
    table.props.itemsPath = 'data.items';
    table.props.totalPath = 'data.total';
    table.props.cursorPath = 'data.next_cursor';
    table.props.loadMoreText = '加载更多';
    return table;
  }

  // 站点列表：列取接口字段，行点击进入 budding 详情。
  function sitesSchema(data) {
    var body = [];
    var table = listTable(
      [
        { key: 'domain', label: '域名' },
        { key: 'type_label', label: '类型' },
        { key: 'endpoint', label: '接入' },
        { key: 'status', label: '状态', text: '${status.label}' },
        { key: 'https', label: 'HTTPS', text: '${https.label}' },
        { key: 'runtime_name', label: '运行环境' },
        { key: 'created_at', label: '创建时间', width: 180 }
      ],
      '/sites',
      'domain',
      null,
      {
        emptyText: '还没有站点，创建后会生成整站目录、nginx vhost 与 PHP 探针'
      }
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
    if (data.can_admin) {
      body.push({
        type: 'flex',
        props: { gap: 8, wrap: true },
        content: {
          body: [
            {
              type: 'button',
              props: { text: '新建站点', color: 'primary' },
              onEvent: {
                click: {
                  actions: [
                    { type: 'navigate', props: { url: prefix + '/sites/new' } }
                  ]
                }
              }
            }
          ]
        }
      });
      body.push(
        writeCard(
          '批量启停',
          '每行一个域名；批量删除仍走站点详情的删除确认。',
          writeForm(
            apiBase + '/sites/batch',
            { type: 'skiffSiteWrite', props: { op: 'batch' } },
            [
              {
                type: 'select',
                props: {
                  name: 'action',
                  label: '操作',
                  defaultValue: 'start',
                  items: [
                    { title: '启动', value: 'start' },
                    { title: '停止', value: 'stop' }
                  ]
                }
              },
              {
                type: 'textarea',
                props: {
                  name: 'domains',
                  label: '域名（每行一个）',
                  rows: 3,
                  required: true,
                  rules: { required: true, messages: { required: '请填写域名' } }
                }
              }
            ],
            '执行批量操作'
          )
        )
      );
    }
    return {
      type: 'page',
      props: {
        title: '网站 · Skiff',
        subtitle: data.can_admin ? '建站、启停、配置与删除均在原生页面完成' : '只读列表；写操作需要管理员权限'
      },
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

    if (data.can_manage) {
      var actions = [];
      if (data.status.code === 'running') {
        actions.push(siteBtn('停止站点', { op: 'power', action: 'stop', domain: data.domain }, 'warning'));
      } else {
        actions.push(siteBtn('启动站点', { op: 'power', action: 'start', domain: data.domain }, 'primary'));
      }
      actions.push(
        siteBtn(
          data.is_default ? '取消默认站点' : '设为默认站点',
          { op: 'default', enabled: !data.is_default, domain: data.domain },
          'secondary'
        )
      );
      actions.push(
        siteBtn('删除站点', { op: 'delete', domain: data.domain }, 'danger', 'text')
      );
      body.push({
        type: 'card',
        props: { title: '站点操作' },
        content: { body: [{ type: 'flex', props: { gap: 8, wrap: true }, content: { body: actions } }] }
      });
    } else {
      body.push({
        type: 'alert',
        props: { type: 'info', text: '写操作需要管理员权限；当前为只读视图。' }
      });
    }

    body.push({
      type: 'flex',
      props: { gap: 12, wrap: true },
      content: {
        body: [
          {
            type: 'link',
            props: {
              text: '查看访问 / 错误日志',
              href: prefix + '/sites/' + encodeURIComponent(data.domain) + '/logs'
            }
          }
        ]
      }
    });

    // 配置：错误时只展示提示；正常时给可编辑 textarea，保存仍走 SITE_SAVE_CONFIG。
    if (data.config.error) {
      body.push({
        type: 'alert',
        props: { type: 'warning', title: '配置读取失败', text: data.config.error }
      });
    } else if (data.can_manage) {
      body.push(
        writeCard(
          'vhost 配置',
          (data.config.path || '—') + (data.config.active ? ' · 已启用' : ' · 未启用'),
          writeForm(
            apiBase + '/sites/' + encodeURIComponent(data.domain) + '/config',
            { type: 'skiffSiteWrite', props: { op: 'config', domain: data.domain } },
            [
              {
                type: 'textarea',
                props: {
                  name: 'content',
                  label: '配置内容',
                  rows: 16,
                  defaultValue: data.config.content,
                  required: true,
                  rules: { required: true, messages: { required: '配置内容不能为空' } }
                }
              }
            ],
            '保存配置'
          )
        )
      );
    } else {
      body.push(
        writeCard(
          'vhost 配置',
          (data.config.path || '—') + '（只读）',
          { type: 'code', props: { modelValue: data.config.content, language: 'nginx' } }
        )
      );
    }

    // 域名：列表逐项删除 + 新增表单。
    var domainBody = [];
    if (!data.aliases || data.aliases.length === 0) {
      domainBody.push(textLine('暂无附加域名。'));
    } else {
      data.aliases.forEach(function (alias) {
        var line = [textLine(alias)];
        if (data.can_manage) {
          line.push(
            siteBtn('删除', { op: 'domains', action: 'remove', alias: alias, domain: data.domain }, 'danger', 'text')
          );
        }
        domainBody.push({
          type: 'flex',
          props: { gap: 8 },
          content: { body: line }
        });
      });
    }
    if (data.can_manage) {
      domainBody.push(
        writeForm(
          apiBase + '/sites/' + encodeURIComponent(data.domain) + '/domains',
          { type: 'skiffSiteWrite', props: { op: 'domains', action: 'add', domain: data.domain } },
          [
            {
              type: 'input-text',
              props: {
                name: 'alias',
                label: '新增附加域名',
                required: true,
                rules: { required: true, messages: { required: '请输入域名' } }
              }
            }
          ],
          '添加'
        )
      );
    }
    body.push({ type: 'card', props: { title: '附加域名' }, content: { body: domainBody } });

    // 证书：生成/强制/关闭/删除 + PEM 上传。
    var sslBody = [
      textLine(data.ssl.cert_exists ? '已检测到证书文件' : '未检测到证书文件'),
      textLine('证书路径：' + (data.ssl.cert_path || '—')),
      textLine('主体：' + (data.ssl.subject || '—')),
      textLine('签发者：' + (data.ssl.issuer || '—')),
      textLine('到期：' + (data.ssl.expires || '—'))
    ];
    if (data.can_manage) {
      sslBody.push({
        type: 'flex',
        props: { gap: 8, wrap: true },
        content: {
          body: [
            siteBtn('生成自签证书', { op: 'ssl', action: 'generate', domain: data.domain }, 'primary'),
            siteBtn(
              data.ssl.force_https ? '关闭强制 HTTPS' : '开启强制 HTTPS',
              { op: 'ssl', action: 'force', enabled: !data.ssl.force_https, domain: data.domain },
              'secondary'
            ),
            siteBtn('关闭 HTTPS', { op: 'ssl', action: 'disable', domain: data.domain }, 'warning'),
            siteBtn(
              '删除证书',
              { op: 'ssl', action: 'delete_cert', domain: data.domain },
              'danger',
              'text'
            )
          ]
        }
      });
      sslBody.push(
        writeForm(
          apiBase + '/sites/' + encodeURIComponent(data.domain) + '/ssl',
          { type: 'skiffSiteWrite', props: { op: 'ssl', action: 'upload', domain: data.domain } },
          [
            { type: 'textarea', props: { name: 'cert_pem', label: '证书 PEM', rows: 6 } },
            { type: 'textarea', props: { name: 'key_pem', label: '私钥 PEM', rows: 6 } }
          ],
          '上传证书'
        )
      );
    }
    body.push({ type: 'card', props: { title: '证书 / HTTPS' }, content: { body: sslBody } });

    // 备份开关。
    var backupBody = [];
    if (!data.backup.available) {
      backupBody.push({
        type: 'alert',
        props: { type: 'warning', text: '内核任务服务未初始化，站点备份暂不可用。' }
      });
    } else {
      backupBody.push(textLine(data.backup.enabled ? '备份已开启（每日 03:00）' : '备份未开启'));
      backupBody.push(textLine('上次：' + (data.backup.last_text || '从未执行')));
      backupBody.push(textLine('下次：' + (data.backup.next_text || '—')));
      if (data.can_manage) {
        backupBody.push(
          siteBtn(
            data.backup.enabled ? '关闭备份' : '开启备份',
            { op: 'backup', enabled: !data.backup.enabled, domain: data.domain },
            data.backup.enabled ? 'warning' : 'primary'
          )
        );
      }
    }
    body.push({ type: 'card', props: { title: '站点备份' }, content: { body: backupBody } });

    return {
      type: 'page',
      props: {
        title: data.domain + ' · 网站',
        subtitle: data.can_manage ? '写操作复用既有权限、CSRF 与审计' : '只读概览'
      },
      body: body
    };
  }

  function siteBtn(text, props, color, variant) {
    return {
      type: 'button',
      props: { text: text, color: color, variant: variant || 'tonal' },
      onEvent: {
        click: {
          actions: [{ type: 'skiffSiteWrite', props: props }]
        }
      }
    };
  }

  /// 新建站点（原生化）：类型/域名/目录/运行环境/反代/别名与旧表单同名。
  function siteNewSchema(runtimes) {
    var typeQuery = '';
    try {
      typeQuery = new URLSearchParams(location.search).get('type') || '';
    } catch (e) {
      typeQuery = '';
    }
    var items = (runtimes || []).map(function (r) {
      return {
        title:
          r.kind.label + ' ' + (r.version || '') + ' · ' + r.name +
          (r.state && r.state.code !== 'running' ? '（' + r.state.label + '）' : ''),
        value: r.id
      };
    });
    return {
      type: 'page',
      props: { title: '新建站点 · Skiff', subtitle: '提交后进入站点详情页' },
      body: [
        {
          type: 'alert',
          props: {
            type: 'info',
            text: '运行环境站点需选择已运行的实例；静态站点只需目录；反向代理需填目标地址。别名每行一个。'
          }
        },
        writeForm(
          apiBase + '/sites',
          { type: 'skiffSiteWrite', props: { op: 'create' } },
          [
            {
              type: 'select',
              props: {
                name: 'site_type',
                label: '站点类型',
                defaultValue: typeQuery === 'runtime' || typeQuery === 'proxy' ? typeQuery : 'static',
                items: [
                  { title: '运行环境（PHP / Node）', value: 'runtime' },
                  { title: '静态站点', value: 'static' },
                  { title: '反向代理', value: 'proxy' }
                ]
              }
            },
            {
              type: 'input-text',
              props: {
                name: 'domain',
                label: '域名（必填）',
                required: true,
                rules: { required: true, messages: { required: '请输入域名' } }
              }
            },
            {
              type: 'input-text',
              props: { name: 'root_dir', label: '站点目录（可留空，由系统生成）' }
            },
            {
              type: 'select',
              props: {
                name: 'runtime_id',
                label: '运行环境（runtime 类型必选）',
                items: items
              }
            },
            {
              type: 'input-text',
              props: { name: 'proxy_target', label: '反代目标（proxy 类型必填，如 http://127.0.0.1:3000）' }
            },
            {
              type: 'checkbox',
              props: { name: 'proxy_websocket', label: '反代启用 WebSocket' }
            },
            {
              type: 'textarea',
              props: { name: 'aliases', label: '附加域名（每行一个）', rows: 3 }
            }
          ],
          '创建站点'
        )
      ]
    };
  }

  function runtimesSchema(images, catalog) {
    var body = [];
    var table = listTable(
      [
        { key: 'kind', label: '类型', text: '${kind.label}' },
        { key: 'name', label: '实例' },
        { key: 'version_display', label: '版本' },
        { key: 'version_note', label: '说明' },
        { key: 'state', label: '状态', text: '${state.label}' },
        { key: 'site_count', label: '站点数', type: 'number' },
        { key: 'endpoint_short', label: '接入' }
      ],
      '/runtimes',
      'id',
      null,
      {
        emptyText: '还没有运行环境，点击下方「新建运行环境」安装后即可绑定站点'
      }
    );
    table.onEvent = {
      rowClick: {
        actions: [{ type: 'navigate', props: { url: prefix + '/runtimes/${id}' } }]
      }
    };
    body.push(table);
    var kinds = ((catalog && catalog.kinds) || []).map(function (k) {
      return { title: k.label, value: k.key };
    });
    body.push(
      writeCard(
        '安装运行环境',
        'PHP 会按所选版本拉取镜像并安装默认扩展；其余类型按目录能力登记或安装。安装进度在列表状态列与详情日志中实时显示。',
        writeForm(
          apiBase + '/runtimes',
          { type: 'skiffRuntimeCreate' },
          [
            {
              type: 'select',
              props: { name: 'kind', label: '类型', items: kinds }
            },
            {
              type: 'input-text',
              props: {
                name: 'version',
                label: '版本',
                hint: '留空使用该类型默认版本；PHP 默认 ' +
                  ((catalog && catalog.default_php_version) || '8.5')
              }
            },
            {
              type: 'input-text',
              props: { name: 'name', label: '实例名（可选，默认按类型自动生成）' }
            }
          ],
          '创建并安装'
        )
      )
    );
    var imagesForm = writeForm(
      apiBase + '/runtimes/images',
      { type: 'skiffRuntimeImages' },
      [
        {
          type: 'input-text',
          props: {
            name: 'nginx_image',
            label: 'Web 服务镜像标签',
            hint: 'OpenResty 基础镜像；留空使用内置默认值'
          }
        },
        {
          type: 'input-text',
          props: {
            name: 'php_image',
            label: 'PHP 基础镜像标签',
            hint: '仅影响之后新建或重建的 PHP 实例'
          }
        }
      ],
      '保存镜像参数'
    );
    imagesForm.props.initialValues = {
      nginx_image: (images && images.nginx_image) || '',
      php_image: (images && images.php_image) || ''
    };
    body.push(writeCard('镜像参数', '镜像前缀与加速源在 Docker 设置中维护。', imagesForm));
    return {
      type: 'page',
      props: { title: '运行环境 · Skiff', subtitle: '安装、配置与扩展走同一业务入口' },
      body: body
    };
  }

  /// 运行环境按钮：写操作统一走 `skiffRuntime*` 动作。
  function runtimeBtn(text, actionType, props) {
    return {
      type: 'button',
      props: { text: text, variant: 'text', color: 'primary' },
      onEvent: { click: { actions: [{ type: actionType, props: props }] } }
    };
  }

  function runtimeActionBtn(text, op, id, confirmText, color) {
    return {
      type: 'button',
      props: { text: text, variant: 'text', color: color || 'primary' },
      onEvent: {
        click: {
          actions: [
            {
              type: 'skiffRuntimeAction',
              props: { id: id, op: op, confirm_text: confirmText || '' }
            }
          ]
        }
      }
    };
  }

  function runtimeDetailSchema(data, runtimeId) {
    var rt = data.runtime || {};
    var body = [
      {
        type: 'card',
        props: { title: (rt.kind && rt.kind.label) + ' / ' + rt.name },
        content: {
          body: [
            detailLines('实例', [
              '版本：' + (rt.version_display || rt.version) + '（' + (rt.image || '') + '）',
              '状态：' + ((rt.state && rt.state.label) || '') + '；容器：' + (rt.container_state || '—'),
              '接入：' + (rt.endpoint_full || rt.endpoint_short || '—'),
              '站点数：' + (rt.site_count || 0)
            ]),
            {
              type: 'flex',
              props: { gap: 8, wrap: true },
              content: {
                body: [
                  runtimeActionBtn('启动', 'start', runtimeId),
                  runtimeActionBtn('停止', 'stop', runtimeId, rt.site_count ? '该环境被站点使用，停止会影响全部绑定站点。确认停止？' : ''),
                  runtimeActionBtn('重启', 'restart', runtimeId, rt.site_count ? '该环境被站点使用，重启会短暂中断全部绑定站点。确认重启？' : ''),
                  runtimeActionBtn('重建', 'rebuild', runtimeId, '重建会按当前配置重新创建容器，期间服务中断。确认重建？'),
                  runtimeActionBtn('同步', 'sync', runtimeId),
                  runtimeActionBtn('删除', 'delete', runtimeId, '删除运行环境不可恢复，绑定站点会受影响。确认删除？', 'danger')
                ]
              }
            },
            {
              type: 'flex',
              props: { gap: 8, wrap: true },
              content: {
                body: [
                  runtimeBtn('实测版本', 'skiffRuntimeProbe', { id: runtimeId }),
                  {
                    type: 'link',
                    props: {
                      text: '查看运行日志',
                      href: prefix + '/runtimes/' + encodeURIComponent(runtimeId) + '/logs'
                    }
                  }
                ]
              }
            }
          ]
        }
      }
    ];

    if (data.config_mounts_stale) {
      body.push({
        type: 'alert',
        props: {
          type: 'warning',
          title: '配置目录未挂载',
          text: '当前容器未挂载 php.ini / conf.d，配置能保存但不生效；请先执行「重建」再编辑配置。'
        }
      });
    }

    if (data.is_php) {
      var cfg = data.config || {};
      var configForm = writeForm(
        apiBase + '/runtimes/' + encodeURIComponent(runtimeId) + '/config',
        { type: 'skiffRuntimeConfig', props: { id: runtimeId } },
        [
          { type: 'input-text', props: { name: 'memory_limit', label: 'memory_limit' } },
          { type: 'input-text', props: { name: 'upload_max_filesize', label: 'upload_max_filesize' } },
          { type: 'input-text', props: { name: 'post_max_size', label: 'post_max_size' } },
          { type: 'input-text', props: { name: 'max_execution_time', label: 'max_execution_time（秒）' } },
          { type: 'input-text', props: { name: 'pm', label: '进程管理 pm（dynamic/static/ondemand）' } },
          { type: 'input-text', props: { name: 'max_children', label: 'max_children' } },
          { type: 'input-text', props: { name: 'start_servers', label: 'start_servers' } },
          { type: 'input-text', props: { name: 'min_spare_servers', label: 'min_spare_servers' } },
          { type: 'input-text', props: { name: 'max_spare_servers', label: 'max_spare_servers' } },
          { type: 'input-text', props: { name: 'max_requests', label: 'max_requests' } }
        ],
        '保存并重载 FPM'
      );
      configForm.props.initialValues = {
        memory_limit: cfg.memory_limit || '',
        upload_max_filesize: cfg.upload_max_filesize || '',
        post_max_size: cfg.post_max_size || '',
        max_execution_time: String(cfg.max_execution_time || ''),
        pm: cfg.pm || '',
        max_children: String(cfg.max_children || ''),
        start_servers: String(cfg.start_servers || ''),
        min_spare_servers: String(cfg.min_spare_servers || ''),
        max_spare_servers: String(cfg.max_spare_servers || ''),
        max_requests: String(cfg.max_requests || '')
      };
      body.push(writeCard('PHP 运行参数', '保存后自动重载 FPM；非法值会被拒绝并保留原配置。', configForm));

      var fileRows = (data.files || []).map(function (f) {
        return {
          key: f.key,
          label: f.label,
          state: f.exists ? '已就绪' : '未创建'
        };
      });
      var fileTable = statusTable(
        [
          { key: 'label', label: '配置文件' },
          { key: 'state', label: '状态' }
        ],
        fileRows,
        'key',
        { emptyText: '没有可编辑的配置文件' }
      );
      fileTable.props = Object.assign({}, fileTable.props, {
        rowActions: [
          {
            type: 'button',
            props: { text: '编辑', variant: 'text', color: 'primary' },
            onEvent: {
              click: {
                actions: [
                  {
                    type: 'navigate',
                    props: {
                      url:
                        prefix +
                        '/runtimes/' +
                        encodeURIComponent(runtimeId) +
                        '?file=${key}'
                    }
                  }
                ]
              }
            }
          }
        ]
      });
      body.push({
        type: 'card',
        props: { title: '配置文件' },
        content: { body: [fileTable] }
      });
      if (data.editor) {
        var editorForm = writeForm(
          apiBase +
            '/runtimes/' +
            encodeURIComponent(runtimeId) +
            '/config-file',
          { type: 'skiffRuntimeFile', props: { id: runtimeId, key: data.editor.key } },
          [
            {
              type: 'textarea',
              props: {
                name: 'content',
                label: data.editor.label + '（' + data.editor.container_path + '）',
                rows: 18
              }
            }
          ],
          '保存配置文件'
        );
        editorForm.props.initialValues = { content: data.editor.content || '' };
        body.push(writeCard('编辑：' + data.editor.label, data.editor.note || '保存前会在容器内做语法校验。', editorForm));
      }

      var extRows = (data.extensions || []).map(function (e) {
        return {
          key: e.key,
          label: e.label,
          category: e.category,
          source: e.source,
          state: e.enabled ? '已启用' : '未启用',
          note: e.support_note || e.description
        };
      });
      var extTable = statusTable(
        [
          { key: 'label', label: '扩展' },
          { key: 'category', label: '分类' },
          { key: 'source', label: '来源' },
          { key: 'state', label: '状态' },
          { key: 'note', label: '说明' }
        ],
        extRows,
        'key',
        { emptyText: '还没有可用扩展' }
      );
      extTable.props = Object.assign({}, extTable.props, {
        rowActions: [
          {
            type: 'button',
            props: { text: '安装', variant: 'text', color: 'primary' },
            onEvent: {
              click: {
                actions: [
                  {
                    type: 'skiffRuntimeExt',
                    props: { id: runtimeId, key: '${key}', op: 'install' }
                  }
                ]
              }
            }
          },
          {
            type: 'button',
            props: { text: '卸载', variant: 'text', color: 'danger' },
            onEvent: {
              click: {
                actions: [
                  {
                    type: 'skiffRuntimeExt',
                    props: { id: runtimeId, key: '${key}', op: 'uninstall', confirm_text: '卸载扩展需要重启容器，绑定站点会短暂中断。确认卸载 ${key}？' }
                  }
                ]
              }
            }
          }
        ]
      });
      body.push({
        type: 'card',
        props: { title: 'PHP 扩展' },
        content: { body: [extTable] }
      });
      body.push(
        writeCard(
          '按名字安装 PECL 扩展',
          '用于目录里没有的扩展；安装任务会编译并重启一次容器。',
          writeForm(
            apiBase + '/runtimes/' + encodeURIComponent(runtimeId) + '/pecl',
            { type: 'skiffRuntimePecl', props: { id: runtimeId } },
            [
              { type: 'input-text', props: { name: 'name', label: 'PECL 扩展名' } },
              { type: 'input-text', props: { name: 'version', label: '版本（可选）' } }
            ],
            '安装 PECL 扩展'
          )
        )
      );
    }

    if (data.is_node) {
      var node = data.node || {};
      var port = data.port || {};
      var nodeForm = writeForm(
        apiBase + '/runtimes/' + encodeURIComponent(runtimeId) + '/node-config',
        { type: 'skiffRuntimeNode', props: { id: runtimeId } },
        [
          { type: 'input-text', props: { name: 'command', label: '启动命令', hint: '如 npm start；留空使用镜像默认' } },
          { type: 'input-text', props: { name: 'health_path', label: '健康检查路径（如 /healthz）' } },
          { type: 'input-text', props: { name: 'node_env', label: 'NODE_ENV' } },
          { type: 'textarea', props: { name: 'env', label: '额外环境变量（每行 KEY=VALUE）', rows: 4 } }
        ],
        '保存 Node.js 配置'
      );
      nodeForm.props.initialValues = {
        command: node.command || '',
        health_path: node.health_path || '',
        node_env: node.node_env || '',
        env: node.env || ''
      };
      body.push(writeCard('Node.js 配置', '配置变化会触发一次重建；未变化不重启。', nodeForm));
      if (port.editable) {
        var portForm = writeForm(
          apiBase + '/runtimes/' + encodeURIComponent(runtimeId) + '/port',
          { type: 'skiffRuntimePort', props: { id: runtimeId } },
          [
            {
              type: 'input-text',
              props: {
                name: 'port',
                label: '固定端口' + (port.range ? '（' + port.range + '）' : '')
              }
            }
          ],
          '保存端口'
        );
        portForm.props.initialValues = { port: String(port.value || '') };
        body.push(
          writeCard(
            '端口',
            (rt.site_count ? '当前有 ' + rt.site_count + ' 个站点绑定，改端口会重写全部站点反代并重建容器。' : '端口由内核分配，修改后会重建容器。'),
            portForm
          )
        );
      }
    }

    var siteRows = (data.sites || []).map(function (s) {
      return { domain: s.domain, site_type: s.site_type, status: s.status };
    });
    if (siteRows.length) {
      var siteTable = statusTable(
        [
          { key: 'domain', label: '站点' },
          { key: 'site_type', label: '类型' },
          { key: 'status', label: '状态' }
        ],
        siteRows,
        'domain'
      );
      siteTable.props = Object.assign({}, siteTable.props, {
        rowActions: [
          {
            type: 'button',
            props: { text: '解绑', variant: 'text', color: 'danger' },
            onEvent: {
              click: {
                actions: [
                  {
                    type: 'skiffRuntimeUnbind',
                    props: { id: runtimeId, domain: '${domain}' }
                  }
                ]
              }
            }
          }
        ]
      });
      body.push({
        type: 'card',
        props: { title: '绑定站点' },
        content: { body: [siteTable] }
      });
    }

    body.push(
      writeCard(
        '克隆',
        '复制当前配置与新实例；端口由内核重新分配，名称可留空（默认追加 -copy）。',
        writeForm(
          apiBase + '/runtimes/' + encodeURIComponent(runtimeId) + '/clone',
          { type: 'skiffRuntimeClone', props: { id: runtimeId } },
          [{ type: 'input-text', props: { name: 'name', label: '新实例名（可留空）' } }],
          '克隆并安装'
        )
      )
    );
    body.push(noticeCard('安装、扩展与配置文件编辑都在后台任务中执行；失败原因会写审计并显示在日志页。'));
    return {
      type: 'page',
      props: {
        title: '运行环境详情 · Skiff',
        subtitle: (rt.kind && rt.kind.label) + ' / ' + rt.name
      },
      body: body
    };
  }

  function containersSchema(data, filter) {
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
    // budding 的 button 只渲染 <button type="button">，不消费 `to`（没有 href、
    // 也不会触发路由）。「全部 / 仅异常」是同 path 只换 query 的页内跳转，
    // 必须走 `skiffQueryNav`：SDK 的页面加载键是 `path#name`，query 变化不会
    // 触发重载，需要动作里显式 `refreshPage`。
    var allButton = {
      type: 'button',
      props: {
        text: '全部',
        size: 'small',
        variant: filter === 'error' ? 'text' : 'tonal'
      },
      onEvent: {
        click: {
          actions: [
            {
              type: 'skiffQueryNav',
              props: { path: prefix + '/containers', query: {} }
            }
          ]
        }
      }
    };
    if (filter !== 'error') allButton.props.color = 'primary';
    var errorButton = {
      type: 'button',
      props: {
        text: '仅异常',
        size: 'small',
        variant: filter === 'error' ? 'tonal' : 'text'
      },
      onEvent: {
        click: {
          actions: [
            {
              type: 'skiffQueryNav',
              props: { path: prefix + '/containers', query: { filter: 'error' } }
            }
          ]
        }
      }
    };
    if (filter === 'error') errorButton.props.color = 'primary';
    body.push({
      type: 'flex',
      props: { gap: 8 },
      content: {
        body: [
          allButton,
          errorButton,
          {
            type: 'text',
            props: {
              text:
                (filter === 'error' ? '异常容器 ' : '共 ') +
                data.total +
                ' 个' +
                (data.docker_available ? '' : '（Docker 不可用）')
            }
          }
        ]
      }
    });
    var table = listTable(
      [
        { key: 'name', label: '名称', width: 200 },
        { key: 'short_id', label: 'ID', width: 110 },
        { key: 'image', label: '镜像', width: 260 },
        { key: 'state', label: '状态', text: '${state.label}', width: 100, align: 'center' },
        { key: 'status', label: '状态描述', width: 220 },
        { key: 'project', label: 'Compose 项目', width: 160 }
      ],
      '/containers',
      'id',
      filter === 'error' ? { filter: 'error' } : null,
      {
        emptyText: !data.docker_available
          ? 'Docker 不可用，容器列表为空'
          : filter === 'error'
            ? '没有异常容器'
            : '还没有容器，可在下方新建'
      }
    );
    table.props.actionsLabel = '操作';
    table.props.rowActions = [
      {
        type: 'button',
        props: { text: '详情', variant: 'text', color: 'primary', size: 'small' },
        onEvent: {
          click: {
            actions: [
              {
                type: 'navigate',
                props: { url: prefix + '/containers/${id}' }
              }
            ]
          }
        }
      }
    ];
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
    if (data.can_admin) {
      body.push({
        type: 'flex',
        props: { gap: 8 },
        content: {
          body: [
            {
              type: 'button',
              props: { text: '新建容器', color: 'primary' },
              onEvent: {
                click: {
                  actions: [
                    {
                      type: 'navigate',
                      props: { url: prefix + '/containers/new' }
                    }
                  ]
                }
              }
            }
          ]
        }
      });
    }
    return {
      type: 'page',
      props: {
        title: '容器 · Skiff',
        subtitle: '启停/重启/删除在详情页操作，资源限制可在详情页编辑'
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
    // 日志只需 DockerRead，与详情读取同权限；终端才需要 DockerExec。
    actions.push({
      type: 'link',
      props: {
        text: '查看日志',
        href: prefix + '/containers/' + encodeURIComponent(c.id) + '/logs'
      }
    });
    // 终端权限与执行票据同源（DockerExec），不与启停的 DockerOperate 混用。
    if (data.can_exec) {
      actions.push({
        type: 'link',
        props: {
          text: '打开终端',
          href: prefix + '/containers/' + encodeURIComponent(c.id) + '/terminal'
        }
      });
    }
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
    if (data.can_delete) {
      body.push(
        writeCard(
          '编辑资源限制',
          '留空表示清除该限制（Engine 默认）。当前生效值见上方「资源限制」。',
          limitsForm(c.id, data.limits)
        )
      );
    } else {
      body.push({
        type: 'alert',
        props: { type: 'info', text: '资源限制编辑需要 Docker 生命周期权限。' }
      });
    }
    return {
      type: 'page',
      props: { title: c.name + ' · 容器', subtitle: '操作与资源限制均走 /ui/v1' },
      body: body
    };
  }

  /// 卡片 + 说明 + 表单的统一外壳（写操作页共用）。
  function writeCard(title, notice, form) {
    var content = [];
    if (notice) content.push(textLine(notice));
    content.push(form);
    return { type: 'card', props: { title: title }, content: { body: content } };
  }

  function limitsForm(containerId, limits) {
    var current = limits || {};
    function field(name, label) {
      var value = current[name] || '';
      return {
        type: 'input-text',
        props: {
          name: name,
          label: label,
          placeholder: value ? '当前 ' + value : '当前未设置'
        }
      };
    }
    return writeForm(
      apiBase + '/containers/' + encodeURIComponent(containerId) + '/limits',
      { type: 'skiffDockerWrite', props: { kind: 'container', op: 'limits', id: containerId } },
      [
        field('cpu_shares', 'CPU 份额'),
        field('cpu_period', 'CPU 周期（μs）'),
        field('cpu_quota', 'CPU 配额（μs）'),
        field('memory_limit_mb', '内存上限（MB）'),
        field('memory_swap_mb', '内存+交换（MB）'),
        field('pids_limit', 'PIDs 上限')
      ],
      '保存限制'
    );
  }

  /// 新建容器（原生化）：字段与旧 SSR 表单同名，解析与校验仍在后端复用。
  function containerNewSchema() {
    function area(name, label) {
      return { type: 'textarea', props: { name: name, label: label, rows: 3 } };
    }
    return {
      type: 'page',
      props: { title: '新建容器 · Skiff', subtitle: '提交后进入容器详情页' },
      body: [
        {
          type: 'alert',
          props: {
            type: 'info',
            text: '端口、环境变量、绑定与卷均按每行一条；网络每行一个名称。留空表示不设置。'
          }
        },
        writeForm(
          apiBase + '/containers',
          { type: 'skiffDockerWrite', props: { kind: 'container', op: 'create' } },
          [
            {
              type: 'input-text',
              props: {
                name: 'image',
                label: '镜像（必填）',
                required: true,
                rules: { required: true, messages: { required: '请输入镜像' } }
              }
            },
            { type: 'input-text', props: { name: 'name', label: '容器名（可选）' } },
            area('ports', '端口映射（host:container[/udp]，每行一条）'),
            area('env', '环境变量（KEY=VALUE，每行一条）'),
            area('binds', '宿主绑定（源:目标[:ro]，每行一条）'),
            area('volumes', '命名卷（卷名:目标[:ro]，每行一条）'),
            {
              type: 'input-text',
              props: {
                name: 'restart',
                label: '重启策略（如 unless-stopped）'
              }
            },
            area('networks', '网络（每行一个，可留空）'),
            { type: 'input-text', props: { name: 'cpu_shares', label: 'CPU 份额' } },
            { type: 'input-text', props: { name: 'cpu_period', label: 'CPU 周期（μs）' } },
            { type: 'input-text', props: { name: 'cpu_quota', label: 'CPU 配额（μs）' } },
            { type: 'input-text', props: { name: 'memory_limit_mb', label: '内存上限（MB）' } },
            {
              type: 'input-text',
              props: { name: 'memory_swap_mb', label: '内存+交换（MB）' }
            },
            { type: 'input-text', props: { name: 'pids_limit', label: 'PIDs 上限' } }
          ],
          '创建容器'
        )
      ]
    };
  }

  /// 资源页（镜像 / 网络 / 卷）：游标列表 + 可选行内删除，Docker 不可用时置顶告警。
  function resourceSchema(title, subtitle, columns, data, path, options) {
    options = options || {};
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
    var table = listTable(columns, path, 'id', null, {
      emptyText: options.emptyText
    });
    if (options.rowActions) table.props.rowActions = options.rowActions;
    body.push(table);
    if (options.extra && options.extra.length > 0) body = body.concat(options.extra);
    return { type: 'page', props: { title: title, subtitle: subtitle }, body: body };
  }

  /// 行内删除：镜像 / 网络 / 卷共用一个 `skiffDockerWrite`（确认与刷新由动作统一处理）。
  /// `visible` 为可选条件表达式（如网络的 `${can_remove}`），缺省每行都显示。
  function removeRowAction(kind, visible) {
    var node = {
      type: 'button',
      props: { text: '删除', variant: 'text', color: 'danger' },
      onEvent: {
        click: {
          actions: [
            {
              type: 'skiffDockerWrite',
              props: {
                kind: kind,
                op: 'remove',
                id: '${id}',
                name: '${name}'
              }
            }
          ]
        }
      }
    };
    if (visible) node.visibleOn = visible;
    return node;
  }

  function imagesSchema(data) {
    var extra = [];
    if (data.can_admin) {
      extra.push(
        writeCard(
          '拉取镜像',
          '镜像名支持 tag 与仓库前缀，例如 nginx:1.27。',
          writeForm(
            apiBase + '/images/pull',
            { type: 'skiffDockerWrite', props: { kind: 'image', op: 'pull' } },
            [
              {
                type: 'input-text',
                props: {
                  name: 'image',
                  label: '镜像名',
                  required: true,
                  rules: { required: true, messages: { required: '请输入镜像名' } }
                }
              }
            ],
            '拉取'
          )
        )
      );
    }
    return resourceSchema(
      '镜像 · Skiff',
      data.can_admin ? '拉取与删除走 /ui/v1' : '只读列表；写操作需要管理员权限',
      [
        { key: 'name', label: '镜像' },
        { key: 'meta', label: '大小' },
        { key: 'warn', label: '提示' }
      ],
      data,
      '/images',
      {
        rowActions: data.can_admin ? [removeRowAction('image')] : null,
        extra: extra,
        emptyText: data.docker_available
          ? '还没有拉取镜像，拉取后新建容器或运行环境可直接使用'
          : 'Docker 不可用，镜像列表为空'
      }
    );
  }

  function networksSchema(data) {
    var extra = [];
    if (data.can_admin) {
      extra.push(
        writeCard(
          '创建网络',
          '名称只允许字母数字与 _-.',
          writeForm(
            apiBase + '/networks',
            { type: 'skiffDockerWrite', props: { kind: 'network', op: 'create' } },
            [nameField('网络名')],
            '创建'
          )
        )
      );
    }
    return resourceSchema(
      '网络 · Skiff',
      data.can_admin ? '创建与删除走 /ui/v1' : '只读列表；写操作需要管理员权限',
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
      '/networks',
      {
        rowActions: data.can_admin ? [removeRowAction('network', '${can_remove}')] : null,
        extra: extra,
        emptyText: data.docker_available
          ? '还没有自定义网络，需要隔离或固定子网时创建'
          : 'Docker 不可用，网络列表为空'
      }
    );
  }

  function volumesSchema(data) {
    var extra = [];
    if (data.can_admin) {
      extra.push(
        writeCard(
          '创建卷',
          '名称只允许字母数字与 _-.',
          writeForm(
            apiBase + '/volumes',
            { type: 'skiffDockerWrite', props: { kind: 'volume', op: 'create' } },
            [nameField('卷名')],
            '创建'
          )
        )
      );
    }
    return resourceSchema(
      '卷 · Skiff',
      data.can_admin ? '创建与删除走 /ui/v1' : '只读列表；写操作需要管理员权限',
      [
        { key: 'name', label: '名称' },
        { key: 'meta', label: '驱动与挂载点' },
        { key: 'warn', label: '提示' }
      ],
      data,
      '/volumes',
      {
        rowActions: data.can_admin ? [removeRowAction('volume')] : null,
        extra: extra,
        emptyText: data.docker_available
          ? '还没有命名卷，创建后可在新建容器时挂载'
          : 'Docker 不可用，卷列表为空'
      }
    );
  }

  function nameField(label) {
    return {
      type: 'input-text',
      props: {
        name: 'name',
        label: label,
        required: true,
        rules: { required: true, messages: { required: '请输入名称' } }
      }
    };
  }

  function writeForm(url, submitAction, fields, submitText) {
    return {
      type: 'form',
      props: {
        api: { method: 'POST', url: url, sendOn: false },
        actionsAlign: 'end'
      },
      onEvent: { submit: [submitAction] },
      body: fields,
      content: {
        actions: [
          {
            type: 'button',
            props: { text: submitText, color: 'primary' },
            action: { type: 'submit' }
          }
        ]
      }
    };
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
      'name',
      {
        emptyText: data.docker_available
          ? '还没有 compose 项目，可在下方编辑并部署'
          : 'Docker 不可用，compose 项目列表为空'
      }
    );
    table.props.rowActions = [
      composeButton('启动', 'start'),
      composeButton('停止', 'stop'),
      composeButton('重启', 'restart'),
      composeButton('清理', 'down')
    ];
    body.push(table);
    if (data.can_admin) {
      body.push(
        writeCard(
          '编辑 / 校验 / 部署',
          'compose 源文件不持久化：校验只解析，不触达 Docker；部署会创建/更新项目。',
          writeForm(
            apiBase + '/compose/validate',
            { type: 'skiffComposeEdit' },
            [
              {
                type: 'input-text',
                props: {
                  name: 'project',
                  label: '项目名（必填，不能为 skiff / sites）',
                  required: true,
                  rules: { required: true, messages: { required: '请填写项目名' } }
                }
              },
              {
                type: 'textarea',
                props: {
                  name: 'compose',
                  label: 'docker-compose.yml',
                  rows: 14,
                  required: true,
                  rules: { required: true, messages: { required: '请粘贴 compose 内容' } }
                }
              },
              {
                type: 'select',
                props: {
                  name: 'action',
                  label: '操作',
                  defaultValue: 'validate',
                  items: [
                    { title: '仅校验', value: 'validate' },
                    { title: '校验并部署', value: 'up' }
                  ]
                }
              }
            ],
            '执行'
          )
        )
      );
    } else {
      body.push({
        type: 'alert',
        props: { type: 'info', text: '编辑、校验与部署需要 Docker 生命周期权限。' }
      });
    }
    return {
      type: 'page',
      props: { title: '编排 · Skiff', subtitle: '项目列表、启停、校验与部署均在原生页面完成' },
      body: body
    };
  }

  /// Compose 编辑/校验/部署：校验结果直接弹窗，部署后刷新项目列表。
  function composeEdit(args, ctx) {
    var values = formValues(ctx);
    var op = formText(values.action) === 'up' ? 'up' : 'validate';
    return submitJson('/compose/' + op, {
      project: formText(values.project),
      compose: formText(values.compose)
    })
      .then(function (data) {
        if (op === 'up') {
          var failed = data.failed || 0;
          if (failed > 0) {
            window.alert('项目 ' + data.project + ' 部署完成，但有 ' + failed + ' 个服务失败。');
          } else {
            window.alert('项目 ' + data.project + ' 已部署。');
          }
          location.reload();
          return;
        }
        window.alert(
          '校验通过\n项目：' +
            data.project +
            '\n服务顺序：' +
            (data.services || []).join(' → ') +
            '\n网络：' +
            ((data.networks || []).join('、') || '—') +
            '\n卷：' +
            ((data.volumes || []).join('、') || '—')
        );
      })
      .catch(function (error) {
        window.alert('校验失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  function tasksSchema(data) {
    var body = [];
    var table = listTable(
      [
        { key: 'name', label: '任务' },
        { key: 'method', label: '方式' },
        { key: 'target', label: '目标' },
        { key: 'expr', label: '调度' },
        { key: 'next_text', label: '下次运行' },
        { key: 'state_label', label: '状态' },
        { key: 'last_text', label: '最近结果' }
      ],
      '/tasks',
      'id',
      null,
      {
        emptyText: '还没有计划任务，站点详情页可开启每日备份'
      }
    );
    table.onEvent = {
      rowClick: {
        actions: [{ type: 'navigate', props: { url: prefix + '/tasks/${id}' } }]
      }
    };
    if (data.can_admin) {
      table.props = Object.assign({}, table.props, {
        rowActions: [
          taskRowButton('切换启停', {
            op: 'toggle',
            id: '${id}',
            enabled: '${enabled}'
          }),
          taskRowButton('立即运行', { op: 'run', id: '${id}' })
        ]
      });
    }
    body.push(table);
    if (!data.can_admin) {
      body.push({
        type: 'alert',
        props: { type: 'info', text: '启停、运行与删除需要管理员权限。' }
      });
    }
    return {
      type: 'page',
      props: { title: '定时任务 · Skiff', subtitle: '启停与运行结果均回写既有任务审计' },
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
            'started_text',
            { emptyText: '还没有运行记录' }
          )
        ]
      }
    });
    if (data.can_admin) {
      var actions = [
        {
          type: 'button',
          props: { text: t.enabled ? '停用任务' : '启用任务', color: 'secondary' },
          onEvent: {
            click: {
              actions: [
                {
                  type: 'skiffTaskWrite',
                  props: { op: 'toggle', id: t.id, enabled: !t.enabled }
                }
              ]
            }
          }
        },
        {
          type: 'button',
          props: { text: '立即运行', color: 'primary' },
          onEvent: {
            click: {
              actions: [{ type: 'skiffTaskWrite', props: { op: 'run', id: t.id } }]
            }
          }
        }
      ];
      if (!t.builtin) {
        actions.push({
          type: 'button',
          props: { text: '删除任务', color: 'danger', variant: 'text' },
          onEvent: {
            click: {
              actions: [
                {
                  type: 'skiffTaskWrite',
                  props: { op: 'delete', id: t.id, name: t.name }
                }
              ]
            }
          }
        });
      }
      body.push({
        type: 'card',
        props: { title: '任务操作' },
        content: { body: [{ type: 'flex', props: { gap: 8, wrap: true }, content: { body: actions } }] }
      });
    } else {
      body.push({
        type: 'alert',
        props: { type: 'info', text: '写操作需要管理员权限；当前为只读视图。' }
      });
    }
    return {
      type: 'page',
      props: { title: t.name + ' · 定时任务', subtitle: '启停、运行与删除均走 /ui/v1' },
      body: body
    };
  }

  function jobsSchema() {
    var body = [];
    var table = listTable(
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
      '/jobs',
      'id',
      null,
      {
        emptyText: '还没有长耗时任务，安装运行环境等操作会在这里显示进度'
      }
    );
    table.onEvent = {
      rowClick: {
        actions: [{ type: 'navigate', props: { url: prefix + '/jobs/${id}' } }]
      }
    };
    body.push(table);
    return {
      type: 'page',
      props: { title: '作业中心 · Skiff', subtitle: '列表只读；取消与重试在作业详情页' },
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
            : [
                {
                  type: 'code',
                  props: {
                    modelValue: logLines.join('\n'),
                    language: 'plaintext',
                    readonly: true,
                    height: 320,
                    lineNumbers: true,
                    wrap: false,
                    copyable: true,
                    ariaLabel: '作业日志（首屏）'
                  }
                }
              ]
      }
    });
    body.push({
      type: 'flex',
      props: { gap: 12, wrap: true },
      content: {
        body: [
          {
            type: 'link',
            props: {
              text: '查看完整日志（按需加载）',
              href: prefix + '/jobs/' + encodeURIComponent(data.id) + '/logs'
            }
          }
        ]
      }
    });
    if (data.can_admin && (data.can_cancel || data.can_retry)) {
      var jobActions = [];
      if (data.can_cancel) {
        jobActions.push({
          type: 'button',
          props: { text: data.cancel_requested ? '已请求取消' : '请求取消', color: 'warning' },
          onEvent: {
            click: {
              actions: [{ type: 'skiffJobWrite', props: { op: 'cancel', id: data.id } }]
            }
          }
        });
      }
      if (data.can_retry) {
        jobActions.push({
          type: 'button',
          props: { text: '重试作业', color: 'primary' },
          onEvent: {
            click: {
              actions: [{ type: 'skiffJobWrite', props: { op: 'retry', id: data.id } }]
            }
          }
        });
      }
      body.push({
        type: 'card',
        props: { title: '作业操作' },
        content: { body: [{ type: 'flex', props: { gap: 8, wrap: true }, content: { body: jobActions } }] }
      });
    } else if (data.can_admin) {
      body.push(textLine('当前状态下没有可用的取消或重试操作。'));
    } else {
      body.push({
        type: 'alert',
        props: { type: 'info', text: '取消与重试需要管理员权限。' }
      });
    }
    return {
      type: 'page',
      props: { title: data.title + ' · 作业', subtitle: '只读详情' },
      body: body
    };
  }

  // 设置页：账户安全（口令 / TOTP / 公钥）+ 面板表单（监听地址 / 入口前缀 / TLS）。
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
          text: '修改口令后其他页面才恢复访问；请在下方「修改口令」表单完成修改。'
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
    var accountLines = [
      '两步验证（TOTP）：' + (data.account.totp_enabled ? '已启用' : '未启用')
    ];
    if (data.account.must_change_password) {
      accountLines.push('当前账号仍在使用首启口令，请尽快修改。');
    }
    body.push(detailLines('账户安全', accountLines));
    body.push(detailLines('修改口令', ['修改成功后当前会话会轮换，无需重新登录。']));
    body.push({
      type: 'form',
      onEvent: { submit: [{ type: 'skiffPasswordChange' }] },
      props: {
        api: { method: 'POST', url: apiBase + '/settings/password', sendOn: false },
        successMessage: '口令已修改',
        actionsAlign: 'end'
      },
      body: [
        {
          type: 'input-text',
          props: {
            name: 'old_password',
            label: '当前口令',
            type: 'password',
            autocomplete: 'current-password',
            required: true,
            rules: { required: true, messages: { required: '请输入当前口令' } }
          }
        },
        {
          type: 'input-text',
          props: {
            name: 'new_password',
            label: '新口令',
            type: 'password',
            autocomplete: 'new-password',
            required: true,
            rules: { required: true, messages: { required: '请输入新口令' } }
          }
        },
        {
          type: 'input-text',
          props: {
            name: 'confirm_password',
            label: '确认新口令',
            type: 'password',
            autocomplete: 'new-password',
            required: true,
            rules: { required: true, messages: { required: '请再次输入新口令' } }
          }
        }
      ],
      content: {
        actions: [
          {
            type: 'button',
            props: { text: '修改口令', color: 'primary' },
            action: { type: 'submit' }
          }
        ]
      }
    });

    if (data.account.totp_enabled) {
      body.push(detailLines('两步验证（TOTP）', ['已启用：登录时需要输入动态码。']));
    } else if (data.account.totp_pending_secret) {
      body.push(
        detailLines('两步验证（TOTP）', [
          '密钥：' + data.account.totp_pending_secret,
          'otpauth URI：' + data.account.totp_pending_uri,
          '在验证器中录入后输入当前动态码完成启用。'
        ])
      );
      body.push({
        type: 'form',
        onEvent: { submit: [{ type: 'skiffTotpEnable' }] },
        props: {
          api: { method: 'POST', url: apiBase + '/settings/totp/enable', sendOn: false },
          actionsAlign: 'end'
        },
        body: [
          {
            type: 'input-text',
            props: {
              name: 'code',
              label: '动态码（6 位）',
              required: true,
              rules: { required: true, messages: { required: '请输入动态码' } }
            }
          }
        ],
        content: {
          actions: [
            {
              type: 'button',
              props: { text: '完成启用', color: 'primary' },
              action: { type: 'submit' }
            }
          ]
        }
      });
    } else {
      body.push(
        noticeCard('生成密钥后，用验证器 App 录入密钥或 otpauth URI，再输入动态码完成启用。')
      );
      body.push({
        type: 'button',
        props: { text: '生成 TOTP 密钥', variant: 'tonal' },
        onEvent: { click: { actions: [{ type: 'skiffTotpSetup' }] } }
      });
    }
    var backupCodes = takeBackupCodes();
    if (backupCodes.length > 0) {
      body.push({
        type: 'alert',
        props: {
          type: 'warning',
          title: '备份码（只显示一次，请立即保存）',
          text: backupCodes.join('   ')
        }
      });
    }

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
        var keyNodes = data.keys.map(function (k) {
          return {
            type: 'flex',
            props: { align: 'center', gap: 8 },
            content: {
              body: [
                {
                  type: 'text',
                  props: {
                    text: k.fingerprint_short + '… · ' + (k.label || '（无备注）')
                  }
                },
                {
                  type: 'button',
                  props: { text: '撤销', variant: 'text', color: 'danger' },
                  onEvent: {
                    click: {
                      actions: [
                        { type: 'skiffKeyRemove', props: { key_id: k.key_id } }
                      ]
                    }
                  }
                }
              ]
            }
          };
        });
        body.push(detailLines('信任公钥（TOFU）', ['共 ' + data.keys.length + ' 个；撤销后该公钥签名的包将无法安装/升级。']));
        body.push({ type: 'flex', props: { direction: 'column', gap: 8 }, content: { body: keyNodes } });
      }
      body.push(detailLines('导入信任公钥', ['公钥为 Ed25519 的 64 位十六进制字符串，请与发布者公布值逐位核对。']));
      body.push({
        type: 'form',
        onEvent: { submit: [{ type: 'skiffKeyImport' }] },
        props: {
          api: { method: 'POST', url: apiBase + '/settings/keys', sendOn: false },
          actionsAlign: 'end'
        },
        body: [
          {
            type: 'input-text',
            props: {
              name: 'public_key',
              label: '公钥（64 位十六进制）',
              required: true,
              rules: { required: true, messages: { required: '请输入公钥' } }
            }
          },
          { type: 'input-text', props: { name: 'label', label: '备注（可选）' } }
        ],
        content: {
          actions: [
            {
              type: 'button',
              props: { text: '导入公钥', color: 'primary' },
              action: { type: 'submit' }
            }
          ]
        }
      });
    }
    return {
      type: 'page',
      props: { title: '设置 · Skiff', subtitle: '口令、安全入口与面板偏好' },
      body: body
    };
  }

  /// 菜单设置行内按钮（rowActions 元素必须是完整节点）。
  function menuBtn(text, props, active) {
    return {
      type: 'button',
      props: { text: text, variant: active ? 'tonal' : 'text', color: 'primary' },
      onEvent: { click: { actions: [{ type: 'skiffMenuWrite', props: props }] } }
    };
  }

  /// 菜单设置：布局/内容宽度 + 每个菜单项的上移下移、显隐与固定。
  function menuSettingsSchema(data) {
    var body = [
      {
        type: 'alert',
        props: {
          type: 'info',
          text: '顺序、隐藏与固定只影响当前账号；换账号或换设备不共享。插件升级新增的菜单会自动插到相邻位置。'
        }
      },
      writeCard(
        '布局',
        '默认「左右（侧栏）」；只改变呈现方式，菜单结构与顺序不变。',
        {
          type: 'flex',
          props: { gap: 8, wrap: true },
          content: {
            body: [
              menuBtn('上下（顶栏）', { action: 'layout', layout: 'top' }, data.layout === 'top'),
              menuBtn('左右（侧栏）', { action: 'layout', layout: 'left' }, data.layout === 'left')
            ]
          }
        }
      )
    ];
    var widthForm = writeForm(
      apiBase + '/settings/menu',
      { type: 'skiffMenuWrite', props: { action: 'width' } },
      [
        {
          type: 'select',
          props: {
            name: 'width',
            label: '内容宽度',
            items: [
              { title: '自适应（铺满）', value: 'auto' },
              { title: '固定', value: 'fixed' }
            ]
          }
        },
        {
          type: 'input-text',
          props: { name: 'max', label: '固定时最大宽度（px，960–4096）' }
        }
      ],
      '保存内容宽度'
    );
    widthForm.props.initialValues = {
      width: data.width,
      max: String(data.max_width)
    };
    body.push(
      writeCard(
        '内容宽度',
        '默认「自适应」：侧栏之外的可用宽度全部给内容；固定时正文最多展开到设定上限（默认 1440px）。',
        widthForm
      )
    );

    (data.groups || []).forEach(function (group) {
      var rows = (group.items || []).map(function (item) {
        var flags = [];
        if (item.plugin) flags.push('插件');
        if (item.hidden) flags.push('已隐藏');
        if (item.pinned) flags.push('已固定');
        return {
          key: item.key,
          label: item.label,
          state: flags.join(' / ') || '—',
          toggle_action: item.hidden ? 'show' : 'hide',
          pin_action: item.pinned ? 'unpin' : 'pin'
        };
      });
      if (rows.length === 0) return;
      var table = statusTable(
        [
          { key: 'label', label: '菜单' },
          { key: 'state', label: '状态' }
        ],
        rows,
        'key'
      );
      table.props = Object.assign({}, table.props, {
        rowActions: [
          menuBtn('↑', { action: 'up', key: '${key}' }),
          menuBtn('↓', { action: 'down', key: '${key}' }),
          menuBtn('显隐', { action: '${toggle_action}', key: '${key}' }),
          menuBtn('固定', { action: '${pin_action}', key: '${key}' })
        ]
      });
      body.push({
        type: 'card',
        props: { title: group.label },
        content: { body: [table] }
      });
    });

    body.push({
      type: 'flex',
      props: { gap: 8, wrap: true },
      content: {
        body: [menuBtn('恢复默认', { action: 'reset' }, false)]
      }
    });
    body.push(
      noticeCard('恢复默认会清除本账号的顺序、隐藏与固定偏好；布局回到管理员默认值。')
    );
    return {
      type: 'page',
      props: { title: '菜单设置 · Skiff', subtitle: '偏好按账号保存，写操作复用 settings/menu 业务入口' },
      body: body
    };
  }

  /// Docker 设置：镜像加速源与镜像前缀；只有管理员可编辑。
  function dockerSettingsSchema(data) {
    var body = [
      {
        type: 'alert',
        props: {
          type: 'info',
          text:
            '镜像加速源写入宿主 /etc/docker/daemon.json 的 registry-mirrors，用于加速所有镜像拉取。写入前会备份原文件；JSON 非法或 Docker 重启失败会回滚；检测到运行中容器时只落盘、不自动重启。'
        }
      }
    ];
    if (!data.can_admin) {
      body.push(
        detailLines('镜像拉取参数', [
          '镜像加速源：' + (data.registry_mirror || '未设置'),
          '镜像前缀：' + (data.image_prefix || '未设置'),
          '只有管理员可以修改 Docker 设置。'
        ])
      );
      return {
        type: 'page',
        props: { title: 'Docker 设置 · Skiff', subtitle: '只读；写操作需要管理员权限' },
        body: body
      };
    }
    var form = writeForm(
      apiBase + '/settings/docker',
      { type: 'skiffDockerWrite' },
      [
        {
          type: 'input-text',
          props: {
            name: 'registry_mirror',
            label: '镜像加速地址',
            hint: 'http:// 或 https:// 开头；多个用逗号或换行分隔'
          }
        },
        {
          type: 'input-text',
          props: {
            name: 'image_prefix',
            label: '镜像前缀（可选）',
            hint: '拉取站点运行环境镜像（nginx / PHP）时拼在镜像名前；不填则直接使用原镜像名'
          }
        }
      ],
      '保存'
    );
    form.props.initialValues = {
      registry_mirror: data.registry_mirror,
      image_prefix: data.image_prefix
    };
    body.push(writeCard('镜像拉取参数', '只写宿主配置，不改动已有容器。', form));
    return {
      type: 'page',
      props: { title: 'Docker 设置 · Skiff', subtitle: '镜像拉取参数' },
      body: body
    };
  }

  /// 菜单偏好写操作：复用 `/ui/v1/settings/menu`（同一校验与审计）。
  function menuWrite(args, ctx) {
    var action = args.action || '';
    if (action === 'reset' && !window.confirm('确认恢复默认菜单偏好？本账号的顺序、隐藏与固定将被清除。')) {
      return Promise.resolve();
    }
    var payload = {
      action: action,
      key: args.key || '',
      layout: args.layout || '',
      width: args.width || '',
      max: args.max || ''
    };
    if (action === 'width') {
      var values = formValues(ctx);
      payload.width = formText(values.width).trim();
      payload.max = formText(values.max).trim();
    }
    return submitJson('/settings/menu', payload)
      .then(function (data) {
        window.alert((data && data.message) || '已保存菜单偏好');
        location.reload();
      })
      .catch(function (error) {
        window.alert('保存失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  /// Docker 设置写操作：复用 `/ui/v1/settings/docker`（同一落盘与审计）。
  function dockerSettingsWrite(args, ctx) {
    var values = formValues(ctx);
    return submitJson('/settings/docker', {
      registry_mirror: formText(values.registry_mirror).trim(),
      image_prefix: formText(values.image_prefix).trim()
    })
      .then(function (data) {
        window.alert((data && data.message) || '已保存');
        location.reload();
      })
      .catch(function (error) {
        window.alert('保存失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  /// 运行环境写操作统一收尾：成功提示、失败展示，均复用 `/ui/v1` 错误信封。
  function runtimeDone(data, fallback) {
    window.alert((data && data.message) || fallback || '操作已完成');
    location.reload();
  }

  function runtimeFail(error) {
    window.alert('操作失败：' + (error && error.message ? error.message : String(error)));
  }

  function runtimeNum(raw) {
    var text = formText(raw).trim();
    if (!text) return null;
    var n = parseInt(text, 10);
    return isNaN(n) ? null : n;
  }

  function runtimeCreate(args, ctx) {
    var v = formValues(ctx);
    return submitJson('/runtimes', {
      kind: formText(v.kind).trim(),
      version: formText(v.version).trim(),
      name: formText(v.name).trim()
    })
      .then(function (data) {
        runtimeDone(data, '安装任务已提交');
      })
      .catch(runtimeFail);
  }

  function runtimeImagesWrite(args, ctx) {
    var v = formValues(ctx);
    return submitJson('/runtimes/images', {
      nginx_image: formText(v.nginx_image).trim(),
      php_image: formText(v.php_image).trim()
    })
      .then(function (data) {
        runtimeDone(data, '镜像参数已保存');
      })
      .catch(runtimeFail);
  }

  function runtimeAction(args) {
    if (args.confirm_text && !window.confirm(args.confirm_text)) return Promise.resolve();
    return submitJson(
      '/runtimes/' + encodeURIComponent(args.id) + '/action/' + encodeURIComponent(args.op),
      {}
    )
      .then(function (data) {
        runtimeDone(data, '操作已完成');
      })
      .catch(runtimeFail);
  }

  function runtimeProbe(args) {
    return submitJson('/runtimes/' + encodeURIComponent(args.id) + '/probe', {})
      .then(function (data) {
        window.alert(
          data.ok ? '实测版本：' + data.version : '探测失败：' + (data.error || '未知错误')
        );
      })
      .catch(runtimeFail);
  }

  function runtimeConfigWrite(args, ctx) {
    var v = formValues(ctx);
    return submitJson('/runtimes/' + encodeURIComponent(args.id) + '/config', {
      memory_limit: formText(v.memory_limit).trim(),
      upload_max_filesize: formText(v.upload_max_filesize).trim(),
      post_max_size: formText(v.post_max_size).trim(),
      max_execution_time: runtimeNum(v.max_execution_time),
      pm: formText(v.pm).trim(),
      max_children: runtimeNum(v.max_children),
      start_servers: runtimeNum(v.start_servers),
      min_spare_servers: runtimeNum(v.min_spare_servers),
      max_spare_servers: runtimeNum(v.max_spare_servers),
      max_requests: runtimeNum(v.max_requests)
    })
      .then(function (data) {
        runtimeDone(data, 'PHP 配置已保存');
      })
      .catch(runtimeFail);
  }

  function runtimeNodeWrite(args, ctx) {
    var v = formValues(ctx);
    return submitJson('/runtimes/' + encodeURIComponent(args.id) + '/node-config', {
      command: formText(v.command).trim(),
      health_path: formText(v.health_path).trim(),
      node_env: formText(v.node_env).trim(),
      env: formText(v.env)
    })
      .then(function (data) {
        runtimeDone(data, 'Node.js 配置已保存');
      })
      .catch(runtimeFail);
  }

  function runtimePortWrite(args, ctx) {
    var v = formValues(ctx);
    var port = runtimeNum(v.port);
    if (port === null) {
      window.alert('端口必须是整数');
      return Promise.resolve();
    }
    if (!window.confirm('改端口会重写全部绑定站点反代并重建容器，确认继续？')) {
      return Promise.resolve();
    }
    return submitJson('/runtimes/' + encodeURIComponent(args.id) + '/port', {
      port: port,
      confirm: true
    })
      .then(function (data) {
        runtimeDone(data, '端口已保存');
      })
      .catch(runtimeFail);
  }

  function runtimeFileWrite(args, ctx) {
    var v = formValues(ctx);
    return submitJson('/runtimes/' + encodeURIComponent(args.id) + '/config-file', {
      key: args.key,
      content: formText(v.content)
    })
      .then(function (data) {
        runtimeDone(data, '配置文件已保存');
      })
      .catch(runtimeFail);
  }

  function runtimeExt(args) {
    if (args.confirm_text && !window.confirm(args.confirm_text)) return Promise.resolve();
    return submitJson(
      '/runtimes/' +
        encodeURIComponent(args.id) +
        '/extensions/' +
        encodeURIComponent(args.key) +
        '/' +
        encodeURIComponent(args.op),
      {}
    )
      .then(function (data) {
        runtimeDone(data, '扩展任务已提交');
      })
      .catch(runtimeFail);
  }

  function runtimePecl(args, ctx) {
    var v = formValues(ctx);
    return submitJson('/runtimes/' + encodeURIComponent(args.id) + '/pecl', {
      name: formText(v.name).trim(),
      version: formText(v.version).trim()
    })
      .then(function (data) {
        runtimeDone(data, 'PECL 安装任务已提交');
      })
      .catch(runtimeFail);
  }

  function runtimeClone(args, ctx) {
    var v = formValues(ctx);
    return submitJson('/runtimes/' + encodeURIComponent(args.id) + '/clone', {
      name: formText(v.name).trim()
    })
      .then(function (data) {
        runtimeDone(data, '克隆任务已提交');
      })
      .catch(runtimeFail);
  }

  function runtimeUnbind(args) {
    if (!window.confirm('解绑后站点会转为静态并停止，确认解绑 ' + args.domain + '？')) {
      return Promise.resolve();
    }
    return submitJson(
      '/runtimes/' +
        encodeURIComponent(args.id) +
        '/sites/' +
        encodeURIComponent(args.domain) +
        '/unbind',
      {}
    )
      .then(function (data) {
        runtimeDone(data, '站点已解绑');
      })
      .catch(runtimeFail);
  }

  /// Web 服务（OpenResty）：状态、安装进度、主配置/性能参数、日志与模块。
  function webNum(v) {
    return v === null || v === undefined ? '—' : String(v);
  }

  function webSchema(data, query, logs) {
    var status = data.status || {};
    var body = [];
    if (!status.available) {
      body.push({
        type: 'alert',
        props: {
          type: 'warning',
          title: 'Web 服务不可用',
          text:
            status.reason ||
            status.stub_reason ||
            'OpenResty 容器未运行；配置可以保存，但需要先启动容器才能生效。'
        }
      });
    }
    body.push({
      type: 'card',
      props: { title: '运行状态' },
      content: {
        body: [
          detailLines('概览', [
            '容器：' + (status.container_state || '—') + '；版本：' + (status.version || '—'),
            '运行时长：' + (status.uptime || '—') + '；最近 reload：' + (status.last_reload || '—'),
            '配置检查：' + (status.config_test || '—'),
            'active ' + webNum(status.active) +
              ' / accepts ' + webNum(status.accepts) +
              ' / handled ' + webNum(status.handled) +
              ' / requests ' + webNum(status.requests),
            'reading ' + webNum(status.reading) +
              ' / writing ' + webNum(status.writing) +
              ' / waiting ' + webNum(status.waiting),
            '探测地址：' + (status.probe_url || '—') +
              (status.probe_error ? '（' + status.probe_error + '）' : ''),
            '主配置：' + (status.conf_path || '—') +
              (status.conf_has_stub ? '（含 stub_status）' : '（未含 stub_status）')
          ])
        ]
      }
    });

    var install = data.install || {};
    if (install.running || install.error || (install.steps || []).length) {
      body.push({
        type: 'alert',
        props: {
          type: install.error ? 'danger' : install.running ? 'info' : 'success',
          title: install.running ? '正在安装 Web 服务' : install.error ? '安装失败' : '安装已完成',
          text:
            (install.error || '') +
            (install.steps || [])
              .map(function (s) {
                return s.name + '：' + s.state + (s.detail ? '（' + s.detail + '）' : '');
              })
              .join('；')
        }
      });
    }

    var config = data.config || {};
    var params = config.params || {};
    var mainForm = writeForm(
      apiBase + '/web/config',
      { type: 'skiffWebConfig' },
      [
        {
          type: 'textarea',
          props: { name: 'content', label: '主配置 ' + (config.main_path || 'nginx.conf'), rows: 20 }
        }
      ],
      '校验并重载'
    );
    mainForm.props.initialValues = { content: config.main_content || '' };
    body.push(
      writeCard('主配置', '保存前执行 nginx -t；失败会拒绝写入并保留当前配置。', mainForm)
    );

    var paramsForm = writeForm(
      apiBase + '/web/params',
      { type: 'skiffWebParams' },
      [
        { type: 'input-text', props: { name: 'worker_processes', label: 'worker_processes' } },
        { type: 'input-text', props: { name: 'worker_connections', label: 'worker_connections（64–65535）' } },
        { type: 'input-text', props: { name: 'keepalive_timeout', label: 'keepalive_timeout（0–3600 秒）' } },
        { type: 'input-text', props: { name: 'client_max_body_size', label: 'client_max_body_size' } },
        { type: 'checkbox', props: { name: 'gzip', label: '启用 gzip' } },
        { type: 'input-text', props: { name: 'gzip_types', label: 'gzip_types（空格分隔）' } }
      ],
      '保存性能参数'
    );
    paramsForm.props.initialValues = {
      worker_processes: params.worker_processes || '',
      worker_connections: String(params.worker_connections || ''),
      keepalive_timeout: String(params.keepalive_timeout || ''),
      client_max_body_size: params.client_max_body_size || '',
      gzip: !!params.gzip,
      gzip_types: params.gzip_types || ''
    };
    body.push(
      writeCard('性能参数', '数字字段非法会被拒绝，不会静默存成默认值。', paramsForm)
    );
    body.push({
      type: 'card',
      props: { title: '包含配置与站点配置' },
      content: {
        body: [
          detailLines('include：' + (config.include_path || '—'), [config.include_content || '（空）'])
        ].concat(
          (config.sites || []).map(function (s) {
            return detailLines(s.domain + ' · ' + s.path, [s.content || '（空）']);
          })
        )
      }
    });

    var logForm = writeForm(
      apiBase + '/web/logs',
      { type: 'skiffWebLogs' },
      [
        {
          type: 'select',
          props: {
            name: 'type',
            label: '日志',
            items: [
              { title: '访问日志', value: 'access' },
              { title: '错误日志', value: 'error' }
            ]
          }
        },
        {
          type: 'select',
          props: {
            name: 'lines',
            label: '范围',
            items: [
              { title: '50 行', value: '50' },
              { title: '200 行', value: '200' },
              { title: '500 行', value: '500' }
            ]
          }
        },
        {
          type: 'select',
          props: {
            name: 'level',
            label: '级别（错误日志）',
            items: [
              { title: '全部', value: 'all' },
              { title: 'error', value: 'error' },
              { title: 'warn', value: 'warn' },
              { title: 'info', value: 'info' }
            ]
          }
        },
        { type: 'input-text', props: { name: 'status', label: '状态码（如 404 / 4xx）' } },
        { type: 'input-text', props: { name: 'q', label: '关键字' } }
      ],
      '加载日志'
    );
    logForm.props.initialValues = {
      type: query.get('type') || 'access',
      lines: query.get('lines') || '200',
      level: query.get('level') || 'all',
      status: query.get('status') || '',
      q: query.get('q') || ''
    };
    var logBody = [logForm];
    if (logs) {
      logBody.push(
        detailLines('读取范围', [
          '文件：' + (logs.path || '—'),
          '本次加载 ' + logs.rows.length + ' 行；已扫描 ' + logs.scanned_text +
            '（' + logs.scanned_lines + ' 行），文件共 ' + logs.file_text,
          logs.truncated ? '已截断：只读取文件尾部 2 MiB' : '未截断',
          logs.reason || ''
        ])
      );
      if ((logs.status_counts || []).length) {
        logBody.push(
          detailLines('状态码分布', logs.status_counts.map(function (c) {
            return c.label + '：' + c.count;
          }))
        );
      }
      var logRows = logs.rows.map(function (line, i) {
        return { idx: String(i + 1), line: line };
      });
      logBody.push(
        statusTable(
          [
            { key: 'idx', label: '#' },
            { key: 'line', label: '日志' }
          ],
          logRows,
          'idx',
          { emptyText: '本次查询没有日志行' }
        )
      );
    } else {
      logBody.push(noticeCard('默认不读取日志：选择日志与范围后点「加载日志」。'));
    }
    body.push({ type: 'card', props: { title: '日志' }, content: { body: logBody } });

    var modules = data.modules || {};
    body.push({
      type: 'card',
      props: { title: '模块' },
      content: {
        body: [
          detailLines('nginx -V', [
            '版本：' + (modules.version || '—'),
            modules.available ? '' : '不可用：' + (modules.reason || ''),
            '编译参数：' + (modules.configure_args || []).join(' '),
            '模块列表：' + (modules.modules || []).join(' ')
          ])
        ]
      }
    });
    return {
      type: 'page',
      props: { title: 'Web 服务 · Skiff', subtitle: 'OpenResty 状态、配置、日志与模块' },
      body: body
    };
  }

  /// 主配置保存：复用 `/ui/v1/web/config`（nginx -t + reload + 审计）。
  function webConfigWrite(args, ctx) {
    var values = formValues(ctx);
    return submitJson('/web/config', { content: formText(values.content) })
      .then(function (data) {
        window.alert((data && data.message) || '主配置已保存');
        location.reload();
      })
      .catch(function (error) {
        window.alert('保存失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  /// 性能参数保存：复用 `/ui/v1/web/params`。
  function webParamsWrite(args, ctx) {
    var values = formValues(ctx);
    var connections = runtimeNum(values.worker_connections);
    var keepalive = runtimeNum(values.keepalive_timeout);
    if (connections === null || keepalive === null) {
      window.alert('worker_connections 与 keepalive_timeout 必须是整数');
      return Promise.resolve();
    }
    var gzip = formText(values.gzip);
    return submitJson('/web/params', {
      worker_processes: formText(values.worker_processes).trim(),
      worker_connections: connections,
      keepalive_timeout: keepalive,
      client_max_body_size: formText(values.client_max_body_size).trim(),
      gzip: gzip === 'true' || gzip === 'on' || values.gzip === true,
      gzip_types: formText(values.gzip_types).trim()
    })
      .then(function (data) {
        window.alert((data && data.message) || '性能参数已保存');
        location.reload();
      })
      .catch(function (error) {
        window.alert('保存失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  /// 日志按需加载：把筛选条件带回 URL，页面再走 `/ui/v1/web/logs`（不自动读文件）。
  function webLogsLoad(args, ctx) {
    var values = formValues(ctx);
    var qs = new URLSearchParams();
    qs.set('logs', '1');
    qs.set('type', formText(values.type).trim() || 'access');
    qs.set('lines', formText(values.lines).trim() || '200');
    qs.set('level', formText(values.level).trim() || 'all');
    if (formText(values.status).trim()) qs.set('status', formText(values.status).trim());
    if (formText(values.q).trim()) qs.set('q', formText(values.q).trim());
    location.href = prefix + '/sites/web?' + qs.toString();
    return Promise.resolve();
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

  /// 一次性备份码：放在 sessionStorage，页面重载后展示一次即清除。
  function takeBackupCodes() {
    try {
      var raw = window.sessionStorage.getItem('skiff_backup_codes');
      if (!raw) return [];
      window.sessionStorage.removeItem('skiff_backup_codes');
      var list = JSON.parse(raw);
      return Array.isArray(list) ? list : [];
    } catch (e) {
      return [];
    }
  }

  /// `/ui/v1` JSON 写请求：统一 CSRF 头与错误信封解析，失败抛出可展示文案。
  function submitJson(path, payload) {
    return fetch(apiBase + path, {
      method: 'POST',
      credentials: 'same-origin',
      headers: {
        Accept: 'application/json',
        'Content-Type': 'application/json',
        'x-csrf-token': (sessionState && sessionState.csrf_token) || ''
      },
      body: JSON.stringify(payload || {})
    })
      .then(function (resp) {
        return readJson(resp).then(function (body) {
          return { status: resp.status, body: body };
        });
      })
      .then(function (res) {
        if (res.status !== 200) throw new Error(errorMessage(res));
        return res.body.data || {};
      });
  }

  /// 入口前缀内的 JSON 写请求（如 exec 票据）：不加 `/ui/v1` 前缀，返回原始响应体。
  function postPrefixJson(url, payload) {
    return fetch(url, {
      method: 'POST',
      credentials: 'same-origin',
      headers: {
        Accept: 'application/json',
        'Content-Type': 'application/json',
        'x-csrf-token': (sessionState && sessionState.csrf_token) || ''
      },
      body: JSON.stringify(payload || {})
    }).then(function (resp) {
      return readJson(resp).then(function (body) {
        if (resp.status !== 200) {
          throw new Error(
            body && body.error && body.error.message
              ? body.error.message
              : '服务返回状态 ' + resp.status
          );
        }
        return body || {};
      });
    });
  }

  /// 终端：每次打开/重连都走既有 `POST /{p}/containers/{id}/exec/ticket`
  /// （Admin + CSRF + `Verb::Exec` 策略），票据绑定会话与容器、单次使用。
  function loadTerminalUrl(containerId) {
    return postPrefixJson(
      prefix + '/containers/' + encodeURIComponent(containerId) + '/exec/ticket',
      {}
    ).then(function (body) {
      var ticket = body.ticket || '';
      if (!ticket) throw new Error('未拿到终端票据');
      var proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
      return {
        expiresIn: body.expires_in || 0,
        wsUrl:
          proto +
          '//' +
          window.location.host +
          prefix +
          '/containers/' +
          encodeURIComponent(containerId) +
          '/exec?ticket=' +
          encodeURIComponent(ticket)
      };
    });
  }

  /// 终端页：budding `terminal` 组件内联渲染（@budding/terminal Pack 自带 xterm 与样式）。
  /// `resizeFormat: 'none'`：内核 exec WS 只透传 stdin，不解析 resize JSON。
  function terminalSchema(containerId, info) {
    return {
      type: 'page',
      props: { title: '容器终端', subtitle: '复用单次票据 + 既有 exec WS 通道' },
      body: [
        {
          type: 'alert',
          props: {
            type: 'info',
            text:
              '终端在容器内启动 /bin/sh；票据绑定当前会话与容器、' +
              (info.expiresIn || 30) +
              ' 秒内有效且只用一次。断开或离开页面后请重新连接（会重新取票）。'
          }
        },
        {
          type: 'flex',
          props: { gap: 12, wrap: true, align: 'center' },
          content: {
            body: [
              {
                type: 'link',
                props: {
                  text: '返回容器详情',
                  href: prefix + '/containers/' + encodeURIComponent(containerId)
                }
              },
              {
                type: 'button',
                props: { text: '重新连接（重新取票）', variant: 'tonal' },
                onEvent: {
                  click: { actions: [{ type: 'skiffTerminalReconnect' }] }
                }
              }
            ]
          }
        },
        {
          type: 'terminal',
          props: {
            wsUrl: info.wsUrl,
            height: 560,
            fontSize: 13,
            theme: 'auto',
            readOnly: false,
            reconnect: false,
            reconnectLimit: 0,
            resizeFormat: 'none',
            ariaLabel: '容器终端'
          }
        }
      ]
    };
  }

  function formValues(ctx) {
    try {
      return ctx.scope.get('values') || {};
    } catch (e) {
      return {};
    }
  }

  /// 改密：成功后轮换会话并回写新 CSRF，然后整页刷新让表单与状态归位。
  function passwordChange(args, ctx) {
    var values = formValues(ctx);
    return submitJson('/settings/password', {
      old_password: String(values.old_password || ''),
      new_password: String(values.new_password || ''),
      confirm_password: String(values.confirm_password || '')
    })
      .then(function (data) {
        if (sessionState) sessionState.csrf_token = data.csrf_token || sessionState.csrf_token;
        window.alert(data.message || '口令已修改');
        window.location.reload();
      })
      .catch(function (error) {
        window.alert('修改失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  function totpSetup() {
    return submitJson('/settings/totp/setup', {})
      .then(function () {
        window.location.reload();
      })
      .catch(function (error) {
        window.alert('生成密钥失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  function totpEnable(args, ctx) {
    var values = formValues(ctx);
    return submitJson('/settings/totp/enable', { code: String(values.code || '') })
      .then(function (data) {
        window.sessionStorage.setItem(
          'skiff_backup_codes',
          JSON.stringify(data.backup_codes || [])
        );
        window.alert(data.message || '两步验证已启用');
        window.location.reload();
      })
      .catch(function (error) {
        window.alert('启用失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  function keyImport(args, ctx) {
    var values = formValues(ctx);
    return submitJson('/settings/keys', {
      public_key: String(values.public_key || ''),
      label: String(values.label || '')
    })
      .then(function (data) {
        window.alert(data.message || '已导入公钥');
        window.location.reload();
      })
      .catch(function (error) {
        window.alert('导入失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  function keyRemove(args) {
    if (!window.confirm('撤销后该公钥签名的包将无法安装/升级，确定撤销？')) {
      return Promise.resolve();
    }
    return submitJson('/settings/keys/' + encodeURIComponent(args.key_id), {})
      .then(function (data) {
        window.alert(data.message || '已撤销该公钥');
        window.location.reload();
      })
      .catch(function (error) {
        window.alert('撤销失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  // ---- 文件管理：分片上传适配器与目录动作 --------------------------------

  /// budding 的平台请求适配器：内核只在提供 `request` 时才保留 `upload`
  /// （见上游 `createPlatformAdapter`），因此这里给出与默认 fetch 同语义的最小实现：
  /// 同源 Cookie、JSON 解析、非 2xx 抛错（错误文案取信封 message）。
  function apiRequest(req) {
    var method = (req && req.method) || 'GET';
    var headers = {};
    var key;
    if (req && req.headers) {
      for (key in req.headers) headers[key] = req.headers[key];
    }
    if (!headers.Accept) headers.Accept = 'application/json';
    var url = String((req && req.url) || '');
    if (req && req.params) {
      var qs = new URLSearchParams();
      Object.keys(req.params).forEach(function (k) {
        qs.append(k, String(req.params[k]));
      });
      var text = qs.toString();
      if (text) url += (url.indexOf('?') >= 0 ? '&' : '?') + text;
    }
    var opts = { method: method, credentials: 'same-origin', headers: headers };
    if (req && req.signal) opts.signal = req.signal;
    if (req && req.data !== undefined && req.data !== null) {
      if (typeof FormData !== 'undefined' && req.data instanceof FormData) {
        opts.body = req.data;
      } else {
        headers['Content-Type'] = 'application/json';
        opts.body = JSON.stringify(req.data);
      }
    }
    return fetch(url, opts).then(function (resp) {
      return readJson(resp).then(function (body) {
        if (!resp.ok) {
          var message =
            body && body.error && body.error.message
              ? body.error.message
              : '服务返回状态 ' + resp.status;
          var error = new Error(message);
          error.status = resp.status;
          error.body = body;
          throw error;
        }
        return body;
      });
    });
  }

  /// 宿主上传适配器（budding `PlatformAdapter.upload`）：组件给 FormData，
  /// 这里按单片上限切片，逐片 POST 到既有 `/{p}/files/upload`（同授权、同审计），
  /// 并按字节回报进度；失败直接抛错，由组件展示重试。
  function uploadFile(req) {
    var form = req && req.body;
    var file = form && form.get ? form.get('file') : null;
    if (!file) return Promise.reject(new Error('未选择文件'));
    var dir = '';
    try {
      dir = new URL(String(req.url || ''), window.location.origin).searchParams.get('dir') || '';
    } catch (e) {
      dir = '';
    }
    if (!dir) return Promise.reject(new Error('缺少上传目标目录'));
    var target = dir.replace(/\/+$/, '') + '/' + file.name;
    var chunkBytes = 256 * 1024;
    var offset = 0;
    function nextChunk() {
      if (req.signal && req.signal.aborted) {
        return Promise.reject(new Error('上传已取消'));
      }
      var end = Math.min(offset + chunkBytes, file.size);
      var url =
        prefix +
        '/files/upload?path=' +
        encodeURIComponent(target) +
        '&offset=' +
        offset;
      return fetch(url, {
        method: 'POST',
        credentials: 'same-origin',
        headers: {
          Accept: 'application/json',
          'Content-Type': 'application/octet-stream',
          'x-csrf-token': (sessionState && sessionState.csrf_token) || ''
        },
        body: file.slice(offset, end),
        signal: req.signal
      })
        .then(function (resp) {
          return readJson(resp).then(function (body) {
            return { status: resp.status, body: body };
          });
        })
        .then(function (res) {
          if (res.status !== 200) throw new Error(errorMessage(res));
          offset = end;
          if (typeof req.onProgress === 'function') {
            req.onProgress({ loaded: offset, total: file.size });
          }
          if (offset < file.size) return nextChunk();
          return { data: { path: target, size: file.size, message: '已上传 ' + file.name } };
        });
    }
    return nextChunk().then(function (result) {
      window.alert(result.data.message);
      window.location.reload();
      return result;
    });
  }

  function fileOpen(args) {
    var path = String(args.path || '');
    var isDir = args.dir === true || args.dir === 'true';
    window.location.href = isDir
      ? prefix + '/files?path=' + encodeURIComponent(path)
      : prefix + '/files/view?path=' + encodeURIComponent(path);
  }

  function fileDownload(args) {
    if (args.dir === true || args.dir === 'true') {
      window.alert('目录不能下载，请先进入目录选择文件。');
      return;
    }
    window.location.href =
      prefix + '/files/download?path=' + encodeURIComponent(String(args.path || ''));
  }

  function fileMkdir(args, ctx) {
    var values = formValues(ctx);
    var name = String(values.name || '').trim();
    if (!name) {
      window.alert('请输入目录名');
      return Promise.resolve();
    }
    var base = String(args.dir || '').replace(/\/+$/, '');
    return submitJson('/files/mkdir', { path: base + '/' + name })
      .then(function (data) {
        window.alert(data.message || '目录已创建');
        window.location.reload();
      })
      .catch(function (error) {
        window.alert('新建目录失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  function fileRename(args) {
    var current = String(args.path || '');
    var name = String(args.name || '');
    var next = window.prompt('新的名称', name);
    if (next === null) return Promise.resolve();
    next = next.trim();
    if (!next || next === name) return Promise.resolve();
    var parent = current.replace(/\/[^/]*$/, '');
    return submitJson('/files/rename', { from: current, to: parent + '/' + next })
      .then(function (data) {
        window.alert(data.message || '已重命名');
        window.location.reload();
      })
      .catch(function (error) {
        window.alert('重命名失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  function fileRemove(args) {
    var name = String(args.name || '');
    var path = String(args.path || '');
    if (!window.confirm('确认删除 ' + name + '？删除后进入回收站，可在回收站恢复。')) {
      return Promise.resolve();
    }
    return submitJson('/files/remove', { path: path })
      .then(function (data) {
        window.alert(
          (data.message || '已移入回收站') +
            (data.trash_id ? '（回收站编号 ' + data.trash_id + '）' : '')
        );
        window.location.reload();
      })
      .catch(function (error) {
        window.alert('删除失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  function logPageUrl(part, domain, tail, keyword) {
    return (
      prefix +
      '/sites/' +
      encodeURIComponent(domain) +
      '/logs?part=' +
      (part === 'error' ? 'error' : 'access') +
      '&tail=' +
      encodeURIComponent(String(tail || '200')) +
      (keyword ? '&q=' + encodeURIComponent(keyword) : '')
    );
  }

  function logSwitch(args) {
    window.location.href = logPageUrl(
      args.part,
      args.domain,
      args.tail,
      String(args.q || '')
    );
  }

  function logFilter(args, ctx) {
    var values = formValues(ctx);
    window.location.href = logPageUrl(
      args.part,
      args.domain,
      args.tail,
      String(values.q || '').trim()
    );
  }

  function logReload() {
    window.location.reload();
  }

  // ---- 审计日志（第十五批） ----------------------------------------------
  // 只读；后端 `/ui/v1/audit` 用签名 keyset 游标，前端只按地址栏参数取页。

  function auditPageUrl(keyword, cursor) {
    var params = [];
    if (keyword) params.push('q=' + encodeURIComponent(keyword));
    if (cursor) params.push('cursor=' + encodeURIComponent(cursor));
    return prefix + '/audit' + (params.length ? '?' + params.join('&') : '');
  }

  function auditSearch(args, ctx) {
    var values = formValues(ctx);
    window.location.href = auditPageUrl(String(values.q || '').trim(), '');
  }

  function auditNext(args) {
    window.location.href = auditPageUrl(
      String(args.q || ''),
      String(args.cursor || '')
    );
  }

  function auditSchema(data) {
    var keyword = String(data.keyword || '');
    var items = (data.items || []).map(function (a) {
      var copy = {};
      Object.keys(a).forEach(function (k) {
        copy[k] = a[k];
      });
      copy.key = String(a.id);
      copy.ts_label = formatTime(a.ts);
      return copy;
    });
    var body = [
      {
        type: 'form',
        onEvent: { submit: [{ type: 'skiffAuditSearch' }] },
        props: {
          api: { method: 'POST', url: apiBase + '/audit', sendOn: false },
          actionsAlign: 'end'
        },
        body: [
          {
            type: 'input-text',
            props: {
              name: 'q',
              label: '关键字（操作者 / IP / 动作 / 对象 / 详情）',
              defaultValue: keyword
            }
          }
        ],
        content: {
          actions: [
            {
              type: 'button',
              props: { text: '查询', variant: 'tonal' },
              action: { type: 'submit' }
            }
          ]
        }
      },
      {
        type: 'flex',
        props: { gap: 12, wrap: true, align: 'center' },
        content: {
          body: [
            { type: 'link', props: { text: '重置', href: prefix + '/audit' } },
            { type: 'text', props: { text: '按时间倒序；只读，权限沿用 AuditRead。', size: 'small' } }
          ]
        }
      },
      statusTable(
        [
          { key: 'ts_label', label: '时间', width: 170 },
          { key: 'actor', label: '操作者' },
          { key: 'ip', label: 'IP' },
          { key: 'action', label: '动作' },
          { key: 'target', label: '对象' },
          { key: 'result', label: '结果' },
          { key: 'detail', label: '详情' }
        ],
        items,
        'key',
        { height: 420, emptyText: '暂无审计记录' }
      )
    ];
    if (data.has_more && data.next_cursor) {
      body.push({
        type: 'button',
        props: { text: '下一页（更早记录）', variant: 'tonal' },
        onEvent: {
          click: {
            actions: [
              {
                type: 'skiffAuditNext',
                props: { q: keyword, cursor: data.next_cursor }
              }
            ]
          }
        }
      });
    }
    body.push(
      textLine(
        '本页 ' +
          items.length +
          ' 条' +
          (data.has_more ? '，还有更早记录。' : '，已到末尾。') +
          (keyword ? '关键字：' + keyword : '')
      )
    );
    return {
      type: 'page',
      props: {
        title: '审计日志 · Skiff',
        subtitle: '关键字过滤 + 签名游标（只读）'
      },
      body: body
    };
  }

  // 插件中心：列表（已安装 / 发现 / 可更新）与信息页（身份、授权、管理动作）。
  function pluginActions(p, includeDelete) {
    var actions = [
      {
        type: 'button',
        props: { text: p.enabled ? '停用插件' : '启用插件', variant: 'tonal' },
        onEvent: {
          click: { actions: [{ type: 'skiffPluginToggle', props: { id: p.id } }] }
        }
      }
    ];
    if (includeDelete) {
      actions.push({
        type: 'button',
        props: { text: '卸载（保留数据）', variant: 'tonal' },
        onEvent: {
          click: {
            actions: [{ type: 'skiffPluginDelete', props: { id: p.id, delete_data: false } }]
          }
        }
      });
      actions.push({
        type: 'button',
        props: { text: '卸载并删除数据', color: 'danger' },
        onEvent: {
          click: {
            actions: [{ type: 'skiffPluginDelete', props: { id: p.id, delete_data: true } }]
          }
        }
      });
    }
    return actions;
  }

  function pluginCard(p, canAdmin, includeDelete) {
    var lines = [
      '标识：' + p.id + ' · 版本 ' + p.version + ' · API ' + p.api,
      '状态：' + p.status.label,
      '签名：' + (p.signed ? '已验签' : '未验签') + ' · 沙箱：' + (p.reduced ? '降权' : '完整'),
      '宿主能力：' + (p.requires || '无'),
      '动作权限：' + (p.permissions || '无'),
      '对象授权：' + (p.grants || '无')
    ];
    if (p.takedown_note) lines.push('下游提示：' + p.takedown_note);
    var body = [detailLines(p.name, lines)];
    body.push({
      type: 'link',
      props: { text: '信息与管理页', href: prefix + '/plugins/' + encodeURIComponent(p.id) }
    });
    if (canAdmin) {
      body.push({
        type: 'flex',
        props: { gap: 8 },
        content: { body: pluginActions(p, includeDelete) }
      });
    }
    return { type: 'card', props: { title: p.name }, content: { body: body } };
  }

  // ---- 文件管理：平铺目录 + 面包屑 + 虚拟列表 -----------------------------

  function humanSize(bytes) {
    var n = Number(bytes) || 0;
    if (n < 1024) return n + ' B';
    var units = ['KB', 'MB', 'GB', 'TB'];
    var i = -1;
    do {
      n = n / 1024;
      i += 1;
    } while (n >= 1024 && i < units.length - 1);
    return (n >= 10 ? Math.round(n) : Math.round(n * 10) / 10) + ' ' + units[i];
  }

  function formatTime(value) {
    if (!value) return '—';
    var d = new Date(value);
    if (isNaN(d.getTime())) return value;
    return d.toLocaleString();
  }

  function breadcrumbFlex(path) {
    var segments = String(path || '/')
      .split('/')
      .filter(function (s) {
        return s !== '';
      });
    var body = [
      {
        type: 'link',
        props: { text: '根目录', href: prefix + '/files?path=' + encodeURIComponent('/') }
      }
    ];
    var acc = '';
    segments.forEach(function (seg) {
      acc += '/' + seg;
      body.push({ type: 'text', props: { text: '/' } });
      body.push({
        type: 'link',
        props: { text: seg, href: prefix + '/files?path=' + encodeURIComponent(acc) }
      });
    });
    return { type: 'flex', props: { gap: 4, wrap: true, align: 'center' }, content: { body: body } };
  }

  function rootsLine(data) {
    if (!data.roots || !data.roots.length) {
      return textLine('可直接在面包屑路径内浏览。');
    }
    return {
      type: 'flex',
      props: { gap: 12, wrap: true },
      content: {
        body: data.roots.map(function (r) {
          return {
            type: 'link',
            props: { text: r.label, href: prefix + '/files?path=' + encodeURIComponent(r.path) }
          };
        })
      }
    };
  }

  function fileRowSchema() {
    return {
      type: 'flex',
      props: { gap: 12, align: 'center' },
      content: {
        body: [
          {
            type: 'button',
            props: { text: '${item.name_label}', variant: 'text' },
            onEvent: {
              click: {
                actions: [
                  { type: 'skiffFileOpen', props: { path: '${item.path}', dir: '${item.dir}' } }
                ]
              }
            }
          },
          { type: 'text', props: { text: '${item.size_label}', size: 'small' } },
          { type: 'text', props: { text: '${item.mtime_label}', size: 'small' } },
          {
            type: 'button',
            props: { text: '下载', variant: 'text' },
            onEvent: {
              click: {
                actions: [
                  {
                    type: 'skiffFileDownload',
                    props: { path: '${item.path}', dir: '${item.dir}' }
                  }
                ]
              }
            }
          },
          {
            type: 'button',
            props: { text: '重命名', variant: 'text' },
            onEvent: {
              click: {
                actions: [
                  {
                    type: 'skiffFileRename',
                    props: { path: '${item.path}', name: '${item.name}' }
                  }
                ]
              }
            }
          },
          {
            type: 'button',
            props: { text: '删除', variant: 'text', color: 'danger' },
            onEvent: {
              click: {
                actions: [
                  {
                    type: 'skiffFileRemove',
                    props: { path: '${item.path}', name: '${item.name}' }
                  }
                ]
              }
            }
          }
        ]
      }
    };
  }

  // ---- 代码查看器与日志查看器（第十三批） --------------------------------

  var LANG_BY_EXT = {
    js: 'javascript',
    mjs: 'javascript',
    cjs: 'javascript',
    ts: 'typescript',
    json: 'json',
    css: 'css',
    scss: 'scss',
    less: 'less',
    html: 'markup',
    htm: 'markup',
    xml: 'markup',
    md: 'markdown',
    rs: 'rust',
    py: 'python',
    sh: 'bash',
    bash: 'bash',
    zsh: 'bash',
    yml: 'yaml',
    yaml: 'yaml',
    toml: 'toml',
    sql: 'sql',
    go: 'go',
    java: 'java',
    php: 'php',
    c: 'c',
    h: 'c',
    cpp: 'cpp',
    hpp: 'cpp',
    ini: 'ini',
    conf: 'ini'
  };

  function languageOf(name) {
    var ext = String(name || '')
      .split('.')
      .pop()
      .toLowerCase();
    return LANG_BY_EXT[ext] || 'plaintext';
  }

  function fileViewSchema(data) {
    var text = String(data.text || '');
    var binary = text.indexOf('\u0000') >= 0;
    var name = String(data.path || '').split(/[\\/]/).pop();
    var parent = String(data.path || '').replace(/[\\/][^\\/]*$/, '');
    var body = [
      {
        type: 'flex',
        props: { gap: 12, wrap: true, align: 'center' },
        content: {
          body: [
            {
              type: 'link',
              props: {
                text: '返回目录',
                href: prefix + '/files?path=' + encodeURIComponent(parent)
              }
            },
            {
              type: 'button',
              props: { text: '下载', variant: 'text' },
              onEvent: {
                click: {
                  actions: [{ type: 'skiffFileDownload', props: { path: data.path } }]
                }
              }
            }
          ]
        }
      }
    ];
    if (binary) {
      body.push({
        type: 'alert',
        props: { type: 'warning', text: '该文件疑似二进制（含 NUL 字节），未渲染内容。' }
      });
    }
    if (data.truncated) {
      body.push({
        type: 'alert',
        props: {
          type: 'info',
          text:
            '文件超过单次读取上限（' +
            data.max_edit_bytes +
            ' 字节），当前只显示开头部分；可下载查看完整内容。'
        }
      });
    }
    body.push({
      type: 'code',
      props: {
        modelValue: binary ? '' : text,
        language: languageOf(name),
        readonly: true,
        height: 560,
        lineNumbers: true,
        wrap: false,
        copyable: true,
        ariaLabel: name
      }
    });
    body.push({
      type: 'text',
      props: {
        text:
          '大小 ' +
          humanSize(data.size) +
          (data.truncated ? '（已截断）' : '') +
          ' · 只读 · 高亮由 @budding/code Pack 在前端完成',
        size: 'small'
      }
    });
    return {
      type: 'page',
      props: { title: name + ' · 代码查看器', subtitle: '只读，纯前端高亮（不引 WASM）' },
      body: body
    };
  }

  function logSwitchButton(text, props) {
    return {
      type: 'button',
      props: { text: text, variant: 'tonal' },
      onEvent: { click: { actions: [{ type: 'skiffLogSwitch', props: props }] } }
    };
  }

  function siteLogsSchema(domain, part, tail, keyword, data) {
    var lines = data.lines || [];
    var body = [
      {
        type: 'flex',
        props: { gap: 8, wrap: true, align: 'center' },
        content: {
          body: [
            {
              type: 'link',
              props: {
                text: '返回站点详情',
                href: prefix + '/sites/' + encodeURIComponent(domain)
              }
            },
            logSwitchButton('访问日志', {
              domain: domain,
              part: 'access',
              tail: tail,
              q: keyword
            }),
            logSwitchButton('错误日志', {
              domain: domain,
              part: 'error',
              tail: tail,
              q: keyword
            }),
            {
              type: 'button',
              props: { text: '刷新', variant: 'text' },
              onEvent: { click: { actions: [{ type: 'skiffLogReload' }] } }
            }
          ]
        }
      },
      {
        type: 'form',
        onEvent: {
          submit: [
            {
              type: 'skiffLogFilter',
              props: { domain: domain, part: part, tail: tail }
            }
          ]
        },
        props: {
          api: { method: 'POST', url: apiBase + '/sites/logs', sendOn: false },
          actionsAlign: 'end'
        },
        body: [
          {
            type: 'input-text',
            props: {
              name: 'q',
              label: '关键字（只过滤文件尾部窗口）',
              defaultValue: keyword
            }
          }
        ],
        content: {
          actions: [
            {
              type: 'button',
              props: { text: '过滤', variant: 'tonal' },
              action: { type: 'submit' }
            }
          ]
        }
      }
    ];
    if (lines.length === 0) {
      body.push({ type: 'alert', props: { type: 'info', text: '暂无日志。' } });
    } else {
      body.push({
        type: 'code',
        props: {
          modelValue: lines.join('\n'),
          language: 'plaintext',
          readonly: true,
          height: 560,
          lineNumbers: true,
          wrap: false,
          copyable: true,
          ariaLabel: domain + ' ' + part + ' 日志'
        }
      });
    }
    body.push({
      type: 'text',
      props: {
        text:
          '共 ' +
          lines.length +
          ' 行（文件尾部最多 2MB 窗口、最新在前）· ' +
          (data.log_dir || ''),
        size: 'small'
      }
    });
    return {
      type: 'page',
      props: {
        title: domain + ' · ' + (part === 'error' ? '错误日志' : '访问日志'),
        subtitle: '定高滚动 · 按需拉取（不加载 SSE）'
      },
      body: body
    };
  }

  /// 作业日志：按 `after` 游标最多取 10 页，避免长作业一次性拉爆页面。
  function loadJobLogs(jobId) {
    var lines = [];
    var after = 0;
    var pages = 0;
    var job = null;
    function step() {
      return fetchRawJson(
        prefix + '/jobs/' + encodeURIComponent(jobId) + '/logs?after=' + after
      ).then(function (batch) {
        job = batch.job || job;
        var entries = batch.entries || [];
        entries.forEach(function (entry) {
          var when = entry.at ? formatTime(new Date(entry.at * 1000)) : '';
          lines.push(
            (when ? when + ' ' : '') +
              '[' +
              (entry.stream || 'stdout') +
              '] ' +
              entry.line
          );
        });
        pages += 1;
        var maxSeq = batch.max_seq || 0;
        if (entries.length > 0 && maxSeq > after && pages < 10) {
          after = maxSeq;
          return step();
        }
        return { lines: lines, job: job || {}, truncated: entries.length > 0 && pages >= 10 };
      });
    }
    return step();
  }

  function jobLogsSchema(jobId, result) {
    var lines = result.lines || [];
    var job = result.job || {};
    var body = [
      {
        type: 'flex',
        props: { gap: 8, wrap: true, align: 'center' },
        content: {
          body: [
            {
              type: 'link',
              props: {
                text: '返回作业详情',
                href: prefix + '/jobs/' + encodeURIComponent(jobId)
              }
            },
            {
              type: 'button',
              props: { text: '刷新', variant: 'text' },
              onEvent: { click: { actions: [{ type: 'skiffLogReload' }] } }
            }
          ]
        }
      }
    ];
    if (lines.length === 0) {
      body.push({ type: 'alert', props: { type: 'info', text: '暂无日志。' } });
    } else {
      body.push({
        type: 'code',
        props: {
          modelValue: lines.join('\n'),
          language: 'plaintext',
          readonly: true,
          height: 560,
          lineNumbers: true,
          wrap: false,
          copyable: true,
          ariaLabel: '作业日志'
        }
      });
    }
    body.push({
      type: 'text',
      props: {
        text:
          '共 ' +
          lines.length +
          ' 行 · 状态 ' +
          (job.status_label || '—') +
          (result.truncated ? ' · 仅加载前 10 页，可刷新继续' : ''),
        size: 'small'
      }
    });
    return {
      type: 'page',
      props: {
        title: '作业日志 · ' + jobId,
        subtitle: '按需加载（不做 SSE 实时流）'
      },
      body: body
    };
  }

  /// 日志读取失败的统一落地页（Docker 不可用 / 权限 / 实例不存在）。
  function logErrorPage(title, backHref, backText, error) {
    return {
      type: 'page',
      props: { title: title, subtitle: '读取失败' },
      body: [
        {
          type: 'alert',
          props: {
            type: 'error',
            text:
              '无法读取日志：' +
              (error && error.message ? error.message : String(error))
          }
        },
        { type: 'link', props: { text: backText, href: backHref } }
      ]
    };
  }

  /// 容器日志：一次性尾部快照 + 刷新，不建 SSE 长连接。
  function containerLogsSchema(containerId, data) {
    var text = String(data.text || '');
    var lines = text.split('\n').filter(function (line) {
      return line !== '';
    });
    var body = [
      {
        type: 'flex',
        props: { gap: 8, wrap: true, align: 'center' },
        content: {
          body: [
            {
              type: 'link',
              props: {
                text: '返回容器详情',
                href: prefix + '/containers/' + encodeURIComponent(containerId)
              }
            },
            {
              type: 'button',
              props: { text: '刷新', variant: 'text' },
              onEvent: { click: { actions: [{ type: 'skiffLogReload' }] } }
            }
          ]
        }
      }
    ];
    if (lines.length === 0) {
      body.push({ type: 'alert', props: { type: 'info', text: '暂无日志。' } });
    } else {
      body.push({
        type: 'code',
        props: {
          modelValue: lines.join('\n'),
          language: 'plaintext',
          readonly: true,
          height: 560,
          lineNumbers: true,
          wrap: false,
          copyable: true,
          ariaLabel: (data.name || containerId) + ' 容器日志'
        }
      });
    }
    body.push({
      type: 'text',
      props: {
        text:
          '共 ' +
          lines.length +
          ' 行 · 取尾部 ' +
          (data.tail || 500) +
          ' 行 · 状态 ' +
          (data.state || '—'),
        size: 'small'
      }
    });
    return {
      type: 'page',
      props: {
        title: (data.name || containerId) + ' · 容器日志',
        subtitle: '尾部快照；点「刷新」取最新，暂不建实时跟随'
      },
      body: body
    };
  }

  /// 运行环境日志：复用既有 `runtime_logs` 读取路径，一次性快照 + 刷新。
  function runtimeLogsSchema(runtimeId, data) {
    var text = String(data.text || '');
    var lines = text.split('\n').filter(function (line) {
      return line !== '';
    });
    var body = [
      {
        type: 'flex',
        props: { gap: 8, wrap: true, align: 'center' },
        content: {
          body: [
            {
              type: 'link',
              props: {
                text: '返回运行环境详情',
                href: prefix + '/runtimes/' + encodeURIComponent(runtimeId)
              }
            },
            {
              type: 'button',
              props: { text: '刷新', variant: 'text' },
              onEvent: { click: { actions: [{ type: 'skiffLogReload' }] } }
            }
          ]
        }
      }
    ];
    if (lines.length === 0) {
      body.push({ type: 'alert', props: { type: 'info', text: '暂无日志。' } });
    } else {
      body.push({
        type: 'code',
        props: {
          modelValue: lines.join('\n'),
          language: 'plaintext',
          readonly: true,
          height: 560,
          lineNumbers: true,
          wrap: false,
          copyable: true,
          ariaLabel: runtimeId + ' 运行日志'
        }
      });
    }
    body.push({
      type: 'text',
      props: {
        text: '共 ' + lines.length + ' 行 · 取尾部 ' + (data.tail || 300) + ' 行',
        size: 'small'
      }
    });
    return {
      type: 'page',
      props: { title: runtimeId + ' · 运行环境日志', subtitle: '尾部快照，不做 SSE 实时流' },
      body: body
    };
  }

  function filesSchema(data) {
    var body = [];
    if (data.truncated) {
      body.push({
        type: 'alert',
        props: {
          type: 'warning',
          text: '目录条目超过单页上限（1000 条），当前只显示前 1000 条；请进入子目录继续浏览。'
        }
      });
    }
    var items = (data.entries || []).map(function (e) {
      var copy = {};
      Object.keys(e).forEach(function (k) {
        copy[k] = e[k];
      });
      copy.name_label = e.dir ? e.name + '/' : e.name;
      copy.size_label = e.dir ? '—' : humanSize(e.size);
      copy.mtime_label = formatTime(e.mtime);
      return copy;
    });
    body.push({
      type: 'card',
      props: { title: '位置' },
      content: { body: [breadcrumbFlex(data.path), rootsLine(data)] }
    });
    if (data.can_admin) {
      body.push({
        type: 'flex',
        props: { gap: 12, wrap: true },
        content: {
          body: [
            {
              type: 'link',
              props: { text: '回收站', href: prefix + '/files/trash' }
            },
            {
              type: 'link',
              props: { text: '配额管理', href: prefix + '/files/quotas' }
            }
          ]
        }
      });
    }
    body.push({
      type: 'card',
      props: { title: '新建与上传' },
      content: {
        body: [
          {
            type: 'form',
            onEvent: {
              submit: [{ type: 'skiffFileMkdir', props: { dir: data.path } }]
            },
            props: {
              api: { method: 'POST', url: apiBase + '/files/mkdir', sendOn: false },
              actionsAlign: 'end'
            },
            body: [
              {
                type: 'input-text',
                props: {
                  name: 'name',
                  label: '新目录名',
                  required: true,
                  rules: { required: true, messages: { required: '请输入目录名' } }
                }
              }
            ],
            content: {
              actions: [
                {
                  type: 'button',
                  props: { text: '新建目录', variant: 'tonal' },
                  action: { type: 'submit' }
                }
              ]
            }
          },
          {
            type: 'file',
            props: {
              name: 'file',
              label: '选择文件后立即上传（单文件，分片 256 KiB）',
              multiple: false,
              uploadField: 'file',
              uploadApi: {
                method: 'POST',
                url: apiBase + '/files/upload?dir=' + encodeURIComponent(data.path)
              },
              uploadProgressTarget: 'uploadProgress'
            }
          }
        ]
      }
    });
    body.push({
      type: 'virtual-list',
      props: {
        items: items,
        itemKey: 'path',
        itemHeight: 52,
        height: 560,
        emptyText: '目录为空',
        ariaLabel: '目录条目'
      },
      content: { body: fileRowSchema() }
    });
    body.push({
      type: 'text',
      props: {
        text: '共 ' + items.length + ' 项；目录在前、按名称排序。删除进回收站，可在回收站恢复。',
        size: 'small'
      }
    });
    return {
      type: 'page',
      props: { title: '文件管理 · Skiff', subtitle: '平铺目录 + 面包屑（不使用树形）' },
      body: body
    };
  }

  function formatEpoch(ts) {
    if (!ts) return '—';
    var d = new Date(Number(ts) * 1000);
    if (isNaN(d.getTime())) return '—';
    var pad = function (v) {
      return String(v).padStart(2, '0');
    };
    return (
      d.getFullYear() + '-' + pad(d.getMonth() + 1) + '-' + pad(d.getDate()) +
      ' ' + pad(d.getHours()) + ':' + pad(d.getMinutes())
    );
  }

  function filesTrashSchema(data) {
    var retention = data.retention_days || 0;
    var items = (data.items || []).map(function (item) {
      var copy = {};
      Object.keys(item).forEach(function (k) {
        copy[k] = item[k];
      });
      copy.path_label = item.original_path;
      copy.kind_label = item.kind === 'dir' ? '目录' : item.kind === 'symlink' ? '链接' : '文件';
      copy.size_label = humanSize(item.size);
      copy.files_label = String(item.files || 0);
      copy.deleted_label = formatEpoch(item.deleted_at);
      copy.actor_label = (item.actor_type || '') + ':' + (item.actor_id || '');
      copy.expires_label = formatEpoch(item.expires_at);
      return copy;
    });
    var body = [
      {
        type: 'alert',
        props: {
          type: 'info',
          text:
            '删除的文件先移入回收站，默认保留 ' +
            retention +
            ' 天，到期由内置任务 trash-prune 彻底删除。数据根、plugins/、官方目录与 skiff.db 在删除和恢复路径上都会被硬拒。'
        }
      },
      {
        type: 'flex',
        props: { gap: 8, wrap: true },
        content: {
          body: [
            trashBtn('刷新', { op: 'refresh' }, 'secondary'),
            trashBtn('清空回收站', { op: 'clear' }, 'danger')
          ]
        }
      }
    ];
    if (items.length === 0) {
      body.push(noticeCard('回收站为空。'));
    } else {
      var table = statusTable(
        [
          { key: 'path_label', label: '原路径' },
          { key: 'kind_label', label: '类型' },
          { key: 'size_label', label: '大小' },
          { key: 'files_label', label: '文件数' },
          { key: 'deleted_label', label: '删除时间' },
          { key: 'actor_label', label: '操作者' },
          { key: 'expires_label', label: '到期时间' }
        ],
        items,
        'id'
      );
      table.props = Object.assign({}, table.props, {
        rowActions: [
          trashBtn('恢复', { op: 'restore', id: '${id}', path: '${original_path}' }),
          trashBtn('彻底删除', { op: 'purge', id: '${id}', path: '${original_path}' }, 'danger')
        ]
      });
      body.push(table);
    }
    return {
      type: 'page',
      props: { title: '回收站 · Skiff', subtitle: '恢复与彻底删除复用既有 files JSON 接口' },
      body: body
    };
  }

  function trashBtn(text, props, color) {
    return {
      type: 'button',
      props: { text: text, variant: 'text', color: color || 'primary' },
      onEvent: { click: { actions: [{ type: 'skiffTrashWrite', props: props }] } }
    };
  }

  function filesQuotasSchema(data) {
    var quotas = (data.quotas || []).map(function (q) {
      var copy = {};
      Object.keys(q).forEach(function (k) {
        copy[k] = q[k];
      });
      copy.bytes_label =
        humanSize(q.used_bytes) + ' / ' + (q.max_bytes == null ? '不限' : humanSize(q.max_bytes));
      copy.files_label =
        String(q.used_files || 0) + ' / ' + (q.max_files == null ? '不限' : String(q.max_files));
      copy.updated_label = formatEpoch(q.updated_at);
      copy.truncated_label = q.truncated ? '不完整（仅下界）' : '';
      return copy;
    });
    var body = [
      {
        type: 'alert',
        props: {
          type: 'info',
          text:
            '已用量是缓存计数：每次成功操作后增量修正，外部改动可能造成偏差；「重新统计」会递归扫描整棵目录树，成本为 O(条目数)。'
        }
      },
      writeCard(
        '设置配额',
        '容量上限与文件数上限至少填一项；留空表示不限。重复保存同一路径会覆盖原配额。',
        writeForm(
          prefix + '/files/quotas/set',
          { type: 'skiffQuotaWrite', props: { op: 'set' } },
          [
            {
              type: 'input-text',
              props: {
                name: 'path',
                label: '目录树（绝对路径）',
                required: true,
                rules: { required: true, messages: { required: '请输入绝对路径' } }
              }
            },
            { type: 'input-text', props: { name: 'max_bytes', label: '容量上限（字节，留空不限）' } },
            { type: 'input-text', props: { name: 'max_files', label: '文件数上限（留空不限）' } }
          ],
          '保存配额'
        )
      )
    ];
    if (quotas.length === 0) {
      body.push(noticeCard('尚未设置目录配额。'));
    } else {
      var table = statusTable(
        [
          { key: 'root', label: '目录树' },
          { key: 'bytes_label', label: '已用容量 / 上限' },
          { key: 'files_label', label: '已用文件数 / 上限' },
          { key: 'truncated_label', label: '统计' },
          { key: 'updated_label', label: '最近统计' }
        ],
        quotas,
        'root'
      );
      table.props = Object.assign({}, table.props, {
        rowActions: [
          trashBtn('重新统计', { op: 'scan', path: '${root}' }),
          trashBtn('取消配额', { op: 'cancel', path: '${root}' }, 'danger')
        ]
      });
      body.push(table);
    }
    return {
      type: 'page',
      props: { title: '配额管理 · Skiff', subtitle: '设置、重扫与取消均复用既有 files JSON 接口' },
      body: body
    };
  }

  function pluginsSchema(data, tab) {
    // 菜单深链 `/plugins/discover|updates` 与页内标签都以路径表达；
    // 旧 `?tab=` 保留兼容（书签与既有链接）。
    tab = tab || new URLSearchParams(location.search).get('tab') || 'installed';
    var body = [];
    body.push({
      type: 'flex',
      props: { gap: 12 },
      content: {
        body: [
          { type: 'link', props: { text: '已安装 (' + data.plugins.length + ')', href: prefix + '/plugins' } },
          { type: 'link', props: { text: '发现', href: prefix + '/plugins/discover' } },
          { type: 'link', props: { text: '可更新', href: prefix + '/plugins/updates' } }
        ]
      }
    });
    body.push(
      noticeCard(
        '已启用 ' +
          data.enabled_count +
          ' 个 · 已停用 ' +
          data.disabled_count +
          ' 个；启停与卸载只对管理员开放。'
      )
    );
    if (tab === 'discover') {
      body.push({
        type: 'alert',
        props: {
          type: 'info',
          title: '发现',
          text: '上传已验签 .wbpkg 尚未原生渲染；商店安装已可用。旧版发现页不再作为入口。'
        }
      });
      return {
        type: 'page',
        props: { title: '插件中心 · Skiff', subtitle: '发现' },
        body: body
      };
    }
    if (tab === 'updates') {
      var downstream = data.plugins.filter(function (p) {
        return p.takedown_note;
      });
      body.push({
        type: 'alert',
        props: {
          type: 'info',
          title: '可更新',
          text: '内核尚未接入更新检查源；下方只列出目录已撤回/下架的已装版本。'
        }
      });
      downstream.forEach(function (p) {
        body.push(pluginCard(p, false, false));
      });
      return {
        type: 'page',
        props: { title: '插件中心 · Skiff', subtitle: '可更新' },
        body: body
      };
    }
    if (data.plugins.length === 0) {
      body.push(noticeCard('还没有安装插件；到「发现」上传已验签的 .wbpkg。'));
    } else {
      data.plugins.forEach(function (p) {
        body.push(pluginCard(p, data.can_admin, data.can_admin));
      });
    }
    return {
      type: 'page',
      props: { title: '插件中心 · Skiff', subtitle: '安装、授权、启停与更新' },
      body: body
    };
  }

  function pluginInfoSchema(data) {
    var p = data.plugin;
    var body = [];
    body.push(
      detailLines('身份', [
        '标识：' + p.id,
        '名称：' + p.name + ' · 版本 ' + p.version + ' · API ' + p.api,
        '状态：' + p.status.label,
        '签名状态：' + (p.signed ? '已验签' : '未验签'),
        '密钥指纹：' + (p.key_id || '未签名包'),
        '沙箱模式：' + (p.reduced ? '降权沙箱' : '完整沙箱'),
        '声明能力面：' + (p.requires || '无'),
        '动作权限：' + (p.permissions || '无')
      ])
    );
    if (p.takedown_note) {
      body.push({ type: 'alert', props: { type: 'warning', text: p.takedown_note } });
    }
    body.push(
      detailLines(
        '对象授权',
        data.grants.length
          ? data.grants.map(function (g) {
              return g.kind + ':' + g.value;
            })
          : ['无对象授权']
      )
    );
    body.push(
      detailLines(
        '插件间接口',
        ['提供：' + (data.interop_provides.length ? data.interop_provides.join('；') : '无')].concat([
          '允许调用：' + (data.interop_consumes.length ? data.interop_consumes.join('；') : '无')
        ])
      )
    );
    if (data.menus.length === 0) {
      body.push(noticeCard('该插件没有声明菜单或功能页。'));
    } else {
      var declared = [];
      data.menus.forEach(function (group) {
        declared.push(group.label);
        group.children.forEach(function (child) {
          declared.push('　↳ ' + child.label);
        });
      });
      body.push(detailLines('声明的功能页', declared));
      body.push({
        type: 'alert',
        props: {
          type: 'info',
          title: '功能页由随包 Schema 原生渲染',
          text: '已声明 schema 的功能页从左侧菜单进入，由壳页直接渲染；未声明 schema 的旧视图仍保留在旧 SSR 工作台（显式 ?legacy=1），不再作为面板内入口。'
        }
      });
    }
    if (data.can_admin) {
      body.push({
        type: 'flex',
        props: { gap: 8 },
        content: { body: pluginActions(p, true) }
      });
    }
    body.push({
      type: 'link',
      props: { text: '返回插件中心', href: prefix + '/plugins' }
    });
    return {
      type: 'page',
      props: { title: p.name + ' · 插件', subtitle: p.id },
      body: body
    };
  }

  function pluginToggle(args) {
    var id = args.id;
    return fetch(apiBase + '/plugins/' + encodeURIComponent(id) + '/toggle', {
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
        window.alert(res.body.data.enabled ? '插件已启用' : '插件已停用');
        location.reload();
      })
      .catch(function (error) {
        window.alert('操作失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  function pluginDelete(args) {
    var id = args.id;
    var deleteData = args.delete_data === true;
    var text = deleteData
      ? '确认卸载 ' + id + ' 并删除其数据目录？此操作不可恢复。'
      : '确认卸载 ' + id + '？数据目录会保留。';
    if (!window.confirm(text)) return Promise.resolve();
    return fetch(apiBase + '/plugins/' + encodeURIComponent(id) + '/delete', {
      method: 'POST',
      credentials: 'same-origin',
      headers: {
        Accept: 'application/json',
        'Content-Type': 'application/json',
        'x-csrf-token': (sessionState && sessionState.csrf_token) || ''
      },
      body: JSON.stringify({ delete_data: deleteData })
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
        location.assign(
          (res.body && res.body.data && res.body.data.redirect_to) || prefix + '/plugins'
        );
      })
      .catch(function (error) {
        window.alert('卸载失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  // 商店：目录只读快照 + 插件安装（下载验签后二次确认）。
  var pendingInstall = null;
  /// 暂存的安装确认页数据；确认路由按它渲染，刷新后失效（需回商店重选）。
  var pendingConfirm = null;

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
    if (upgrade.can_download && upgrade.available) {
      body.push({
        type: 'button',
        props: { text: '下载升级包 ' + upgrade.available, variant: 'tonal' },
        onEvent: {
          click: {
            actions: [
              { type: 'skiffUpgradeDownload', props: { version: upgrade.available } }
            ]
          }
        }
      });
    }
    return {
      type: 'page',
      props: { title: '商店 · Skiff', subtitle: '目录快照与插件安装' },
      body: body
    };
  }

  /// 升级包下载：复用 `/ui/v1/store/upgrades/{version}/download`，
  /// 不跳旧商店；校验、暂存与 `upgrade.download` 审计由内核同一入口完成。
  function storeUpgradeDownload(args) {
    var version = (args && args.version) || '';
    if (!version) {
      window.alert('目录中没有可下载的升级版本。');
      return Promise.resolve();
    }
    window.alert('正在下载并校验 ' + version + ' 升级包，请稍候…');
    return fetch(apiBase + '/store/upgrades/' + encodeURIComponent(version) + '/download', {
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
        if (res.status !== 200 || !res.body || !res.body.data) {
          window.alert(errorMessage(res));
          return;
        }
        var data = res.body.data;
        window.alert(
          '升级包 ' + data.version + ' 已暂存并通过 sha256 校验：' + data.path
        );
      })
      .catch(function (error) {
        window.alert('升级包下载失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  function storeInstall(args, ctx, runtime) {
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
        pendingConfirm = res.body.data;
        if (runtime && typeof runtime.navigateTo === 'function') {
          runtime.navigateTo({ path: prefix + '/store/confirm' });
          return;
        }
        // 兜底：没有路由能力时退回商店页（避免二次嵌入另一个 Vue 实例）。
        window.location.href = prefix + '/store';
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
        pendingConfirm = null;
        window.alert('安装完成');
        location.assign((res.body && res.body.data && res.body.data.redirect_to) || prefix + '/plugins');
      })
      .catch(function (error) {
        window.alert('安装失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  /// 统一的 `/ui/v1` 数据读取：401 / 非 200 / 缺 `data` 都转成可展示的错误。
  function fetchData(path) {
    return fetch(apiBase + path, {
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
        if (res.status === 401) throw new Error('会话已过期，请刷新页面后重新登录。');
        if (res.status !== 200 || !res.body || !res.body.data) {
          throw new Error(
            res.body && res.body.error && res.body.error.message
              ? res.body.error.message
              : '服务返回状态 ' + res.status
          );
        }
        return res.body.data;
      });
  }

  /// 直接读非 `/ui/v1` 的既有 JSON 接口（如作业增量日志），不假设 `data` 信封。
  function fetchRawJson(url) {
    return fetch(url, {
      method: 'GET',
      credentials: 'same-origin',
      headers: { Accept: 'application/json' }
    }).then(function (resp) {
      return readJson(resp).then(function (body) {
        if (resp.status === 401) throw new Error('会话已过期，请刷新页面后重新登录。');
        if (resp.status !== 200) {
          throw new Error(
            body && body.error && body.error.message
              ? body.error.message
              : '服务返回状态 ' + resp.status
          );
        }
        return body;
      });
    });
  }

  /// 未适配页面的统一占位（保留旧 SSR 入口，仅限非插件页）。
  function notReadyPage(title, text, oldHref, oldText) {
    var body = [{ type: 'alert', props: { type: 'warning', title: '该功能正在适配中', text: text } }];
    if (oldHref) {
      body.push({ type: 'link', props: { text: oldText || '打开旧版页面', href: prefix + oldHref } });
    }
    return { type: 'page', props: { title: title, subtitle: '该功能正在适配中' }, body: body };
  }

  /// 路由 Schema 加载：`skiff:<kind>` 由本脚本按当前地址栏与 `/ui/v1` 数据生成；
  /// 其它 URL 走普通 JSON 读取（保留给将来的远程 Schema）。
  function loadJson(url, info) {
    if (typeof url === 'string' && url.indexOf('skiff:') === 0) {
      return pageSchema(url.slice('skiff:'.length));
    }
    return fetch(url, { credentials: 'same-origin', headers: { Accept: 'application/json' } }).then(
      function (resp) {
        return readJson(resp);
      }
    );
  }

  // ---- 插件功能页：随包 Schema + RPC ----

  function pluginBase(id) {
    return prefix + '/plugins/' + encodeURIComponent(id);
  }

  /// 壳页唯一的插件动作通道：与旧 iframe 桥完全同一条 RPC 路径
  /// （CSRF 头、权限矩阵、逐次审计都由内核裁决）。
  function pluginRpcCall(id, method, params) {
    return fetch(pluginBase(id) + '/rpc', {
      method: 'POST',
      credentials: 'same-origin',
      headers: {
        Accept: 'application/json',
        'content-type': 'application/json',
        'x-csrf-token': (sessionState && sessionState.csrf_token) || ''
      },
      body: JSON.stringify({ method: method, params: params == null ? null : params })
    }).then(function (resp) {
      return readJson(resp).then(function (body) {
        return { status: resp.status, body: body };
      });
    })
      .then(function (res) {
        if (res.status !== 200) {
          throw new Error(errorMessage(res));
        }
        return res.body ? res.body.result : null;
      });
  }

  function pluginQuery() {
    var out = {};
    new URLSearchParams(location.search).forEach(function (value, key) {
      out[key] = value;
    });
    return out;
  }

  /// 绑定路径求值：`data.*` 取已取回的数据，`query.*` 取地址栏参数。
  function pluginGet(ctx, path) {
    var parts = String(path).split('.');
    var cur = parts[0] === 'data' ? ctx.data : parts[0] === 'query' ? ctx.query : null;
    for (var i = 1; i < parts.length && cur != null; i++) {
      cur = cur[parts[i]];
    }
    return cur;
  }

  var PLUGIN_BINDING = /^\{\{\s*([A-Za-z0-9_.-]+)\s*\}\}$/;
  var PLUGIN_INLINE = /\{\{\s*([A-Za-z0-9_.-]+)\s*\}\}/g;

  function pluginSubstitute(value, ctx) {
    var exact = PLUGIN_BINDING.exec(value);
    if (exact) {
      var bound = pluginGet(ctx, exact[1]);
      return bound == null ? null : bound;
    }
    return value.replace(PLUGIN_INLINE, function (_, path) {
      var v = pluginGet(ctx, path);
      return v == null ? '' : String(v);
    });
  }

  /// 递归解析：`when` 条件、`{{data.x}}` 绑定；不改动 budding 自己的
  /// `${row.field}` 行内插值（那是渲染期由组件处理的）。
  function pluginResolve(value, ctx) {
    if (Array.isArray(value)) {
      var arr = [];
      value.forEach(function (item) {
        var resolved = pluginResolve(item, ctx);
        if (resolved !== null && resolved !== undefined) arr.push(resolved);
      });
      return arr;
    }
    if (!value || typeof value !== 'object') return value;
    if (typeof value.when === 'string') {
      var raw = value.when.trim();
      var negate = raw.charAt(0) === '!';
      if (negate) raw = raw.slice(1).trim();
      var bound = PLUGIN_BINDING.exec(raw);
      var ok = bound ? !!pluginGet(ctx, bound[1]) : false;
      if (negate) ok = !ok;
      if (!ok) return null;
    }
    var out = {};
    Object.keys(value).forEach(function (key) {
      if (key === 'when') return;
      var v = value[key];
      if (typeof v === 'string') out[key] = pluginSubstitute(v, ctx);
      else if (v && typeof v === 'object') out[key] = pluginResolve(v, ctx);
      else out[key] = v;
    });
    return out;
  }

  function pluginMetaById(id) {
    var keys = Object.keys(pluginMeta);
    for (var i = 0; i < keys.length; i++) {
      if (pluginMeta[keys[i]].plugin_id === id) return pluginMeta[keys[i]];
    }
    return null;
  }

  /// 页内视图切换：budding 1.8.0 没有 page-tabs，先用按钮行承担 L3 入口。
  function pluginTabs(page, id, view) {
    var meta = pluginMetaById(id);
    if (!meta || !meta.views || meta.views.length < 2) return page;
    var tabs = meta.views.map(function (v) {
      var props = { text: v.label, variant: 'text' };
      if (v.view === view) props.color = 'primary';
      return {
        type: 'button',
        props: props,
        onEvent: {
          click: { actions: [{ type: 'skiffPluginView', props: { view: v.view } }] }
        }
      };
    });
    page.body = [
      { type: 'flex', props: { gap: 8, wrap: true }, content: { body: tabs } }
    ].concat(page.body || []);
    return page;
  }

  function pluginErrorPage(id, view, error) {
    var detail = error && error.message ? error.message : String(error);
    return {
      type: 'page',
      props: { title: '插件功能页', subtitle: id + ' / ' + view },
      body: [
        {
          type: 'alert',
          props: { type: 'error', title: '插件功能页加载失败', text: detail }
        },
        { type: 'link', props: { text: '返回插件页', href: prefix + '/plugins' } }
      ]
    };
  }

  function pluginPage(id, view) {
    if (!id || !view) {
      return Promise.resolve(
        pluginErrorPage(id || '未知插件', view || '未知视图', '地址栏缺少插件 id 或视图名')
      );
    }
    return fetch(pluginBase(id) + '/schema/' + encodeURIComponent(view), {
      credentials: 'same-origin',
      headers: { Accept: 'application/json' }
    })
      .then(function (resp) {
        return readJson(resp).then(function (body) {
          return { status: resp.status, body: body };
        });
      })
      .then(function (res) {
        if (res.status !== 200) {
          if (res.status === 403) throw new Error('当前角色无权访问该插件功能页');
          if (res.status === 404) throw new Error('该视图没有随包 schema（可能仍是旧 HTML 界面）');
          throw new Error(errorMessage(res));
        }
        var doc = res.body;
        if (!doc || !doc.page) throw new Error('schema 格式不正确');
        var ctx = { data: {}, query: pluginQuery() };
        var names = Object.keys(doc.data || {});
        return Promise.all(
          names.map(function (name) {
            var decl = doc.data[name] || {};
            var params = pluginResolve(decl.params || {}, ctx);
            return pluginRpcCall(id, decl.method, params).then(function (value) {
              return [name, value];
            });
          })
        ).then(function (pairs) {
          pairs.forEach(function (pair) {
            ctx.data[pair[0]] = pair[1];
          });
          return pluginTabs(pluginResolve(doc.page, ctx), id, view);
        });
      })
      .catch(function (error) {
        return pluginErrorPage(id, view, error);
      });
  }

  /// 页内视图切换（只允许跳到同一插件的视图）。
  /// 页内导航：`view` 之外的同级 props 与表单值都作为 query 参数。
  /// 用于「查看某 jail 的封禁」这类只读下钻；不写 URL、不跨插件。
  function pluginViewAction(args, ctx) {
    var parts = routeParts();
    var id = parts[0] === 'plugins' ? parts[1] : '';
    if (!id || !args || !args.view) return Promise.resolve();
    var params = {};
    var reserved = { view: 1, query: 1 };
    if (args.query && typeof args.query === 'object') {
      Object.keys(args.query).forEach(function (k) {
        if (!reserved[k]) params[k] = args.query[k];
      });
    }
    Object.keys(args).forEach(function (key) {
      if (!reserved[key]) params[key] = args[key];
    });
    var values = ctx ? formValues(ctx) : {};
    Object.keys(values).forEach(function (key) {
      var v = values[key];
      if (v !== null && v !== undefined && String(v).trim() !== '') params[key] = v;
    });
    var qs = new URLSearchParams();
    Object.keys(params).forEach(function (key) {
      var v = params[key];
      if (v !== null && v !== undefined && String(v).trim() !== '') qs.append(key, String(v));
    });
    var target = pluginBase(id) + '/' + encodeURIComponent(args.view);
    var text = qs.toString();
    if (text) target += '?' + text;
    if (location.pathname + location.search === target) location.reload();
    else location.assign(target);
    return Promise.resolve();
  }

  /// schema 声明的写操作：表单值 + 静态 params 合并后走统一 RPC 通道。
  function pluginRpcAction(args, ctx) {
    var parts = routeParts();
    var id = parts[0] === 'plugins' ? parts[1] : '';
    if (!id) {
      window.alert('无法定位当前插件');
      return Promise.resolve();
    }
    var method = args && args.method;
    if (!method) {
      window.alert('动作缺少 method');
      return Promise.resolve();
    }
    if (args.confirm && !window.confirm(String(args.confirm))) {
      return Promise.resolve();
    }
    var params = {};
    if (args.params && typeof args.params === 'object') {
      Object.keys(args.params).forEach(function (k) {
        params[k] = args.params[k];
      });
    }
    var reserved = { method: 1, params: 1, confirm: 1, reload: 1 };
    Object.keys(args).forEach(function (key) {
      if (!reserved[key]) params[key] = args[key];
    });
    // 表单字段优先于静态 params；空值等价于未填写，不随请求发送。
    var values = ctx ? formValues(ctx) : {};
    Object.keys(values).forEach(function (key) {
      var v = values[key];
      if (v !== null && v !== undefined && String(v).trim() !== '') params[key] = v;
    });
    return pluginRpcCall(id, method, params)
      .then(function () {
        location.reload();
      })
      .catch(function (error) {
        window.alert('操作失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  function pageSchema(kind) {
    var parts = routeParts();
    if (kind === 'overview') {
      return fetchData('/overview').then(function (data) {
        // 健康槽位不再走 iframe：从 overview 拿到提供者后，用统一 RPC 取数，
        // 失败（未启用/无权限）也只影响这一块，不拖垮整页。
        if (!data.health_plugin) {
          data.health = {
            error: data.health_reason || '未启用主机监控插件'
          };
          return overviewSchema(data);
        }
        return pluginRpcCall(data.health_plugin, 'snapshot', {})
          .then(function (rpc) {
            data.health = rpc && rpc.sample ? rpc.sample : null;
            if (!data.health) data.health = { error: '插件没有返回采样数据' };
            return overviewSchema(data);
          })
          .catch(function (error) {
            data.health = {
              error: error && error.message ? error.message : String(error)
            };
            return overviewSchema(data);
          });
      });
    }
    if (kind === 'sites') {
      // 列表本身由表格 `source` 游标取数；这里只读 `can_admin` 等页面级元数据。
      return fetchData('/sites?limit=1').then(sitesSchema);
    }
    if (kind === 'site-new') {
      return fetchData('/runtimes?limit=200')
        .then(function (data) {
          return siteNewSchema(data.items || []);
        })
        .catch(function () {
          return siteNewSchema([]);
        });
    }
    if (kind === 'site-detail') {
      return fetchData('/sites/' + encodeURIComponent(parts[1])).then(siteDetailSchema);
    }
    if (kind === 'runtimes') {
      return Promise.all([
        fetchRawJson(apiBase + '/runtimes/images'),
        fetchRawJson(apiBase + '/runtimes/catalog')
      ]).then(function (res) {
        return runtimesSchema((res[0] && res[0].data) || {}, (res[1] && res[1].data) || {});
      });
    }
    if (kind === 'runtime-detail') {
      var runtimeId = parts[1] || '';
      var fileKey = new URLSearchParams(location.search).get('file') || '';
      var detailUrl =
        '/runtimes/' +
        encodeURIComponent(runtimeId) +
        (fileKey ? '?file=' + encodeURIComponent(fileKey) : '');
      return fetchData(detailUrl).then(function (data) {
        return runtimeDetailSchema(data || {}, runtimeId);
      });
    }
    if (kind === 'containers') {
      var containerFilter =
        new URLSearchParams(location.search).get('filter') === 'error' ? 'error' : '';
      // 计数与列表必须同一筛选口径（接口的 total 已按 filter 计算）。
      var containerQuery = '/containers?limit=1' + (containerFilter ? '&filter=error' : '');
      return fetchData(containerQuery).then(function (data) {
        return containersSchema(data, containerFilter);
      });
    }
    if (kind === 'container-new') {
      return Promise.resolve(containerNewSchema());
    }
    if (kind === 'container-detail') {
      return fetchData('/containers/' + encodeURIComponent(parts[1])).then(containerDetailSchema);
    }
    if (kind === 'images') {
      return fetchData('/images?limit=1').then(imagesSchema);
    }
    if (kind === 'networks') {
      return fetchData('/networks?limit=1').then(networksSchema);
    }
    if (kind === 'volumes') {
      return fetchData('/volumes?limit=1').then(volumesSchema);
    }
    if (kind === 'compose') {
      return fetchData('/compose').then(composeSchema);
    }
    if (kind === 'tasks') {
      return fetchData('/tasks?limit=1').then(tasksSchema);
    }
    if (kind === 'task-detail') {
      return fetchData('/tasks/' + encodeURIComponent(parts[1])).then(taskDetailSchema);
    }
    if (kind === 'jobs') {
      return Promise.resolve(jobsSchema());
    }
    if (kind === 'job-detail') {
      return fetchData('/jobs/' + encodeURIComponent(parts[1])).then(jobDetailSchema);
    }
    if (kind === 'files-trash') {
      return fetchRawJson(prefix + '/files/trash/list').then(function (data) {
        return filesTrashSchema(data || {});
      });
    }
    if (kind === 'files-quotas') {
      return fetchRawJson(prefix + '/files/quotas/list').then(function (data) {
        return filesQuotasSchema(data || {});
      });
    }
    if (kind === 'files') {
      var filePath = new URLSearchParams(location.search).get('path') || '';
      return fetchData(
        '/files' + (filePath ? '?path=' + encodeURIComponent(filePath) : '')
      ).then(filesSchema);
    }
    if (kind === 'file-view') {
      var viewPath = new URLSearchParams(location.search).get('path') || '';
      if (!viewPath) {
        return Promise.resolve(
          notReadyPage('代码查看器', '缺少 path 参数。', '/files', '返回文件管理')
        );
      }
      return fetchData('/files/read?path=' + encodeURIComponent(viewPath)).then(fileViewSchema);
    }
    if (kind === 'site-logs') {
      var logDomain = parts[1] || '';
      var logQuery = new URLSearchParams(location.search);
      var logPart = logQuery.get('part') === 'error' ? 'error' : 'access';
      var logTail = logQuery.get('tail') || '200';
      var logKeyword = logQuery.get('q') || '';
      return fetchData(
        '/sites/' +
          encodeURIComponent(logDomain) +
          '/logs?part=' +
          logPart +
          '&tail=' +
          encodeURIComponent(logTail) +
          '&q=' +
          encodeURIComponent(logKeyword)
      ).then(function (data) {
        return siteLogsSchema(logDomain, logPart, logTail, logKeyword, data);
      });
    }
    if (kind === 'job-logs') {
      var logJobId = parts[1] || '';
      return loadJobLogs(logJobId).then(function (result) {
        return jobLogsSchema(logJobId, result);
      });
    }
    if (kind === 'container-logs') {
      var logContainerId = parts[1] || '';
      return fetchData(
        '/containers/' + encodeURIComponent(logContainerId) + '/logs?tail=500'
      ).then(
        function (data) {
          return containerLogsSchema(logContainerId, data);
        },
        function (error) {
          return logErrorPage(
            '容器日志',
            prefix + '/containers/' + encodeURIComponent(logContainerId),
            '返回容器详情',
            error
          );
        }
      );
    }
    if (kind === 'runtime-logs') {
      var logRuntimeId = parts[1] || '';
      return fetchData(
        '/runtimes/' + encodeURIComponent(logRuntimeId) + '/logs?tail=300'
      ).then(
        function (data) {
          return runtimeLogsSchema(logRuntimeId, data);
        },
        function (error) {
          return logErrorPage(
            '运行环境日志',
            prefix + '/runtimes/' + encodeURIComponent(logRuntimeId),
            '返回运行环境详情',
            error
          );
        }
      );
    }
    if (kind === 'container-terminal') {
      var terminalId = parts[1] || '';
      return loadTerminalUrl(terminalId).then(
        function (info) {
          return terminalSchema(terminalId, info);
        },
        function (error) {
          return {
            type: 'page',
            props: { title: '容器终端', subtitle: '打开失败' },
            body: [
              {
                type: 'alert',
                props: {
                  type: 'error',
                  text:
                    '无法打开终端：' +
                    (error && error.message ? error.message : String(error))
                }
              },
              {
                type: 'link',
                props: {
                  text: '返回容器详情',
                  href: prefix + '/containers/' + encodeURIComponent(terminalId)
                }
              }
            ]
          };
        }
      );
    }
    if (kind === 'plugins') {
      var pluginTab = parts[1] === 'discover' || parts[1] === 'updates' ? parts[1] : '';
      return fetchData('/plugins').then(function (data) {
        return pluginsSchema(data, pluginTab);
      });
    }
    if (kind === 'plugin-info') {
      return fetchData('/plugins/' + encodeURIComponent(parts[1])).then(pluginInfoSchema);
    }
    if (kind === 'plugin-view') {
      return pluginPage(parts[1] || '', parts[2] || '');
    }
    if (kind === 'store') {
      var refresh = new URLSearchParams(location.search).get('refresh') === '1' ? '?refresh=1' : '';
      return fetchData('/store' + refresh).then(storeSchema);
    }
    if (kind === 'store-confirm') {
      if (!pendingConfirm) {
        return Promise.resolve({
          type: 'page',
          props: { title: '安装确认', subtitle: '安装确认已失效' },
          body: [
            {
              type: 'alert',
              props: {
                type: 'warning',
                title: '安装确认已失效',
                text: '页面刷新或超时后暂存的安装信息已清空，请回到商店重新选择版本。'
              }
            },
            {
              type: 'link',
              props: { text: '返回商店', href: prefix + '/store' }
            }
          ]
        });
      }
      return Promise.resolve(storeConfirmSchema(pendingConfirm));
    }
    if (kind === 'web') {
      var webQuery = new URLSearchParams(location.search);
      var base = fetchData('/web');
      if (webQuery.get('logs') === '1') {
        var logParams = new URLSearchParams();
        logParams.set('load', '1');
        logParams.set('kind', webQuery.get('type') || 'access');
        logParams.set('lines', webQuery.get('lines') || '200');
        logParams.set('level', webQuery.get('level') || 'all');
        if (webQuery.get('status')) logParams.set('status', webQuery.get('status'));
        if (webQuery.get('q')) logParams.set('q', webQuery.get('q'));
        return Promise.all([base, fetchData('/web/logs?' + logParams.toString())]).then(
          function (res) {
            return webSchema(res[0], webQuery, res[1]);
          }
        );
      }
      return base.then(function (data) {
        return webSchema(data, webQuery, null);
      });
    }
    if (kind === 'settings') {
      return fetchData('/settings').then(settingsSchema);
    }
    if (kind === 'menu-settings') {
      return fetchData('/settings/menu').then(menuSettingsSchema);
    }
    if (kind === 'docker-settings') {
      return fetchData('/settings/docker').then(dockerSettingsSchema);
    }
    if (kind === 'audit') {
      var auditParams = new URLSearchParams(location.search);
      var auditQ = auditParams.get('q') || '';
      var auditCursor = auditParams.get('cursor') || '';
      var auditQuery =
        '?limit=50' +
        (auditQ ? '&q=' + encodeURIComponent(auditQ) : '') +
        (auditCursor ? '&cursor=' + encodeURIComponent(auditCursor) : '');
      return fetchData('/audit' + auditQuery).then(auditSchema);
    }
    return Promise.resolve(
      notReadyPage('页面不存在', '地址栏路径没有对应的前端路由，请从左侧菜单进入。')
    );
  }

  /// 同 path 仅 query 变化的页内跳转：SDK 的页面加载键是 `path#name` + `pageVersion`，
  /// 只换 query 不会重新拉取页面 schema。这里走宿主路由（history push 同步生效）
  /// 后显式 refreshPage，触发且只触发一次内容重载。
  function queryNav(args, ctx, runtime) {
    var path = (args && args.path) || '';
    var query = (args && args.query) || {};
    if (!path) return Promise.resolve();
    var search = new URLSearchParams(query).toString();
    var target = path + (search ? '?' + search : '');
    if (!runtime || typeof runtime.navigateTo !== 'function') {
      window.location.assign(target);
      return Promise.resolve();
    }
    // 已在目标地址时不再 push，避免重复历史项；内容仍刷新一次。
    if (window.location.pathname + window.location.search !== target) {
      runtime.navigateTo({ path: path, query: query });
    }
    if (typeof runtime.refreshPage === 'function') {
      runtime.refreshPage();
    } else {
      window.location.reload();
    }
    return Promise.resolve();
  }

  /// 浏览器前进/后退回到「同 path、仅 query 不同」的地址时，SDK 不会重载页面内容，
  /// 这里整页刷新兜底；跨页前进/后退仍交给宿主路由，避免双次渲染与状态丢失。
  function watchQueryOnlyHistory() {
    var renderedPath = window.location.pathname;
    ['pushState', 'replaceState'].forEach(function (name) {
      var original = window.history[name];
      if (typeof original !== 'function') return;
      window.history[name] = function () {
        var out = original.apply(this, arguments);
        renderedPath = window.location.pathname;
        return out;
      };
    });
    window.addEventListener('popstate', function () {
      if (window.location.pathname === renderedPath) {
        window.location.reload();
      } else {
        renderedPath = window.location.pathname;
      }
    });
  }

  function boot() {
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
          showError(
            res.body && res.body.error && res.body.error.message
              ? res.body.error.message
              : '服务返回状态 ' + res.status
          );
          return;
        }
        if (!window.BuddingSDK || typeof window.BuddingSDK.embed !== 'function') {
          showError('前端产物未加载（budding 脚本不可用）。');
          return;
        }
        sessionState = res.body.data;
        return fetchData('/menu').then(function (menu) {
          embed(shellSchema(sessionState), buildRoutes(menu));
        });
      })
      .catch(function (error) {
        showError('无法连接服务：' + (error && error.message ? error.message : String(error)));
      });
  }

  watchQueryOnlyHistory();
  boot();
})();

