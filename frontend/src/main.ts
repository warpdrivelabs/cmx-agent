import "./styles/seed.css";
import "./styles/tokens.css";
import "./styles/ui5-bridge.css";
import "./styles/base.css";
import "./styles/ws.css";
import "./app/cmx-agent-app.css";
import { bootstrapUi5 } from "./platform/ui5/bootstrap";
import { themeStore } from "./platform/stores/theme-store";
import { sessionStore } from "./platform/stores/session-store";
import { settingsStore } from "./platform/stores/settings-store";
import "./app/cmx-agent-app";

// 装载 UI5（一次）并应用持久化主题（tone 双写：seed 块 + UI5 变量底座）
bootstrapUi5();
themeStore.init();
sessionStore.start(); // 被动事件总线订阅（IM 遥控实时显示）
void settingsStore.init(); // 两旋钮 localStorage 记忆恢复
