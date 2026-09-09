/** @type {import('stylelint').Config} */
export default {
  rules: {
    // 裸色值禁令：默认全禁（组件与别名层），seed.css 豁免
    "color-no-hex": true
  },
  overrides: [
    {
      // 旧工作台主题定义层（--bg/--aqua 等变量本体，与 seed 同类豁免）
      files: ["src/styles/ws.css"],
      rules: {
        "color-no-hex": null
      }
    },
    {
      // 解析 Lit 组件内的 css`` 模板
      files: ["**/*.ts"],
      customSyntax: "postcss-lit",
      rules: {
        "color-no-hex": true
      }
    },
    {
      // 种子层：全工程唯一定义原始值的地方
      files: ["src/styles/seed.css"],
      rules: {
        "color-no-hex": null
      }
    }
  ]
};
