/** 设置域 store（方案 §8.3）：两旋钮 + 模型 + 模型配置 + IM/插件/连接器（面板唯一数据源）。 */
import { call } from "../bridge/call";
import { storeBus } from "./bus";
import type { SelectableStore } from "../store-controller";
import type { ApprovalPolicy, Policy, SandboxMode } from "../../protocol/policy";
import type {
  Connector,
  ListModelsData,
  ModelConfigData,
  PluginInfo,
  ListProvidersData,
  ProviderEntry,
  ProviderConfigData
} from "../../protocol/response";

const POLICY_KEY = "cmx-agent.policy";

export interface ModelConfig {
  base_url?: string;
  model?: string;
  temperature?: number;
  timeout_ms?: number;
  api_key_masked?: string;
  has_api_key?: boolean;
  candidates?: string[];
}

export interface ImBinding {
  provider: string;
  open_id: string;
  created_at?: string;
}

/** 兼容别名（面板按 installed/market 合并列表消费）。 */
export type { PluginInfo } from "../../protocol/response";

export interface SettingsState {
  policy: Policy;
  models: string[];
  currentModel: string;
  /** 完整模型信息（provider/候选/可配置），模型弹层用。 */
  modelsInfo: ListModelsData | null;
  modelConfig: ModelConfig | null;
  imBindings: ImBinding[];
  imCode: string;
  installedPlugins: PluginInfo[];
  marketPlugins: PluginInfo[];
  /** 已装 + 市场合并（旧插件面板兼容消费）。 */
  plugins: PluginInfo[];
  connectors: Connector[];
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
  // 后端默认 workspace-write/on-request（旧 UI 同源）；显示默认档为「默认权限：工作区可写 · 按需审批」
  return { sandbox: "workspace-write", approval: "on-request" };
}

class SettingsStore implements SelectableStore<SettingsState> {
  private state: SettingsState = {
    policy: readPolicy(),
    models: [],
    currentModel: "",
    modelsInfo: null,
    modelConfig: null,
    imBindings: [],
    imCode: "",
    installedPlugins: [],
    marketPlugins: [],
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
    if (saved.sandbox !== "workspace-write" || saved.approval !== "on-request") {
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

  async loadModels(): Promise<ListModelsData | null> {
    const res = await call<ListModelsData>({ cmd: "list_models" });
    if (res.ok && res.data) {
      const d = res.data;
      this.set({
        modelsInfo: d,
        models: (d.candidates ?? []).map((c) => c.model),
        currentModel: d.current ?? ""
      });
      return d;
    }
    return null;
  }

  /** 多 Provider 快照（旧 openModelMenu / mcfg 左列数据源）。 */
  async listProviders(): Promise<ProviderEntry[]> {
    const res = await call<ListProvidersData>({ cmd: "list_providers" });
    if (res.ok && res.data) return res.data.providers ?? [];
    return [];
  }

  async setActiveProvider(id: string): Promise<{ ok: boolean; note?: string; error?: string }> {
    const res = await call({ cmd: "set_active_provider", id });
    if (res.ok) {
      return { ok: true, note: (res.data as { note?: string } | undefined)?.note };
    }
    return { ok: false, error: res.error?.message ?? "切换失败" };
  }

  async deleteProvider(id: string): Promise<{ ok: boolean; note?: string; error?: string }> {
    const res = await call({ cmd: "delete_provider", id });
    if (res.ok) {
      return { ok: true, note: (res.data as { note?: string } | undefined)?.note };
    }
    return { ok: false, error: res.error?.message ?? "删除失败" };
  }

  async getProviderConfig(id: string): Promise<ProviderConfigData | null> {
    const res = await call<ProviderConfigData>({ cmd: "get_model_config", id });
    if (res.ok && res.data) return res.data;
    return null;
  }

  async setModel(model: string, providerId?: string): Promise<boolean> {
    const res = await call<{ model: string; note?: string }>({
      cmd: "set_model",
      model,
      provider_id: providerId
    });
    if (res.ok) {
      this.set({ currentModel: model });
      return true;
    }
    this.set({ error: res.error?.message ?? "模型切换失败" });
    return false;
  }

  async loadModelConfig(): Promise<void> {
    const res = await call<ModelConfigData>({ cmd: "get_model_config" });
    if (res.ok && res.data) this.set({ modelConfig: res.data });
  }

  async saveModelConfig(cfg: ModelConfig, apiKey?: string): Promise<boolean> {
    const res = await call({
      cmd: "set_model_config",
      base_url: cfg.base_url ?? "",
      model: cfg.model ?? "",
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

  /** 多 Provider 保存（旧 saveModelConfig 重制版：id 定位条目，name 支持自定义）。 */
  async saveProviderConfig(p: {
    id?: string;
    name?: string;
    base_url: string;
    model: string;
    temperature: number;
    timeout_ms: number;
    api_key_action: "keep" | "set";
    api_key_value?: string;
  }): Promise<{ ok: boolean; note?: string; id?: string; error?: string }> {
    const res = await call({
      cmd: "set_model_config",
      id: p.id,
      name: p.name,
      base_url: p.base_url,
      model: p.model,
      temperature: p.temperature,
      timeout_ms: p.timeout_ms,
      api_key_action: p.api_key_action,
      api_key_value: p.api_key_value
    });
    if (res.ok) {
      const d = res.data as { note?: string; id?: string } | undefined;
      return { ok: true, note: d?.note, id: d?.id };
    }
    return { ok: false, error: res.error?.message ?? "保存失败" };
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
    const res = await call<{ installed: PluginInfo[]; market: PluginInfo[] }>({
      cmd: "list_plugins"
    });
    if (res.ok && res.data) {
      const installed = res.data.installed ?? [];
      const market = res.data.market ?? [];
      this.set({
        installedPlugins: installed,
        marketPlugins: market,
        plugins: [...installed, ...market.map((m) => ({ ...m, installed: false, market: true }))]
      });
    }
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

  async loadConnectors(): Promise<Connector[]> {
    const res = await call<{ connectors: Connector[] }>({ cmd: "list_connectors" });
    if (res.ok && res.data) {
      this.set({ connectors: res.data.connectors ?? [] });
      return this.state.connectors;
    }
    return [];
  }
}

export const settingsStore = new SettingsStore();
