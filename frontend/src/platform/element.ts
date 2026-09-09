import { LitElement } from "lit";

/**
 * cmx-agent 业务组件基类：统一挂载点与调试标识（后续按 Phase 1 扩展错误冒泡等）。
 */
export abstract class CmxAgentElement extends LitElement {
  /** 组件调试标签（控制台过滤用）。 */
  protected get debugTag(): string {
    return this.tagName.toLowerCase();
  }
}
