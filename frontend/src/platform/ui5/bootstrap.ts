/**
 * UI5 装载唯一入口（方案 §6.3）：
 * 导入 Assets（i18n / 字体 / 主题参数，全部本地打包不出网）、按需注册控件。
 * UI5 WebComponents 2.x 在首个控件模块导入时自动 boot，无需手动调用。
 * 不暴露主题切换 API——主题归 theme-store（13.3）。
 */
import "./assets";
import "@ui5/webcomponents/dist/Button.js";
import "@ui5/webcomponents/dist/Input.js";
import "@ui5/webcomponents/dist/Label.js";
import "@ui5/webcomponents/dist/MessageStrip.js";
import "@ui5/webcomponents/dist/Title.js";

export function bootstrapUi5(): void {
  // 2.x 自动 boot；本函数保留为显式装载入口（Assets 副作用 import 已在模块顶部执行）
}
