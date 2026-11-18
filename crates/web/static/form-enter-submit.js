/**
 * budding 表单「回车即提交」补丁（壳页与登录页共用）。
 *
 * budding 的 `form` 把动作按钮渲染成普通 `button`（没有原生 submit 语义），
 * 于是键盘用户在筛选框 / 新建弹窗 / 登录框里按回车没有任何反应，只能挪到鼠标。
 * 这里做一次全局委托：回车落在单行 `input` 上且处于 `.bud-form` 内时，点主按钮。
 *
 * 三条边界：
 *   - `textarea` 保留换行（只认 `INPUT`）；
 *   - 多选 / 文件 / 复选类输入（`checkbox` / `radio` / `file` / `date` 等）回车另有
 *     语义，不接管；
 *   - 已被上游或组件 `preventDefault` 的事件不再接管，避免双提交。
 *
 * 事件委托挂在 `document` 上，因此**脚本加载时表单还没渲染也没关系**：登录页的
 * 表单是 `budding-login.js` 后渲染的，壳页各页表单是路由切换后渲染的，都能命中。
 * 两个页面都引用本文件，用 `window.__skiffEnterSubmit` 防重复绑定。
 */
(function () {
  if (window.__skiffEnterSubmit) return;
  window.__skiffEnterSubmit = true;

  var SINGLE_LINE = {
    text: 1,
    search: 1,
    password: 1,
    email: 1,
    number: 1,
    tel: 1,
    url: 1,
  };

  document.addEventListener('keydown', function (event) {
    if (event.key !== 'Enter' || event.defaultPrevented || event.isComposing) return;
    var el = event.target;
    if (!el || el.tagName !== 'INPUT') return;
    var type = (el.getAttribute('type') || 'text').toLowerCase();
    if (!SINGLE_LINE[type]) return;
    var form = el.closest('.bud-form');
    if (!form) return;
    var btn = form.querySelector('.bud-form-actions__primary button');
    if (!btn) btn = form.querySelector('.bud-form-actions button');
    if (!btn || btn.disabled) return;
    event.preventDefault();
    btn.click();
  });
})();
