// 首屏主题：在样式表之前同步执行，按 localStorage 选择设置 <html data-theme>，
// 避免暗色用户刷新时先渲染浅色再跳成深色（闪白）。键名必须与 skiff.js 保持一致。
(function () {
  var KEY = "skiff.theme";
  try {
    var saved = localStorage.getItem(KEY);
    if (saved === "dark" || saved === "light") {
      document.documentElement.setAttribute("data-theme", saved);
    }
  } catch (e) {
    // 隐私模式 / 禁用存储：保持样式表默认的浅色。
  }
})();
