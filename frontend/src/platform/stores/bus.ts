/** store 间领域事件总线（方案 §8.0：store 不互相调用 action，经事件解耦）。 */

export interface DomainEvents {
  "auth:changed": { user: unknown | null };
  "policy:changed": { sandbox: string; approval: string };
  "session:switched": { sessionId: string | null };
  "theme:changed": { tone: "light" | "dark"; skin: "plain" | "none" };
}

type EventKey = keyof DomainEvents;

class EventBus {
  private listeners = new Map<EventKey, Set<(payload: never) => void>>();

  on<K extends EventKey>(event: K, cb: (payload: DomainEvents[K]) => void): () => void {
    const set = this.listeners.get(event) ?? new Set();
    set.add(cb as (payload: never) => void);
    this.listeners.set(event, set);
    return () => set.delete(cb as (payload: never) => void);
  }

  emit<K extends EventKey>(event: K, payload: DomainEvents[K]): void {
    this.listeners.get(event)?.forEach((cb) => {
      (cb as (p: DomainEvents[K]) => void)(payload);
    });
  }
}

export const storeBus = new EventBus();
