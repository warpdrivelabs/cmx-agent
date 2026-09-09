/** 会话元数据与事件分页窗口（对齐 store.rs SessionMeta / get_events_window）。 */

import type { SessionEvent } from "./session-event";

export interface SessionMeta {
  id: string;
  title?: string | null;
  system?: string | null;
  created_at: string;
  updated_at: string;
  event_count: number;
}

export interface EventsWindow {
  events: SessionEvent[];
  /** 会话事件总数。 */
  total: number;
  /** 本窗口首事件在全集中的下标（向前翻页游标）。 */
  start: number;
  /** 会话标题。 */
  title: string;
}
