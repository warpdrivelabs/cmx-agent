/** 前门响应信封（对齐 protocol.rs AppResponse）。 */

import type { EventsWindow, SessionMeta } from "./events-window";
import type { SessionEvent } from "./session-event";

export interface AppResponse<TData = unknown> {
  ok: boolean;
  data?: TData;
  error?: AppError;
}

export interface AppError {
  code: string;
  message: string;
}

/** 各命令 data 结构（核心命令精确；连接器/插件/模型细节 Phase 3/4 迁移时按实测收紧）。 */
export interface AuthUser {
  username?: string;
  display_name?: string;
  [key: string]: unknown;
}

export type LoginData = { user: AuthUser | null };
export type CurrentUserData = { user: AuthUser | null };
export type ListSessionsData = { sessions: SessionMeta[] };
export type GetEventsData = EventsWindow;
export type DeleteSessionData = { deleted: string };
export type CreateSessionData = SessionMeta;
export type SetPolicyData = { sandbox: string; approval: string };
export type ApproveData = {
  resolved: boolean;
  call_id: string;
  approved: boolean;
  all: boolean;
};
export type ImBindingsData = { bindings: Array<Record<string, unknown>> };

/** 模型列表（对齐旧 UI openModelMenu：provider + 候选 + 当前 + 可配置标记）。 */
export type ModelCandidate = { model: string; label?: string };
export type ListModelsData = {
  provider?: string;
  base_url?: string;
  current: string;
  candidates: ModelCandidate[];
  configurable?: boolean;
};
export type SetModelData = { model: string; note?: string };

/** 多 Provider（对齐旧 UI 重制版：list_providers / set_active_provider / delete_provider）。 */
export type ProviderCandidate = { model: string; label?: string };
export type ProviderEntry = {
  id: string;
  name?: string;
  model?: string;
  active?: boolean;
  builtin?: boolean;
  configured_key?: boolean;
  candidates?: ProviderCandidate[];
};
export type ListProvidersData = { providers: ProviderEntry[] };
export type ProviderConfigData = {
  name?: string;
  base_url?: string;
  api_key_masked?: string;
  model?: string;
  temperature?: number;
  timeout_ms?: number;
  candidates?: string[];
  builtin?: boolean;
  active?: boolean;
};
export type SaveModelConfigData = { note?: string; id?: string };

/** 连接器（对齐旧 UI loadConnectors：卡片 + 健康状态）。 */
export type ConnectorStatus = {
  online: boolean;
  service_name?: string;
  uptime_secs?: number;
  latency_ms?: number;
};
export type Connector = {
  id: string;
  name: string;
  service: string;
  base_url: string;
  description?: string;
  tools?: string[];
  status?: ConnectorStatus;
};
export type ListConnectorsData = { connectors: Connector[] };

/** 插件（对齐旧 UI 插件 master-detail；市场条目带内嵌 manifest 供安装）。 */
export type PluginInfo = {
  name: string;
  kind: string;
  version?: string;
  author?: string;
  description?: string;
  icon?: string;
  homepage?: string;
  enabled?: boolean;
  requires_approval?: boolean;
  permissions?: string[];
  base_url?: string;
  method?: string;
  path?: string;
  command?: string;
  args?: string[];
  module?: string;
  invoke?: string;
  runtime?: string;
  manifest?: Record<string, unknown>;
  [key: string]: unknown;
};
export type ListPluginsData = { installed: PluginInfo[]; market: PluginInfo[] };
export type PluginOpData = { note?: string; name?: string; [key: string]: unknown };

/** 模型配置（旧版单条命令遗留，多 Provider 版见 ProviderConfigData）。 */
export type ModelConfigData = {
  base_url?: string;
  api_key_masked?: string;
  model?: string;
  temperature?: number;
  timeout_ms?: number;
  candidates?: string[];
  configurable?: boolean;
};
export type LegacyModelConfigSaveData = { note?: string };

/** 尾加载窗口内的审批挂起（从事件流重放恢复用）。 */
export type ApprovalRequest = Extract<SessionEvent, { kind: "approval_requested" }> & {
  seq: number;
  session_id: string;
};
