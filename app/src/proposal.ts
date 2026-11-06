/*
 * AI 建议的内联预览：原文里要替换的部分划掉，下面一块显示新内容；
 * Tab 接受、Esc 拒绝。接受就是一次普通编辑，进撤销栈，⌘Z 能撤回。
 *
 * 同一时间只有一条建议。调用方用 propose() 开一条，拿到的句柄往里续写流式文字、改状态；
 * 建议被拒绝、被新的一条顶掉、或用户在那段原文里动了手，都会回调 onCancel，调用方据此中止请求。
 *
 * 一条建议可以改好几处（报错修复：原因和报错常常不在一处），一起接受、一起拒绝；
 * 状态栏和按钮挂在第一处下面。
 */

import { Annotation, Prec, StateEffect, StateField, type EditorState, type Extension } from "@codemirror/state";
import { Decoration, EditorView, keymap, ViewPlugin, WidgetType, type DecorationSet, type ViewUpdate } from "@codemirror/view";
import { t } from "./i18n";

/**
 * streaming：还在生成；ready：生成完，没做编译校验；verifying：校验中；
 * verified：校验通过；unverified：会多出编译错误（仍可接受，但标红）；failed：没拿到可用的建议
 */
export type ProposalStatus = "streaming" | "ready" | "verifying" | "verified" | "unverified" | "failed";

/** 一处改动：[from, to) 换成 text */
export type ProposalPart = { from: number; to: number; text: string };

type Proposal = {
  id: number;
  /** 按位置排好、互不重叠；至少一处 */
  parts: ProposalPart[];
  status: ProposalStatus;
  /** 状态的补充说明：出错原因、校验发现的错误 */
  note: string | null;
  /** 推理模型的思考过程，正文出来前给用户看个动静 */
  think: string;
  /** 开始的时刻（ms），生成中显示已等了多久 */
  started: number;
};

export type ProposalHandle = {
  /** 流式续写 */
  append: (text: string) => void;
  /** 续写思考过程（不进结果） */
  think: (text: string) => void;
  /** 整段换掉（例如去掉模型多给的代码围栏） */
  setText: (text: string) => void;
  /** 换成好几处改动（按位置排好、互不重叠） */
  setParts: (parts: ProposalPart[]) => void;
  setStatus: (status: ProposalStatus, note?: string | null) => void;
  /** 当前文字（第一处）；建议已结束时为 null */
  text: () => string | null;
  /** 要替换的原文此刻在哪（第一处，随用户在别处的编辑移动）；建议已结束时为 null */
  range: () => { from: number; to: number } | null;
  /** 全部改动此刻的位置与文字；建议已结束时为 null */
  parts: () => ProposalPart[] | null;
  /** 主动撤掉（不回调 onCancel） */
  dismiss: () => void;
};

type Callbacks = { onCancel?: () => void; onAccept?: () => void };

const setProposal = StateEffect.define<Proposal | null>();
/** 这次清掉建议是因为接受了它，不算取消 */
const accepted = Annotation.define<boolean>();
/** 调用方主动撤掉的，也不回调 onCancel */
const quiet = Annotation.define<boolean>();

const field = StateField.define<Proposal | null>({
  create: () => null,
  update(p, tr) {
    for (const e of tr.effects) if (e.is(setProposal)) return e.value;
    if (!p || !tr.docChanged) return p;
    // 用户改到了要被替换的哪一段原文：建议对不上了，整条撤掉
    let touched = false;
    tr.changes.iterChangedRanges((fromA, toA) => {
      for (const x of p.parts) if (x.from === x.to ? fromA < x.from && toA > x.from : fromA < x.to && toA > x.from) touched = true;
    });
    if (touched) return null;
    const parts: ProposalPart[] = [];
    for (const x of p.parts) {
      const from = tr.changes.mapPos(x.from, x.from === x.to ? -1 : 1);
      const to = x.from === x.to ? from : tr.changes.mapPos(x.to, -1);
      if (to < from) return null;
      parts.push({ ...x, from, to });
    }
    return { ...p, parts };
  },
  provide: (f) => EditorView.decorations.compute([f], (s) => decorations(s.field(f), s)),
});

function decorations(p: Proposal | null, state: EditorState): DecorationSet {
  if (!p) return Decoration.none;
  return Decoration.set(
    p.parts.flatMap((x, i) => [
      ...(x.to > x.from ? [Decoration.mark({ class: "cm-ai-del" }).range(x.from, x.to)] : []),
      // 块挂件放在原文最后一行的行尾，显示在它下面
      Decoration.widget({ widget: new ProposalWidget(p, i), block: true, side: 1 }).range(lineEnd(state, x.to)),
    ]),
    true
  );
}

function lineEnd(state: EditorState, pos: number) {
  return state.doc.lineAt(pos).to;
}

/** 建议里的第 i 处；第一处带思考过程、状态栏和按钮 */
class ProposalWidget extends WidgetType {
  constructor(
    readonly p: Proposal,
    readonly i: number
  ) {
    super();
  }

  eq(other: ProposalWidget) {
    const a = other.p;
    const b = this.p;
    return a.id === b.id && other.i === this.i && a.parts[other.i].text === b.parts[this.i].text && a.status === b.status && a.note === b.note && a.think === b.think;
  }

  toDOM(view: EditorView) {
    const dom = document.createElement("div");
    dom.className = "cm-ai-proposal";
    dom.setAttribute("role", "group");
    dom.setAttribute("aria-label", t("editor.ai.label"));
    dom.innerHTML =
      '<div class="cm-ai-think"><span></span></div>' +
      '<pre class="cm-ai-ins"></pre>' +
      '<div class="cm-ai-bar"><span class="cm-ai-status"></span><span class="cm-ai-spacer"></span>' +
      '<button type="button" class="cm-ai-btn" data-act="reject"></button>' +
      '<button type="button" class="cm-ai-btn primary" data-act="accept"></button></div>';
    // 按钮不抢焦点，点完光标还在编辑器里
    dom.addEventListener("mousedown", (e) => {
      if ((e.target as HTMLElement).closest("button")) e.preventDefault();
    });
    dom.addEventListener("click", (e) => {
      const act = (e.target as HTMLElement).closest("button")?.dataset.act;
      if (act === "accept") acceptProposal(view);
      else if (act === "reject") rejectProposal(view);
    });
    this.fill(dom);
    // 生成中每秒刷新一次计时；updateDOM 会换上新的 Proposal，从 shown 里取最新的
    timers.set(dom, window.setInterval(() => {
      const p = shown.get(dom);
      if (p?.status === "streaming") (dom.querySelector(".cm-ai-status") as HTMLElement).textContent = statusText(p);
    }, 1000));
    return dom;
  }

  updateDOM(dom: HTMLElement) {
    this.fill(dom);
    return true;
  }

  destroy(dom: HTMLElement) {
    window.clearInterval(timers.get(dom));
    timers.delete(dom);
  }

  private fill(dom: HTMLElement) {
    const p = this.p;
    const text = p.parts[this.i].text;
    const lead = this.i === 0;
    shown.set(dom, p);
    dom.dataset.status = p.status;
    // 思考过程只在正文出来之前显示末尾几行，正文一出来（或换上了阶段说明）就收起
    const think = dom.querySelector(".cm-ai-think") as HTMLElement;
    think.hidden = !(lead && p.status === "streaming" && !text && !p.note && p.think.trim());
    if (!think.hidden) (think.firstElementChild as HTMLElement).textContent = p.think.slice(-THINK_TAIL).trimStart();
    const ins = dom.querySelector(".cm-ai-ins") as HTMLElement;
    ins.textContent = text;
    ins.hidden = !text && p.status !== "streaming";
    if (p.status === "streaming") ins.append(Object.assign(document.createElement("span"), { className: "cm-ai-caret" }));
    (dom.querySelector(".cm-ai-bar") as HTMLElement).hidden = !lead;
    const status = dom.querySelector(".cm-ai-status") as HTMLElement;
    status.textContent = statusText(p);
    const accept = dom.querySelector('[data-act="accept"]') as HTMLButtonElement;
    accept.disabled = !acceptable(p);
    accept.innerHTML = "";
    accept.append(t("editor.ai.accept"), kbd("Tab"));
    const reject = dom.querySelector('[data-act="reject"]') as HTMLButtonElement;
    reject.innerHTML = "";
    reject.append(t(p.status === "streaming" ? "editor.ai.stop" : "editor.ai.reject"), kbd("Esc"));
  }

  ignoreEvent() {
    return true;
  }
}

const shown = new WeakMap<HTMLElement, Proposal>();
const timers = new WeakMap<HTMLElement, number>();
/** 思考过程只留末尾这么多字，够撑满三行 */
const THINK_TAIL = 400;

function kbd(key: string) {
  return Object.assign(document.createElement("kbd"), { textContent: key });
}

function statusText(p: Proposal) {
  switch (p.status) {
    case "streaming": {
      // 分三段：还没动静 → 在思考 → 在写；调用方给了阶段说明（定位原因……）就显示它；等久了带上秒数
      const phase = p.parts[0].text ? t("editor.ai.streaming") : (p.note ?? (p.think ? t("editor.ai.thinking") : t("editor.ai.waiting")));
      const secs = Math.floor((Date.now() - p.started) / 1000);
      return secs >= 2 ? `${phase} ${t("editor.ai.elapsed", { s: secs })}` : phase;
    }
    case "ready":
      return p.note ?? (acceptable(p) ? "" : t("editor.ai.empty"));
    case "verifying":
      return t("editor.ai.verifying");
    case "verified":
      return p.note ?? t("editor.ai.verified");
    case "unverified":
      // note 是整句（assist.ts 的 judge 拼好的）
      return p.note ?? t("editor.ai.unverified", { detail: "" });
    case "failed":
      return p.note ?? t("editor.ai.empty");
  }
}

const acceptable = (p: Proposal) => p.status !== "streaming" && p.status !== "failed" && p.parts.some((x) => x.text.length > 0 || x.to > x.from);

/* ---------- 命令 ---------- */

let nextId = 1;
const callbacks = new Map<number, Callbacks>();

export function hasProposal(state: EditorState) {
  return state.field(field, false) != null;
}

/** 接受当前建议。正在生成时什么也不做（但吞掉按键，免得 Tab 插进一个缩进） */
export function acceptProposal(view: EditorView) {
  const p = view.state.field(field, false);
  if (!p) return false;
  if (!acceptable(p)) return true;
  const cb = callbacks.get(p.id);
  const first = p.parts[0];
  view.dispatch({
    changes: p.parts.map((x) => ({ from: x.from, to: x.to, insert: x.text })),
    selection: { anchor: first.from + first.text.length },
    effects: setProposal.of(null),
    annotations: accepted.of(true),
    userEvent: "input.ai",
    scrollIntoView: true,
  });
  cb?.onAccept?.();
  view.focus();
  return true;
}

export function rejectProposal(view: EditorView) {
  if (!view.state.field(field, false)) return false;
  view.dispatch({ effects: setProposal.of(null) });
  view.focus();
  return true;
}

/**
 * 开一条建议：把 [from, to) 换成模型给的文字（from === to 就是在那里插入）。
 * 已有的建议会被顶掉（回调它的 onCancel）。只读编辑器里返回 null
 */
export function propose(view: EditorView, range: { from: number; to: number }, cb: Callbacks = {}): ProposalHandle | null {
  if (view.state.readOnly) return null;
  const id = nextId++;
  callbacks.set(id, cb);
  const from = Math.max(0, Math.min(range.from, view.state.doc.length));
  const to = Math.max(from, Math.min(range.to, view.state.doc.length));
  view.dispatch({
    effects: [setProposal.of({ id, parts: [{ from, to, text: "" }], status: "streaming", note: null, think: "", started: Date.now() }), EditorView.scrollIntoView(lineEnd(view.state, to), { y: "nearest" })],
  });

  const current = () => {
    const p = view.state.field(field, false);
    return p && p.id === id ? p : null;
  };
  const update = (patch: Partial<Proposal>) => {
    const p = current();
    if (p) view.dispatch({ effects: setProposal.of({ ...p, ...patch }) });
  };
  return {
    append: (text) => {
      const p = current();
      if (p) update({ parts: [{ ...p.parts[0], text: p.parts[0].text + text }, ...p.parts.slice(1)] });
    },
    think: (text) => {
      const p = current();
      if (p) update({ think: p.think + text });
    },
    setText: (text) => {
      const p = current();
      if (p) update({ parts: [{ ...p.parts[0], text }, ...p.parts.slice(1)] });
    },
    setParts: (parts) => {
      const p = current();
      if (!p || !parts.length) return;
      const len = view.state.doc.length;
      const clamped = parts.map((x) => ({ ...x, from: Math.max(0, Math.min(x.from, len)), to: Math.max(0, Math.min(x.to, len)) }));
      view.dispatch({ effects: [setProposal.of({ ...p, parts: clamped }), EditorView.scrollIntoView(lineEnd(view.state, clamped[0].to), { y: "center" })] });
    },
    setStatus: (status, note = null) => update({ status, note }),
    text: () => current()?.parts[0].text ?? null,
    range: () => {
      const p = current();
      return p ? { from: p.parts[0].from, to: p.parts[0].to } : null;
    },
    parts: () => current()?.parts.map((x) => ({ ...x })) ?? null,
    dismiss: () => {
      if (current()) view.dispatch({ effects: setProposal.of(null), annotations: quiet.of(true) });
    },
  };
}

/** 建议被撤掉（拒绝、被顶掉、原文被改）时通知调用方；接受和主动撤掉的不算 */
const watcher = ViewPlugin.fromClass(
  class {
    update(u: ViewUpdate) {
      const before = u.startState.field(field, false);
      const after = u.state.field(field, false);
      if (!before || (after && after.id === before.id)) return;
      const cb = callbacks.get(before.id);
      callbacks.delete(before.id);
      if (u.transactions.some((tr) => tr.annotation(accepted) || tr.annotation(quiet))) return;
      // 回调里可能 dispatch，挪出更新周期
      if (cb?.onCancel) queueMicrotask(cb.onCancel);
    }
  }
);

const theme = EditorView.baseTheme({
  ".cm-ai-del": {
    backgroundColor: "var(--danger-soft)",
    textDecoration: "line-through",
    textDecorationColor: "var(--danger)",
  },
  ".cm-ai-proposal": {
    margin: "4px 18px 8px 10px",
    border: "1px solid var(--accent-line)",
    borderRadius: "var(--r)",
    backgroundColor: "var(--raised)",
    boxShadow: "var(--shadow-sm)",
    overflow: "hidden",
    fontFamily: "var(--font-ui)",
  },
  ".cm-ai-proposal[data-status=unverified], .cm-ai-proposal[data-status=failed]": { borderColor: "var(--danger-line)" },
  ".cm-ai-think": {
    // 三行高，内容贴底，新字从下面冒出来
    display: "flex",
    flexDirection: "column",
    justifyContent: "flex-end",
    maxHeight: "calc(3 * 1.6em)",
    overflow: "hidden",
    padding: "6px 10px",
    borderBottom: "1px solid var(--line)",
    color: "var(--ink-3)",
    fontSize: "var(--fs-xs)",
    lineHeight: "1.6",
    whiteSpace: "pre-wrap",
    overflowWrap: "anywhere",
    maskImage: "linear-gradient(to bottom, transparent, #000 1.6em)",
  },
  ".cm-ai-think[hidden], .cm-ai-bar[hidden]": { display: "none" },
  ".cm-ai-ins": {
    margin: "0",
    padding: "6px 10px",
    backgroundColor: "var(--accent-soft)",
    color: "var(--ink)",
    fontFamily: "var(--font-mono)",
    fontSize: "inherit",
    lineHeight: "1.65",
    whiteSpace: "pre-wrap",
    overflowWrap: "anywhere",
  },
  ".cm-ai-caret": {
    display: "inline-block",
    width: "7px",
    height: "1.1em",
    marginLeft: "1px",
    verticalAlign: "text-bottom",
    backgroundColor: "var(--accent)",
    animation: "cm-ai-blink 1s steps(2) infinite",
  },
  "@keyframes cm-ai-blink": { "50%": { opacity: "0" } },
  ".cm-ai-bar": {
    display: "flex",
    alignItems: "center",
    gap: "6px",
    padding: "4px 6px 4px 10px",
    borderTop: "1px solid var(--line)",
    fontSize: "var(--fs-xs)",
    color: "var(--ink-3)",
  },
  ".cm-ai-proposal[data-status=verified] .cm-ai-status": { color: "var(--accent-text)" },
  ".cm-ai-proposal[data-status=unverified] .cm-ai-status, .cm-ai-proposal[data-status=failed] .cm-ai-status": { color: "var(--danger)" },
  ".cm-ai-spacer": { flex: "1" },
  ".cm-ai-btn": {
    display: "inline-flex",
    alignItems: "center",
    gap: "6px",
    height: "24px",
    padding: "0 8px",
    border: "1px solid var(--line-strong)",
    borderRadius: "var(--r-sm)",
    backgroundColor: "var(--raised)",
    color: "var(--ink)",
    fontFamily: "var(--font-ui)",
    fontSize: "var(--fs-xs)",
    cursor: "pointer",
  },
  ".cm-ai-btn.primary": { backgroundColor: "var(--accent)", borderColor: "var(--accent)", color: "var(--on-accent)" },
  ".cm-ai-btn:disabled": { opacity: "0.45", cursor: "default" },
  ".cm-ai-btn kbd": { fontFamily: "var(--font-ui)", fontSize: "var(--fs-2xs)", opacity: "0.7" },
});

/** 编辑器里装上它，propose() 才有地方显示 */
export function proposals(): Extension {
  return [
    field,
    watcher,
    theme,
    Prec.highest(
      keymap.of([
        { key: "Tab", run: acceptProposal },
        { key: "Escape", run: rejectProposal },
      ])
    ),
  ];
}
