/** 轻量 hash 路由（方案 §6.4）：#/login | #/chat/:id | #/settings/:section。 */

export type Route =
  { name: "login" } | { name: "chat"; sessionId?: string } | { name: "settings"; section?: string };

export function parseHash(hash: string): Route {
  const path = hash.replace(/^#/, "").replace(/^\/+/, "");
  if (path === "login") return { name: "login" };
  const chat = path.match(/^chat(?:\/([^/]+))?/);
  if (chat) return { name: "chat", sessionId: chat[1] ? decodeURIComponent(chat[1]) : undefined };
  const settings = path.match(/^settings(?:\/([^/]+))?/);
  if (settings) return { name: "settings", section: settings[1] };
  return { name: "chat" };
}

export function navigate(path: `#/${string}`): void {
  if (window.location.hash === path) return;
  window.location.hash = path;
}

export function onRoute(cb: (route: Route) => void): () => void {
  const handler = () => cb(parseHash(window.location.hash));
  window.addEventListener("hashchange", handler);
  handler();
  return () => window.removeEventListener("hashchange", handler);
}
