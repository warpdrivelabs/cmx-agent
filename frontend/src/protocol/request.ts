/** JSON 前门请求（对齐 cmx-agent-app protocol.rs AppRequest，serde tag="cmd" snake_case）。 */

import type { Policy } from "./policy";

export type AppRequest =
  | { cmd: "create_session"; id: string }
  | { cmd: "send"; session_id: string; text: string }
  | { cmd: "get_events"; session_id: string; limit?: number; before?: number }
  | { cmd: "list_sessions" }
  | { cmd: "delete_session"; session_id: string }
  | { cmd: "list_connectors" }
  | { cmd: "list_plugins" }
  | { cmd: "list_providers" }
  | { cmd: "set_active_provider"; id: string }
  | { cmd: "delete_provider"; id: string }
  | { cmd: "install_plugin"; manifest: unknown }
  | { cmd: "uninstall_plugin"; name: string }
  | { cmd: "toggle_plugin"; name: string; enabled: boolean }
  | { cmd: "list_models" }
  | { cmd: "set_model"; model: string; provider_id?: string }
  | { cmd: "set_policy"; sandbox: Policy["sandbox"]; approval: Policy["approval"] }
  | { cmd: "get_model_config"; id?: string }
  | {
      cmd: "set_model_config";
      id?: string;
      name?: string;
      base_url: string;
      model: string;
      temperature?: number;
      timeout_ms?: number;
      api_key_action?: "keep" | "set";
      api_key_value?: string;
    }
  | { cmd: "login"; username: string; password: string }
  | { cmd: "current_user" }
  | { cmd: "logout" }
  | {
      cmd: "approve";
      call_id: string;
      approved: boolean;
      all?: boolean;
      session_id?: string;
    }
  | { cmd: "im_bind_gen_code" }
  | { cmd: "im_list_bindings" }
  | { cmd: "im_unbind"; provider: string; open_id: string };
