/*
 * 问题面板与原始日志。问题由服务端解析（logparse.rs），这里只管怎么摆给人看：
 * 错误先说「这是哪一类、通常该去哪找」，再给 TeX 的原话和停下处的源码；同类警告聚成一组；
 * 排版提示默认收起。每条都能跳到原始日志里对应的那几行，日志里的问题行也能跳回来。
 *
 * 原始日志可能有几 MB（几百页的书、宏死循环）：按行虚拟滚动，只画看得见的几十行；
 * 按「== 程序 第 N 遍」分段，编完默认只展开有问题的段和最后一遍。
 */

import { Fragment, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { HERE_MARK, type ErrorKind, type Issue, type IssueLevel } from "./api";
import Icon from "./Icon";
import { t, useLocale } from "./i18n";
import { Spinner } from "./ui";

/** 一类警告按 key 聚成一组的 */
const GROUPED = ["undefinedRef", "undefinedCite", "multiplyDefined", "missingChar"] as const;
type Grouped = (typeof GROUPED)[number];
const grouped = (k: string): k is Grouped => (GROUPED as readonly string[]).includes(k);
/** 警告分组的先后：先是 PDF 里会露馅的（?? / [?] / 豆腐块） */
const WARNING_ORDER = ["undefinedRef", "undefinedCite", "multiplyDefined", "missingChar", "bib", "font", "floatTooLarge", "other"];
/** 一组里最多直接列几个 key */
const CHIPS_MAX = 12;

/** 跳到日志的请求；n 递增，同一处点两次也会再滚一次 */
export type LogFocus = { from: number; to: number; n: number };

const levelIcon = (l: IssueLevel) => (l === "error" ? "error" : l === "warning" ? "alert" : "info");

function where(i: Issue) {
  if (!i.file) return "";
  if (!i.line) return i.file;
  return i.lineEnd ? `${i.file}:${i.line}–${i.lineEnd}` : `${i.file}:${i.line}`;
}

/** TeX 停下处的源码：断点前正常显示，断点画一道竖线，断点后（还没读到的）淡一些 */
function HereLine({ here, line }: { here: string; line?: number }) {
  const [before, after = ""] = here.split(HERE_MARK);
  return (
    <div className="issue-here">
      {line != null && <span className="issue-here-ln">{line}</span>}
      <code>
        <span>{before}</span>
        <span className="issue-here-mark" aria-hidden="true" />
        <span className="issue-here-rest">{after}</span>
      </code>
    </div>
  );
}

export function IssueList({
  issues,
  boxTotal,
  running,
  failed,
  focusId,
  canOpen,
  onOpen,
  onFix,
  onShowLog,
}: {
  issues: Issue[];
  /** 排版提示的总数（列表里的可能因上限少一些） */
  boxTotal: number;
  running: boolean;
  /** 编译没过：警告多半是半途的（第一遍的「引用未定义」），默认收起 */
  failed: boolean;
  /** 从日志跳回来要亮一下的那条 */
  focusId: { id: string; n: number } | null;
  canOpen: (i: Issue) => boolean;
  onOpen: (i: Issue) => void;
  /** 让 AI 修这个错误；没配置 AI 时不传，按钮不出现 */
  onFix?: (i: Issue) => void;
  onShowLog: (i: Issue) => void;
}) {
  useLocale();
  const [showBoxes, setShowBoxes] = useState(false);
  const [showWarnings, setShowWarnings] = useState<boolean | null>(null);
  const box = useRef<HTMLDivElement>(null);

  // 从日志跳回来：展开它所在的组，滚到它
  useEffect(() => {
    if (!focusId) return;
    const hit = issues.find((i) => i.id === focusId.id);
    if (hit?.level === "box") setShowBoxes(true);
    if (hit?.level === "warning") setShowWarnings(true);
    requestAnimationFrame(() => {
      const el = box.current?.querySelector<HTMLElement>(`[data-issue="${CSS.escape(focusId.id)}"]`);
      if (!el) return;
      el.scrollIntoView({ block: "nearest" });
      el.classList.remove("flash");
      void el.offsetWidth;
      el.classList.add("flash");
    });
  }, [focusId, issues]);

  if (running)
    return (
      <div className="issues-empty">
        <Spinner size={13} /> {t("workspace.issues.running")}
      </div>
    );

  const errors = issues.filter((i) => i.level === "error");
  const warnings = issues.filter((i) => i.level === "warning" && i.kind !== "summary");
  const boxes = issues.filter((i) => i.level === "box").sort((a, b) => (b.pt ?? 0) - (a.pt ?? 0));
  if (!errors.length && !warnings.length && !boxes.length)
    return (
      <div className="issues-empty">
        <Icon name="check" /> {t("workspace.issues.none")}
      </div>
    );

  const warningsOpen = showWarnings ?? !failed;
  const groups = WARNING_ORDER.map((kind) => ({ kind, items: warnings.filter((w) => w.kind === kind) })).filter((g) => g.items.length);

  const locButton = (i: Issue) => {
    const loc = where(i);
    if (!loc) return null;
    return canOpen(i) ? (
      <button className="issue-loc" onClick={() => onOpen(i)} title={t("workspace.issues.jump")}>
        {loc}
      </button>
    ) : (
      <span className="issue-loc static">{loc}</span>
    );
  };
  const logButton = (i: Issue) => (
    <button className="btn btn-ghost btn-sm issue-act" onClick={() => onShowLog(i)} title={t("workspace.issues.inLogTitle")}>
      <Icon name="terminal" size={13} />
      {t("workspace.issues.inLog")}
    </button>
  );

  const errorCard = (i: Issue) => {
    const fixable = onFix && canOpen(i);
    const kind = i.kind as ErrorKind;
    // 「当场出错」没什么可解释的，TeX 的原话就够了
    const explain = kind !== "here";
    return (
      <li key={i.id} className="issue-card error" data-issue={i.id}>
        <div className="issue-card-head">
          <Icon name="error" size={15} />
          <span className="issue-title">{t(`workspace.issues.kind.${kind}`, { source: i.source ?? "" })}</span>
          {locButton(i)}
        </div>
        {explain && <p className="issue-explain">{t(`workspace.issues.explain.${kind}`)}</p>}
        <div className="issue-tex">{i.message}</div>
        {i.here && <HereLine here={i.here} line={i.file ? i.line : undefined} />}
        {i.runaway && (
          <div className="issue-runaway">
            <span>{t("workspace.issues.runaway")}</span>
            <code>{i.runaway}</code>
          </div>
        )}
        <div className="issue-actions">
          {canOpen(i) && (
            <button className="btn btn-sm issue-act" onClick={() => onOpen(i)}>
              <Icon name="locate" size={13} />
              {t("workspace.issues.jump")}
            </button>
          )}
          {logButton(i)}
          {fixable && (
            <button className="btn btn-sm issue-fix" onClick={() => onFix(i)} title={t("workspace.issues.fixTitle")}>
              <Icon name="sparkle" size={13} />
              {t("workspace.issues.fix")}
            </button>
          )}
        </div>
      </li>
    );
  };

  /** 普通的一行：图标、原话、位置；悬停出「在日志中查看」 */
  const row = (i: Issue) => {
    const body = (
      <>
        <Icon name={levelIcon(i.level)} size={14} />
        <span className="issue-msg">{i.message}</span>
        <span className="issue-loc">{where(i)}</span>
      </>
    );
    return (
      <li key={i.id} className="issue-row" data-issue={i.id}>
        {canOpen(i) ? (
          <button className={`issue ${i.level}`} onClick={() => onOpen(i)} title={t("workspace.issues.jump")}>
            {body}
          </button>
        ) : (
          <div className={`issue ${i.level}`}>{body}</div>
        )}
        <button className="btn btn-ghost btn-sm btn-icon issue-log" onClick={() => onShowLog(i)} title={t("workspace.issues.inLogTitle")} aria-label={t("workspace.issues.inLogTitle")}>
          <Icon name="terminal" size={13} />
        </button>
      </li>
    );
  };

  /** 同类聚成一组：标题带个数，下面一排 key，点哪个跳到它第一次出现的地方 */
  const group = (kind: Grouped, items: Issue[]) => {
    const keys = new Map<string, Issue>();
    for (const i of items) if (i.key && !keys.has(i.key)) keys.set(i.key, i);
    const shown = [...keys.entries()].slice(0, CHIPS_MAX);
    const first = items[0];
    return (
      <li key={kind} className="issue-group-row" data-issue={first.id}>
        <div className="issue warning">
          <Icon name="alert" size={14} />
          <span className="issue-msg">
            <span className="issue-title">{t(`workspace.issues.kind.${kind}`, { source: "" })}</span>
            <span className="issue-count">{keys.size || items.length}</span>
            <span className="issue-hint">{t(`workspace.issues.explain.${kind}`)}</span>
          </span>
          <button className="btn btn-ghost btn-sm btn-icon issue-log" onClick={() => onShowLog(first)} title={t("workspace.issues.inLogTitle")} aria-label={t("workspace.issues.inLogTitle")}>
            <Icon name="terminal" size={13} />
          </button>
        </div>
        <div className="issue-chips">
          {shown.map(([key, i]) =>
            canOpen(i) ? (
              <button key={key} className="issue-chip" onClick={() => onOpen(i)} title={where(i)}>
                {key}
              </button>
            ) : (
              <span key={key} className="issue-chip static" title={where(i) || undefined}>
                {key}
              </span>
            )
          )}
          {keys.size > shown.length && <span className="issue-chip more">{t("workspace.issues.more", { count: keys.size - shown.length })}</span>}
        </div>
      </li>
    );
  };

  return (
    <div ref={box}>
      {errors.length > 0 && <ul className="issues">{errors.map(errorCard)}</ul>}
      {groups.length > 0 && failed && (
        <button className="btn btn-ghost btn-sm box-toggle" onClick={() => setShowWarnings(!warningsOpen)} title={t("workspace.issues.warningsHalfway")}>
          <Icon name="chevron" size={13} className={`caret${warningsOpen ? " open" : ""}`} />
          {t("workspace.issues.warnings", { count: warnings.length })}
        </button>
      )}
      {groups.length > 0 && warningsOpen && (
        <ul className="issues">
          {groups.map((g) => (grouped(g.kind) ? group(g.kind, g.items) : <Fragment key={g.kind}>{g.items.map(row)}</Fragment>))}
        </ul>
      )}
      {!errors.length && !groups.length && (
        <div className="issues-empty">
          <Icon name="check" /> {t("workspace.issues.none")}
        </div>
      )}
      {boxes.length > 0 && (
        <>
          <button className="btn btn-ghost btn-sm box-toggle" onClick={() => setShowBoxes((s) => !s)}>
            <Icon name="chevron" size={13} className={`caret${showBoxes ? " open" : ""}`} />
            {t("workspace.issues.boxes", { count: boxTotal })}
          </button>
          {showBoxes && (
            <>
              <ul className="issues">{boxes.map(row)}</ul>
              {boxTotal > boxes.length && <div className="issues-note">{t("workspace.issues.boxesCapped", { shown: boxes.length, count: boxTotal })}</div>}
            </>
          )}
        </>
      )}
    </div>
  );
}

/* ---------- 原始日志 ---------- */

/** 每行的高度（px），与样式表里 .rawlog-row 一致 */
const ROW = 19;
const OVERSCAN = 20;

type Section = { start: number; end: number; title: string; pass: boolean };
type Row = { kind: "head"; sec: number } | { kind: "line"; idx: number };

function splitLines(text: string) {
  return text.replace(/\r\n?/g, "\n").split("\n");
}

/** 按 `== …` 分段；第一个分段行之前的（引擎回退的说明之类）自成一段，没有标题 */
function sectionsOf(lines: string[]): Section[] {
  const out: Section[] = [];
  let cur: Section = { start: 0, end: 0, title: "", pass: false };
  lines.forEach((l, i) => {
    if (l.startsWith("== ")) {
      cur.end = i;
      if (cur.end > cur.start || cur.title) out.push(cur);
      // 匹配服务端 engine.rs 写进日志的分隔行，不是界面文案
      cur = { start: i, end: i, title: l.slice(3), pass: /第 \d+ 遍$/.test(l) }; // i18n-ignore
    }
  });
  cur.end = lines.length;
  if (cur.end > cur.start || cur.title) out.push(cur);
  return out;
}

export function RawLog({
  text,
  follow,
  issues,
  focus,
  onIssue,
}: {
  text: string;
  /** 正在编译：一直贴着末尾 */
  follow: boolean;
  issues: Issue[];
  focus: LogFocus | null;
  /** 点日志里问题行左边的标记，跳回问题面板 */
  onIssue: (i: Issue) => void;
}) {
  useLocale();
  const ref = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const [view, setView] = useState({ top: 0, height: 400 });
  /** 用户自己点开、折起的段（段号 → 是否折起）；没点过的按默认 */
  const [touched, setTouched] = useState<Map<number, boolean>>(new Map());
  const [query, setQuery] = useState("");
  const [hit, setHit] = useState(0);
  const [lit, setLit] = useState<{ from: number; to: number } | null>(null);
  // 新的一次编译：上次定位的高亮、用户对各段的开合（段号对不上了）都作废
  useEffect(() => {
    if (text) return;
    setLit(null);
    setTouched(new Map());
  }, [text]);

  // 编译中日志只往后长：接着切新来的那段，不必每次整份重切
  const cache = useRef<{ text: string; lines: string[] }>({ text: "", lines: [] });
  const lines = useMemo(() => {
    const c = cache.current;
    let next: string[];
    if (c.text && text.length > c.text.length && text.startsWith(c.text) && text[c.text.length] === "\n") {
      next = c.lines.concat(splitLines(text.slice(c.text.length + 1)));
    } else next = text ? splitLines(text) : [];
    cache.current = { text, lines: next };
    return next;
  }, [text]);
  const sections = useMemo(() => sectionsOf(lines), [lines]);

  /** 每行属于哪条问题（取最严重的）：行首标记的颜色、点了跳回哪条 */
  const marks = useMemo(() => {
    const rank = { error: 0, warning: 1, box: 2 } as const;
    const m = new Map<number, Issue>();
    for (const i of issues) {
      if (i.kind === "summary") continue;
      for (let k = i.logRange[0]; k <= i.logRange[1]; k++) {
        const prev = m.get(k);
        if (!prev || rank[i.level] < rank[prev.level]) m.set(k, i);
      }
    }
    return m;
  }, [issues]);

  // 编完：多段的日志默认只展开有问题的段和最后一遍 TeX 及其后。在渲染时就算好，
  // 不等 effect 再折，免得刚算好的滚动位置被随后的折叠打乱
  const auto = useMemo(() => {
    if (follow || sections.length < 2) return new Set<number>();
    const lastPass = sections.reduce((a, s, k) => (s.pass ? k : a), sections.length - 1);
    const lines = [...marks.keys()];
    const keep = (s: Section, k: number) => k >= lastPass || lines.some((n) => n - 1 >= s.start && n - 1 < s.end);
    return new Set(sections.map((s, k) => (keep(s, k) ? -1 : k)).filter((k) => k >= 0));
  }, [follow, sections, marks]);
  const folded = useMemo(() => {
    const out = new Set(auto);
    for (const [k, v] of touched) {
      if (v) out.add(k);
      else out.delete(k);
    }
    return out;
  }, [auto, touched]);

  const rows = useMemo(() => {
    const out: Row[] = [];
    sections.forEach((s, k) => {
      const body = s.title ? s.start + 1 : s.start;
      if (s.title) out.push({ kind: "head", sec: k });
      if (!folded.has(k)) for (let i = body; i < s.end; i++) out.push({ kind: "line", idx: i });
    });
    return out;
  }, [sections, folded]);

  const matches = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return [];
    const out: number[] = [];
    lines.forEach((l, i) => {
      if (l.toLowerCase().includes(q)) out.push(i);
    });
    return out;
  }, [lines, query]);

  /** 要滚到的那一行：所在段折着就先展开，等行排好了再滚 */
  const target = useRef<number | null>(null);
  const rowsRef = useRef(rows);
  rowsRef.current = rows;
  const applyTarget = () => {
    const el = ref.current;
    const idx = target.current;
    if (!el || idx == null) return;
    const r = rowsRef.current.findIndex((x) => x.kind === "line" && x.idx === idx);
    if (r < 0) return;
    el.scrollTop = Math.max(0, r * ROW - el.clientHeight / 3);
    target.current = null;
  };
  useLayoutEffect(applyTarget, [rows]);
  const scrollToLine = (idx: number) => {
    const sec = sections.findIndex((s) => idx >= s.start && idx < s.end);
    if (sec >= 0 && folded.has(sec)) setTouched((t) => new Map(t).set(sec, false));
    target.current = idx;
    applyTarget();
  };

  // 从问题面板跳过来
  useEffect(() => {
    if (!focus) return;
    stick.current = false;
    setLit({ from: focus.from - 1, to: focus.to - 1 });
    scrollToLine(focus.from - 1);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focus]);

  useEffect(() => {
    if (matches.length) scrollToLine(matches[Math.min(hit, matches.length - 1)]);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [matches, hit]);

  useLayoutEffect(() => {
    const el = ref.current;
    if (el && follow && stick.current) el.scrollTop = el.scrollHeight;
  }, [rows, follow]);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setView({ top: el.scrollTop, height: el.clientHeight }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const first = Math.max(0, Math.floor(view.top / ROW) - OVERSCAN);
  const last = Math.min(rows.length, Math.ceil((view.top + view.height) / ROW) + OVERSCAN);
  const current = matches.length ? matches[Math.min(hit, matches.length - 1)] : -1;
  const matchSet = useMemo(() => new Set(matches), [matches]);
  const digits = String(lines.length).length;

  const toggle = (k: number) => setTouched((t) => new Map(t).set(k, !folded.has(k)));

  return (
    <div className="rawlog">
      <div className="rawlog-find">
        <Icon name="search" size={13} />
        <input
          value={query}
          placeholder={t("workspace.logFind")}
          aria-label={t("workspace.logFind")}
          onChange={(e) => {
            setQuery(e.target.value);
            setHit(0);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" && matches.length) {
              e.preventDefault();
              setHit((h) => (h + (e.shiftKey ? matches.length - 1 : 1)) % matches.length);
            } else if (e.key === "Escape") setQuery("");
          }}
        />
        {query.trim() && <span className="dim">{matches.length ? t("workspace.logFindCount", { n: Math.min(hit, matches.length - 1) + 1, count: matches.length }) : t("workspace.logFindNone")}</span>}
      </div>
      <div
        className="log-body rawlog-body"
        ref={ref}
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
          setView({ top: el.scrollTop, height: el.clientHeight });
        }}
      >
        {!lines.length ? (
          <pre>{t("workspace.noOutput")}</pre>
        ) : (
          <div className="rawlog-space" style={{ height: rows.length * ROW + 24 }}>
            {rows.slice(first, last).map((r, k) => {
              const top = (first + k) * ROW;
              if (r.kind === "head") {
                const s = sections[r.sec];
                const open = !folded.has(r.sec);
                return (
                  <button key={`h${r.sec}`} className="rawlog-row rawlog-head" style={{ top }} onClick={() => toggle(r.sec)} aria-expanded={open}>
                    <Icon name="chevron" size={12} className={`caret${open ? " open" : ""}`} />
                    <span>{s.title}</span>
                    {!open && <span className="dim">{t("workspace.logFolded", { count: s.end - s.start - 1 })}</span>}
                  </button>
                );
              }
              const m = marks.get(r.idx + 1);
              const cls = [
                "rawlog-row",
                m ? `mark-${m.level}` : "",
                lit && r.idx >= lit.from && r.idx <= lit.to ? "lit" : "",
                matchSet.has(r.idx) ? "match" : "",
                r.idx === current ? "current" : "",
              ]
                .filter(Boolean)
                .join(" ");
              return (
                <div key={r.idx} className={cls} style={{ top }}>
                  <span className="rawlog-ln" style={{ width: `${digits + 1}ch` }}>
                    {r.idx + 1}
                  </span>
                  {m && m.logRange[0] === r.idx + 1 ? (
                    <button className="rawlog-dot" onClick={() => onIssue(m)} title={t("workspace.logBackToIssue")} aria-label={t("workspace.logBackToIssue")} />
                  ) : (
                    <span className="rawlog-dot" />
                  )}
                  <span className="rawlog-text">{lines[r.idx]}</span>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
