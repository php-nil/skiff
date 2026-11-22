/**
 * budding 壳页启动器（Skiff 侧静态资源，非上游产物）。
 *
 * 为什么单独一个 script：全局 CSP 是 `script-src 'self'`，不能内联；而 budding 的
 * CSS 目录靠 `document.currentScript` 推导，所以产物只能用普通 `<script src>` 加载
 * （不能内联、不能 import()）。
 *
 * 链路：GET /ui/v1/session → 入口根再取 GET /ui/v1/overview 渲染只读概览；
 * 其余页面仍 embed 占位卡片。失败显示错误而不是白屏。
 * 未认证由服务端 303 到登录页（页面请求既有行为）；运行期 401（会话过期）直接
 * `location.replace` 硬跳登录页，见 `showSessionExpired`。
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

  // 安全 escape：把裸 "/" 替换成全角斜杠 ／ (U+FF0F)。
  // 根因：budding 表达式引擎在解析 modelValue 字符串时，遇到裸 "/" 会误认为
  // 正则字面量起始符，若后续没有闭合 "/" 即抛 "Unterminated regular expression"
  // 并导致整个 code 组件渲染崩溃（SKIFF_BASE%/ 这类 bash 参数展开片段就是触发器）。
  // 替换规则：被非 "/" 字符夹住的 "/" → 全角；连续 "//"（如 http://）保留。
  // 已知副作用：全角斜杠视觉近似但不可作为 shell/path 使用；code 组件 copy 按钮
  // 返回的也是 escape 后的文本，后续可按需在 copyable handler 里反替换。
  function buddySafeString(text) {
    if (text == null) return text;
    if (typeof text !== 'string') return text;
    return text
      .replace(/([^\/])\/([^\/])/g, '$1／$2')
      .replace(/^\/|\/$/gm, '／');
  }

  // 服务端下发的 Schema 也要过同一层安全转义：`code` 节点的 modelValue 若带裸 "/"，
  // budding 表达式引擎会当成正则起始符并整棵树编译失败。前端构建器与这里共用同一规则。
  function prepareServerSchema(node) {
    if (!node || typeof node !== 'object') return node;
    if (Array.isArray(node)) {
      node.forEach(prepareServerSchema);
      return node;
    }
    if (node.type === 'code' && node.props && typeof node.props.modelValue === 'string') {
      node.props.modelValue = buddySafeString(node.props.modelValue);
    }
    if (node.type === 'button' && node.action && node.action.type === 'dialog') {
      registerServerDialog(node);
    }
    Object.keys(node).forEach(function (key) {
      prepareServerSchema(node[key]);
    });
    return node;
  }

  /// 取服务端产出的页面骨架：`body.schema` 过安全转义后交给 SDK。
  /// 数据仍由页面内的 `source` 自取，这里只负责「拿到布局」。
  function fetchServerSchema(path) {
    return fetchRawJson(apiBase + path).then(function (body) {
      if (!body || !body.schema) throw new Error('服务端未返回页面结构');
      return prepareServerSchema(body.schema);
    });
  }

  // 服务端弹窗按钮同样要进弹窗注册表：提交失败时 dialogError 按 key 重开同一张
  // 表单并保留用户输入，行为与前端 dialogFormButton 一致。
  //
  // 插槽位置：budding 1.8.3 起 dialog 的正文/底部在动作顶层 `content`
  // （写进 `props.body` 会整页 CompileError），故只认 `content.body`。
  function registerServerDialog(button) {
    var layer = button.action.props || {};
    var slot = button.action.content || {};
    var body = slot.body || [];
    if (!Array.isArray(body) || body.length === 0) return;
    var form = null;
    for (var i = 0; i < body.length; i += 1) {
      if (body[i] && body[i].type === 'form') {
        form = body[i];
        break;
      }
    }
    if (!form || !form.props || !form.props.api) return;
    var submit = form.onEvent && form.onEvent.submit && form.onEvent.submit[0];
    if (!submit || !submit.props) return;
    var key = submit.props.dialog || newDialogKey();
    submit.props.dialog = key;
    var submitButton = form.content && form.content.actions && form.content.actions[0];
    dialogRegistry[key] = {
      title: layer.title,
      width: layer.width,
      url: form.props.api.url,
      action: submit,
      fields: form.body,
      submitText: submitButton && submitButton.props ? submitButton.props.text : ''
    };
  }


  function textLine(text) {
    return { type: 'text', props: { text: text } };
  }


  var sessionState = null;

  function errorMessage(res) {
    return res.body && res.body.error && res.body.error.message
      ? res.body.error.message
      : '服务返回状态 ' + res.status;
  }

  // ---- 宿主桥：运行时实例、确认框与轻提示 --------------------------------
  // 为什么需要这一层：SDK 在 `platform.confirm` 缺省时退化为浏览器原生
  // `window.confirm`；而 `embed()` 只回 `{manifest, unmount}`，拿不到实例。
  // 实例在 `install()` 时同时写进 Vue app 的 `globalProperties.$budding` 与
  // provides，这里从挂载节点反查，再用实例自己的图层（和 `dialog` 动作同一渲染
  // 栈）呈现确认框——不手写遮罩、不复制弹窗实现。
  var runtimeRef = null;

  function buddingRuntime() {
    if (runtimeRef) return runtimeRef;
    var app = mount.__vue_app__;
    if (!app) return null;
    var props = app.config && app.config.globalProperties;
    if (props && props.$budding) {
      runtimeRef = props.$budding;
      return runtimeRef;
    }
    var provides = app._context && app._context.provides;
    if (provides && typeof Reflect !== 'undefined' && Reflect.ownKeys) {
      // provides 的键可能是 Symbol，`Object.keys` 枚举不到。
      var keys = Reflect.ownKeys(provides);
      for (var i = 0; i < keys.length; i++) {
        var value = provides[keys[i]];
        if (value && typeof value.openLayer === 'function' && typeof value.toast === 'function') {
          runtimeRef = value;
          return runtimeRef;
        }
      }
    }
    return null;
  }

  /// 轻提示：运行时不可用（embed 尚未完成）时只写控制台，不阻塞页面。
  ///
  /// 写操作普遍是「toast 后整页刷新」，刷新会立刻销毁提示节点，所以成功/失败
  /// 类提示同时写入 sessionStorage，由新页面消费一次（`consumeFlashToast`）。
  function toast(message, level) {
    var text = String(message === undefined || message === null ? '' : message);
    if (!text) return;
    var lv = level || 'info';
    var runtime = buddingRuntime();
    if (runtime && typeof runtime.toast === 'function') {
      runtime.toast(text, lv);
    } else if (window.console && console.warn) {
      console.warn('[skiff] ' + text);
    }
    if (lv === 'success' || lv === 'warning' || lv === 'error') {
      try {
        sessionStorage.setItem('skiff.flash', JSON.stringify({ message: text, level: lv, t: Date.now() }));
      } catch (e) {
        /* 隐私模式禁用 storage 时只损失跨刷新的提示 */
      }
    }
  }

  /// 新页面消费一次跨刷新的提示；超过 10 秒视为过期，避免翻旧账。
  function consumeFlashToast() {
    var raw = null;
    try {
      raw = sessionStorage.getItem('skiff.flash');
      sessionStorage.removeItem('skiff.flash');
    } catch (e) {
      return;
    }
    if (!raw) return;
    try {
      var flash = JSON.parse(raw);
      if (!flash || !flash.message || Date.now() - Number(flash.t || 0) > 10000) return;
      var runtime = buddingRuntime();
      if (runtime && typeof runtime.toast === 'function') {
        runtime.toast(String(flash.message), flash.level || 'info');
      }
    } catch (e) {
      /* 损坏的缓存直接丢弃 */
    }
  }

  /// 命令式确认：直接复用 budding 运行时 `confirm`（内建图层，Esc 取消）。
  ///
  /// 自定义写操作在 JS 里需要 `Promise<boolean>`，而 Schema 动作链用内置
  /// `confirm` 动作；这里只做参数映射，不再自建图层与按钮动作。
  function hostConfirm(args) {
    var title = (args && args.title) || '确认操作';
    var content =
      args && args.content !== undefined && args.content !== null ? String(args.content) : '';
    var runtime = buddingRuntime();
    if (runtime && typeof runtime.confirm === 'function') {
      var payload = { title: title };
      if (content) payload.content = content;
      if (args && args.level) payload.level = args.level;
      else if (args && args.danger) payload.level = 'error';
      if (args && args.confirmText) payload.confirmText = args.confirmText;
      if (args && args.cancelText) payload.cancelText = args.cancelText;
      if (args && args.requireText) payload.requireText = args.requireText;
      return Promise.resolve(runtime.confirm(payload));
    }
    return Promise.resolve(window.confirm(content ? title + '\n\n' + content : title));
  }

  // 容器启停/重启/删除/强制终止：走 /ui/v1 同策略、同审计的 JSON 端点。
  function containerAction(args) {
    var id = args.id;
    var action = args.action;
    var name = args.name || id;
    var guard = Promise.resolve(true);
    if (action === 'delete') {
      guard = hostConfirm({
        title: '删除容器',
        content: '确认删除容器 ' + name + '？删除后不可恢复，容器内未持久化的数据会丢失。',
        danger: true
      });
    } else if (action === 'kill') {
      guard = hostConfirm({
        title: '强制终止容器',
        content: 'kill 会直接终止 ' + name + ' 内的进程，不等待优雅退出，未落盘的数据可能丢失。',
        danger: true
      });
    }
    return guard.then(function (ok) {
      if (!ok) return;
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
            toast(errorMessage(res), 'error');
            return;
          }
          toast(action === 'delete' ? '容器已删除' : '容器操作已提交', 'success');
          if (action === 'delete') location.href = prefix + '/docker/containers';
          else location.reload();
        })
        .catch(function (error) {
          toast('操作失败：' + (error && error.message ? error.message : String(error)), 'error');
        });
    });
  }

  /// 列表行内启停：状态来自当前行，停止/启动共用同一容器动作通道。
  function containerToggle(args) {
    var running = String(args.status || '') === 'running';
    return containerAction({
      id: args.id,
      name: args.name,
      action: running ? 'stop' : 'start'
    });
  }


  /// 定时任务写操作：切换启停、立即运行、删除（内置任务由后端拒绝删除）。
  function taskWrite(args) {
    var op = args.op;
    var guard =
      op === 'delete'
        ? hostConfirm({
            title: '删除任务',
            content: '确认删除任务 ' + (args.name || args.id) + '？',
            danger: true
          })
        : Promise.resolve(true);
    var payload = {};
    var path = '/tasks/' + encodeURIComponent(args.id) + '/' + op;
    if (op === 'toggle') {
      var current = args.enabled === true || args.enabled === 'true';
      payload = { enabled: !current };
    }
    return guard
      .then(function (ok) {
        if (!ok) return;
        return submitJson(path, payload).then(function () {
          toast(op === 'delete' ? '任务已删除' : '任务已更新', 'success');
          location.reload();
        });
      })
      .catch(function (error) {
        toast('操作失败：' + (error && error.message ? error.message : String(error)), 'error');
      });
  }

  /// 作业写操作：请求取消、重试；重试成功后跳到新作业详情。
  function jobWrite(args) {
    var op = args.op;
    var guard =
      op === 'cancel'
        ? hostConfirm({
            title: '取消作业',
            content: '确认请求取消该作业？已在执行中的步骤可能仍需等待收尾。'
          })
        : Promise.resolve(true);
    return guard
      .then(function (ok) {
        if (!ok) return;
        return submitJson('/jobs/' + encodeURIComponent(args.id) + '/' + op, {}).then(function (data) {
          if (op === 'retry' && data.id) {
            location.href = prefix + '/jobs/' + encodeURIComponent(data.id);
            return;
          }
          toast((data && data.message) || '操作已提交', 'success');
          location.reload();
        });
      })
      .catch(function (error) {
        toast('操作失败：' + (error && error.message ? error.message : String(error)), 'error');
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
      return hostConfirm({
        title: '清空回收站',
        content: '将永久删除全部条目，且不会进入任何备份。确认清空？',
        danger: true
      }).then(function (ok) {
        if (!ok) return;
        return postPrefixJson(base + 'clear', {})
          .then(function (data) {
            var failed = (data.result && data.result.failed) || [];
            toast(
              failed.length
                ? '已清理部分条目，' + failed.length + ' 项失败：' + failed.join('；')
                : '已清空回收站',
              failed.length ? 'warning' : 'success'
            );
            location.reload();
          })
          .catch(function (error) {
            toast('清空失败：' + (error && error.message ? error.message : String(error)), 'error');
          });
      });
    }
    if (op === 'purge') {
      return hostConfirm({
        title: '彻底删除',
        content: '确认彻底删除 ' + (args.path || '该条目') + '？不可恢复。',
        danger: true
      }).then(function (ok) {
        if (!ok) return;
        return postPrefixJson(base + 'purge', { id: args.id })
          .then(function () {
            toast('已彻底删除', 'success');
            location.reload();
          })
          .catch(function (error) {
            toast(
              '彻底删除失败：' + (error && error.message ? error.message : String(error)),
              'error'
            );
          });
      });
    }
    if (op === 'restore') {
      return postPrefixJson(base + 'restore', { id: args.id, overwrite: false })
        .then(function () {
          toast('已恢复', 'success');
          location.reload();
        })
        .catch(function (error) {
          var message = error && error.message ? error.message : String(error);
          return hostConfirm({
            title: '覆盖后重试',
            content: '恢复失败：' + message + '\n\n是否覆盖现有对象后重试？',
            danger: true
          }).then(function (ok) {
            if (!ok) {
              toast('已取消恢复', 'info');
              return;
            }
            return postPrefixJson(base + 'restore', { id: args.id, overwrite: true })
              .then(function () {
                toast('已覆盖恢复', 'success');
                location.reload();
              })
              .catch(function (retryError) {
                toast(
                  '恢复失败：' +
                    (retryError && retryError.message ? retryError.message : String(retryError)),
                  'error'
                );
              });
          });
        });
    }
    toast('未知的回收站操作：' + op, 'error');
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
      if (!path || path.charAt(0) !== '/') {
        toast('请填写目录树的绝对路径。', 'warning');
        return Promise.resolve();
      }
      if (!bytes && !files) {
        toast('容量上限与文件数上限至少设置一项；如需取消请用行内「取消配额」。', 'warning');
        return Promise.resolve();
      }
      return postPrefixJson(base + 'set', {
        path: path,
        max_bytes: bytes ? Number(bytes) : null,
        max_files: files ? Number(files) : null
      })
        .then(function () {
          toast('已保存配额：' + path, 'success');
          location.reload();
        })
        .catch(function (error) {
          toast('保存配额失败：' + (error && error.message ? error.message : String(error)), 'error');
        });
    }
    if (op === 'scan') {
      return hostConfirm({
        title: '重新统计',
        content: '重新统计会递归扫描整棵目录树，可能耗时较久。\n\n' + path
      }).then(function (ok) {
        if (!ok) return;
        return postPrefixJson(base + 'scan', { path: path })
          .then(function () {
            toast('统计完成：' + path, 'success');
            location.reload();
          })
          .catch(function (error) {
            toast(
              '重新统计失败：' + (error && error.message ? error.message : String(error)),
              'error'
            );
          });
      });
    }
    if (op === 'cancel') {
      return hostConfirm({
        title: '取消配额',
        content: '确认取消该目录的配额？\n\n' + path,
        danger: true
      }).then(function (ok) {
        if (!ok) return;
        return postPrefixJson(base + 'cancel', { path: path })
          .then(function () {
            toast('已取消配额', 'success');
            location.reload();
          })
          .catch(function (error) {
            toast('取消配额失败：' + (error && error.message ? error.message : String(error)), 'error');
          });
      });
    }
    toast('未知的配额操作：' + op, 'error');
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

  /// 容器写入口：新建与资源限制。
  ///
  /// 镜像 / 网络 / 卷的增删已改为页面 JSON 里的 `confirm` + `ajax` 声明，
  /// 不再经过这里——需要命令式拼 payload 的只剩容器这两个动作。
  function dockerWrite(args, ctx) {
    var values = formValues(ctx);
    var op = args.op;
    var path = '';
    var payload = {};
    if (op === 'create') {
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
    } else if (op === 'limits') {
      path = '/containers/' + encodeURIComponent(args.id) + '/limits';
      payload = {
        cpu_shares: formText(values.cpu_shares),
        cpu_period: formText(values.cpu_period),
        cpu_quota: formText(values.cpu_quota),
        memory_limit_mb: formText(values.memory_limit_mb),
        memory_swap_mb: formText(values.memory_swap_mb),
        pids_limit: formText(values.pids_limit)
      };
    } else {
      toast('未知的容器操作：' + op, 'error');
      return Promise.resolve();
    }
    return submitJson(path, payload)
      .then(function (data) {
        if (op === 'create' && data.id) {
          location.href = prefix + '/docker/containers/' + encodeURIComponent(data.id);
          return;
        }
        closeTopLayer();
        toast('操作已完成', 'success');
        location.reload();
      })
      .catch(function (error) {
        toast('操作失败：' + (error && error.message ? error.message : String(error)), 'error');
      });
  }

  /// 站点写操作统一入口：建站、启停、默认、域名、配置、证书、备份、删除、批量。
  /// 权限、CSRF 与审计全部由 `/ui/v1` 复用既有站点命令。
  function siteWrite(args, ctx) {
    var values = formValues(ctx);
    var op = args.op;
    var id = args.id;
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
      path = '/sites/' + encodeURIComponent(id) + '/' + args.action;
    } else if (op === 'default') {
      path = '/sites/' + encodeURIComponent(id) + '/default';
      payload = { enabled: !!args.enabled };
    } else if (op === 'domains') {
      path = '/sites/' + encodeURIComponent(id) + '/domains';
      payload = { action: args.action, alias: args.alias || formText(values.alias) };
    } else if (op === 'config') {
      path = '/sites/' + encodeURIComponent(id) + '/config';
      payload = { content: formText(values.content) };
    } else if (op === 'ssl') {
      path = '/sites/' + encodeURIComponent(id) + '/ssl';
      payload = {
        action: args.action,
        enabled: !!args.enabled,
        cert_pem: formText(values.cert_pem),
        key_pem: formText(values.key_pem)
      };
    } else if (op === 'policy') {
      path = '/sites/' + encodeURIComponent(id) + '/policy';
      payload = {
        rewrite: formText(values.rewrite),
        limit_rate_kbps: Number(formText(values.limit_rate_kbps) || 0)
      };
    } else if (op === 'backup') {
      path = '/sites/' + encodeURIComponent(id) + '/backup';
      payload = {
        enabled: args.enabled === undefined ? !!values.enabled : !!args.enabled,
        interval: formText(values.interval) || args.interval || 'daily',
        time: formText(values.time) || args.time || '03:00'
      };
    } else if (op === 'restore') {
      // 恢复是覆盖式动作：服务端会先预检归档、再打恢复前快照，最后解包。
      path = '/sites/' + encodeURIComponent(id) + '/backups/restore';
      payload = { archive: args.archive || '', confirm: domain };
    } else if (op === 'delete') {
      path = '/sites/' + encodeURIComponent(id) + '/delete';
      payload = { confirm: domain };
    } else if (op === 'batch') {
      path = '/sites/batch';
      payload = {
        action: args.action || formText(values.action),
        domains: formLines(values.domains)
      };
    } else {
      toast('未知的站点操作：' + op, 'error');
      return Promise.resolve();
    }
    function run() {
      return submitJson(path, payload)
        .then(function (data) {
          if (op === 'restore') {
            // 恢复是后台任务（预检 → 快照 → 解包），不等结果，也不立刻刷新列表
            // （此时新快照还没生成）；进度与结果在任务中心看。
            toast(
              (data && data.message) || '恢复已开始：进度与结果见任务中心',
              'success'
            );
            return;
          }
          if (op === 'delete') {
            toast('站点已删除', 'success');
            location.href = prefix + '/sites/list';
            return;
          }
          // 独立页 `/sites/new` 仍保留「创建后进入详情」；列表弹窗则关闭并刷新列表。
          if (op === 'create' && data.id && !args.dialog) {
            location.href = prefix + '/sites/' + encodeURIComponent(data.id);
            return;
          }
          if (args.dialog) closeTopLayer();
          toast(
            (data && data.message) || (op === 'create' ? '站点已创建' : '操作已完成'),
            'success'
          );
          location.reload();
        })
        .catch(function (error) {
          formError(
            args,
            '操作失败：' + (error && error.message ? error.message : String(error)),
            values
          );
        });
    }
    // 破坏性动作统一二次确认；危险语义直接体现在确认框按钮上。
    var guard = Promise.resolve(true);
    if (op === 'power' && args.action === 'stop') {
      guard = hostConfirm({
        title: '停止站点',
        content: '停止后 ' + domain + ' 会立即不可访问，确认停止？',
        danger: true
      });
    } else if (op === 'ssl' && args.action === 'disable') {
      guard = hostConfirm({
        title: '关闭 HTTPS',
        content: '关闭后 ' + domain + ' 只保留 HTTP 访问，已签发的证书不会被删除。确认关闭？',
        danger: true
      });
    } else if (op === 'ssl' && args.action === 'delete_cert') {
      guard = hostConfirm({
        title: '删除站点证书',
        content: '删除后该站点的 HTTPS 将不可用，确认删除？',
        danger: true
      });
    } else if (op === 'ssl' && args.action === 'force' && args.enabled) {
      guard = hostConfirm({
        title: '开启强制 HTTPS',
        content: '开启后所有 HTTP 请求会跳转到 HTTPS，证书缺失时站点将无法访问。确认开启？'
      });
    } else if (op === 'domains' && args.action === 'remove') {
      guard = hostConfirm({
        title: '移除附加域名',
        content: '确认从 ' + domain + ' 移除附加域名 ' + (args.alias || '') + '？该域名将不再指向本站点。',
        danger: true
      });
    } else if (op === 'batch') {
      var batchAction = args.action || formText(values.action);
      if (batchAction === 'stop') {
        guard = hostConfirm({
          title: '批量停止站点',
          content: '列表中的站点将全部停止并立即不可访问，确认执行？',
          danger: true
        });
      }
    }
    return guard.then(function (ok) {
      if (!ok) return;
      return run();
    });
  }

  /// 列表行内启停：状态来自当前行，停止仍走 `siteWrite` 的危险确认。
  function siteToggle(args) {
    var running = String(args.status || '') === 'running';
    return siteWrite({
      op: 'power',
      action: running ? 'stop' : 'start',
      id: args.id,
      domain: args.domain
    });
  }

  /// 内核内置菜单图标名 → budding 包内图标子集（`Budding Icons`）。
  ///
  /// 1.8.0-r7 的子集含镜像、磁盘、文件夹等业务字形，全部语义名直接映射，
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

  /// 页面 Schema 资产地址：`static/budding/pages/<name>`。
  ///
  /// 已迁出的页面不再由本脚本拼装表结构——路由的 `schema` 直接给资产 URL，
  /// SDK 经 `platform.loadJson` 取回 JSON 后自行编译。这样「页面长什么样」全部
  /// 落在可 diff 的 JSON 里，脚本只留运行时胶水（取数、动作、主题、路由）。
  function pageAsset(name) {
    return prefix + '/static/budding/pages/' + name;
  }

  /// 已迁移到 budding 的菜单项：稳定键 → 路由路径（相对入口前缀）。
  /// 未列出的键保留内核深链 `href`（插件功能页仍是内核 SSR + iframe）。
  ///
  /// `schema` 两种形态：JSON 资产 URL（已迁出）与 `skiff:<kind>`（仍在
  /// `pageSchema()` 里拼装，迁移完成后逐个替换）。
  /// 页内互斥子视图（L3）不在这里登记，统一见 `SUBVIEW_TABS`。
  var BUILTIN_PAGES = {
    overview: { path: '', schema: 'skiff:overview' },
    'core/sites/list': { path: '/sites/list', schema: 'skiff:sites' },
    'core/sites/web': { path: '/sites/web', schema: 'skiff:web' },
    'core/sites/env': { path: '/sites/runtimes', schema: 'skiff:runtimes' },
    'core/docker/containers': { path: '/docker/containers', schema: 'skiff:containers' },
    'core/docker/images': { path: '/docker/images', schema: pageAsset('docker-images.json') },
    'core/docker/networks': { path: '/docker/networks', schema: pageAsset('docker-networks.json') },
    'core/docker/volumes': { path: '/docker/volumes', schema: pageAsset('docker-volumes.json') },
    'core/docker/compose': { path: '/docker/compose', schema: 'skiff:compose' },
    'core/plugins/installed': { path: '/plugins/installed', schema: 'skiff:plugins' },
    'core/plugins/discover': { path: '/plugins/discover', schema: 'skiff:plugins' },
    'core/plugins/updates': { path: '/plugins/updates', schema: 'skiff:plugins' },
    'core/plugins/store': { path: '/plugins/store', schema: 'skiff:store' },
    'core/tasks': { path: '/tasks', schema: 'skiff:tasks' },
    'core/jobs': { path: '/jobs', schema: 'skiff:jobs' },
    'core/files': { path: '/files', schema: 'skiff:files' },
    'core/files/trash': { path: '/files/trash', schema: 'skiff:files-trash' },
    'core/files/quotas': { path: '/files/quotas', schema: 'skiff:files-quotas' },
    'core/settings/security': { path: '/settings/security', schema: 'skiff:settings' },
    'core/settings/menu': { path: '/settings/menu', schema: 'skiff:menu-settings' },
    'core/settings/ui': { path: '/settings/ui', schema: 'skiff:ui-settings' },
    'core/docker/settings': { path: '/docker/settings', schema: 'skiff:docker-settings' },
    'core/settings/audit': { path: '/settings/audit', schema: 'skiff:audit' }
  };

  /// 插件菜单键 → `/ui/v1/menu` 下发的视图元数据（id / 默认视图 / 视图列表）。
  var pluginMeta = {};

  /// 插件默认视图的地址：菜单项自身路径，也是侧栏高亮的落点。
  function pluginViewHref(meta) {
    return (
      prefix +
      '/plugins/' +
      encodeURIComponent(meta.plugin_id) +
      '/' +
      encodeURIComponent(meta.default_view)
    );
  }

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
          // 侧栏高亮必须落在「这个插件自己那一项」上：`/plugins` 是所有插件页的
          // 公共前缀，指到那里会让「插件 → 已安装」跟着一起高亮。
          meta: { activeMenu: pluginViewHref(meta) }
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
        if (!child) return;
        children.push(child);
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
    // 三级子视图：默认视图就是 L2 自身，其余各登记一条隐藏路由（不进侧栏）。
    // 参数段（`:domain` / `:id`）原样留在路由模式里，由内核路由按实际地址匹配。
    SUBVIEW_TABS.forEach(function (sub) {
      var base = sub.pattern.join('/');
      sub.tabs.slice(1).forEach(function (tab) {
        routes.push({
          path: prefix + '/' + base + '/' + tab.segs.join('/'),
          title: sub.title + ' · ' + tab.title,
          hidden: true,
          schema: tab.schema || sub.schema,
          meta: { activeMenu: prefix + sub.menu }
        });
      });
    });
    // 详情页：菜单不出现，但地址栏直达与列表行内跳转都走同一路由。
    routes.push({
      path: prefix + '/sites/new',
      title: '新建站点',
      hidden: true,
      schema: 'skiff:site-new',
      meta: { activeMenu: prefix + '/sites/list' }
    });
    routes.push({
      path: prefix + '/sites/:id',
      title: '站点详情',
      hidden: true,
      schema: 'skiff:site-detail',
      meta: { activeMenu: prefix + '/sites/list' }
    });
    routes.push({
      path: prefix + '/sites/runtimes/:id',
      title: '运行环境详情',
      hidden: true,
      schema: 'skiff:runtime-detail',
      meta: { activeMenu: prefix + '/sites/runtimes' }
    });
    routes.push({
      path: prefix + '/docker/containers/new',
      title: '新建容器',
      hidden: true,
      schema: pageAsset('docker-container-new.json'),
      meta: { activeMenu: prefix + '/docker/containers' }
    });
    routes.push({
      path: prefix + '/docker/containers/:id',
      title: '容器详情',
      hidden: true,
      schema: 'skiff:container-detail',
      meta: { activeMenu: prefix + '/docker/containers' }
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
      path: prefix + '/docker/containers/:id/terminal',
      title: '容器终端',
      hidden: true,
      schema: 'skiff:container-terminal',
      meta: { activeMenu: prefix + '/docker/containers' }
    });
    routes.push({
      path: prefix + '/docker/containers/:id/logs',
      title: '容器日志',
      hidden: true,
      schema: 'skiff:container-logs',
      meta: { activeMenu: prefix + '/docker/containers' }
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
      meta: { activeMenu: prefix + '/settings/security' }
    });
    routes.push({
      path: prefix + '/docker/settings',
      title: 'Docker 设置',
      hidden: true,
      schema: 'skiff:docker-settings',
      meta: { activeMenu: prefix + '/docker/settings' }
    });
    routes.push({
      path: prefix + '/settings/ui',
      title: '界面设置',
      hidden: true,
      schema: 'skiff:ui-settings',
      meta: { activeMenu: prefix + '/settings/security' }
    });
    routes.push({
      path: prefix + '/plugins/:id',
      title: '插件详情',
      hidden: true,
      schema: 'skiff:plugin-info',
      meta: { activeMenu: prefix + '/plugins/installed' }
    });
    routes.push({
      path: prefix + '/plugins/store/confirm',
      title: '安装确认',
      hidden: true,
      schema: 'skiff:store-confirm',
      meta: { activeMenu: prefix + '/plugins/store' }
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
          meta: { activeMenu: pluginViewHref(meta) }
        });
      });
    });
    return routes;
  }

  /// 壳页 Schema：后台骨架 + 路由出口。菜单与当前页都由内核路由表驱动。
  ///
  /// 注意：budding 1.8.0-r7 的 schema 根没有注册 `theme` 顶层属性，写进树里会
  /// 触发 CompileError（整棵树拒绝编译）。主题改由壳页内联 `<style>` 下发（`spa.rs::theme_css`），并给
  /// 外壳补 `bud-admin--chrome` 类，见 `markShellChrome`。
  function shellSchema() {
    var schema = {
      type: 'admin',
      // `bar: false` 关掉内容区顶部那条通栏（品牌 / 页面标题 / 账号 / 退出登录）：
      // 页内子视图切换改由内核三级菜单承担（见 `mountSubNav`），账号与退出登录
      // 已搬进「设置」页（见 `settingsSchema`）。`actions` 槽在 bar 关闭时不渲染，
      // 因此这里不再放任何节点，避免出现「看不见但仍在 Tab 序里」的控件。
      props: { title: 'Skiff', brandTitle: 'Skiff', bar: false, tabsMode: 'single', breadcrumbs: false },
      content: {
        // 原生面包屑与三级菜单重复：那是「路径」，这里是「菜单」，只用后者。
        // 1.8.4 起 `content.breadcrumbs` 插槽已移除，关闭面包屑改走 props.breadcrumbs=false。
        body: [{ type: 'outlet' }]
      }
    };
    return schema;
  }

  /// 给外壳补 `bud-admin--chrome` 类。
  ///
  /// r5 的编译期校验拒绝在 schema 根写 `theme`，但外壳的侧栏 / 顶栏 / 页签条
  /// 换底色依赖 `bud-admin--chrome`。这里在挂载后补类，颜色仍由文档级变量提供。
  /// 外壳的 class 由 Vue 绑定，路由切换重渲染会把手工类抹掉，所以观察器常驻。
  var chromeObserver = null;

  function markShellChrome() {
    function mark() {
      var shell = document.querySelector('.bud-admin');
      if (!shell || shell.classList.contains('bud-admin--chrome')) return;
      shell.classList.add('bud-admin--chrome');
    }
    mark();
    if (chromeObserver || typeof MutationObserver === 'undefined') return;
    chromeObserver = new MutationObserver(function () {
      mark();
      // 路由切换会让外壳重渲染，三级菜单节点可能被一起替换掉，这里顺手补回。
      mountSubNav();
    });
    chromeObserver.observe(document.body, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ['class']
    });
  }

  // ---- 内容区三级菜单（L3） -------------------------------------------------
  // L3 是「同一页面内的互斥子视图」，形态对齐旧版 `<nav class="tabs">`：横向平铺、
  // 当前项高亮、没有子视图就整条不出现；不用面包屑表达位置——位置已由左侧菜单的
  // 当前项承担，再来一条「一级 › 二级 › 三级」只会和侧栏重复。
  //
  // 为什么内核自己渲染：`bar: false` 之后外壳不提供 L3 入口，budding 1.8.0 也没有
  // page-tabs 组件。节点挂在页面容器 `.bud-page` 内（标题之下、正文之上），底色跟随
  // 内容区而不是侧栏 chrome，样式在 `static/skiff-shell.css`。

  /// 三级菜单登记表（核心页）。
  ///
  /// - `pattern`：路径段模式，`:name` 捕获一段（站点域名、运行环境 id）并回填到每个 tab；
  /// - `tabs[0]`：默认视图，附加段为空，就是 L2 自身；
  /// - `schema`：该子视图的页面构造器名，单个 tab 可覆盖（日志走独立构造器）；
  /// - `menu`：子视图隐藏路由的 `activeMenu`，让侧栏高亮停在 L2；
  /// - `blocked`：同前缀下不参与匹配的字面段（`/sites/new` 不是站点详情）。
  ///
  /// 插件多视图不在这里登记：视图列表随 `/ui/v1/menu` 的 `views` 下发。
  var SUBVIEW_TABS = [
    {
      pattern: ['sites', 'web'],
      schema: 'skiff:web',
      menu: '/sites/web',
      title: 'Web 服务',
      tabs: [
        { title: '状态', segs: [] },
        { title: '配置', segs: ['config'] },
        { title: '日志', segs: ['logs'] },
        { title: '模块', segs: ['modules'] }
      ]
    },
    {
      pattern: ['sites', ':id'],
      schema: 'skiff:site-detail',
      menu: '/sites/list',
      title: '站点详情',
      blocked: ['new', 'list', 'runtimes', 'web'],
      tabs: [
        { title: '概览', segs: [] },
        { title: '域名', segs: ['domains'] },
        { title: '配置', segs: ['config'] },
        { title: '证书', segs: ['ssl'] },
        { title: '备份', segs: ['backup'] },
        { title: '日志', segs: ['logs'], schema: 'skiff:site-logs' }
      ]
    },
    {
      pattern: ['sites', 'runtimes', ':id'],
      schema: 'skiff:runtime-detail',
      menu: '/sites/runtimes',
      title: '运行环境详情',
      tabs: [
        { title: '基础信息', segs: [] },
        { title: '配置', segs: ['config'] },
        { title: '扩展', segs: ['extensions'] },
        { title: '站点', segs: ['sites'] },
        { title: '管理', segs: ['manage'] },
        { title: '日志', segs: ['logs'], schema: 'skiff:runtime-logs' }
      ]
    }
  ];

  /// tab 的绝对路径：模式里的 `:name` 用捕获值替换，其余段原样拼接。
  function subviewPath(pattern, params, segs) {
    var out = pattern.map(function (seg) {
      return seg.charAt(0) === ':' ? encodeURIComponent(params[seg] || '') : seg;
    });
    return prefix + '/' + out.concat(segs).join('/');
  }

  /// 当前路径段 → 命中的三级菜单；没有 L3 返回 `null`。
  ///
  /// 命中要求「剩余段完全等于某个 tab 的附加段」，因此 `/sites/web` 这种字面模式
  /// 排在参数模式之前，`/sites/a.com/unknown` 也不会误判成站点详情。
  function matchSubviews(segs) {
    for (var i = 0; i < SUBVIEW_TABS.length; i++) {
      var group = SUBVIEW_TABS[i];
      if (segs.length < group.pattern.length) continue;
      var params = {};
      var ok = true;
      for (var k = 0; k < group.pattern.length; k++) {
        var pat = group.pattern[k];
        if (pat.charAt(0) === ':') params[pat] = segs[k];
        else if (pat !== segs[k]) {
          ok = false;
          break;
        }
      }
      if (!ok) continue;
      var blocked = group.blocked || [];
      Object.keys(params).forEach(function (name) {
        if (blocked.indexOf(params[name]) >= 0) ok = false;
      });
      if (!ok) continue;
      var rest = segs.slice(group.pattern.length);
      var tabs = group.tabs.map(function (tab) {
        return { title: tab.title, path: subviewPath(group.pattern, params, tab.segs) };
      });
      var active = null;
      group.tabs.forEach(function (tab, idx) {
        if (tab.segs.length !== rest.length) return;
        var same = true;
        tab.segs.forEach(function (seg, n) {
          if (seg !== rest[n]) same = false;
        });
        if (same) active = tabs[idx].path;
      });
      if (!active) continue;
      return { tabs: tabs, active: active };
    }
    return null;
  }

  /// 插件多视图：视图清单来自 `/ui/v1/menu` 的 `views`，单视图插件不出三级菜单。
  /// 插件信息页 `/plugins/<id>`（两段）没有子视图，同样不出。
  function pluginSubviews(segs) {
    if (segs[0] !== 'plugins' || !segs[1] || segs.length < 3) return null;
    var meta = pluginMetaById(segs[1]);
    if (!meta || !meta.views || meta.views.length < 2) return null;
    var current = segs[2];
    var hit = meta.views.filter(function (v) {
      return v.view === current;
    })[0];
    if (!hit) return null;
    return {
      tabs: meta.views.map(function (v) {
        return {
          title: v.label,
          path: pluginBase(meta.plugin_id) + '/' + encodeURIComponent(v.view)
        };
      }),
      active: pluginBase(meta.plugin_id) + '/' + encodeURIComponent(hit.view)
    };
  }

  function pathNavigate(path) {
    if (!path) return;
    var runtime = buddingRuntime();
    if (runtime && typeof runtime.navigateTo === 'function') {
      runtime.navigateTo({ path: path });
    } else {
      window.location.assign(path);
    }
  }

  /// 三级菜单的挂载点：页面容器优先（与正文同内边距），退化为内容区。
  function subNavHost() {
    return (
      document.querySelector('.bud-admin__body .bud-page') ||
      document.querySelector('.bud-admin__main')
    );
  }

  function renderSubNav() {
    var host = subNavHost();
    if (!host) return;
    var segs = routeParts();
    var found = matchSubviews(segs) || pluginSubviews(segs);
    var nav = document.getElementById('skiff-subnav');
    // 没有子视图（或只有一个）就整条不出现：不占位、不留空壳。
    if (!found || found.tabs.length < 2) {
      if (nav) nav.remove();
      return;
    }
    if (!nav) {
      nav = document.createElement('nav');
      nav.id = 'skiff-subnav';
      nav.className = 'skiff-subnav';
      nav.setAttribute('aria-label', '子菜单');
      nav.addEventListener('click', function (e) {
        var target = e.target;
        if (!(target instanceof Element)) return;
        var link = target.closest('a.skiff-subnav__item');
        if (!link) return;
        e.preventDefault();
        pathNavigate(link.getAttribute('href'));
      });
    }
    // 标题之下、正文之上；页面没有标题时作为首个内容块。
    var after =
      host.querySelector('.bud-page-subtitle') || host.querySelector('.bud-page-title');
    if (after) {
      if (after.nextElementSibling !== nav) host.insertBefore(nav, after.nextElementSibling);
    } else if (host.firstElementChild !== nav) {
      host.insertBefore(nav, host.firstChild);
    }
    // 重建闸门：只有「落在别的 tab」或「视图清单变了」才改 DOM，
    // 否则外壳的重渲染观察器会被自己的写入反复触发。
    var sig =
      found.active +
      '|' +
      found.tabs
        .map(function (tab) {
          return tab.title;
        })
        .join(',');
    if (nav.getAttribute('data-sig') === sig) return;
    nav.setAttribute('data-sig', sig);
    nav.textContent = '';
    // 轨道层单独承担横向滚动：菜单自身要留 overflow: visible，
    // 否则吸顶遮罩（::before）会被裁掉。
    var track = document.createElement('div');
    track.className = 'skiff-subnav__track';
    found.tabs.forEach(function (tab) {
      var on = tab.path === found.active;
      var a = document.createElement('a');
      a.className = 'skiff-subnav__item' + (on ? ' skiff-subnav__item--on' : '');
      a.href = tab.path;
      a.textContent = tab.title;
      if (on) a.setAttribute('aria-current', 'page');
      track.appendChild(a);
    });
    nav.appendChild(track);
  }

  /// 吸顶判定：菜单贴住内容区顶部时补 `is-stuck`，由它撑开 24px 遮罩，
  /// 否则菜单上方那条页内边距会漏出下方滚动内容。CSS 无法自查吸顶，只能标记。
  function markSubNavStuck() {
    var nav = document.getElementById('skiff-subnav');
    var scroller = document.querySelector('.bud-admin__body');
    if (!nav || !scroller) return;
    var pad = parseFloat(getComputedStyle(scroller).paddingTop) || 0;
    var delta = nav.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
    nav.classList.toggle('is-stuck', delta <= pad + 0.5);
  }

  var subNavScrollBound = false;

  /// 外壳重渲染会连页面容器一起替换，因此每次观察都补挂一次（有闸门，成本很低）。
  function mountSubNav() {
    renderSubNav();
    markSubNavStuck();
    if (subNavScrollBound) return;
    var scroller = document.querySelector('.bud-admin__body');
    if (!scroller) return;
    subNavScrollBound = true;
    scroller.addEventListener('scroll', markSubNavStuck, { passive: true });
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
      // 全局 context 合入每个页面根作用域，供 JSON 页面写 `${prefix}` / `${api}`：
      // 前缀只有壳页知道，Schema 里不能出现硬编码的入口段。
      context: { prefix: prefix, api: apiBase },
      platform: {
        loadJson: loadJson,
        request: apiRequest,
        upload: uploadFile
      },
      actions: {
        skiffContainerAction: containerAction,
        skiffContainerToggle: containerToggle,
        skiffDockerWrite: dockerWrite,
        skiffQueryNav: queryNav,
        skiffSiteWrite: siteWrite,
        skiffSiteToggle: siteToggle,
        skiffTaskWrite: taskWrite,
        skiffJobWrite: jobWrite,
        skiffTrashWrite: trashWrite,
        skiffQuotaWrite: quotaWrite,
        skiffComposeAction: composeAction,
        skiffComposeEdit: composeEdit,
        skiffSettingsSave: settingsSave,
        skiffMenuWrite: menuWrite,
        skiffUiThemeWrite: uiThemeWrite,
        skiffUiPreset: uiPresetApply,
        // 与资源动作 `skiffDockerWrite` 必须保持不同名字：同名会按对象字面量
        // 后者覆盖前者，镜像/卷/网络/容器写操作会被静默劫持到设置保存。
        skiffDockerSettingsWrite: dockerSettingsWrite,
        skiffRuntimeCreate: runtimeCreate,
        skiffRuntimeImages: runtimeImagesWrite,
        skiffRuntimeAction: runtimeAction,
        skiffRuntimeProbe: runtimeProbe,
        skiffRuntimeConfig: runtimeConfigWrite,
        skiffRuntimeNode: runtimeNodeWrite,
        skiffRuntimePort: runtimePortWrite,
        skiffRuntimeFile: runtimeFileWrite,
        skiffRuntimeConfigEdit: runtimeConfigEdit,
        skiffRuntimeExt: runtimeExt,
        skiffRuntimePecl: runtimePecl,
        skiffRuntimeClone: runtimeClone,
        skiffRuntimeUnbind: runtimeUnbind,
        skiffWebConfig: webConfigWrite,
        skiffWebEnv: webEnvAction,
        skiffWebParams: webParamsWrite,
        skiffWebLogs: webLogsLoad,
        skiffPasswordChange: passwordChange,
        skiffTotpSetup: totpSetup,
        skiffTotpEnable: totpEnable,
        skiffKeyImport: keyImport,
        skiffKeyRemove: keyRemove,
        skiffFileOpen: fileOpen,
        skiffFileDownload: fileDownload,
        skiffFileWrite: fileWrite,
        skiffFileMkdir: fileMkdir,
        skiffFileRename: fileRename,
        skiffFileRemove: fileRemove,
        skiffFileCopy: fileCopy,
        skiffFileChmod: fileChmod,
        skiffFileChown: fileChown,
        skiffFileArchive: fileArchive,
        skiffFileExtract: fileExtract,
        skiffLogSwitch: logSwitch,
        skiffLogFilter: logFilter,
        skiffLogReload: logReload,
        skiffSearch: search,
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
            toast('退出失败：' + (error && error.message ? error.message : String(error)), 'error');
          });
        }
      }
    };
    if (cssHref) options.css = cssHref;
    registerPacks();
    window.BuddingSDK.embed(mount, schema, options)
      .then(function () {
        // 预先解析一次，避免第一次确认时才扫 provides（结果会被缓存）。
        buddingRuntime();
        consumeFlashToast();
      })
      .catch(function (error) {
        showError('初始化失败：' + (error && error.message ? error.message : String(error)));
      });
  }

  /// 登录态失效：立即硬跳独立登录页。
  ///
  /// 登录页不在 budding 路由表内，走前端路由会落到未知页；这里不做提示、不等待、
  /// 不重新 embed，直接 `location.replace`，并保留当前路径作为 `next`。
  var sessionExpiredShown = false;

  function loginUrl() {
    var next = window.location.pathname + window.location.search;
    return prefix + '/login?next=' + encodeURIComponent(next);
  }

  function showSessionExpired() {
    if (sessionExpiredShown) return;
    sessionExpiredShown = true;
    window.location.replace(loginUrl());
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


  function detailLines(title, lines) {
    return {
      type: 'card',
      props: { title: title },
      content: { body: lines.map(textLine) }
    };
  }


  function writeForm(url, submitAction, fields, submitText, initialValues) {
    var props = {
      api: { method: 'POST', url: url, sendOn: false },
      actionsAlign: 'end'
    };
    if (initialValues) props.initialValues = initialValues;
    return {
      type: 'form',
      props: props,
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
    var guard =
      action === 'down'
        ? hostConfirm({
            title: '清理 compose 项目',
            content: '确认清理项目 ' + project + '？内核托管的容器会被删除，卷默认保留。',
            danger: true
          })
        : Promise.resolve(true);
    return guard.then(function (ok) {
      if (!ok) return;
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
            toast(errorMessage(res), 'error');
            return;
          }
          toast('项目 ' + project + ' 已' + (action === 'down' ? '清理' : '更新'), 'success');
          location.reload();
        })
        .catch(function (error) {
          toast('操作失败：' + (error && error.message ? error.message : String(error)), 'error');
        });
    });
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
          toast(
            failed > 0
              ? '项目 ' + data.project + ' 部署完成，但有 ' + failed + ' 个服务失败。'
              : '项目 ' + data.project + ' 已部署。',
            failed > 0 ? 'warning' : 'success'
          );
          location.reload();
          return;
        }
        toast(
          '校验通过 · 服务 ' +
            ((data.services || []).join(' → ') || '—') +
            ' · 网络 ' +
            ((data.networks || []).join('、') || '—'),
          'success'
        );
      })
      .catch(function (error) {
        toast('校验失败：' + (error && error.message ? error.message : String(error)), 'error');
      });
  }

  /// 搜索 / 翻页跳转：读表单 `q`（或显式 `args.q`），按 `URLSearchParams` 编码后
  /// 整页跳转。必须是宿主动作——budding 的 `navigate` 读不到表单值，`link` 只改
  /// 地址栏不重新取数，两者都替代不了「换筛选条件要重新加载」。
  function search(args, ctx) {
    var q = args.q;
    if (q === undefined) q = formValues(ctx).q;
    var params = new URLSearchParams();
    var text = String(q == null ? '' : q).trim();
    if (text) params.set('q', text);
    if (args.cursor) params.set('cursor', String(args.cursor));
    var query = params.toString();
    window.location.href = String(args.base || location.pathname) + (query ? '?' + query : '');
  }


  /// 菜单偏好写操作：复用 `/ui/v1/settings/menu`（同一校验与审计）。
  function menuWrite(args) {
    var action = args.action || '';
    var guard =
      action === 'reset'
        ? hostConfirm({
            title: '恢复默认菜单',
            content: '确认恢复默认菜单偏好？本账号的顺序、隐藏与固定将被清除。',
            danger: true
          })
        : Promise.resolve(true);
    var payload = {
      action: action,
      key: args.key || ''
    };
    return guard
      .then(function (ok) {
        if (!ok) return;
        return submitJson('/settings/menu', payload).then(function (data) {
          toast((data && data.message) || '已保存菜单偏好', 'success');
          location.reload();
        });
      })
      .catch(function (error) {
        toast('保存失败：' + (error && error.message ? error.message : String(error)), 'error');
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
        toast((data && data.message) || '已保存', 'success');
        location.reload();
      })
      .catch(function (error) {
        toast('保存失败：' + (error && error.message ? error.message : String(error)), 'error');
      });
  }

  /// 界面设置写操作：复用 `/ui/v1/settings/ui`（同一校验与审计）。
  function uiThemeWrite(args, ctx) {
    var values = formValues(ctx);
    var primary = formText(values.primary).trim();
    var menu = formText(values.menu).trim();
    var page = formText(values.page).trim();
    var menuCustom = !!values.menu_custom;
    var hex = /^#[0-9a-fA-F]{6}$/;
    var invalid = '';
    if (!hex.test(primary)) invalid = '主色';
    else if (!hex.test(page)) invalid = '页面底色';
    else if (menuCustom && !hex.test(menu)) invalid = '菜单底色';
    if (invalid) {
      formError(args, invalid + '必须是 #rrggbb 格式，例如 #1867c0', values);
      return Promise.resolve();
    }
    return submitJson('/settings/ui', {
      primary: primary,
      // 关闭自定义时留空，服务端按页面底色自动推导，避免旧值在下次开启时意外复活。
      menu: menuCustom ? menu : '',
      page: page,
      menu_custom: menuCustom
    })
      .then(function (data) {
        toast((data && data.message) || '已保存配色', 'success');
        location.reload();
      })
      .catch(function (error) {
        formError(
          args,
          '保存失败：' + (error && error.message ? error.message : String(error)),
          values
        );
      });
  }

  /// 预设方案：一次提交整套颜色；服务端仍做严格校验与对比度校正。
  function uiPresetApply(args) {
    return submitJson('/settings/ui', {
      primary: formText(args.primary).trim(),
      menu: formText(args.menu).trim(),
      page: formText(args.page).trim(),
      menu_custom: !!args.menu_custom
    })
      .then(function (data) {
        toast((data && data.message) || '已应用预设配色', 'success');
        location.reload();
      })
      .catch(function (error) {
        toast(
          '应用预设失败：' + (error && error.message ? error.message : String(error)),
          'error'
        );
      });
  }

  /// 运行环境写操作统一收尾：成功提示、失败展示，均复用 `/ui/v1` 错误信封。
  function runtimeDone(data, fallback) {
    toast((data && data.message) || fallback || '操作已完成', 'success');
    location.reload();
  }

  function runtimeFail(error) {
    toast('操作失败：' + (error && error.message ? error.message : String(error)), 'error');
  }

  function runtimeNum(raw) {
    var text = formText(raw).trim();
    if (!text) return null;
    var n = parseInt(text, 10);
    return isNaN(n) ? null : n;
  }

  function runtimeCreate(args, ctx) {
    var v = formValues(ctx);
    var kind = formText(v.kind).trim();
    return submitJson('/runtimes', {
      kind: kind,
      version: formText(v.version).trim(),
      name: formText(v.name).trim(),
      // 非 PHP 不发该字段（null = 不改动），否则会用一个与安装无关的选择
      // 去改写全局 apk 源。
      apk_mirror: kind === 'php' ? formText(v.apk_mirror).trim() : null
    })
      .then(function (data) {
        if (args && args.dialog) closeTopLayer();
        runtimeDone(data, '安装任务已提交');
      })
      .catch(function (error) {
        formError(
          args,
          '操作失败：' + (error && error.message ? error.message : String(error)),
          v
        );
      });
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
    var guard = args.confirm_text
      ? hostConfirm({ title: '确认操作', content: String(args.confirm_text), danger: true })
      : Promise.resolve(true);
    return guard
      .then(function (ok) {
        if (!ok) return;
        return submitJson(
          '/runtimes/' + encodeURIComponent(args.id) + '/action/' + encodeURIComponent(args.op),
          {}
        ).then(function (data) {
          // 删除后详情页已无对应实体，留在原地只会渲染「运行环境不存在」。
          if (args.op === 'delete') {
            toast((data && data.message) || '运行环境已删除', 'success');
            window.location.href = prefix + '/sites/runtimes';
            return;
          }
          runtimeDone(data, '操作已完成');
        });
      })
      .catch(runtimeFail);
  }

  function runtimeProbe(args) {
    return submitJson('/runtimes/' + encodeURIComponent(args.id) + '/probe', {})
      .then(function (data) {
        toast(
          data.ok ? '实测版本：' + data.version : '探测失败：' + (data.error || '未知错误'),
          data.ok ? 'success' : 'warning'
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
    if (port === null || port < 1 || port > 65535) {
      toast('端口必须是 1–65535 之间的整数', 'warning');
      return Promise.resolve();
    }
    return hostConfirm({
      title: '修改运行环境端口',
      content: '改端口会重写全部绑定站点反代并重建容器，确认继续？',
      danger: true
    }).then(function (ok) {
      if (!ok) return;
      return submitJson('/runtimes/' + encodeURIComponent(args.id) + '/port', {
        port: port,
        confirm: true
      })
        .then(function (data) {
          runtimeDone(data, '端口已保存');
        })
        .catch(runtimeFail);
    });
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

  /// 打开配置文件编辑视图。`?file=` 只是查询参数，沿用内页 navigate 会被路由
  /// 判定为同一页而不重取 schema，URL 变了编辑器却不出现，因此这里整页跳转。
  function runtimeConfigEdit(args) {
    window.location.href =
      prefix +
      '/sites/runtimes/' +
      encodeURIComponent(args.id) +
      '/config?file=' +
      encodeURIComponent(args.key);
  }

  function runtimeExt(args) {
    var guard = args.confirm_text
      ? hostConfirm({ title: '确认操作', content: String(args.confirm_text), danger: true })
      : Promise.resolve(true);
    return guard
      .then(function (ok) {
        if (!ok) return;
        return submitJson(
          '/runtimes/' +
            encodeURIComponent(args.id) +
            '/extensions/' +
            encodeURIComponent(args.key) +
            '/' +
            encodeURIComponent(args.op),
          {}
        ).then(function (data) {
          runtimeDone(data, '扩展任务已提交');
        });
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
    return hostConfirm({
      title: '解绑站点',
      content: '解绑后站点会转为静态并停止，确认解绑 ' + args.domain + '？',
      danger: true
    }).then(function (ok) {
      if (!ok) return;
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
    });
  }


  /// Web 服务容器动作：复用 `POST /ui/v1/runtimes/web/action/{op}`（同一命令层与审计）。
  function webEnvAction(args) {
    var op = args.op;
    var guard = Promise.resolve(true);
    if (op === 'rebuild') {
      guard = hostConfirm({
        title: '重新安装 Web 服务',
        content: '将重建 OpenResty 容器，期间所有站点不可访问。确认继续？',
        danger: true
      });
    } else if (op === 'delete') {
      guard = hostConfirm({
        title: '删除 Web 服务容器',
        content: '删除后所有站点立即不可访问，配置与站点目录会保留。确认删除？',
        danger: true
      });
    }
    return guard
      .then(function (ok) {
        if (!ok) return;
        return submitJson('/runtimes/web/action/' + encodeURIComponent(op), { confirm: true }).then(
          function (data) {
            // 安装/重建：提交成功即刷新进入当前作业，不弹阻塞提示，
            // 避免旧页面的完成记录在提示期间继续可见。
            if (op === 'install' || op === 'rebuild') {
              location.reload();
              return;
            }
            toast((data && data.message) || '操作已完成', 'success');
            location.reload();
          }
        );
      })
      .catch(function (error) {
        toast('操作失败：' + (error && error.message ? error.message : String(error)), 'error');
      });
  }

  /// 主配置保存：复用 `/ui/v1/web/config`（nginx -t + reload + 审计）。
  function webConfigWrite(args, ctx) {
    var values = formValues(ctx);
    return submitJson('/web/config', { content: formText(values.content) })
      .then(function (data) {
        toast((data && data.message) || '主配置已保存', 'success');
        location.reload();
      })
      .catch(function (error) {
        toast('保存失败：' + (error && error.message ? error.message : String(error)), 'error');
      });
  }

  /// 性能参数保存：复用 `/ui/v1/web/params`。
  function webParamsWrite(args, ctx) {
    var values = formValues(ctx);
    var connections = runtimeNum(values.worker_connections);
    var keepalive = runtimeNum(values.keepalive_timeout);
    if (
      connections === null ||
      connections < 64 ||
      connections > 65535 ||
      keepalive === null ||
      keepalive < 0 ||
      keepalive > 3600
    ) {
      toast('worker_connections 需为 64–65535 的整数，keepalive_timeout 需为 0–3600 的整数', 'warning');
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
        toast((data && data.message) || '性能参数已保存', 'success');
        location.reload();
      })
      .catch(function (error) {
        toast('保存失败：' + (error && error.message ? error.message : String(error)), 'error');
      });
  }

  /// 日志按需加载：把筛选条件带回 URL，页面再走 `/ui/v1/web/logs`（不自动读文件）。
  function webLogsLoad(args, ctx) {
    var values = formValues(ctx);
    var qs = new URLSearchParams();
    qs.set('type', formText(values.type).trim() || 'access');
    qs.set('lines', formText(values.lines).trim() || '200');
    qs.set('level', formText(values.level).trim() || 'all');
    if (formText(values.status).trim()) qs.set('status', formText(values.status).trim());
    if (formText(values.q).trim()) qs.set('q', formText(values.q).trim());
    // 日志已是三级子视图：查询条件走 URL query，路径固定 /sites/web/logs。
    location.href = prefix + '/sites/web/logs?' + qs.toString();
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
          toast(errorMessage(res), 'error');
          return;
        }
        toast((res.body && res.body.data && res.body.data.message) || '已保存', 'success');
        location.reload();
      })
      .catch(function (error) {
        toast('保存失败：' + (error && error.message ? error.message : String(error)), 'error');
      });
  }


  /// JSON 写请求的唯一通道：统一 CSRF 头、401 跳登录、错误信封解析。
  /// 两个出口只差「URL 前缀」与「响应取哪一层」，语义差异写在下面各自的包装里。
  function postJson(url, payload) {
    return fetch(url, {
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
        if (res.status === 401) {
          showSessionExpired();
          throw new Error('登录状态已失效，正在跳转到登录页。');
        }
        if (res.status !== 200) throw new Error(errorMessage(res));
        return res.body;
      });
  }

  /// `/ui/v1` 写请求：解包信封的 `data`（业务结果都在这层）。
  function submitJson(path, payload) {
    return postJson(apiBase + path, payload).then(function (body) {
      return (body && body.data) || {};
    });
  }

  /// 入口前缀内的写请求（如 exec 票据）：返回整个响应体，票根不在 `data` 里。
  function postPrefixJson(url, payload) {
    return postJson(url, payload).then(function (body) {
      return body || {};
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
                  href: prefix + '/docker/containers/' + encodeURIComponent(containerId)
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
        toast(data.message || '口令已修改', 'success');
        window.location.reload();
      })
      .catch(function (error) {
        toast('修改失败：' + (error && error.message ? error.message : String(error)), 'error');
      });
  }

  function totpSetup() {
    return submitJson('/settings/totp/setup', {})
      .then(function () {
        window.location.reload();
      })
      .catch(function (error) {
        toast('生成密钥失败：' + (error && error.message ? error.message : String(error)), 'error');
      });
  }

  function totpEnable(args, ctx) {
    var values = formValues(ctx);
    return submitJson('/settings/totp/enable', { code: String(values.code || '') })
      .then(function (data) {
        // 备份码只显示一次：以前经 sessionStorage 中转、刷新后由页面弹出，
        // 页面改由服务端产出后没有这个钩子，改为当场弹窗，避免静默丢失。
        var codes = data.backup_codes || [];
        if (codes.length === 0) {
          toast(data.message || '两步验证已启用', 'success');
          window.location.reload();
          return;
        }
        hostConfirm({
          title: '备份码（只显示一次，请立即保存）',
          content: codes.join('   ')
        }).then(function () {
          toast(data.message || '两步验证已启用', 'success');
          window.location.reload();
        });
      })
      .catch(function (error) {
        toast('启用失败：' + (error && error.message ? error.message : String(error)), 'error');
      });
  }

  function keyImport(args, ctx) {
    var values = formValues(ctx);
    return submitJson('/settings/keys', {
      public_key: String(values.public_key || ''),
      label: String(values.label || '')
    })
      .then(function (data) {
        toast(data.message || '已导入公钥', 'success');
        window.location.reload();
      })
      .catch(function (error) {
        toast('导入失败：' + (error && error.message ? error.message : String(error)), 'error');
      });
  }

  function keyRemove(args) {
    return hostConfirm({
      title: '撤销公钥',
      content: '撤销后该公钥签名的包将无法安装/升级，确认撤销？',
      danger: true
    }).then(function (ok) {
      if (!ok) return;
      return submitJson('/settings/keys/' + encodeURIComponent(args.key_id), {})
        .then(function (data) {
          toast(data.message || '已撤销该公钥', 'success');
          window.location.reload();
        })
        .catch(function (error) {
          toast('撤销失败：' + (error && error.message ? error.message : String(error)), 'error');
        });
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
    // 声明式写操作（页面 JSON 的 `ajax` 动作、表单 `api`）与命令式写共用一把 CSRF 头：
    // 内核所有写端点都校验 `x-csrf-token`，缺了直接 403。统一在这里补，Schema 不必
    // 自己拼 header——否则「用 budding 内置能力写页面」的第一步就得回退到手写动作。
    if (method !== 'GET' && method !== 'HEAD') {
      var hasCsrf = false;
      for (var name in headers) {
        if (name.toLowerCase() === 'x-csrf-token') hasCsrf = true;
      }
      if (!hasCsrf) headers['x-csrf-token'] = (sessionState && sessionState.csrf_token) || '';
    }
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
        if (resp.status === 401) showSessionExpired();
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
          if (res.status === 401) {
            showSessionExpired();
            throw new Error('登录状态已失效，正在跳转到登录页。');
          }
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
      toast(result.data.message, 'success');
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
      toast('目录不能下载，请先进入目录选择文件。', 'warning');
      return;
    }
    window.location.href =
      prefix + '/files/download?path=' + encodeURIComponent(String(args.path || ''));
  }

  /// 关闭最上层弹层（确认框之外的业务弹窗用它收尾）。
  function closeTopLayer() {
    var runtime = buddingRuntime();
    if (runtime && typeof runtime.closeLayer === 'function') runtime.closeLayer();
  }

  /// 单段文件名校验：不允许分隔符，避免「名字」变成跨目录移动。
  function validEntryName(name) {
    return !!name && name !== '.' && name !== '..' && !/[\\/]/.test(name);
  }

  function fileMkdir(args, ctx) {
    var values = formValues(ctx);
    var name = String(values.name || '').trim();
    if (!validEntryName(name)) {
      toast('目录名不能为空，也不能包含 / 或 \\', 'warning');
      return Promise.resolve();
    }
    var base = String(args.dir || '').replace(/\/+$/, '');
    return submitJson('/files/mkdir', { path: base + '/' + name })
      .then(function (data) {
        toast(data.message || '目录已创建', 'success');
        window.location.reload();
      })
      .catch(function (error) {
        toast('新建目录失败：' + (error && error.message ? error.message : String(error)), 'error');
      });
  }

  /// `POST /ui/v1/files/write`：带 `path` 时覆盖保存，否则按 `dir` + 文件名新建。
  function fileWrite(args, ctx) {
    var values = formValues(ctx);
    var target = String(args.path || '').trim();
    if (!target) {
      var name = String(values.name || '').trim();
      if (!validEntryName(name)) {
        formError(args, '文件名不能为空，也不能包含 / 或 \\', values);
        return Promise.resolve();
      }
      var base = String(args.dir || '').replace(/\/+$/, '');
      target = base + '/' + name;
    }
    return submitJson('/files/write', { path: target, content: String(values.content || '') })
      .then(function () {
        closeTopLayer();
        toast('已保存', 'success');
        window.location.reload();
      })
      .catch(function (error) {
        formError(
          args,
          '保存失败：' + (error && error.message ? error.message : String(error)),
          values
        );
      });
  }

  function fileRename(args, ctx) {
    var values = formValues(ctx);
    var current = String(args.path || '');
    var name = String(args.name || '');
    var next = String(values.name || '').trim();
    if (!validEntryName(next)) {
      formError(args, '新名称不能为空，也不能包含 / 或 \\', values);
      return Promise.resolve();
    }
    if (next === name) {
      closeTopLayer();
      return Promise.resolve();
    }
    var parent = current.replace(/[\\/][^\\/]*$/, '');
    return submitJson('/files/rename', { from: current, to: parent + '/' + next })
      .then(function (data) {
        closeTopLayer();
        toast(data.message || '已重命名', 'success');
        window.location.reload();
      })
      .catch(function (error) {
        formError(
          args,
          '重命名失败：' + (error && error.message ? error.message : String(error)),
          values
        );
      });
  }

  function fileCopy(args, ctx) {
    var values = formValues(ctx);
    var from = String(args.path || '').trim();
    var to = String(values.to || '').trim();
    if (!from || !to) {
      formError(args, '请填写目标路径', values);
      return Promise.resolve();
    }
    if (to.charAt(0) !== '/') {
      formError(args, '目标路径必须是绝对路径（以 / 开头）', values);
      return Promise.resolve();
    }
    return submitJson('/files/copy', { from: from, to: to, overwrite: !!values.overwrite })
      .then(function (data) {
        closeTopLayer();
        toast((data.message || '已复制') + '：' + (data.to || to), 'success');
        window.location.reload();
      })
      .catch(function (error) {
        formError(
          args,
          '复制失败：' + (error && error.message ? error.message : String(error)),
          values
        );
      });
  }

  function fileChmod(args, ctx) {
    var values = formValues(ctx);
    var path = String(args.path || '');
    var mode = String(values.mode || '').trim();
    if (!mode) {
      formError(args, '请填写八进制权限（如 755）', values);
      return Promise.resolve();
    }
    if (!MODE_RULE.pattern || !new RegExp(MODE_RULE.pattern).test(mode)) {
      formError(args, '权限必须是八进制（如 755 或 0755）', values);
      return Promise.resolve();
    }
    return submitJson('/files/chmod', { path: path, mode: mode })
      .then(function (data) {
        closeTopLayer();
        toast('权限已修改为 ' + (data.mode !== undefined ? data.mode.toString(8) : mode), 'success');
        window.location.reload();
      })
      .catch(function (error) {
        formError(
          args,
          '修改权限失败：' + (error && error.message ? error.message : String(error)),
          values
        );
      });
  }

  function fileChown(args, ctx) {
    var values = formValues(ctx);
    var path = String(args.path || '');
    var uidRaw = String(values.uid === undefined || values.uid === null ? '' : values.uid).trim();
    var gidRaw = String(values.gid === undefined || values.gid === null ? '' : values.gid).trim();
    if (!uidRaw || !gidRaw) {
      formError(args, '请同时填写 uid 与 gid', values);
      return Promise.resolve();
    }
    if (!/^[0-9]+$/.test(uidRaw) || !/^[0-9]+$/.test(gidRaw)) {
      formError(args, 'uid 与 gid 必须是非负整数', values);
      return Promise.resolve();
    }
    var uid = Number(uidRaw);
    var gid = Number(gidRaw);
    if (uid > 4294967295 || gid > 4294967295) {
      formError(args, 'uid 与 gid 不能超过 4294967295', values);
      return Promise.resolve();
    }
    // 后端 DTO 是 u32；这里必须发数字，字符串会 422。
    return submitJson('/files/chown', { path: path, uid: uid, gid: gid })
      .then(function (data) {
        closeTopLayer();
        toast(data.message || '属主已修改', data.root_owned ? 'warning' : 'success');
        window.location.reload();
      })
      .catch(function (error) {
        formError(
          args,
          '修改属主失败：' + (error && error.message ? error.message : String(error)),
          values
        );
      });
  }

  function fileArchive(args, ctx) {
    var values = formValues(ctx);
    var source = String(args.path || '').trim();
    var dest = String(values.dest || '').trim();
    var format = String(values.format || 'tar.gz').trim();
    if (!dest) {
      formError(args, '请填写压缩包路径', values);
      return Promise.resolve();
    }
    if (dest.charAt(0) !== '/') {
      formError(args, '压缩包路径必须是绝对路径（以 / 开头）', values);
      return Promise.resolve();
    }
    return submitJson('/files/archive', {
      sources: [source],
      dest: dest,
      format: format,
      overwrite: !!values.overwrite
    })
      .then(function (data) {
        closeTopLayer();
        toast('已创建压缩包 ' + (data.path || dest), 'success');
        window.location.reload();
      })
      .catch(function (error) {
        formError(
          args,
          '压缩失败：' + (error && error.message ? error.message : String(error)),
          values
        );
      });
  }

  function fileExtract(args, ctx) {
    var values = formValues(ctx);
    var archive = String(args.path || '').trim();
    var dest = String(values.dest || '').trim();
    if (!dest) {
      formError(args, '请填写解压目标目录', values);
      return Promise.resolve();
    }
    if (dest.charAt(0) !== '/') {
      formError(args, '目标目录必须是绝对路径（以 / 开头）', values);
      return Promise.resolve();
    }
    return submitJson('/files/extract', {
      archive: archive,
      dest: dest,
      overwrite: !!values.overwrite
    })
      .then(function (data) {
        closeTopLayer();
        var skipped = (data.skipped_symlinks || 0) + (data.skipped_special || 0);
        toast(
          '已解压 ' + (data.files || 0) + ' 个文件' + (skipped > 0 ? '，跳过 ' + skipped + ' 个特殊条目' : ''),
          skipped > 0 ? 'warning' : 'success'
        );
        window.location.reload();
      })
      .catch(function (error) {
        formError(
          args,
          '解压失败：' + (error && error.message ? error.message : String(error)),
          values
        );
      });
  }

  function fileRemove(args) {
    var name = String(args.name || '');
    var path = String(args.path || '');
    return hostConfirm({
      title: '删除文件',
      content: '确认删除 ' + name + '？删除后进入回收站，可在回收站恢复。',
      danger: true
    }).then(function (ok) {
      if (!ok) return;
      return submitJson('/files/remove', { path: path })
        .then(function (data) {
          toast(
            (data.message || '已移入回收站') +
              (data.trash_id ? '（回收站编号 ' + data.trash_id + '）' : ''),
            'success'
          );
          window.location.reload();
        })
        .catch(function (error) {
          toast('删除失败：' + (error && error.message ? error.message : String(error)), 'error');
        });
    });
  }

  function logPageUrl(id, part, tail, keyword) {
    return (
      prefix +
      '/sites/' +
      encodeURIComponent(id) +
      '/logs?part=' +
      (part === 'error' ? 'error' : 'access') +
      '&tail=' +
      encodeURIComponent(String(tail || '200')) +
      (keyword ? '&q=' + encodeURIComponent(keyword) : '')
    );
  }

  function logSwitch(args) {
    window.location.href = logPageUrl(
      args.id,
      args.part,
      args.tail,
      String(args.q || '')
    );
  }

  function logFilter(args, ctx) {
    var values = formValues(ctx);
    window.location.href = logPageUrl(
      args.id,
      args.part,
      args.tail,
      String(values.q || '').trim()
    );
  }

  function logReload() {
    window.location.reload();
  }

  /// 弹窗表单注册表：提交失败或本地校验不通过时，按 key 还原同一张表单。
  var dialogRegistry = {};
  var dialogSeq = 0;

  function newDialogKey() {
    dialogSeq += 1;
    return 'skiff-dialog-' + dialogSeq;
  }

  /// 弹窗内联错误：关掉当前图层后重开一张「错误摘要 + 同样字段」的弹窗。
  ///
  /// 为什么不用 toast：toast 一闪而过且关掉后用户看不到原因。这里保留用户
  /// 已填的值（`initialValues`），只在顶部追加 `alert`，字段级错误回填仍等 U3。
  function dialogError(key, message, values) {
    var text = String(message === undefined || message === null ? '' : message);
    var def = key ? dialogRegistry[key] : null;
    var runtime = buddingRuntime();
    if (
      !def ||
      !runtime ||
      typeof runtime.openLayer !== 'function' ||
      typeof runtime.closeLayer !== 'function' ||
      typeof runtime.deriveScope !== 'function'
    ) {
      toast('操作失败：' + text, 'error');
      return;
    }
    var body = [
      {
        type: 'alert',
        props: { type: 'error', title: '提交未完成', text: text }
      }
    ];
    // 原弹窗的 notice 可能含 `${item.*}` 行作用域占位符，重开图层时已无该作用域；
    // 这里只保留错误摘要与用户输入，不复制会退化成字面量的提示。
    var action = {
      type: def.action.type,
      props: Object.assign({}, def.action.props || {}, { dialog: key })
    };
    body.push(writeForm(def.url, action, def.fields, def.submitText, values || {}));
    runtime.closeLayer();
    runtime.openLayer(
      {
        title: def.title,
        width: def.width,
        body: body,
        footer: [
          {
            type: 'button',
            props: { text: '取消', variant: 'outlined' },
            action: { type: 'close' }
          }
        ]
      },
      { scope: runtime.deriveScope({}, null) }
    );
  }

  /// 弹窗内错误优先就地展示；页面级表单没有 `dialog` 上下文时退回 toast。
  function formError(args, message, values) {
    if (args && args.dialog) dialogError(args.dialog, message, values);
    else toast(message, 'error');
  }

  var MODE_RULE = { pattern: '^0?[0-7]{3,4}$', messages: { pattern: '请输入八进制权限，如 755' } };


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
          toast(errorMessage(res), 'error');
          return;
        }
        toast(res.body.data.enabled ? '插件已启用' : '插件已停用', 'success');
        location.reload();
      })
      .catch(function (error) {
        toast('操作失败：' + (error && error.message ? error.message : String(error)), 'error');
      });
  }

  function pluginDelete(args) {
    var id = args.id;
    var deleteData = args.delete_data === true;
    var text = deleteData
      ? '确认卸载 ' + id + ' 并删除其数据目录？此操作不可恢复。'
      : '确认卸载 ' + id + '？数据目录会保留。';
    return hostConfirm({ title: '卸载插件', content: text, danger: true }).then(function (ok) {
      if (!ok) return;
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
            toast(errorMessage(res), 'error');
            return;
          }
          location.assign(
            (res.body && res.body.data && res.body.data.redirect_to) || prefix + '/plugins/installed'
          );
        })
        .catch(function (error) {
          toast('卸载失败：' + (error && error.message ? error.message : String(error)), 'error');
        });
    });
  }

  // 商店：目录只读快照 + 插件安装（下载验签后二次确认）。
  var pendingInstall = null;
  /// 暂存的安装确认页数据；确认路由按它渲染，刷新后失效（需回商店重选）。
  var pendingConfirm = null;


  /// 升级包下载：复用 `/ui/v1/store/upgrades/{version}/download`，
  /// 不跳旧商店；校验、暂存与 `upgrade.download` 审计由内核同一入口完成。
  function storeUpgradeDownload(args) {
    var version = (args && args.version) || '';
    if (!version) {
      toast('目录中没有可下载的升级版本。', 'warning');
      return Promise.resolve();
    }
    toast('正在下载并校验 ' + version + ' 升级包，请稍候…', 'info');
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
          toast(errorMessage(res), 'error');
          return;
        }
        var data = res.body.data;
        toast('升级包 ' + data.version + ' 已暂存并通过 sha256 校验：' + data.path, 'success');
      })
      .catch(function (error) {
        toast('升级包下载失败：' + (error && error.message ? error.message : String(error)), 'error');
      });
  }

  function storeInstall(args, ctx, runtime) {
    if (!args || args.installable === false) {
      toast((args && args.install_note) || '该条目当前不可安装', 'warning');
      return Promise.resolve();
    }
    var name = args.name;
    // 下载 + 验签是同步阻塞的单次请求，暂无进度事件；用明确提示占位。
    toast('正在下载并验签 ' + name + ' ' + (args.version || '') + '，请稍候…', 'info');
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
          toast(errorMessage(res), 'error');
          return;
        }
        pendingInstall = res.body.data;
        pendingConfirm = res.body.data;
        if (runtime && typeof runtime.navigateTo === 'function') {
          runtime.navigateTo({ path: prefix + '/plugins/store/confirm' });
          return;
        }
        // 兜底：没有路由能力时退回商店页（避免二次嵌入另一个 Vue 实例）。
        window.location.href = prefix + '/plugins/store';
      })
      .catch(function (error) {
        toast('安装准备失败：' + (error && error.message ? error.message : String(error)), 'error');
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
              click: { actions: [{ type: 'navigate', props: { url: prefix + '/plugins/store' } }] }
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
      toast('待确认的安装已失效，请返回商店重新发起。', 'warning');
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
          toast(errorMessage(res), 'error');
          return;
        }
        pendingInstall = null;
        pendingConfirm = null;
        toast('插件已安装', 'success');
        location.assign((res.body && res.body.data && res.body.data.redirect_to) || prefix + '/plugins/installed');
      })
      .catch(function (error) {
        toast('安装失败：' + (error && error.message ? error.message : String(error)), 'error');
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
        if (res.status === 401) {
          showSessionExpired();
          throw new Error('登录状态已失效，正在跳转到登录页。');
        }
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
        if (resp.status === 401) {
          showSessionExpired();
          throw new Error('登录状态已失效，正在跳转到登录页。');
        }
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
        if (res.status === 401) showSessionExpired();
        if (res.status !== 200) {
          // 原始响应挂在错误上：`inspect` 结果图要显示 HTTP 状态与错误码，
          // 只有 message 的话「哪个分支被拒、拒在哪一层」看不出区别。
          var error = new Error(errorMessage(res));
          error.rpc = res;
          throw error;
        }
        return res.body ? res.body.result : null;
      });
  }

  /// 把一次 RPC 的请求与原始响应摊开给人看（插件探针工具用）。
  /// 失败分支同样渲染——「预期失败」是插件契约里的一等公民，不是异常弹窗。
  function showRpcResult(id, method, params, outcome) {
    // 请求写成两行小字而不是 code 块：这个图层的主角是响应体，请求只是「发去哪、
    // 带了什么」，用 code 会把弹窗撑高一屏，反而把 JSON 挤到折叠线以下。
    var request = [
      { type: 'text', props: { text: 'POST ' + pluginBase(id) + '/rpc', size: 'small' } },
      {
        type: 'text',
        props: {
          text: 'method = ' + method + ' · params = ' + JSON.stringify(params || {}),
          size: 'small'
        }
      }
    ];    var raw = outcome.raw || {};
    var status = outcome.ok ? 200 : raw.status || '—';
    var payload = outcome.ok
      ? outcome.result
      : raw.body || { error: { message: String(outcome.error || '') } };
    var text = '';
    try {
      text = JSON.stringify(payload, null, 2);
    } catch (e) {
      text = String(payload);
    }
    var layer = request.concat([
      {
        type: 'alert',
        props: {
          type: outcome.ok ? 'success' : 'error',
          title: 'HTTP ' + status + (outcome.ok ? ' · 成功分支' : ' · 失败分支'),
          text: outcome.ok
            ? '结果由插件原样返回，壳页未做任何改写。'
            : '错误由内核/插件原样透传；预期失败即通过。'
        }
      },
      { type: 'code', props: { modelValue: buddySafeString(text), language: 'json' } }
    ]);
    var runtime = buddingRuntime();
    if (!runtime || typeof runtime.openLayer !== 'function') {
      toast('RPC ' + method + ' → ' + (outcome.ok ? '成功' : '失败：' + error2text(outcome.error)), outcome.ok ? 'success' : 'error');
      return;
    }
    runtime.openLayer(
      { title: 'RPC · ' + method, preset: 'sm', body: layer },
      { scope: runtime.deriveScope({}, null) }
    );
  }

  function error2text(error) {
    return error && error.message ? error.message : String(error);
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
        { type: 'link', props: { text: '返回插件页', href: prefix + '/plugins/installed' } }
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
          if (res.status === 404) {
            // 旧 HTML 视图没有随包 schema：自动回落到旧 SSR 工作台（iframe），
            // 而不是给用户一张“加载失败”。未知视图回落后仍由服务端 404。
            var legacyUrl = new URL(location.href);
            if (legacyUrl.searchParams.get('legacy') !== '1') {
              legacyUrl.searchParams.set('legacy', '1');
              location.replace(legacyUrl.pathname + legacyUrl.search);
              return new Promise(function () {});
            }
            throw new Error('该视图没有随包 schema（可能仍是旧 HTML 界面）');
          }
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
          // 多视图切换由内核三级菜单承担（`pluginSubviews`），页面本身不再插按钮行。
          return pluginResolve(doc.page, ctx);
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
      toast('无法定位当前插件', 'error');
      return Promise.resolve();
    }
    var method = args && args.method;
    if (!method) {
      toast('动作缺少 method', 'error');
      return Promise.resolve();
    }
    var guard = args.confirm
      ? hostConfirm({ title: '确认操作', content: String(args.confirm), danger: true })
      : Promise.resolve(true);
    var params = {};
    if (args.params && typeof args.params === 'object') {
      Object.keys(args.params).forEach(function (k) {
        params[k] = args.params[k];
      });
    }
    // `inspect: true` 是给探针类插件（testkit）用的：只调用、不刷新、不弹 toast，
    // 把请求与原始响应摊在图层里。写操作仍走同一条 RPC 通道，只是呈现方式不同。
    var inspect = args.inspect === true || args.inspect === 'true';
    // `reload` / `view` / `label` / `inspect` 是壳层自己消费的开关，不能顺手
    // 混进 params 发给插件（插件会收到莫名其妙的多余参数）。
    var reserved = { method: 1, params: 1, confirm: 1, reload: 1, inspect: 1, view: 1, label: 1 };
    Object.keys(args).forEach(function (key) {
      if (!reserved[key]) params[key] = args[key];
    });
    // 表单字段优先于静态 params；空值等价于未填写，不随请求发送。
    var values = ctx ? formValues(ctx) : {};
    Object.keys(values).forEach(function (key) {
      var v = values[key];
      if (v !== null && v !== undefined && String(v).trim() !== '') params[key] = v;
    });
    return guard
      .then(function (ok) {
        if (!ok) return;
        if (inspect) {
          return pluginRpcCall(id, method, params).then(
            function (result) {
              showRpcResult(id, method, params, { ok: true, result: result });
            },
            function (error) {
              showRpcResult(id, method, params, { ok: false, error: error, raw: error.rpc });
            }
          );
        }
        return pluginRpcCall(id, method, params).then(function () {
          toast('操作已提交', 'success');
          location.reload();
        });
      })
      .catch(function (error) {
        toast('操作失败：' + error2text(error), 'error');
      });
  }

  function pageSchema(kind) {
    var parts = routeParts();
    if (kind === 'overview') {
      // 健康槽位的插件 RPC 已在服务端取好，前端只渲染。
      return fetchServerSchema('/overview?with_schema=true');
    }
    if (kind === 'sites') {
      // 页面骨架由服务端 UI 端点产出（布局随角色与站点记录变化）；
      // 表格自身按 `source` 走 `/ui/v1/sites` 取数，分页请求不带 `with_schema`。
      return fetchRawJson(apiBase + '/sites?limit=1&with_schema=true').then(function (body) {
        if (!body || !body.schema) throw new Error('站点列表未返回页面结构');
        return prepareServerSchema(body.schema);
      });
    }
    if (kind === 'site-new') {
      // 类型预选来自地址栏；运行环境下拉由服务端按当前用户可见范围给出。
      var newType = new URLSearchParams(location.search).get('type') || '';
      return fetchServerSchema(
        '/pages/sites-new' + (newType ? '?type=' + encodeURIComponent(newType) : '')
      );
    }
    if (kind === 'site-detail') {
      // 第三段是 L3 子视图（空 = 概览）。Schema 由服务端按 tab 产出，前端只负责渲染。
      var siteTab = String(parts[2] || '').toLowerCase();
      var detailPath =
        '/sites/' + encodeURIComponent(parts[1]) + (siteTab ? '?tab=' + encodeURIComponent(siteTab) : '');
      return fetchRawJson(apiBase + detailPath).then(function (body) {
        if (!body || !body.schema) throw new Error('站点详情未返回页面结构');
        return prepareServerSchema(body.schema);
      });
    }
    if (kind === 'runtimes') {
      return fetchServerSchema('/runtimes?limit=1&with_schema=true');
    }
    if (kind === 'runtime-detail') {
      var runtimeId = parts[2] || '';
      // 第四段是 L3 子视图（空 = 概览）；`?file=` 只在配置视图里有意义。
      var runtimeTab = String(parts[3] || '').toLowerCase() || 'overview';
      var fileKey = new URLSearchParams(location.search).get('file') || '';
      var detailQuery = 'tab=' + encodeURIComponent(runtimeTab) + '&with_schema=true';
      if (fileKey) detailQuery += '&file=' + encodeURIComponent(fileKey);
      return fetchServerSchema('/runtimes/' + encodeURIComponent(runtimeId) + '?' + detailQuery);
    }
    if (kind === 'containers') {
      var containerFilter =
        new URLSearchParams(location.search).get('filter') === 'error' ? 'filter=error&' : '';
      // 页面骨架由服务端 UI 端点产出；表格自身按 `source` 游标分页，不带 with_schema。
      return fetchServerSchema('/containers?' + containerFilter + 'limit=1&with_schema=true');
    }
    if (kind === 'container-detail') {
      return fetchServerSchema(
        '/containers/' + encodeURIComponent(parts[2]) + '?with_schema=true'
      );
    }
    if (kind === 'compose') {
      return fetchServerSchema('/compose?with_schema=true');
    }
    if (kind === 'tasks') {
      var tasksQ = new URLSearchParams(location.search).get('q') || '';
      return fetchServerSchema(
        '/tasks?limit=1&with_schema=true' + (tasksQ ? '&q=' + encodeURIComponent(tasksQ) : '')
      );
    }
    if (kind === 'task-detail') {
      return fetchServerSchema('/tasks/' + encodeURIComponent(parts[1]) + '?with_schema=true');
    }
    if (kind === 'jobs') {
      return fetchServerSchema('/jobs?limit=1&with_schema=true');
    }
    if (kind === 'job-detail') {
      return fetchServerSchema('/jobs/' + encodeURIComponent(parts[1]) + '?with_schema=true');
    }
    if (kind === 'files-trash') {
      return fetchRawJson(prefix + '/files/trash/list?with_schema=true').then(function (body) {
        return prepareServerSchema((body || {}).schema);
      });
    }
    if (kind === 'files-quotas') {
      return fetchRawJson(prefix + '/files/quotas/list?with_schema=true').then(function (body) {
        return prepareServerSchema((body || {}).schema);
      });
    }
    if (kind === 'files') {
      var filePath = new URLSearchParams(location.search).get('path') || '';
      return fetchServerSchema(
        '/files' +
          (filePath ? '?path=' + encodeURIComponent(filePath) + '&with_schema=true' : '?with_schema=true')
      );
    }
    if (kind === 'file-view') {
      var viewPath = new URLSearchParams(location.search).get('path') || '';
      if (!viewPath) {
        return Promise.resolve(
          notReadyPage('代码查看器', '缺少 path 参数。', '/files', '返回文件管理')
        );
      }
      return fetchServerSchema(
        '/files/read?path=' + encodeURIComponent(viewPath) + '&with_schema=true'
      );
    }
    if (kind === 'site-logs') {
      var logSiteId = parts[1] || '';
      var logDomain = '';
      var logQuery = new URLSearchParams(location.search);
      var logPart = logQuery.get('part') === 'error' ? 'error' : 'access';
      var logTail = logQuery.get('tail') || '200';
      var logKeyword = logQuery.get('q') || '';
      return fetchServerSchema(
        '/sites/' +
          encodeURIComponent(logSiteId) +
          '/logs?part=' +
          logPart +
          '&tail=' +
          encodeURIComponent(logTail) +
          '&q=' +
          encodeURIComponent(logKeyword) +
          '&with_schema=true'
      );
    }
    if (kind === 'job-logs') {
      var logJobId = parts[1] || '';
      return fetchServerSchema(
        '/jobs/' + encodeURIComponent(logJobId) + '/logs?with_schema=true'
      ).catch(function (error) {
        return logErrorPage(
          '作业日志',
          prefix + '/jobs/' + encodeURIComponent(logJobId),
          '返回作业详情',
          error
        );
      });
    }
    if (kind === 'container-logs') {
      var logContainerId = parts[2] || '';
      return fetchServerSchema(
        '/containers/' + encodeURIComponent(logContainerId) + '/logs?tail=500&with_schema=true'
      ).catch(function (error) {
        return logErrorPage(
          '容器日志',
          prefix + '/docker/containers/' + encodeURIComponent(logContainerId),
          '返回容器详情',
          error
        );
      });
    }
    if (kind === 'runtime-logs') {
      var logRuntimeId = parts[2] || '';
      return fetchServerSchema(
        '/runtimes/' + encodeURIComponent(logRuntimeId) + '/logs?tail=300&with_schema=true'
      ).catch(
        function (error) {
          return logErrorPage(
            '运行环境日志',
            prefix + '/sites/runtimes/' + encodeURIComponent(logRuntimeId),
            '返回运行环境详情',
            error
          );
        }
      );
    }
    if (kind === 'container-terminal') {
      var terminalId = parts[2] || '';
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
                  href: prefix + '/docker/containers/' + encodeURIComponent(terminalId)
                }
              }
            ]
          };
        }
      );
    }
    if (kind === 'plugins') {
      var pluginTab = parts[1] === 'discover' || parts[1] === 'updates' ? parts[1] : 'installed';
      return fetchServerSchema(
        '/plugins?tab=' + encodeURIComponent(pluginTab) + '&with_schema=true'
      );
    }
    if (kind === 'plugin-info') {
      return fetchServerSchema('/plugins/' + encodeURIComponent(parts[1]) + '?with_schema=true');
    }
    if (kind === 'plugin-view') {
      return pluginPage(parts[1] || '', parts[2] || '');
    }
    if (kind === 'store') {
      var refresh =
        new URLSearchParams(location.search).get('refresh') === '1' ? 'refresh=1&' : '';
      return fetchServerSchema('/store?' + refresh + 'with_schema=true');
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
              props: { text: '返回商店', href: prefix + '/plugins/store' }
            }
          ]
        });
      }
      return Promise.resolve(storeConfirmSchema(pendingConfirm));
    }
    if (kind === 'web') {
      var webQuery = new URLSearchParams(location.search);
      var webSub = String(parts[2] || '').toLowerCase();
      var webView =
        webSub === 'config' || webSub === 'logs' || webSub === 'modules' ? webSub : 'status';
      var webParams = ['view=' + webView, 'with_schema=true'];
      if (webView === 'logs') {
        webParams.push('load=1');
        webParams.push('kind=' + encodeURIComponent(webQuery.get('type') || 'access'));
        webParams.push('lines=' + encodeURIComponent(webQuery.get('lines') || '200'));
        webParams.push('level=' + encodeURIComponent(webQuery.get('level') || 'all'));
        if (webQuery.get('status')) {
          webParams.push('status=' + encodeURIComponent(webQuery.get('status')));
        }
        if (webQuery.get('q')) {
          webParams.push('q=' + encodeURIComponent(webQuery.get('q')));
        }
      }
      return fetchServerSchema('/web?' + webParams.join('&'));
    }
    if (kind === 'settings') {
      return fetchServerSchema('/settings?with_schema=true');
    }
    if (kind === 'menu-settings') {
      return fetchServerSchema('/settings/menu?with_schema=true');
    }
    if (kind === 'docker-settings') {
      return fetchServerSchema('/settings/docker?with_schema=true');
    }
    if (kind === 'ui-settings') {
      return fetchServerSchema('/settings/ui?with_schema=true');
    }
    if (kind === 'audit') {
      var auditParams = new URLSearchParams(location.search);
      var auditQ = auditParams.get('q') || '';
      var auditCursor = auditParams.get('cursor') || '';
      var auditQuery =
        '?limit=50&with_schema=true' +
        (auditQ ? '&q=' + encodeURIComponent(auditQ) : '') +
        (auditCursor ? '&cursor=' + encodeURIComponent(auditCursor) : '');
      return fetchServerSchema('/audit' + auditQuery);
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
        // 地址变了就重画三级菜单：SPA 跳转不触发 popstate，只能在这里跟。
        renderSubNav();
        return out;
      };
    });
    window.addEventListener('popstate', function () {
      renderSubNav();
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
          showSessionExpired();
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
          var routes = buildRoutes(menu);
          embed(shellSchema(), routes);
          markShellChrome();
          mountSubNav();
        });
      })
      .catch(function (error) {
        showError('无法连接服务：' + (error && error.message ? error.message : String(error)));
      });
  }

  watchQueryOnlyHistory();
  boot();
})();

