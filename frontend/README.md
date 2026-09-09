# cmx-agent frontend

Web 壳与 Tauri 壳共享的前端工程（Vite + TypeScript 7 + Lit + UI5 WebComponents，npm 管理）。

完整设计见 `documents/plans/20260908_cmx-agent_前端组件化重构方案.md`（工作区根仓）。

## 常用命令

```bash
npm install
npm run dev      # Vite dev server，:5174，/api 代理到 127.0.0.1:18080
npm run check    # tsc + eslint + prettier + stylelint（提交前必须通过）
npm test         # vitest 单测
npm run build    # 产出 dist/（双壳共用）
```

## Token 语义表（组件只允许消费别名层）

| token                                                              | 用途                                      |
| ------------------------------------------------------------------ | ----------------------------------------- |
| `--cmx-agent-color-primary` / `-hover` / `-active` / `-bg`         | 品牌主色系（AntD 蓝），主按钮/链接/选中态 |
| `--cmx-agent-color-success` / `-warning` / `-error` / `-info`      | 语义色（成功提示/警告/错误/信息）         |
| `--cmx-agent-color-text` / `-secondary` / `-tertiary` / `-inverse` | 文本四级梯度                              |
| `--cmx-agent-bg-container` / `-layout` / `-elevated` / `-hover`    | 容器/页面/浮层/悬停背景                   |
| `--cmx-agent-color-border` / `-secondary`                          | 边框两级                                  |
| `--cmx-agent-border-radius` / `-lg` / `-sm`                        | 圆角（6/8/4，AntD 对齐）                  |
| `--cmx-agent-control-height` / `-lg` / `-sm`                       | 控件高度梯度                              |
| `--cmx-agent-font-size` / `-lg` / `-sm`                            | 字号（14/16/12）                          |
| `--cmx-agent-box-shadow` / `-secondary`                            | 浮层阴影（skin=none 时扁平化）            |
| `--cmx-agent-space-xs/sm/md/lg/xl`                                 | 间距 4/8/12/16/24                         |

## 骨架组件（新组件先看这里）

| 组件                                                               | 用途                                          |
| ------------------------------------------------------------------ | --------------------------------------------- |
| `cmx-agent-panel-card`                                             | 「标题+内容+操作条」面板（设置面板/卡片容器） |
| `cmx-agent-filter-list`                                            | 「搜索+列表+空态」（会话/插件等列表）         |
| `cmx-agent-empty-state` / `error-note` / `loading-block` / `toast` | 空态/错误信封/加载/全局反馈                   |

## 强制规范（agent / 开发者必读）

1. 颜色 / 圆角 / 间距 / 字号 / 阴影：只允许 `var(--cmx-agent-*)`；裸值只出现在 `src/styles/seed.css`。
2. 禁止直接使用 `--sap*`（只有 `src/styles/ui5-bridge.css` 可以）与 `--_ui5_*`（任何地方都不可以）。
3. 新组件前先过三问：UI5 有现成控件吗？已有骨架（panel-card / filter-list）能 slot 吗？和现有组件像吗？
4. UI5 有现成控件就用，禁止写转发包装组件。
5. 状态只经 `platform/stores`，组件不 import bridge 协议文件（call / stream / session-event）。
6. 依赖方向单向：`app → features → platform → protocol`；features 之间禁止互相 import。
