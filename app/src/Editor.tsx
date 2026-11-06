import { useCallback, useEffect, useRef, useState, type MouseEvent } from "react";
import { Annotation, Compartment, EditorSelection, EditorState, Prec, StateEffect, StateField, type Text } from "@codemirror/state";
import {
  Decoration,
  drawSelection,
  dropCursor,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  keymap,
  lineNumbers,
} from "@codemirror/view";
import {
  defaultKeymap,
  history,
  historyKeymap,
  indentWithTab,
  redo,
  redoDepth,
  selectAll,
  toggleComment,
  undo,
  undoDepth,
} from "@codemirror/commands";
import {
  bracketMatching,
  HighlightStyle,
  indentOnInput,
  indentUnit,
  StreamLanguage,
  syntaxHighlighting,
} from "@codemirror/language";
import { highlightSelectionMatches, openSearchPanel, search, searchKeymap } from "@codemirror/search";
import { autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap } from "@codemirror/autocomplete";
import { tags as t } from "@lezer/highlight";
import { stex } from "@codemirror/legacy-modes/mode/stex";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { execCopy, readText, writeText } from "./clipboard";
import ContextMenu, { SEP, type MenuEntry } from "./ContextMenu";
import { t as tr, useLocale, type Locale } from "./i18n";
import { latexCompletions } from "./latex";
import { formatTexLines } from "./texfmt";
import { proposals } from "./proposal";
import { runAssist, runFix, type AssistEnv } from "./assist";
import AskBox, { type AskPick } from "./AskBox";
import type { Issue } from "./api";
import { useMenu } from "./menu";
import { MAC, MOD, toast } from "./ui";

/** 跳到某行；n 每次递增，同一行点两次也会再跳 */
export type Goto = { line: number; n: number };

/** 整篇换成新内容（载入外部修改、恢复历史版本）；n 每次递增。不算用户编辑：不触发 onChange，⌘Z 能撤回 */
export type Replace = { text: string; n: number };

/** 让编辑器修一个编译错误（问题面板里点「修复」）；n 每次递增 */
export type FixRequest = { issue: Issue; n: number };

/** 标记「外部换的内容」，更新监听里据此不回调 onChange */
const external = Annotation.define<boolean>();

type Props = {
  path: string;
  /** 只在建编辑器时读；可以直接给上一个编辑器的 Text，省掉拼成字符串 */
  initial: string | Text;
  /** 给的是 CodeMirror 的文档对象：每敲一个字都拼整篇字符串太费，要用时再 toString() */
  onChange: (doc: Text) => void;
  /** 只读：能选、能搜、能复制，不能改（模板预览）。只在建编辑器时读 */
  readOnly?: boolean;
  goto?: Goto | null;
  /** 光标所在行（从 1 起）变了 */
  onLine?: (line: number) => void;
  /** 在 PDF 里定位光标所在行；不传则右键菜单里不出这一项 */
  onLocate?: () => void;
  replace?: Replace | null;
  /** AI 辅助的环境；没配置模型时为 null，不出现 ⌘K 和右键里的 AI 项 */
  assist?: AssistEnv | null;
  fix?: FixRequest | null;
  /** fix 已接手，父组件可以清掉它（免得换文件回来又跑一遍） */
  onFixTaken?: () => void;
};

// 颜色全部走 CSS 变量，浅色/深色切换时不用重建编辑器
const highlight = HighlightStyle.define([
  { tag: t.tagName, color: "var(--syn-command)" },
  { tag: t.atom, color: "var(--syn-arg)" },
  { tag: [t.string, t.standard(t.variableName)], color: "var(--syn-arg)" },
  { tag: t.keyword, color: "var(--syn-math)", fontWeight: "600" },
  { tag: t.number, color: "var(--syn-number)" },
  { tag: t.bracket, color: "var(--syn-bracket)" },
  { tag: [t.comment, t.lineComment], color: "var(--syn-comment)", fontStyle: "italic" },
  { tag: t.invalid, color: "var(--danger)", textDecoration: "underline wavy" },
  // Markdown
  { tag: t.heading1, color: "var(--ink)", fontWeight: "700", fontSize: "1.18em" },
  { tag: t.heading2, color: "var(--ink)", fontWeight: "700", fontSize: "1.08em" },
  { tag: [t.heading3, t.heading4, t.heading5, t.heading6], color: "var(--ink)", fontWeight: "700" },
  { tag: t.strong, fontWeight: "700" },
  { tag: t.emphasis, fontStyle: "italic" },
  { tag: t.strikethrough, textDecoration: "line-through" },
  { tag: [t.link, t.url], color: "var(--syn-command)" },
  { tag: t.monospace, color: "var(--syn-arg)" },
  { tag: t.quote, color: "var(--ink-2)", fontStyle: "italic" },
  { tag: [t.processingInstruction, t.contentSeparator, t.labelName], color: "var(--syn-bracket)" },
]);

const theme = EditorView.theme({
  "&": { height: "100%", fontSize: "var(--editor-size)", color: "var(--ink)", backgroundColor: "var(--editor-bg)" },
  ".cm-scroller": { fontFamily: "var(--font-mono)", lineHeight: "1.65" },
  ".cm-content": { padding: "12px 0 40vh", caretColor: "var(--accent)" },
  ".cm-line": { padding: "0 18px 0 10px" },
  ".cm-gutters": {
    backgroundColor: "var(--editor-bg)",
    color: "var(--ink-3)",
    border: "none",
    paddingLeft: "6px",
  },
  ".cm-lineNumbers .cm-gutterElement": { minWidth: "32px", padding: "0 6px 0 0", fontSize: "var(--fs-xs)" },
  ".cm-activeLine": { backgroundColor: "var(--editor-active)" },
  ".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--ink-2)" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--accent)", borderLeftWidth: "2px" },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": {
    backgroundColor: "var(--editor-selection) !important",
  },
  ".cm-selectionMatch": { backgroundColor: "var(--editor-match)" },
  "&.cm-focused .cm-matchingBracket": { backgroundColor: "var(--editor-match)", outline: "1px solid var(--accent-line)" },
  "&.cm-focused .cm-nonmatchingBracket": { backgroundColor: "var(--danger-soft)" },
  ".cm-searchMatch": { backgroundColor: "var(--warn-soft)", outline: "1px solid var(--warn-line)" },
  ".cm-searchMatch.cm-searchMatch-selected": { backgroundColor: "var(--warn-line)" },
  ".cm-flash": { animation: "cm-flash 1.4s var(--ease)" },
  // 补全浮层与查找栏：跟随主题
  ".cm-tooltip": {
    backgroundColor: "var(--raised)",
    border: "1px solid var(--line)",
    borderRadius: "var(--r)",
    boxShadow: "var(--shadow-lg)",
    overflow: "hidden",
  },
  ".cm-tooltip.cm-tooltip-autocomplete > ul": { fontFamily: "var(--font-mono)", fontSize: "var(--fs-sm)", maxHeight: "16em" },
  ".cm-tooltip.cm-tooltip-autocomplete > ul > li": { padding: "3px 10px 3px 6px", lineHeight: "1.5" },
  ".cm-tooltip-autocomplete ul li[aria-selected]": { backgroundColor: "var(--accent)", color: "var(--on-accent)" },
  ".cm-completionDetail": { fontFamily: "var(--font-ui)", fontStyle: "normal", opacity: 0.7, marginLeft: "1em" },
  ".cm-completionIcon": { opacity: 0.55, width: "1.2em" },
  ".cm-completionMatchedText": { textDecoration: "none", fontWeight: "700" },
  ".cm-panels": { backgroundColor: "var(--panel)", color: "var(--ink)", borderColor: "var(--line)" },
  ".cm-panels.cm-panels-top": { borderBottom: "1px solid var(--line)" },
  ".cm-panel.cm-search": { padding: "8px 10px", fontFamily: "var(--font-ui)", fontSize: "var(--fs-xs)" },
  ".cm-panel.cm-search input, .cm-panel.cm-search button": { fontFamily: "var(--font-ui)", fontSize: "var(--fs-xs)" },
  ".cm-textfield": {
    backgroundColor: "var(--field)",
    color: "var(--ink)",
    border: "1px solid var(--line-strong)",
    borderBottomColor: "var(--field-line)",
    borderRadius: "var(--r-sm)",
    padding: "3px 8px",
  },
  ".cm-button": {
    backgroundImage: "none",
    backgroundColor: "var(--raised)",
    color: "var(--ink)",
    border: "1px solid var(--line-strong)",
    borderRadius: "var(--r-sm)",
    padding: "3px 10px",
  },
  ".cm-panel.cm-search label": {
    display: "inline-flex",
    alignItems: "center",
    gap: "4px",
    verticalAlign: "middle",
    fontSize: "var(--fs-xs)",
    color: "var(--ink-2)",
    margin: "0 4px",
    cursor: "pointer",
  },
  ".cm-panel.cm-search input[type=checkbox]": { margin: 0, accentColor: "var(--accent)", cursor: "pointer" },
  ".cm-panel.cm-search [name=close]": { color: "var(--ink-3)", fontSize: "var(--fs-xl)", right: "8px" },
});

/** CodeMirror 内置界面文案（搜索面板等）随界面语言；切换语言时只重配这一格，不重建编辑器 */
const phrases = (l: Locale) => EditorState.phrases.of(l.messages.editor.phrases as Record<string, string>);

/** 跳行后让那一行闪一下，眼睛好找 */
const flash = StateEffect.define<number>();
const flashLine = Decoration.line({ class: "cm-flash" });
const flashField = StateField.define({
  create: () => Decoration.none,
  update(deco, tr) {
    deco = deco.map(tr.changes);
    // 传 -1 表示清掉
    for (const e of tr.effects) if (e.is(flash)) deco = e.value < 0 ? Decoration.none : Decoration.set([flashLine.range(e.value)]);
    return deco;
  },
  provide: (f) => EditorView.decorations.from(f),
});

/** 整理缩进与空行：有选区只改选区涉及的行，否则整篇。层级总是从文件开头算起 */
function formatDoc(v: EditorView): boolean {
  if (v.state.readOnly) return false;
  const doc = v.state.doc;
  const lines: string[] = [];
  for (let i = 1; i <= doc.lines; i++) lines.push(doc.line(i).text);
  const out = formatTexLines(lines, v.state.facet(indentUnit));
  const sel = v.state.selection.ranges.filter((r) => !r.empty);
  const inRange = (n: number) =>
    !sel.length || sel.some((r) => doc.lineAt(r.from).number <= n && n <= doc.lineAt(r.to).number);
  const changes: { from: number; to: number; insert: string }[] = [];
  out.forEach((text, i) => {
    const line = doc.line(i + 1);
    if (!inRange(line.number)) return;
    if (text === null) changes.push({ from: line.from, to: line.to + 1, insert: "" });
    else if (text !== line.text) changes.push({ from: line.from, to: line.to, insert: text });
  });
  if (!sel.length && doc.length && out[out.length - 1] !== "") changes.push({ from: doc.length, to: doc.length, insert: "\n" });
  if (!changes.length) {
    toast(tr("editor.menu.formatClean"));
    return true;
  }
  v.dispatch({ changes, userEvent: "input.format", scrollIntoView: true });
  return true;
}

/** TeX 系的文件上语法高亮和补全，.md 只高亮，其余按纯文本编辑 */
const isMd = (path: string) => /\.(md|markdown|mdown|mkd)$/i.test(path);
const isTex = (path: string) => /\.(tex|sty|cls|ltx|dtx|bbx|cbx|def|cfg|clo)$/i.test(path);

export default function Editor({ path, initial, onChange, readOnly, goto, onLine, onLocate, replace, assist, fix, onFixTaken }: Props) {
  const locale = useLocale();
  const lang = useRef(new Compartment());
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  useMenu({
    "edit.find": () => {
      if (!view.current) return;
      view.current.focus();
      openSearchPanel(view.current);
    },
    "edit.format": () => {
      if (!view.current || !isTex(path)) return;
      view.current.focus();
      formatDoc(view.current);
    },
  });
  const change = useRef(onChange);
  change.current = onChange;
  const line = useRef(onLine);
  line.current = onLine;
  const env = useRef(assist);
  env.current = assist;
  /** ⌘K 框：贴在哪、改哪一段 */
  const [ask, setAsk] = useState<{ x: number; y: number; from: number; to: number } | null>(null);
  const openAsk = useRef<() => boolean>(() => false);

  useEffect(() => {
    if (!host.current) return;
    const tex = isTex(path);
    const v = new EditorView({
      parent: host.current,
      state: EditorState.create({
        doc: initial,
        extensions: [
          lineNumbers(),
          highlightActiveLineGutter(),
          highlightSpecialChars(),
          history(),
          drawSelection(),
          dropCursor(),
          EditorState.allowMultipleSelections.of(true),
          EditorState.readOnly.of(!!readOnly),
          indentOnInput(),
          bracketMatching(),
          closeBrackets(),
          highlightActiveLine(),
          highlightSelectionMatches(),
          search({ top: true }),
          lang.current.of(phrases(locale)),
          flashField,
          // AI 建议的内联预览；没有建议时什么也不显示
          proposals(),
          Prec.high(keymap.of([{ key: "Mod-k", run: () => openAsk.current() }])),
          keymap.of([
            ...closeBracketsKeymap,
            // ⌘↵ 留给「编译」；默认键表里它会插一个空行，编译一次文件就多一行
            ...defaultKeymap.filter((b) => b.key !== "Mod-Enter"),
            ...searchKeymap,
            ...historyKeymap,
            ...completionKeymap,
            indentWithTab,
          ]),
          ...(tex
            ? [
                StreamLanguage.define(stex),
                // 数学模式的 $ 也自动配对
                EditorState.languageData.of(() => [{ closeBrackets: { brackets: ["(", "[", "{", "$"] } }]),
                autocompletion({ override: [latexCompletions], icons: false }),
                keymap.of([{ key: "Shift-Alt-f", run: formatDoc }]),
              ]
            : isMd(path)
              ? [markdown({ base: markdownLanguage })]
              : []),
          syntaxHighlighting(highlight),
          EditorView.lineWrapping,
          theme,
          EditorView.updateListener.of((u) => {
            if (u.docChanged && !u.transactions.some((tr) => tr.annotation(external))) change.current(u.state.doc);
            if (u.docChanged || u.selectionSet) line.current?.(u.state.doc.lineAt(u.state.selection.main.head).number);
          }),
        ],
      }),
    });
    view.current = v;
    return () => {
      v.destroy();
      view.current = null;
    };
    // initial、path 只在换文件时变化，由父组件用 key 控制重建
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    view.current?.dispatch({ effects: lang.current.reconfigure(phrases(locale)) });
  }, [locale]);

  useEffect(() => {
    const v = view.current;
    if (!v || !replace || v.state.doc.toString() === replace.text) return;
    // 光标留在原来的行附近：外部改动多半只动了几处
    const head = Math.min(v.state.selection.main.head, replace.text.length);
    v.dispatch({
      changes: { from: 0, to: v.state.doc.length, insert: replace.text },
      selection: EditorSelection.cursor(head),
      annotations: external.of(true),
      scrollIntoView: true,
    });
  }, [replace]);

  useEffect(() => {
    const v = view.current;
    if (!v || !goto) return;
    const line = v.state.doc.line(Math.max(1, Math.min(goto.line, v.state.doc.lines)));
    v.dispatch({
      selection: EditorSelection.cursor(line.from),
      effects: [EditorView.scrollIntoView(line.from, { y: "center" }), flash.of(line.from)],
    });
    v.focus();
    const t = window.setTimeout(() => view.current?.dispatch({ effects: flash.of(-1) }), 1500);
    return () => window.clearTimeout(t);
  }, [goto]);

  openAsk.current = () => {
    const v = view.current;
    if (!v || v.state.readOnly) return false;
    if (!env.current) {
      toast(tr("editor.ask.notConfigured"));
      return true;
    }
    const sel = v.state.selection.main;
    // 选区跨行时框贴在选区下面，免得挡住要改的内容
    const below = v.coordsAtPos(sel.to) ?? v.coordsAtPos(sel.head);
    const start = v.coordsAtPos(sel.from) ?? below;
    if (!below || !start) return true;
    setAsk({ x: start.left, y: below.bottom + 6, from: sel.from, to: sel.to });
    return true;
  };

  const submitAsk = (pick: AskPick) => {
    const v = view.current;
    const e = env.current;
    if (!v || !ask || !e) return;
    setAsk(null);
    v.focus();
    void runAssist(v, e, path, { from: ask.from, to: ask.to }, pick);
  };

  useEffect(() => {
    const v = view.current;
    const e = env.current;
    if (!v || !e || !fix?.issue.line) return;
    onFixTaken?.();
    void runFix(v, e, path, fix.issue);
    // 只认 fix 本身的变化
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fix]);

  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const closeMenu = useCallback(() => setMenu(null), []);

  /** 右键点在选区外时先把光标挪过去，和系统编辑器一致；「在 PDF 中定位」也就定位到点的那一行 */
  const openMenu = (e: MouseEvent) => {
    const v = view.current;
    if (!v) return;
    e.preventDefault();
    const pos = v.posAtCoords({ x: e.clientX, y: e.clientY });
    if (pos != null && !v.state.selection.ranges.some((r) => r.from <= pos && pos <= r.to)) {
      v.dispatch({ selection: EditorSelection.cursor(pos) });
    }
    v.focus();
    setMenu({ x: e.clientX, y: e.clientY });
  };

  const menuItems = (v: EditorView): MenuEntry[] => {
    const empty = v.state.selection.ranges.every((r) => r.empty);
    // 只读时改动类的项都灰掉：菜单直接 dispatch，绕得过 readOnly
    const ro = v.state.readOnly;
    // 多选区和 CodeMirror 自己的复制一样，按行拼起来
    const picked = () =>
      v.state.selection.ranges
        .filter((r) => !r.empty)
        .map((r) => v.state.sliceDoc(r.from, r.to))
        .join(v.state.lineBreak);
    const copy = (cut: boolean) => {
      v.focus();
      if (execCopy(cut)) return;
      void writeText(picked())
        .then(() => cut && v.dispatch(v.state.replaceSelection(""), { userEvent: "delete.cut" }))
        .catch(() => toast(tr("common.copyFailed"), "error"));
    };
    const run = (cmd: (v: EditorView) => boolean) => () => {
      cmd(v);
      v.focus();
    };
    return [
      { label: tr("editor.menu.undo"), shortcut: "⌘Z", disabled: ro || !undoDepth(v.state), run: run(undo) },
      { label: tr("editor.menu.redo"), shortcut: MAC ? "⌘⇧Z" : "⌘Y", disabled: ro || !redoDepth(v.state), run: run(redo) },
      SEP,
      { label: tr("editor.menu.cut"), shortcut: "⌘X", disabled: ro || empty, run: () => copy(true) },
      { label: tr("editor.menu.copy"), shortcut: "⌘C", disabled: empty, run: () => copy(false) },
      {
        label: tr("editor.menu.paste"),
        shortcut: "⌘V",
        disabled: ro,
        run: () =>
          void readText()
            .then((text) => v.dispatch(v.state.replaceSelection(text), { userEvent: "input.paste", scrollIntoView: true }))
            .catch(() => toast(tr("editor.menu.pasteBlocked", { kbd: `${MOD}V` })))
            .finally(() => v.focus()),
      },
      { label: tr("editor.menu.selectAll"), shortcut: "⌘A", run: run(selectAll) },
      SEP,
      ...(isTex(path) && !ro
        ? [
            { label: tr("editor.menu.toggleComment"), shortcut: "⌘/", run: run(toggleComment) },
            { label: tr(empty ? "editor.menu.format" : "editor.menu.formatSelection"), shortcut: "⌥⇧F", run: run(formatDoc) },
          ]
        : []),
      ...(onLocate ? [{ label: tr("editor.menu.locate"), shortcut: "⌘⇧J", run: onLocate }] : []),
      ...(env.current && !ro ? [{ label: tr("editor.menu.ask"), shortcut: "⌘K", run: () => void openAsk.current() }] : []),
      SEP,
      { label: tr("editor.menu.find"), shortcut: "⌘F", run: run(openSearchPanel) },
    ];
  };

  return (
    <>
      <div className="editor-host" ref={host} onContextMenu={openMenu} />
      {ask && assist && (
        <AskBox
          x={ask.x}
          y={ask.y}
          hasSelection={ask.to > ask.from}
          models={assist.models}
          onSubmit={submitAsk}
          onClose={() => {
            setAsk(null);
            view.current?.focus();
          }}
        />
      )}
      {menu && view.current && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={menuItems(view.current)}
          label={tr("editor.menu.label")}
          onClose={closeMenu}
        />
      )}
    </>
  );
}
