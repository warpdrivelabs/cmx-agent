import { setTheme as ui5SetTheme } from "@ui5/webcomponents-base/dist/config/Theme.js";

/**
 * UI5 setTheme 唯一薄封装（方案 §13.1）：只允许 theme-store 调用。
 */
export function setTheme(theme: "sap_horizon" | "sap_horizon_dark"): void {
  void ui5SetTheme(theme);
}
