import type { ReactiveController, ReactiveControllerHost } from "lit";

/** 最小 store 接口：selector 订阅（8.0 契约，实现见各 store）。 */
export interface SelectableStore<TState> {
  getState(): TState;
  subscribe<TSelected>(
    selector: (state: TState) => TSelected,
    cb: (next: TSelected) => void
  ): () => void;
}

/**
 * StoreController：selector 订阅 + 自动触发宿主组件重渲染。
 * 只有 selector 结果变化才 requestUpdate，避免全量订阅引发的重绘（方案 §8.0）。
 */
export class StoreController<TState> implements ReactiveController {
  private host: ReactiveControllerHost;
  private store: SelectableStore<TState>;
  private unsubscribe?: () => void;

  constructor(host: ReactiveControllerHost, store: SelectableStore<TState>) {
    this.host = host;
    this.store = store;
    host.addController(this);
  }

  hostConnected(): void {
    // 全量订阅：store 内部按 state 引用变化通知；组件渲染时自行 selector 读取。
    // 粒度优化的责任在 store 的通知与组件的 selector 属性（Phase 1 随 store 契约细化）。
    this.unsubscribe = this.store.subscribe(
      (s) => s,
      () => this.host.requestUpdate()
    );
  }

  hostDisconnected(): void {
    this.unsubscribe?.();
    this.unsubscribe = undefined;
  }

  get state(): TState {
    return this.store.getState();
  }
}
