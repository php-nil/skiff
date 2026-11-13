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
  /// 1.8.0 的图标字体只是组件自用子集，没有完整的菜单图标；这里只映射确有的几个，
  /// 其余不传 icon（宁可不画，也不能让缺失字形回退成 "mdi-xxx" 文本）。
  var MENU_ICONS = {
    clock: 'mdi-clock-outline',
    file: 'mdi-file-document',
    dot: 'mdi-circle'
  };

  function iconFor(name) {
    return MENU_ICONS[name];
  }

  /// 已迁移到 budding 的菜单项：稳定键 → 路由路径（相对入口前缀）。
  /// 未列出的键保留内核深链 `href`（插件功能页仍是内核 SSR + iframe）。
  var BUILTIN_PAGES = {
    overview: { path: '', schema: 'skiff:overview' },
    'core/sites/list': { path: '/sites', schema: 'skiff:sites' },
    'core/sites/env': { path: '/runtimes', schema: 'skiff:runtimes' },
    'core/docker/containers': { path: '/containers', schema: 'skiff:containers' },
    'core/docker/images': { path: '/images', schema: 'skiff:images' },
    'core/docker/networks': { path: '/networks', schema: 'skiff:networks' },
    'core/docker/volumes': { path: '/volumes', schema: 'skiff:volumes' },
    'core/docker/compose': { path: '/compose', schema: 'skiff:compose' },
    'core/plugins/installed': { path: '/plugins', schema: 'skiff:plugins' },
    'core/plugins/store': { path: '/store', schema: 'skiff:store' },
    'core/tasks': { path: '/tasks', schema: 'skiff:tasks' },
    'core/jobs': { path: '/jobs', schema: 'skiff:jobs' },
    'core/files': { path: '/files', schema: 'skiff:files' },
    'core/settings/security': { path: '/settings', schema: 'skiff:settings' },
    'core/settings/audit': { path: '/audit', schema: 'skiff:audit' }
  };

  /// 一个菜单项 → 一条路由记录。命中已迁移页面时给出 budding 路径与 Schema；
  /// 插件自带 HTML/iframe 的入口一律不进路由（新一代前端不承载 iframe 功能页）。
  function menuRoute(key, label, icon, href) {
    if (key.indexOf('plugin/') === 0) return null;
    var page = BUILTIN_PAGES[key];
    if (page) {
      return {
        path: page.path ? prefix + page.path : prefix,
        title: label,
        icon: iconFor(icon),
        schema: page.schema
      };
    }
    if (!href) return null;
    return {
      path: prefix + '/__menu-item/' + encodeURIComponent(key),
      href: href,
      target: '_self',
      title: label,
      icon: iconFor(icon)
    };
  }

  /// 内核菜单树 → budding 路由表（菜单是路由的投影，不另造第二份菜单）。
  function buildRoutes(menuData) {
    var routes = [];
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
      routes.push({
        path: prefix + '/__menu/' + encodeURIComponent(group.key),
        title: group.label,
        icon: iconFor(group.icon),
        meta: { order: order },
        children: children
      });
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
    // 插件功能页：旧 SSR 仍是内核 iframe 槽位，但不是新一代前端的入口；
    // 直达时给统一占位，等插件 UI 以 schema 形式交付后再替换。
    routes.push({
      path: prefix + '/plugins/:id/:view',
      title: '插件功能页',
      hidden: true,
      schema: 'skiff:plugin-view',
      meta: { activeMenu: prefix + '/plugins' }
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
        skiffSiteWrite: siteWrite,
        skiffTaskWrite: taskWrite,
        skiffJobWrite: jobWrite,
        skiffTrashWrite: trashWrite,
        skiffQuotaWrite: quotaWrite,
        skiffComposeAction: composeAction,
        skiffComposeEdit: composeEdit,
        skiffSettingsSave: settingsSave,
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
        skiffPluginToggle: pluginToggle,
        skiffPluginDelete: pluginDelete,
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


  /// 独立 Pack：code-viewer 的语法高亮由 @budding/highlight 提供。
  /// 未注册或加载失败时组件按纯文本工作，不影响只读查看。
  function registerPacks() {
    if (!buddingVersion || !window.BuddingSDK) return;
    if (typeof window.BuddingSDK.registerPack !== 'function') return;
    try {
      window.BuddingSDK.registerPack(
        'highlight',
        prefix +
          '/static/budding/budding-highlight-' +
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
          { type: 'code-viewer', props: { code: data.config.content, language: 'nginx' } }
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

  function runtimesSchema(data) {    var body = [];
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
    var table = statusTable(
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
    );
    table.onEvent = {
      rowClick: {
        actions: [{ type: 'navigate', props: { url: prefix + '/runtimes/${id}' } }]
      }
    };
    body.push(table);
    body.push({
      type: 'alert',
      props: {
        type: 'info',
        text: '运行日志在实例详情页内原生化查看；安装、配置与扩展仍走旧版运行环境页。'
      }
    });
    body.push({
      type: 'link',
      props: { text: '打开旧版运行环境管理（安装 / 配置 / 扩展）', href: prefix + '/m/core/sites/env' }
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

  function resourceSchema(title, subtitle, columns, data, extra) {
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
    if (extra && extra.length > 0) body = body.concat(extra);
    return { type: 'page', props: { title: title, subtitle: subtitle }, body: body };
  }

  /// 逐项删除卡片：每行一个按钮，动作走同一个 `skiffDockerWrite`。
  function deleteCard(kind, items, title) {
    return {
      type: 'card',
      props: { title: title },
      content: {
        body: items.map(function (item) {
          var display = item.name || item.id;
          return {
            type: 'flex',
            props: { gap: 8 },
            content: {
              body: [
                textLine(display),
                {
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
                            id: item.id,
                            name: display
                          }
                        }
                      ]
                    }
                  }
                }
              ]
            }
          };
        })
      }
    };
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
      if (data.items.length > 0) extra.push(deleteCard('image', data.items, '删除镜像'));
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
      extra
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
      var removable = data.items.filter(function (item) {
        return item.can_remove;
      });
      if (removable.length > 0) extra.push(deleteCard('network', removable, '删除网络'));
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
      extra
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
      if (data.items.length > 0) extra.push(deleteCard('volume', data.items, '删除卷'));
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
      extra
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
      'name'
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
            'started_text'
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
                  type: 'code-viewer',
                  props: {
                    code: logLines.join('\n'),
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
        'key'
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
      type: 'code-viewer',
      props: {
        code: binary ? '' : text,
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
          ' · 只读 · 高亮由 @budding/highlight Pack 在前端完成',
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
        type: 'code-viewer',
        props: {
          code: lines.join('\n'),
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
        type: 'code-viewer',
        props: {
          code: lines.join('\n'),
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
        type: 'code-viewer',
        props: {
          code: lines.join('\n'),
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
        subtitle: '尾部快照；需要跟随时请用旧版详情页'
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
        type: 'code-viewer',
        props: {
          code: lines.join('\n'),
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

  function pluginsSchema(data) {
    var tab = new URLSearchParams(location.search).get('tab') || 'installed';
    var body = [];
    body.push({
      type: 'flex',
      props: { gap: 12 },
      content: {
        body: [
          { type: 'link', props: { text: '已安装 (' + data.plugins.length + ')', href: prefix + '/plugins' } },
          { type: 'link', props: { text: '发现', href: prefix + '/plugins?tab=discover' } },
          { type: 'link', props: { text: '可更新', href: prefix + '/plugins?tab=updates' } }
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
          title: '插件功能页正在切换为原生渲染',
          text: '上述功能页由插件自带 HTML 渲染（旧版以 iframe 承载），新一代前端不承载 iframe，因此不再提供跳转入口；功能将以 Schema 形式交付后在此原生渲染。'
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

  function pageSchema(kind) {
    var parts = routeParts();
    if (kind === 'overview') {
      return fetchData('/overview').then(overviewSchema);
    }
    if (kind === 'sites') {
      return fetchData('/sites' + listQuery()).then(sitesSchema);
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
      return fetchData('/runtimes' + listQuery()).then(runtimesSchema);
    }
    if (kind === 'runtime-detail') {
      return Promise.resolve({
        type: 'page',
        props: { title: '运行环境详情', subtitle: '运行日志已原生化；配置仍走旧版页面' },
        body: [
          {
            type: 'alert',
            props: {
              type: 'info',
              text: '运行环境详情的聚合查询尚未迁移，配置、安装与扩展仍由旧版页面承载；运行日志可在这里原生化查看。'
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
                    text: '查看运行日志',
                    href: prefix + '/runtimes/' + encodeURIComponent(parts[1] || '') + '/logs'
                  }
                },
                {
                  type: 'link',
                  props: {
                    text: '打开旧版运行环境页（配置 / 安装 / 扩展）',
                    href: prefix + '/m/core/sites/env'
                  }
                }
              ]
            }
          }
        ]
      });
    }
    if (kind === 'containers') {
      var containerFilter =
        new URLSearchParams(location.search).get('filter') === 'error' ? 'filter=error' : '';
      return fetchData('/containers' + listQuery(containerFilter)).then(containersSchema);
    }
    if (kind === 'container-new') {
      return Promise.resolve(containerNewSchema());
    }
    if (kind === 'container-detail') {
      return fetchData('/containers/' + encodeURIComponent(parts[1])).then(containerDetailSchema);
    }
    if (kind === 'images') {
      return fetchData('/images' + listQuery()).then(imagesSchema);
    }
    if (kind === 'networks') {
      return fetchData('/networks' + listQuery()).then(networksSchema);
    }
    if (kind === 'volumes') {
      return fetchData('/volumes' + listQuery()).then(volumesSchema);
    }
    if (kind === 'compose') {
      return fetchData('/compose').then(composeSchema);
    }
    if (kind === 'tasks') {
      return fetchData('/tasks' + listQuery()).then(tasksSchema);
    }
    if (kind === 'task-detail') {
      return fetchData('/tasks/' + encodeURIComponent(parts[1])).then(taskDetailSchema);
    }
    if (kind === 'jobs') {
      return fetchData('/jobs' + listQuery()).then(jobsSchema);
    }
    if (kind === 'job-detail') {
      return fetchData('/jobs/' + encodeURIComponent(parts[1])).then(jobDetailSchema);
    }
    if (kind === 'files-trash') {
      return fetchRawJson(prefix + '/files/api/trash/list').then(function (data) {
        return filesTrashSchema(data || {});
      });
    }
    if (kind === 'files-quotas') {
      return fetchRawJson(prefix + '/files/api/quotas/list').then(function (data) {
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
      return fetchData('/plugins').then(pluginsSchema);
    }
    if (kind === 'plugin-info') {
      return fetchData('/plugins/' + encodeURIComponent(parts[1])).then(pluginInfoSchema);
    }
    if (kind === 'plugin-view') {
      return Promise.resolve({
        type: 'page',
        props: { title: '插件功能页', subtitle: '插件功能页正在切换为原生渲染' },
        body: [
          {
            type: 'alert',
            props: {
              type: 'info',
              title: '插件功能页正在切换为原生渲染',
              text:
                '该入口属于插件自带的 HTML 界面（旧 SSR 以 iframe 承载），新一代前端不再承载 iframe。插件功能将以 Schema 形式交付后在此原生渲染；当前请使用插件页的列表、详情、启停与卸载能力。'
            }
          },
          {
            type: 'link',
            props: { text: '返回插件页', href: prefix + '/plugins' }
          }
        ]
      });
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
    if (kind === 'settings') {
      return fetchData('/settings').then(settingsSchema);
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

  boot();
})();

