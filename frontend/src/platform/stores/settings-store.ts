/** 设置域 store（方案 §8.3）：两旋钮 + 模型 + 模型配置 + IM/插件/连接器（面板唯一数据源）。 */
import { call } from "../bridge/call";
import { storeBus } from "./bus";
import type { SelectableStore } from "../store-controller";
import type { ApprovalPolicy, Policy, SandboxMode } from "../../protocol/policy";

const POLICY_KEY = "cmx-agent.policy";

export interface ModelConfig {
  base_url: string;
  model: string;
  temperature?: number;
  timeout_ms?: number;
  api_key_masked?: string;
  has_api_key?: boolean;
}

export interface ImBinding {
  provider: string;
  open_id: string;
  created_at?: string;
}

export interface PluginInfo {
  name: string;
  kind?: string;
  version?: string;
  description?: string;
  enabled?: boolean;
  installed?: boolean;
  market?: boolean;
  [key: string]: unknown;
}

export interface ConnectorCard {
  name: string;
  description?: string;
  live?: boolean;
  [key: string]: unknown;
}

export interface SettingsState {
  policy: Policy;
  models: string[];
  currentModel: string;
  modelConfig: ModelConfig | null;
  imBindings: ImBinding[];
  imCode: string;
  plugins: PluginInfo[];
  connectors: ConnectorCard[];
  pending: boolean;
  error: string | null;
}

function readPolicy(): Policy {
  try {
    const raw = localStorage.getItem(POLICY_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as Policy;
      if (parsed.sandbox && parsed.approval) return parsed;
    }
  } catch {
    // 损坏回落默认
  }
  return { sandbox: "read-only", approval: "on-request" };
}

class SettingsStore implements SelectableStore<SettingsState> {
  private state: SettingsState = {
    policy: readPolicy(),
    models: [],
    currentModel: "",
    modelConfig: null,
    imBindings: [],
    imCode: "",
    plugins: [],
    connectors: [],
    pending: false,
    error: null
  };
  private listeners = new Set<() => void>();

  getState(): SettingsState {
    return this.state;
  }

  subscribe<TSelected>(
    selector: (state: SettingsState) => TSelected,
    cb: (next: TSelected) => void
  ): () => void {
    let last = selector(this.state);
    const wrap = () => {
      const next = selector(this.state);
      if (next !== last) {
        last = next;
        cb(next);
      }
    };
    this.listeners.add(wrap);
    return () => this.listeners.delete(wrap);
  }

  private set(patch: Partial<SettingsState>): void {
    this.state = { ...this.state, ...patch };
    this.listeners.forEach((l) => l());
  }

  /** 启动恢复：localStorage 记忆的两旋钮热切（对齐旧 UI 行为）。 */
  async init(): Promise<void> {
    const saved = readPolicy();
    if (saved.sandbox !== "read-only" || saved.approval !== "on-request") {
      await this.setPolicy(saved.sandbox, saved.approval);
    }
  }

  async setPolicy(sandbox: SandboxMode, approval: ApprovalPolicy): Promise<boolean> {
    this.set({ pending: true, error: null });
    const res = await call({ cmd: "set_policy", sandbox, approval });
    if (!res.ok) {
      this.set({ pending: false, error: res.error?.message ?? "策略切换失败" });
      return false;
    }
    const policy = { sandbox, approval };
    this.set({ policy, pending: false });
    try {
      localStorage.setItem(POLICY_KEY, JSON.stringify(policy));
    } catch {
      // 忽略持久化失败
    }
    storeBus.emit("policy:changed", policy);
    return true;
  }

  async loadModels(): Promise<void> {
    const res = await call<{ models: string[]; current?: string }>({ cmd: "list_models" });
    if (res.ok && res.data) {
      this.set({ models: res.data.models ?? [], currentModel: res.data.current ?? "" });
    }
  }

  async setModel(model: string): Promise<boolean> {
    const res = await call<{ model: string }>({ cmd: "set_model", model });
    if (res.ok) {
      this.set({ currentModel: model });
      return true;
    }
    this.set({ error: res.error?.message ?? "模型切换失败" });
    return false;
  }

  async loadModelConfig(): Promise<void> {
    const res = await call<ModelConfig>({ cmd: "get_model_config" });
    if (res.ok && res.data) this.set({ modelConfig: res.data });
  }

  async saveModelConfig(cfg: ModelConfig, apiKey?: string): Promise<boolean> {
    const res = await call({
      cmd: "set_model_config",
      base_url: cfg.base_url,
      model: cfg.model,
      temperature: cfg.temperature,
      timeout_ms: cfg.timeout_ms,
      api_key_action: apiKey ? "set" : "keep",
      api_key_value: apiKey
    });
    if (res.ok) {
      await this.loadModelConfig();
      return true;
    }
    this.set({ error: res.error?.message ?? "保存失败" });
    return false;
  }

  async loadImBindings(): Promise<void> {
    const res = await call<{ bindings: ImBinding[] }>({ cmd: "im_list_bindings" });
    if (res.ok && res.data) this.set({ imBindings: res.data.bindings ?? [] });
  }

  async genImCode(): Promise<string> {
    const res = await call<{ code: string }>({ cmd: "im_bind_gen_code" });
    if (res.ok && res.data) {
      this.set({ imCode: res.data.code });
      return res.data.code;
    }
    this.set({ error: res.error?.message ?? "生成验证码失败" });
    return "";
  }

  async unbindIm(provider: string, openId: string): Promise<boolean> {
    const res = await call({ cmd: "im_unbind", provider, open_id: openId });
    if (res.ok) {
      await this.loadImBindings();
      return true;
    }
    this.set({ error: res.error?.message ?? "解绑失败" });
    return false;
  }

  async loadPlugins(): Promise<void> {
    const res = await call<{ plugins: PluginInfo[] }>({ cmd: "list_plugins" });
    if (res.ok && res.data) this.set({ plugins: res.data.plugins ?? [] });
  }

  async installPlugin(manifest: unknown): Promise<boolean> {
    const res = await call({ cmd: "install_plugin", manifest });
    if (res.ok) {
      await this.loadPlugins();
      return true;
    }
    this.set({ error: res.error?.message ?? "安装失败" });
    return false;
  }

  async uninstallPlugin(name: string): Promise<boolean> {
    const res = await call({ cmd: "uninstall_plugin", name });
    if (res.ok) {
      await this.loadPlugins();
      return true;
    }
    this.set({ error: res.error?.message ?? "卸载失败" });
    return false;
  }

  async togglePlugin(name: string, enabled: boolean): Promise<boolean> {
    const res = await call({ cmd: "toggle_plugin", name, enabled });
    if (res.ok) {
      await this.loadPlugins();
      return true;
    }
    this.set({ error: res.error?.message ?? "切换失败" });
    return false;
  }

  async loadConnectors(): Promise<void> {
    const res = await call<{ connectors: ConnectorCard[] }>({ cmd: "list_connectors" });
    if (res.ok && res.data) this.set({ connectors: res.data.connectors ?? [] });
  }
}

export const settingsStore = new SettingsStore();
