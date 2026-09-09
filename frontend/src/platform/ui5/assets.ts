/**
 * UI5 资产精简注册（方案 §13.2 Phase 1 收敛项）：
 * 聚合 `@ui5/webcomponents/dist/Assets.js` 会注册全部 79 个 locale（dist 曾因此膨胀 98%），
 * 这里只注册 zh_CN + en（其他系统 locale 由 UI5 fallback 到 en）。
 * 主题参数与图标非 per-locale，原样全量注册（按需 chunk）。
 */
import { registerI18nLoader } from "@ui5/webcomponents-base/dist/asset-registries/i18n.js";
import { registerLocaleDataLoader } from "@ui5/webcomponents-base/dist/asset-registries/LocaleData.js";

// 主题参数（theming 包 + 主包；亮暗主题必需，不精简）
import "@ui5/webcomponents-theming/dist/generated/json-imports/Themes.js";
import "@ui5/webcomponents/dist/generated/json-imports/Themes.js";
// 图标文本（非 per-locale）
import "@ui5/webcomponents-icons/dist/generated/json-imports/i18n.js";
// 图标集 loader（SAP-icons-v4/v5；busyindicator / button icon 等控件运行时按需加载）
import "@ui5/webcomponents-icons/dist/json-imports/Icons.js";

/** 主包 messagebundle：zh_CN + en。bundle 名与官方 generated 文件一致（拼接防扫描）。 */
const MAIN_BUNDLE = "@" + "ui5/webcomponents";
registerI18nLoader(MAIN_BUNDLE, "zh_CN", async () => {
  return (await import("@ui5/webcomponents/dist/generated/assets/i18n/messagebundle_zh_CN.json"))
    .default;
});
registerI18nLoader(MAIN_BUNDLE, "en", async () => {
  return (await import("@ui5/webcomponents/dist/generated/assets/i18n/messagebundle_en.json"))
    .default;
});

/** CLDR 本地化数据（日期/数字格式等）：zh_CN + en。 */
registerLocaleDataLoader("zh_CN", async () => {
  return (await import("@ui5/webcomponents-localization/dist/generated/assets/cldr/zh_CN.json"))
    .default as never; // CLDR JSON 含 number 字段，与 UI5 registry 的宽松 index signature 不完全匹配（官方 generated 同样 nocheck）
});
registerLocaleDataLoader("en", async () => {
  return (await import("@ui5/webcomponents-localization/dist/generated/assets/cldr/en.json"))
    .default as never;
});
