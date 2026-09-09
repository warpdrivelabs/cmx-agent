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

/** 尾加载窗口内的审批挂起（从事件流重放恢复用）。 */
export type ApprovalRequest = Extract<SessionEvent["kind"], { kind: "approval_requested" }> & {
  seq: number;
  session_id: string;
};
