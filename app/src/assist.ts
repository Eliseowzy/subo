/*
 * AI 辅助的编排：拿建议填进内联预览 → 编译校验 → 交给用户接受 / 拒绝。
 * 提示词在服务端（assist.rs），这里决定改哪里、怎么判断校验结果。
 *
 * ⌘K：取选区和前后文，流式改写那一段。
 * 报错修复：TeX 报错的位置常常不是原因所在，所以不切片段，先诊断编译拿到宏调用链，
 * 由服务端按错误类别挑证据（diagnose.rs）；模型回 SEARCH/REPLACE 块，可能改好几处。
 */

import { Text, type ChangeSet, type EditorState } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { aiStream, projectApi, type AiLayout, type AiModel, type AssistRequest, type Diagnosis, type Issue } from "./api";
import { t } from "./i18n";
import { propose, type ProposalPart } from "./proposal";

/** 编辑器要的 AI 环境；没配置模型时整个是 null，界面上不出现 AI 入口 */
export type AssistEnv = {
  project: string;
  /** 编译校验（设置里可关） */
  verify: boolean;
  /** 有先后，第一个是首选 */
  models: AiModel[];
  /** 最近一次编译的问题，校验时用来区分「原来就有的」和「这次改出来的」 */
  baseline: Issue[];
};

/** images：⌘K 里附的图（data URL），只有能看图的模型才会带 */
export type EditTask = { instruction?: string; preset?: string; model?: string; images?: string[] };

/** ⌘K 里的预置指令；给模型的说法在服务端 assist.rs 的 preset() */
export const PRESETS = ["polish", "concise", "english", "chinese", "booktabs"] as const;

/** 公式环境：在这些环境里算数学模式（带不带星号都算） */
const MATH_ENVS = new Set(["equation", "align", "alignat", "gather", "multline", "flalign", "eqnarray", "math", "displaymath", "dmath"]);

/**
 * 光标处是不是数学模式。编辑器用的是 stex 旧式高亮，没有可用的语法树，这里自己扫：
 * 公式不能跨空行，所以只看光标所在段落开头到光标这一段，数 `$`、`$$`、`\( \)`、`\[ \]` 和公式环境的开闭。
 * `\text{…}` 之类在公式里切回文字的情形不管
 */
export function inMath(before: string) {
  const blank = [...before.matchAll(/\n[ \t]*\n/g)].pop();
  const para = blank ? before.slice(blank.index + blank[0].length) : before;
  const stack: string[] = [];
  const top = () => stack[stack.length - 1];
  const toggle = (d: string) => (top() === d ? stack.pop() : stack.push(d));
  for (let i = 0; i < para.length; i++) {
    const c = para[i];
    if (c === "%") {
      const nl = para.indexOf("\n", i);
      if (nl < 0) break;
      i = nl;
    } else if (c === "\\") {
      const rest = para.slice(i + 1);
      const env = /^(begin|end)\{([A-Za-z]+)\*?\}/.exec(rest);
      if (env && MATH_ENVS.has(env[2])) {
        if (env[1] === "begin") stack.push(env[2]);
        else if (top() === env[2]) stack.pop();
        i += env[0].length;
      } else if (rest[0] === "(" || rest[0] === "[") {
        stack.push(rest[0]);
        i++;
      } else if ((rest[0] === ")" && top() === "(") || (rest[0] === "]" && top() === "[")) {
        stack.pop();
        i++;
      } else {
        // 转义字符（\$、\\ 等）整个跳过
        i++;
      }
    } else if (c === "$") {
      if (para[i + 1] === "$") {
        toggle("$$");
        i++;
      } else toggle("$");
    }
  }
  return stack.length > 0;
}

/** ⌘K 前后文各带多少行 */
const BEFORE_LINES = 60;
const AFTER_LINES = 30;

/** 模型偶尔不守规矩：包一层代码围栏（有时围栏外还附一份或一句说明）、回显标签、首尾多出空行。
 *  只有一个围栏时只取围栏里的；再按原文的首尾换行对齐 */
export function cleanReply(text: string, target: string) {
  let s = text;
  const fences = [...s.matchAll(/```[\w-]*[ \t]*\n([\s\S]*?)\n?```/g)];
  if (fences.length === 1) s = fences[0][1];
  s = s.replace(/^\s*<TARGET>\n?/, "").replace(/\n?<\/TARGET>\s*$/, "");
  if (!/^\n/.test(target)) s = s.replace(/^\n+/, "");
  if (/\n$/.test(target)) {
    if (!/\n$/.test(s)) s += "\n";
  } else {
    s = s.replace(/\s+$/, "");
  }
  return s;
}

/** 两个问题算不算「同一个」：TeX 的报错文字够区分了，行号会因为改动前后挪动，不比 */
const same = (a: Issue, b: Issue) => a.message === b.message && a.file === b.file;

/** 行范围（含两端） */
type Span = { from: number; to: number };

/**
 * 看校验结果：改动所在的那几行（上下各放宽一行）出了错误，或者出了一个没有位置、原来也没有的错误，
 * 就算这次改出来的。别处的错误不怪它——编译遇错即停（-halt-on-error），修好一处后面的错误才露出来是常事。
 * 报错修复时 fix 给出原来那条错误、以及它在改后文档里挪到了第几行：原处还是同一个错误，就是没修好。
 *
 * 能编译也不一定改对了：layout 是比上次编译成功时新冒出来的排版异常（服务端比过了）。内容比整页还高、
 * 浮动体比页面还大，不管用户想写什么都不对，直接算；一行超出版心只在改动附近才算，用户自己写长的那行不怪 AI
 */
export function judge(
  after: Issue[],
  baseline: Issue[],
  file: string,
  spans: Span[],
  fix: { issue: Issue; line: number | null } | null,
  layout: AiLayout[] = []
): { status: "verified" | "unverified"; note: string | null } {
  const errors = after.filter((i) => i.level === "error");
  const before = baseline.filter((i) => i.level === "error");
  const inSpan = (i: Issue) => i.file === file && i.line != null && spans.some((s) => i.line! >= s.from - 1 && i.line! <= s.to + 1);
  const caused = errors.filter((e) => {
    if (inSpan(e)) {
      // ⌘K 改的段落里原来就有的错误，不算这次改出来的；修复的话改过的地方还出错就是没修好
      return fix != null || !before.some((b) => same(b, e));
    }
    if (fix && same(e, fix.issue) && e.line === fix.line) return true;
    return e.line == null && !before.some((b) => b.message === e.message);
  });
  if (caused.length) {
    const e = caused[0];
    if (fix && same(e, fix.issue)) return { status: "unverified", note: t("editor.ai.stillFails") };
    return {
      status: "unverified",
      note: e.line ? t("editor.ai.unverifiedAt", { detail: e.message, line: e.line }) : t("editor.ai.unverified", { detail: e.message }),
    };
  }
  const near = (l: number | null) => l != null && spans.some((s) => l >= s.from - 1 && l <= s.to + 1);
  const odd = layout.filter((x) => x.kind !== "lineOverflow" || near(x.line));
  if (odd.length) {
    const rank = { pageOverflow: 0, floatTooLarge: 1, lineOverflow: 2 } as const;
    const x = [...odd].sort((a, b) => rank[a.kind] - rank[b.kind])[0];
    const pt = Math.round(x.pt);
    const detail =
      x.kind === "pageOverflow"
        ? t("editor.ai.layoutPage", { pt })
        : x.kind === "floatTooLarge"
          ? t("editor.ai.layoutFloat", { pt })
          : t("editor.ai.layoutLine", { pt, line: x.line ?? 0 });
    return { status: "unverified", note: t("editor.ai.layout", { detail }) };
  }
  if (errors.length) return { status: "verified", note: t("editor.ai.verifiedOthers", { count: errors.length }) };
  return { status: "verified", note: null };
}

/** 模型回的 SEARCH/REPLACE 块 */
export type Edit = { search: string; replace: string };

/** 行号前缀：服务端给的片段每行是 `   15 | 原文`，模型偶尔连它一起抄 */
const GUTTER = /^\s*\d+ \| ?/;

export function parseEdits(raw: string): Edit[] {
  const out: Edit[] = [];
  const re = /^<{5,}[ \t]*SEARCH[^\n]*\n([\s\S]*?)^={5,}[ \t]*\n([\s\S]*?)^>{5,}[ \t]*REPLACE[^\n]*$/gm;
  for (const m of raw.matchAll(re)) {
    let search = m[1].replace(/\n$/, "");
    let replace = m[2].replace(/\n$/, "");
    const lines = search.split("\n");
    if (lines.every((l) => GUTTER.test(l))) {
      search = lines.map((l) => l.replace(GUTTER, "")).join("\n");
      replace = replace
        .split("\n")
        .map((l) => l.replace(GUTTER, ""))
        .join("\n");
    }
    if (search.trim()) out.push({ search, replace });
  }
  return out;
}

/**
 * 在文档里找到每个块要改的地方。SEARCH 按整行比（行尾空白不计），找不到再按原样子串找；
 * 必须恰好一处。哪块对不上就整体返回 null——半套修复比不修更糟。
 * 同一行或相邻行的几处并成一处，免得预览挂件叠在一起
 */
export function locate(doc: Text, edits: Edit[]): ProposalPart[] | null {
  const lines = doc.toString().split("\n");
  const parts: ProposalPart[] = [];
  for (const e of edits) {
    const want = e.search.split("\n").map((l) => l.trimEnd());
    const hits: number[] = [];
    for (let i = 0; i + want.length <= lines.length && hits.length < 2; i++) {
      if (want.every((w, k) => lines[i + k].trimEnd() === w)) hits.push(i);
    }
    let part: ProposalPart | null = null;
    if (hits.length === 1) {
      const from = doc.line(hits[0] + 1).from;
      let to = doc.line(hits[0] + want.length).to;
      // 删掉整行时连换行一起删，不留空行
      if (e.replace === "" && to < doc.length) to += 1;
      part = { from, to, text: e.replace };
    } else if (hits.length === 0) {
      const all = doc.toString();
      const at = all.indexOf(e.search);
      if (at >= 0 && all.indexOf(e.search, at + 1) < 0) part = { from: at, to: at + e.search.length, text: e.replace };
    }
    if (!part) return null;
    parts.push(part);
  }
  parts.sort((a, b) => a.from - b.from);
  const merged: ProposalPart[] = [];
  for (const p of parts) {
    const last = merged[merged.length - 1];
    if (last && p.from < last.to) return null;
    if (last && doc.lineAt(p.from).number <= doc.lineAt(last.to).number + 1) {
      merged[merged.length - 1] = { from: last.from, to: p.to, text: last.text + doc.sliceString(last.to, p.from) + p.text };
    } else merged.push({ ...p });
  }
  return merged;
}

/** 诊断出的错误和问题面板那条是不是同一个：两边是同一个解析器读的，只是诊断编译的日志可能多折了一处，比前缀 */
function sameError(a: string, b: string) {
  const n = (s: string) => s.replace(/\s+/g, " ").trim();
  const x = n(a);
  const y = n(b);
  return x.startsWith(y) || y.startsWith(x);
}

/** 把建议套上后的文档，以及每处改动、原来那条错误在新文档里的行号 */
function applied(state: EditorState, parts: ProposalPart[], errorLine: number | null) {
  const cs: ChangeSet = state.changes(parts.map((p) => ({ from: p.from, to: p.to, insert: p.text })));
  const next = cs.apply(state.doc);
  const spans = parts.map((p) => {
    const at = cs.mapPos(p.from, -1);
    return { from: next.lineAt(at).number, to: next.lineAt(at + p.text.length).number };
  });
  const line = errorLine != null && errorLine <= state.doc.lines ? next.lineAt(cs.mapPos(state.doc.line(errorLine).from, -1)).number : null;
  return { text: next.toString(), spans, line };
}

/** ⌘K：开一条 AI 建议并跑完全程。返回时建议还挂在编辑器里，等用户接受或拒绝 */
export async function runAssist(view: EditorView, env: AssistEnv, file: string, range: { from: number; to: number }, task: EditTask) {
  const doc = view.state.doc;
  const first = doc.lineAt(range.from).number;
  const last = doc.lineAt(range.to).number;
  const target = doc.sliceString(range.from, range.to);
  const req: AssistRequest = {
    kind: "edit",
    file,
    before: doc.sliceString(doc.line(Math.max(1, first - BEFORE_LINES)).from, range.from),
    target,
    after: doc.sliceString(range.to, doc.line(Math.min(doc.lines, last + AFTER_LINES)).to),
    model: task.model,
    instruction: task.instruction,
    preset: task.preset,
    images: task.images?.length ? task.images : undefined,
    inMath: task.images?.length ? inMath(doc.sliceString(Math.max(0, range.from - 20_000), range.from)) : undefined,
  };

  const ctrl = new AbortController();
  const h = propose(view, range, { onCancel: () => ctrl.abort() });
  if (!h) return;
  const api = projectApi(env.project);
  const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

  try {
    await aiStream(api.assistUrl, req, (d) => h.append(d), ctrl.signal, (d) => h.think(d), (f) => h.setStatus("streaming", t("editor.ai.fellBack", f)));
  } catch (e) {
    if (!ctrl.signal.aborted) h.setStatus("failed", message(e));
    return;
  }
  const raw = h.text();
  if (raw == null) return;
  const text = cleanReply(raw, target);
  h.setText(text);
  if (!text.trim() && target.trim()) return h.setStatus("failed", t("editor.ai.empty"));
  if (text === target) return h.setStatus("failed", t("editor.ai.noChange"));
  if (!env.verify) return h.setStatus("ready");

  h.setStatus("verifying");
  const at = h.range();
  if (!at) return;
  const state = view.state;
  const next = state.sliceDoc(0, at.from) + text + state.sliceDoc(at.to);
  const startLine = state.doc.lineAt(at.from).number;
  try {
    const r = await api.verifyAi(file, next, ctrl.signal);
    if (h.text() == null) return;
    if (r.timedOut) return h.setStatus("ready", t("editor.ai.verifyTimeout"));
    const span = { from: startLine, to: startLine + (text.match(/\n/g)?.length ?? 0) };
    const v = judge(r.issues, env.baseline, file, [span], null, r.layout);
    h.setStatus(v.status, v.note);
  } catch (e) {
    // 校验本身没跑成，不妨碍用户自己判断
    if (!ctrl.signal.aborted) h.setStatus("ready", t("editor.ai.verifySkipped", { detail: message(e) }));
  }
}

/** 报错修复：诊断 → 模型给出改哪几处 → 编译校验。返回时建议还挂在编辑器里，等用户接受或拒绝 */
export async function runFix(view: EditorView, env: AssistEnv, file: string, issue: Issue) {
  const doc = view.state.doc;
  const line = doc.line(Math.max(1, Math.min(issue.line ?? 1, doc.lines)));
  const ctrl = new AbortController();
  // 改哪里要等模型说；先在报错行下面挂一条空的，显示进度
  const h = propose(view, { from: line.to, to: line.to }, { onCancel: () => ctrl.abort() });
  if (!h) return;
  const api = projectApi(env.project);
  const message = (e: unknown) => (e instanceof Error ? e.message : String(e));
  const text = doc.toString();

  h.setStatus("streaming", t("editor.ai.diagnosing"));
  let diagnosis: Diagnosis | undefined;
  try {
    const r = await api.diagnoseAi(file, text, ctrl.signal);
    // 诊断出的得是同一个错误；对不上（比如刚改过别处）就退回问题面板那条，没有调用链
    if (r.diagnosis && sameError(r.diagnosis.message, issue.message)) diagnosis = r.diagnosis;
  } catch {
    // 诊断没跑成不妨碍修复，只是少了调用链
    if (ctrl.signal.aborted) return;
  }
  if (h.text() == null) return;
  h.setStatus("streaming", null);

  const req: AssistRequest = {
    kind: "fix",
    file,
    text,
    diagnosis,
    error: { message: issue.message, line: issue.line, context: issue.here },
  };
  let raw = "";
  try {
    // 回的是 SEARCH/REPLACE 块，不往预览里流；收齐了再解析
    await aiStream(
      api.assistUrl,
      req,
      (d) => {
        if (!raw) h.setStatus("streaming", t("editor.ai.streaming"));
        raw += d;
      },
      ctrl.signal,
      (d) => h.think(d),
      (f) => h.setStatus("streaming", t("editor.ai.fellBack", f))
    );
  } catch (e) {
    if (!ctrl.signal.aborted) h.setStatus("failed", message(e));
    return;
  }
  if (h.text() == null) return;
  if (/^\s*NO_FIX\s*$/.test(raw)) return h.setStatus("failed", t("editor.ai.noCause"));
  const edits = parseEdits(raw);
  if (!edits.length) return h.setStatus("failed", t("editor.ai.empty"));
  const parts = locate(view.state.doc, edits);
  if (!parts) return h.setStatus("failed", t("editor.ai.badEdit"));
  if (parts.every((p) => view.state.doc.sliceString(p.from, p.to) === p.text)) return h.setStatus("failed", t("editor.ai.noChange"));
  h.setParts(parts);
  if (!env.verify) return h.setStatus("ready");

  h.setStatus("verifying");
  const at = h.parts();
  if (!at) return;
  const next = applied(view.state, at, issue.line ?? null);
  try {
    const r = await api.verifyAi(file, next.text, ctrl.signal);
    if (h.text() == null) return;
    if (r.timedOut) return h.setStatus("ready", t("editor.ai.verifyTimeout"));
    const v = judge(r.issues, env.baseline, file, next.spans, { issue, line: next.line }, r.layout);
    h.setStatus(v.status, v.note);
  } catch (e) {
    if (!ctrl.signal.aborted) h.setStatus("ready", t("editor.ai.verifySkipped", { detail: message(e) }));
  }
}
