import { LitElement, html, css, type TemplateResult } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { unsafeHTML } from "lit/directives/unsafe-html.js";
import type { ToolCall } from "../../protocol/session-event";
import { renderMarkdown } from "./markdown";
import { toast } from "../common/cmx-agent-toast";

/** esc / compact（对齐旧工具函数）：模型/工具输出一律转义后嵌入。 */
function esc(s: unknown): string {
  return String(s).replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c] ?? c);
}
function compact(v: unknown): string {
  try {
    const s = JSON.stringify(v);
    return s.length > 240 ? s.slice(0, 240) + "…" : s;
  } catch {
    return String(v);
  }
}

const ICON_CHECK = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>`;
const ICON_X = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`;

/** 结果面板操作按钮（复制/分享/点赞/差评），对齐旧 TOOL_ACT_BTNS。 */
const TOOL_ACT_BTNS = html`
  <button class="tcact" data-act="toolCopy" title="复制" aria-label="复制">
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
      <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
    </svg>
  </button>
  <button class="tcact" data-act="toolShare" title="分享" aria-label="分享">
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <circle cx="18" cy="5" r="3" />
      <circle cx="6" cy="12" r="3" />
      <circle cx="18" cy="19" r="3" />
      <line x1="8.59" y1="13.51" x2="15.42" y2="17.49" />
      <line x1="15.41" y1="6.51" x2="8.59" y2="10.49" />
    </svg>
  </button>
  <button class="tcact" data-act="toolLike" title="点赞" aria-label="点赞">
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path
        d="M14 9V5a3 3 0 0 0-3-3l-4 9v11h11.28a2 2 0 0 0 2-1.7l1.38-9a2 2 0 0 0-2-2.3zM7 22H4a2 2 0 0 1-2-2v-7a2 2 0 0 1 2-2h3"
      />
    </svg>
  </button>
  <button class="tcact" data-act="toolDislike" title="差评" aria-label="差评">
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path
        d="M10 15v4a3 3 0 0 0 3 3l4-9V2H5.72a2 2 0 0 0-2 1.7l-1.38 9a2 2 0 0 0 2 2.3zm7-13h2.67A2.31 2.31 0 0 1 22 4v7a2.31 2.31 0 0 1-2.33 2H17"
      />
    </svg>
  </button>
`;

/** U5：红绿 diff（fs_edit old→new）。 */
function renderDiff(oldStr: string, newStr: string): string {
  let h = '<pre class="tdiff">';
  for (const l of String(oldStr).split("\n")) h += `<span class="dl del">- ${esc(l)}</span>`;
  for (const l of String(newStr).split("\n")) h += `<span class="dl add">+ ${esc(l)}</span>`;
  return h + "</pre>";
}
/** U5：Codex 风格补丁按行着色（+绿 -红 @@/*** 头）。 */
function renderPatch(txt: string): string {
  let h = '<pre class="tdiff">';
  for (const raw of String(txt).split("\n")) {
    let cls = "ctx";
    if (raw.startsWith("+")) cls = "add";
    else if (raw.startsWith("-")) cls = "del";
    else if (raw.startsWith("@@") || raw.startsWith("***")) cls = "hunk";
    h += `<span class="dl ${cls}">${esc(raw)}</span>`;
  }
  return h + "</pre>";
}

type ToolInput = {
  prompt?: unknown;
  label?: unknown;
  cmd?: unknown;
  url?: unknown;
  query?: unknown;
  actions?: Array<{ op?: string }>;
  goal?: unknown;
  definitionKey?: unknown;
  taskId?: unknown;
  objectType?: unknown;
  actionType?: unknown;
  dryRun?: unknown;
  reportCode?: unknown;
  scopes?: string[];
  steps?: Array<{ op?: string }>;
  manifest?: { name?: string };
  name?: unknown;
  old_string?: unknown;
  new_string?: unknown;
  patch?: unknown;
  path?: unknown;
  content?: unknown;
};

/** 调用头（对齐旧 renderToolInvoke：按工具名定制表头/正文）。 */
function renderToolInvoke(c: ToolCall): string {
  const name = c.name || "";
  const input = (c.arguments ?? {}) as ToolInput;
  if (name === "task" && input.prompt != null)
    return `<div class="tchead">🤖 <b>子智能体</b> <span class="tcpath">${esc(input.label || "子任务")}</span></div><div class="tcsub">${esc(String(input.prompt))}</div>`;
  if (name === "shell" && input.cmd != null)
    return `<div class="tchead">🔧 <b>shell</b></div><pre class="tterm">$ ${esc(input.cmd)}</pre>`;
  if (name === "web_fetch" && input.url != null)
    return `<div class="tchead">🌐 <b>web_fetch</b> <span class="tcpath">${esc(input.url)}</span></div>`;
  if (name === "web_search" && input.query != null)
    return `<div class="tchead">🔎 <b>web_search</b> <span class="tcpath">${esc(input.query)}</span></div>`;
  if (name === "browser_read" && input.url != null)
    return `<div class="tchead">🖥 <b>browser_read</b> <span class="tcpath">${esc(input.url)}</span></div>`;
  if (name === "browser_screenshot" && input.url != null)
    return `<div class="tchead">📷 <b>browser_screenshot</b> <span class="tcpath">${esc(input.url)}</span></div>`;
  if (name === "browser_do" && input.url != null)
    return `<div class="tchead">⌨️ <b>browser_do</b> <span class="tcpath">${esc(input.url)}</span> <span class="tcmeta">${(input.actions ?? []).length} 步交互</span></div>`;
  if (name === "computer_use" && input.url != null)
    return (
      `<div class="tchead">👁️ <b>computer_use</b> <span class="tcpath">${esc(input.url)}</span></div>` +
      (input.goal ? `<div class="apreason">🎯 ${esc(String(input.goal))}</div>` : "")
    );
  if (name === "flow_start_instance")
    return `<div class="tchead">⚙️ <b>flow 起实例</b> <span class="tcpath">${esc(input.definitionKey || "")}</span></div>`;
  if (name === "flow_complete_task")
    return `<div class="tchead">⚙️ <b>flow 办理任务</b> <span class="tcpath">${esc(input.taskId || "")}</span></div>`;
  if (name === "onto_put_object")
    return `<div class="tchead">🧩 <b>onto 写对象</b> <span class="tcpath">${esc(input.objectType || "")}</span></div>`;
  if (name === "onto_execute_action")
    return `<div class="tchead">🧩 <b>onto 执行动作</b> <span class="tcpath">${esc(input.actionType || "")}</span>${input.dryRun ? ' <span class="tcmeta">试算</span>' : ""}</div>`;
  if (name === "report_compute")
    return `<div class="tchead">📊 <b>report 计算</b> <span class="tcpath">${esc(input.reportCode || "")}</span></div>`;
  if (name === "enterprise_context")
    return `<div class="tchead">🏛️ <b>企业上下文</b> <span class="tcmeta">拉取域模型${input.scopes?.length ? "：" + esc(input.scopes.join("/")) : ""}</span></div>`;
  if (name === "business_chain")
    return (
      `<div class="tchead">🔗 <b>业务联动</b> <span class="tcmeta">${(input.steps ?? []).length} 步流水线</span></div>` +
      ((input.steps ?? []).length
        ? `<div class="cutrace">${(input.steps ?? []).map((s) => `<span class="cuact">${esc(String(s.op ?? "?"))}</span>`).join("")}</div>`
        : "")
    );
  if (name === "plugin_list")
    return `<div class="tchead">🧩 <b>plugin_list</b> <span class="tcmeta">列出已装插件</span></div>`;
  if (name === "plugin_marketplace")
    return `<div class="tchead">🛒 <b>plugin_marketplace</b> <span class="tcpath">${esc(input.url || "")}</span> <span class="tcmeta">浏览远程市场</span></div>`;
  if (name === "plugin_install")
    return `<div class="tchead">🧩 <b>plugin_install</b> <span class="tcpath">${esc((input.manifest && input.manifest.name) || input.name || "")}</span>${input.name && !input.manifest ? ' <span class="tcmeta">来自市场</span>' : ""}</div>`;
  if (name === "fs_edit" && input.old_string != null && input.new_string != null)
    return `<div class="tchead">✏️ <b>fs_edit</b> <span class="tcpath">${esc(input.path || "")}</span></div>${renderDiff(String(input.old_string), String(input.new_string))}`;
  if (name === "apply_patch" && input.patch != null)
    return `<div class="tchead">🩹 <b>apply_patch</b></div>${renderPatch(String(input.patch))}`;
  if (name === "fs_write" && input.content != null)
    return `<div class="tchead">🔧 <b>fs_write</b> <span class="tcpath">${esc(input.path || "")}</span></div><pre class="tcode">${esc(String(input.content))}</pre>`;
  return `<div class="tchead">🔧 调用 <b>${esc(name)}</b> <code>${esc(compact(c.arguments))}</code></div>`;
}

/** 结果体（对齐旧 renderToolResult 主要分支）。 */
function renderToolResult(ev: { ok: boolean; output: unknown }): string {
  if (typeof ev.output === "string") {
    const status = `<span class="tcstatus ok" title="成功">${ICON_CHECK}</span>`;
    return `<div class="tchead">✅ 结果 ${status}</div><pre class="tterm">${esc(ev.output)}</pre>`;
  }
  const o = (ev.output ?? {}) as Record<string, unknown>;
  const out = typeof o.output === "string" ? (o.output as string) : null;
  const err = typeof o.error === "string" ? (o.error as string) : null;
  if (!ev.ok) {
    const tip = err ? `失败 · ${err}` : "调用被拒绝或失败";
    const status = `<span class="tcstatus bad" title="${esc(tip)}">${ICON_X}</span>`;
    let h = `<div class="tchead">❌ 结果 ${status}</div>`;
    if (out) h += `<pre class="tterm">${esc(out)}</pre>`;
    if (err) h += `<pre class="tterm tterr">${esc(err)}</pre>`;
    if (!out && !err) h += `<div class="tcempty">（无输出）</div>`;
    return h;
  }
  if (Array.isArray(o.results) && o.query != null) {
    const items = (o.results as Array<{ title?: string; url?: string; snippet?: string }>)
      .map(
        (x) =>
          `<div class="wsr"><div class="wsr-t">${esc(x.title ?? x.url ?? "")}</div>` +
          `<div class="wsr-u">${esc(x.url ?? "")}</div>` +
          (x.snippet ? `<div class="wsr-s">${esc(x.snippet)}</div>` : "") +
          `</div>`
      )
      .join("");
    return (
      `<div class="tchead">🔎 搜索结果 <span class="tcmeta">${(o.count as number) ?? (o.results as unknown[]).length} 条 · ${esc(String(o.query))}</span></div>` +
      (items || `<div class="tcempty">（无结果）</div>`)
    );
  }
  if (o.bytes != null && o.width != null && o.path != null) {
    const kb = ((o.bytes as number) / 1024).toFixed(0);
    return `<div class="tchead">📷 网页截图 <span class="tcmeta">${o.width}×${o.height} · ${kb} KB</span></div><div class="wf-url">${esc(String(o.path))}</div>`;
  }
  if (o.computer_use === true) {
    const acts = ((o.actions as string[]) ?? [])
      .map((a) => `<span class="cuact">${esc(String(a))}</span>`)
      .join("");
    const badge = o.done
      ? `<span class="apresolved ok">✓ 达成</span>`
      : `<span class="apresolved no">步数用尽</span>`;
    return (
      `<div class="tchead">👁️ computer-use <span class="tcmeta">${(o.steps as number) ?? 0} 步</span></div>` +
      (o.goal ? `<div class="apreason">🎯 ${esc(String(o.goal))}</div>` : "") +
      (acts ? `<div class="cutrace">${acts}</div>` : "") +
      (o.answer ? `<div class="tcsub md">${renderMarkdown(String(o.answer))}</div>` : "") +
      `<div style="margin-top:6px">${badge}</div>`
    );
  }
  if (o.interacted === true && o.text != null) {
    const title = o.title ? `<div class="wf-title">${esc(String(o.title))}</div>` : "";
    return (
      `<div class="tchead">⌨️ 浏览器交互 <span class="tcmeta">${(o.steps as number) ?? 0} 步 · ${(o.chars as string) ?? ""} 字符</span></div>` +
      title +
      (o.url ? `<div class="wf-url">${esc(String(o.url))}</div>` : "") +
      `<pre class="tcode">${esc(String(o.text))}</pre>`
    );
  }
  if (o.rendered === true && o.text != null) {
    const title = o.title ? `<div class="wf-title">${esc(String(o.title))}</div>` : "";
    return (
      `<div class="tchead">🖥 渲染网页 <span class="tcmeta">${(o.chars as string) ?? ""} 字符</span></div>` +
      title +
      (o.url ? `<div class="wf-url">${esc(String(o.url))}</div>` : "") +
      `<pre class="tcode">${esc(String(o.text))}</pre>`
    );
  }
  if (o.text != null && o.url != null && o.status != null) {
    const head = `<div class="tchead">🌐 网页 <span class="tcmeta">${(o.chars as string) ?? ""} 字符 · HTTP ${o.status}</span></div>`;
    const title = o.title ? `<div class="wf-title">${esc(String(o.title))}</div>` : "";
    const link = `<div class="wf-url">${esc(String(o.url))}</div>`;
    return head + title + link + `<pre class="tcode">${esc(String(o.text))}</pre>`;
  }
  if (o.rows != null && Array.isArray(o.columns)) {
    const cols = (o.columns as Array<Record<string, unknown>>)
      .map((c) => {
        if (c.type === "numeric")
          return `<tr><td>${esc(c.name)}</td><td>数值</td><td>min ${c.min} · max ${c.max} · 均值 ${c.mean}</td></tr>`;
        const top = ((c.top as Array<{ value: unknown; count: number }>) ?? [])
          .map((t) => esc(String(t.value)) + "(" + t.count + ")")
          .join("、");
        return `<tr><td>${esc(c.name)}</td><td>文本</td><td>去重 ${c.distinct} · Top ${top}</td></tr>`;
      })
      .join("");
    return (
      `<div class="tchead">📈 数据概览 <span class="tcmeta">${o.rows} 行 · ${(o.columns as unknown[]).length} 列</span></div>` +
      `<table class="tdata"><thead><tr><th>列</th><th>类型</th><th>统计</th></tr></thead><tbody>${cols}</tbody></table>`
    );
  }
  if (o.text != null)
    return `<div class="tchead">✅ 读取 <span class="tcpath">${esc(o.path || "")}</span></div><pre class="tcode">${esc(String(o.text))}</pre>`;
  if (o.tree != null)
    return `<div class="tchead">✅ 目录结构</div><pre class="tcode">${esc(String(o.tree))}</pre>`;
  return `<span class="chip">✅ 结果 <code>${esc(compact(o))}</code></span>`;
}

/** 工具事件卡片（1:1 对齐旧 .tool 卡：tchead/tcmeta/tcacts/tterm/tcode/…）。 */
@customElement("cmx-agent-tool-event-card")
export class CmxAgentToolEventCard extends LitElement {
  @property({ attribute: false }) call?: ToolCall;
  @property({ attribute: false })
  result?: { call_id: string; ok: boolean; output: unknown };
  @property({ attribute: false })
  guard?: { guard: string; phase: string; decision: Record<string, unknown> };
  @property() state: "running" | "done" | "guard" = "running";
  @state() private liked = false;
  @state() private disliked = false;

  static styles = css`
    :host {
      display: block;
    }
    .tool {
      background: var(--panel2);
      border: 1px solid var(--border);
      border-radius: 10px;
      padding: 8px 12px;
      font-size: 12.5px;
      color: var(--ink2);
    }
    .tool.denied {
      border-color: var(--red);
    }
    .tchead {
      display: flex;
      align-items: center;
      gap: 7px;
      font-size: 12.5px;
    }
    .tchead b {
      color: var(--aqua);
    }
    .tool.denied .tchead b {
      color: var(--red);
    }
    .tcpath {
      font-family: "SF Mono", Menlo, monospace;
      font-size: 11.5px;
      color: var(--ink2);
    }
    .tcmeta {
      margin-left: auto;
      color: var(--muted);
      font-size: 11px;
    }
    pre {
      margin: 7px 0 2px;
      font-family: "SF Mono", Menlo, monospace;
      font-size: 11.5px;
      line-height: 1.5;
      overflow: auto;
      max-height: 340px;
    }
    .tterm {
      background: var(--term-bg);
      border: 1px solid var(--border);
      border-radius: 8px;
      padding: 9px 12px;
      color: var(--term-ink);
      white-space: pre;
    }
    .tterm.tterr {
      color: var(--red);
    }
    .tcode {
      background: var(--term-bg, var(--bg));
      border: 1px solid var(--border);
      border-radius: 8px;
      padding: 9px 12px;
      color: var(--ink);
      white-space: pre;
    }
    .tcempty {
      color: var(--muted);
      font-size: 11.5px;
      margin-top: 5px;
    }
    .wsr {
      margin: 8px 0;
      padding: 8px 10px;
      border: 1px solid var(--border);
      border-radius: 8px;
      background: var(--surface);
    }
    .wsr .wsr-t {
      font-size: 13.5px;
      font-weight: 600;
      color: var(--aqua);
      line-height: 1.4;
    }
    .wsr .wsr-u {
      font-size: 11px;
      color: var(--muted);
      font-family: "SF Mono", Menlo, monospace;
      margin: 2px 0;
      word-break: break-all;
    }
    .wsr .wsr-s {
      font-size: 12.5px;
      color: var(--ink2);
      line-height: 1.5;
      margin-top: 3px;
    }
    .wf-title {
      font-size: 14px;
      font-weight: 700;
      color: var(--ink);
      margin: 7px 0 2px;
    }
    .wf-url {
      font-size: 11px;
      color: var(--muted);
      font-family: "SF Mono", Menlo, monospace;
      margin-bottom: 4px;
      word-break: break-all;
    }
    .cutrace {
      display: flex;
      flex-wrap: wrap;
      gap: 5px;
      margin: 7px 0;
    }
    .cuact {
      font-size: 11.5px;
      font-family: "SF Mono", Menlo, monospace;
      color: var(--ink2);
      background: var(--panel);
      border: 1px solid var(--border2);
      border-radius: 6px;
      padding: 2px 8px;
    }
    .tdiff {
      background: var(--term-bg);
      border: 1px solid var(--border);
      border-radius: 8px;
      padding: 8px 0;
      line-height: 1.55;
      max-height: 360px;
    }
    .tdiff .dl {
      display: block;
      padding: 0 12px;
      white-space: pre-wrap;
      word-break: break-word;
    }
    .tdiff .add {
      color: var(--diff-add-ink);
      background: rgba(34, 197, 94, 0.12);
    }
    .tdiff .del {
      color: var(--diff-del-ink);
      background: rgba(229, 96, 95, 0.12);
    }
    .tdiff .hunk {
      color: var(--violet);
    }
    .tdiff .ctx {
      color: var(--ink2);
    }
    .tcsub {
      margin: 7px 0 2px;
      background: var(--panel);
      border: 1px solid var(--violet);
      border-left: 3px solid var(--violet);
      border-radius: 8px;
      padding: 9px 12px;
      font-size: 12.5px;
      color: var(--ink2);
      line-height: 1.55;
    }
    .tcsub.md {
      color: var(--ink);
    }
    .apreason {
      margin: 6px 0 10px;
      font-size: 12.5px;
      color: var(--ink2);
      line-height: 1.5;
    }
    .apresolved {
      margin-top: 6px;
      font-size: 12.5px;
      font-weight: 600;
    }
    .apresolved.ok {
      color: var(--green2);
    }
    .apresolved.no {
      color: var(--red);
    }
    .chip {
      display: inline-flex;
      align-items: center;
      gap: 7px;
      color: var(--ink2);
      font-size: 12.5px;
    }
    .chip code {
      font-family: "SF Mono", Menlo, monospace;
      font-size: 11.5px;
    }
    .tdata {
      border-collapse: collapse;
      margin: 7px 0 2px;
      font-size: 11.5px;
      width: 100%;
    }
    .tdata th,
    .tdata td {
      border: 1px solid var(--border2);
      padding: 4px 9px;
      text-align: left;
      color: var(--ink2);
    }
    .tdata th {
      background: var(--panel2);
      color: var(--ink);
      font-weight: 700;
    }
    .tcacts {
      display: inline-flex;
      align-items: center;
      gap: 1px;
      flex: 0 0 auto;
      margin-left: auto;
    }
    .tcact {
      display: inline-flex;
      align-items: center;
      justify-content: center;
      width: 24px;
      height: 22px;
      border-radius: 6px;
      color: var(--muted);
      background: transparent;
      border: 1px solid transparent;
      cursor: pointer;
      padding: 0;
      transition:
        color 0.12s,
        background 0.12s;
    }
    .tcact:hover {
      background: var(--hover);
      color: var(--ink);
    }
    .tcact svg {
      display: block;
      width: 15px;
      height: 15px;
    }
    .tcact.liked {
      color: var(--aqua);
    }
    .tcact.disliked {
      color: var(--red);
    }
    .tcstatus {
      display: inline-flex;
      align-items: center;
    }
    .tcstatus svg {
      width: 15px;
      height: 15px;
      display: block;
    }
    .tcstatus.ok {
      color: var(--green);
    }
    .tcstatus.bad {
      color: var(--red);
    }
    .head-row {
      display: flex;
      align-items: center;
      gap: 7px;
    }
  `;

  private get denied(): boolean {
    return Boolean(this.result && !this.result.ok);
  }

  private bodyHtml(): TemplateResult {
    if (this.state === "running" && this.call) {
      return html`${unsafeHTML(renderToolInvoke(this.call))}`;
    }
    if (this.state === "done" && this.result) {
      return html`${unsafeHTML(renderToolResult(this.result))}`;
    }
    if (this.state === "guard" && this.guard) {
      return html`<div class="tchead">
          🛡 <b>${this.guard.guard}</b> <span class="tcmeta">${this.guard.phase}</span>
        </div>
        <pre class="tcode">${esc(compact(this.guard.decision))}</pre>`;
    }
    return html``;
  }

  /** 复制/分享取正文（对齐旧 panelText：正文块优先，取全文兜底）。 */
  private collectText(): string {
    const root = this.shadowRoot;
    if (!root) return "";
    const parts = Array.from(root.querySelectorAll(".tterm, .tcode, .tcsub, .tcempty"))
      .map((e) => (e.textContent ?? "").trim())
      .filter(Boolean);
    if (parts.length) return parts.join("\n\n");
    const tbl = root.querySelector(".tdata");
    if (tbl) return (tbl.textContent ?? "").trim();
    return (root.textContent ?? "").trim();
  }

  private async copyText(txt: string): Promise<boolean> {
    try {
      await navigator.clipboard.writeText(txt);
      return true;
    } catch {
      try {
        const ta = document.createElement("textarea");
        ta.value = txt;
        document.body.appendChild(ta);
        ta.select();
        document.execCommand("copy");
        ta.remove();
        return true;
      } catch {
        return false;
      }
    }
  }

  private async onAct(e: Event): Promise<void> {
    const target = e.target as HTMLElement;
    const btn = target.closest(".tcact") as HTMLElement | null;
    if (!btn) return;
    e.stopPropagation();
    const act = btn.dataset.act;
    if (act === "toolCopy") {
      const ok = await this.copyText(this.collectText());
      toast(ok ? "已复制到剪贴板" : "复制失败", ok ? "info" : "error");
    } else if (act === "toolShare") {
      const txt = this.collectText();
      if (navigator.share) {
        try {
          await navigator.share({ text: txt });
          return;
        } catch (err) {
          if (err && (err as Error).name === "AbortError") return;
        }
      }
      const ok = await this.copyText(txt);
      toast(ok ? "已复制，可粘贴分享" : "分享失败", ok ? "info" : "error");
    } else if (act === "toolLike") {
      this.liked = !this.liked;
      if (this.liked) this.disliked = false;
      toast(this.liked ? "已点赞 👍" : "已取消点赞");
    } else if (act === "toolDislike") {
      this.disliked = !this.disliked;
      if (this.disliked) this.liked = false;
      toast(this.disliked ? "已记录反馈，谢谢" : "已取消差评");
    }
  }

  render() {
    if (!this.call && !this.result && !this.guard) return html``;
    return html`
      <div class="tool ${this.denied ? "denied" : ""}" @click=${(e: Event) => void this.onAct(e)}>
        ${this.bodyHtml()}
        <span class="tcacts" style="margin-left:auto">${TOOL_ACT_BTNS}</span>
      </div>
    `;
  }
}
