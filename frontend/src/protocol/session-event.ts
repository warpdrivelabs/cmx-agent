/** 会话事件（对齐 cmx-agent-core event.rs：seq/ts + kind 联合，serde tag="kind" snake_case）。 */

export interface ToolCall {
  id: string;
  name: string;
  arguments: Record<string, unknown>;
}

export type EventKind =
  | { kind: "turn_started"; turn: number; user_input: string }
  | { kind: "user_message"; text: string }
  | { kind: "model_message"; text?: string | null; tool_calls?: ToolCall[] }
  | { kind: "tool_invoked"; call: ToolCall }
  | {
      kind: "guard_decision";
      call_id: string;
      phase: string;
      guard: string;
      decision: Record<string, unknown>;
    }
  | { kind: "approval_requested"; call_id: string; tool: string; reason: string }
  | { kind: "approval_resolved"; call_id: string; approved: boolean; by: string }
  | { kind: "tool_result"; call_id: string; ok: boolean; output: unknown }
  | { kind: "turn_ended"; turn: number; reason: StopReason; steps: number }
  | { kind: "note"; text: string };

export type StopReason = "completed" | "max_steps" | "stopped" | "error";

export interface SessionEvent {
  seq: number;
  ts: string;
  kind: EventKind;
}

/** 总线事件信封（对齐 bus.rs EventEnvelope）：订阅者据此按 session_id 分流。 */
export interface SessionEventEnvelope {
  session_id: string;
  event: SessionEvent;
}

/** 回合流终止标记（stream 通道补发，不在事件日志内）。 */
export type StreamTerminator =
  | { kind: "stream_done" }
  | { kind: "stream_error"; message?: string }
  | { kind: "text_delta"; text: string };
