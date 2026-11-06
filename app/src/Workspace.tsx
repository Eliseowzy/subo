import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Text } from "@codemirror/state";
import {
  api as libraryApi,
  projectApi,
  ServerError,
  type AiInfo,
  type CompatFix,
  type Issue,
  type IssueReport,
  type JobEvent,
  type MissingFile,
  type ProjectInfo,
  type Spot,
  type Symbols,
  type TemplateSummary,
} from "./api";
import { importedText } from "./CaseTools";
import HistoryPanel from "./HistoryPanel";
import { CancelInstall, InstallMeter, useInstall } from "./Install";
import { setProjectSymbols } from "./latex";
import { TexStaleCard } from "./TexFix";
import { kindOf, type Kind } from "./attach";
import Attachment from "./Attachment";
import Editor, { type FixRequest, type Goto, type Replace } from "./Editor";
import type { AssistEnv } from "./assist";
import EngineSwitch, { programName } from "./EngineSwitch";
import FileTree from "./FileTree";
import Icon, { fileIcon } from "./Icon";
import { IssueList, RawLog, type LogFocus } from "./IssuePanel";
import { prefs } from "./prefs";
import Mark from "./Mark";
import MarkdownView, { isMarkdown } from "./MarkdownView";
import Preview from "./Preview";
import ProjectSwitch from "./ProjectSwitch";
import QuickOpen from "./QuickOpen";
import { fromTemplate, templateName } from "./Templates";
import { Gutter, GUTTER, LOG_MIN, LOG_ROOM, PANE_MIN, TREE_MAX, TREE_MIN, useLayout } from "./Split";
import { t, tx, useLocale } from "./i18n";
import { useMenu } from "./menu";
import { duration, fileSize, Kbd, LocaleToggle, MAC, Spinner, rovingKeys, ThemeToggle, toast, toastError } from "./ui";

/** 编译状态与保存状态分开：边打字边编译是常态，两件事不该互相盖掉 */
type Job = "idle" | "running" | "done" | "failed";
type Save = "saved" | "dirty" | "saving" | "error";
type Tab = "issues" | "log";
/** 编译进行到哪：第几遍、这一遍出了几页（上一遍的页数当总数），或在跑哪个辅助工具 */
type Progress = { pass: number; pages: number; total: number | null; tool: "bib" | "index" | "glossary" | "pdf" | null };
const NO_PROGRESS: Progress = { pass: 0, pages: 0, total: null, tool: null };

type Props = {
  id: string;
  /** 只读预览这个模板：能看、编译、下载 PDF，不能改；「使用」才新建项目 */
  template?: string;
  onHome: () => void;
  onSwitch: (id: string) => void;
};

export default function Workspace({ id, template, onHome, onSwitch }: Props) {
  const locale = useLocale();
  const preview = template !== undefined;
  /** 预览中的模板；显示名要按语言查，「使用」也要用它 */
  const [tpl, setTpl] = useState<TemplateSummary | null>(null);
  const [using, setUsing] = useState(false);
  const api = useMemo(() => projectApi(id), [id]);
  const [project, setProject] = useState<ProjectInfo | null>(null);
  /** 已归档的项目只读；取消归档后原地恢复编辑 */
  const archived = !preview && !!project?.archived;
  const locked = preview || archived;
  const [unarchiving, setUnarchiving] = useState(false);
  const [active, setActive] = useState<string | null>(null);
  /** 当前打开的是附件时：它的类型和地址；文本文件为 null */
  const [attach, setAttach] = useState<{ kind: Exclude<Kind, "text">; url: string } | null>(null);
  /** 只给 Markdown 预览用：.tex 打字时不更新，免得每个字都重渲整个工作区 */
  const [content, setContent] = useState("");
  /** 编辑器里最新的全文；换文件、重建编辑器、看历史时从这里取 */
  const docRef = useRef<string | Text>("");
  const [goto, setGoto] = useState<Goto | null>(null);
  const [pdfUrl, setPdfUrl] = useState<string | null>(null);
  /** 正在显示的预览出自哪次编译；SyncTeX 要对着同一次的数据查 */
  const [shown, setShown] = useState<string | null>(null);
  const [mark, setMark] = useState<(Spot & { n: number }) | null>(null);
  const [job, setJob] = useState<Job>("idle");
  const [save, setSave] = useState<Save>("saved");
  const [meta, setMeta] = useState<{ engine: string; ms: number } | null>(null);
  const [progress, setProgress] = useState<Progress>(NO_PROGRESS);
  const [log, setLog] = useState("");
  /** 服务端解析好的问题（logparse.rs），编译结束时随 job.issues 到 */
  const [report, setReport] = useState<IssueReport | null>(null);
  const [logFocus, setLogFocus] = useState<LogFocus | null>(null);
  const [issueFocus, setIssueFocus] = useState<{ id: string; n: number } | null>(null);
  const focusSeq = useRef(0);
  const [showLog, setShowLog] = useState(false);
  const [tab, setTab] = useState<Tab>("issues");
  const [missing, setMissing] = useState<MissingFile[]>([]);
  const [fixes, setFixes] = useState<CompatFix[]>([]);
  /** 项目要的引擎没装（先用 XeLaTeX 试编了）：问题面板提示安装 */
  const [wanted, setWanted] = useState<EngineWanted | null>(null);
  const [stale, setStale] = useState<TexStale | null>(null);
  /** 每次编译结束加一，引擎切换据此重新问探测结果 */
  const [built, setBuilt] = useState(0);
  const [quick, setQuick] = useState(false);
  const [recent, setRecent] = useState<string[]>([]);
  /** .md 的光标行，Markdown 预览跟着走；.tex 不需要，免得每次移光标都重渲 */
  const [mdLine, setMdLine] = useState(1);
  /** .md 的「定位」：每按一次加一，Markdown 预览据此滚过去 */
  const [mdLocate, setMdLocate] = useState(0);
  /** 编译因超时、超了资源限额被终止：问题面板顶上说明 */
  const [stopped, setStopped] = useState<"timeout" | "memory" | "fileSize" | null>(null);
  /** 文件在别处被改过，又有没存的编辑：等用户选留哪份 */
  const [conflict, setConflict] = useState<{ path: string; mine: string } | null>(null);
  const conflictRef = useRef(conflict);
  conflictRef.current = conflict;
  /** 整篇换掉编辑器内容（载入外部修改、恢复历史） */
  const [replace, setReplace] = useState<Replace | null>(null);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [symbols, setSymbols] = useState<Symbols | null>(null);
  /** 光标位置：大纲高亮当前这一节。只在停下来后更新，打字时不重渲 */
  const [here, setHere] = useState<{ file: string; line: number } | null>(null);
  /** AI 辅助的配置；没配模型时编辑器里不出现 AI 入口。设置页回来会重新挂载，自然重读 */
  const [aiInfo, setAiInfo] = useState<AiInfo | null>(null);
  /** 问题面板里点了「修复」：交给打开那个文件的编辑器 */
  const [fix, setFix] = useState<(FixRequest & { file: string }) | null>(null);
  const fixSeq = useRef(0);

  const { layout, setLayout, reset } = useLayout();
  const mainRef = useRef<HTMLElement>(null);
  const editorRef = useRef<HTMLElement>(null);
  const logRef = useRef<HTMLDivElement>(null);

  const activeRef = useRef<string | null>(null);
  const timer = useRef<number | undefined>(undefined);
  const gotoSeq = useRef(0);
  const cursorLine = useRef(1);
  // 防抖期内尚未落盘的编辑；编译前必须先写掉，否则编的是旧稿
  const pending = useRef<{ path: string; text: string | Text } | null>(null);
  /** 打开过的文本文件读到 / 写下时磁盘上的修改时间：保存时带上，轮询时对比，认外部修改 */
  const mtimes = useRef(new Map<string, number | null>());
  /** 正在写盘：这时磁盘上的修改时间会变，轮询不能当成外部修改 */
  const saving = useRef(0);
  const replaceSeq = useRef(0);
  const symbolTimer = useRef<number | undefined>(undefined);
  const hereTimer = useRef<number | undefined>(undefined);

  /** 重取大纲、标签、文献；保存后稍等再取，连着保存只取一次 */
  const refreshSymbols = useCallback(
    (delay = 0) => {
      window.clearTimeout(symbolTimer.current);
      symbolTimer.current = window.setTimeout(
        () =>
          void api
            .symbols()
            .then((sym) => {
              setSymbols(sym);
              setProjectSymbols(sym);
            })
            .catch(() => {}),
        delay
      );
    },
    [api]
  );
  useEffect(() => () => window.clearTimeout(symbolTimer.current), []);
  // 切项目时清掉上一个项目的补全数据
  useEffect(() => () => setProjectSymbols({ labels: [], bib: [] }), [api]);

  /** 编辑器整篇换成 text（不算用户编辑，不触发保存） */
  const replaceEditor = useCallback((text: string) => {
    docRef.current = text;
    setContent(text);
    setReplace({ text, n: ++replaceSeq.current });
  }, []);

  // 编译结束才有；编译中的半截日志不算
  const issues = useMemo(() => {
    if (job === "running" || !report) return [];
    // 「TeX 太旧」卡片已经说清的原始报错不再重复列出，计数也不算
    return stale ? report.issues.filter((i) => !STALE_MSG.test(i.message)) : report.issues;
  }, [report, job, stale]);
  const errors = issues.filter((i) => i.level === "error").length;

  useEffect(() => {
    libraryApi.ai().then(setAiInfo, () => setAiInfo(null));
  }, []);
  const assist: AssistEnv | null = useMemo(
    () =>
      aiInfo && aiInfo.config.models.length > 0 && !locked
        ? { project: id, verify: aiInfo.config.verify, models: aiInfo.config.models, baseline: issues }
        : null,
    [aiInfo, locked, id, issues]
  );
  // 「There were undefined references」之类的总结前面已逐条列过，不算
  const warnings = issues.filter((i) => i.level === "warning" && i.kind !== "summary").length;

  const flush = useCallback(async () => {
    window.clearTimeout(timer.current);
    const p = pending.current;
    if (!p) return;
    pending.current = null;
    setSave("saving");
    saving.current++;
    try {
      const r = await api.save(p.path, p.text.toString(), mtimes.current.get(p.path));
      mtimes.current.set(p.path, r.mtime);
      // 保存途中又有新编辑的话，状态留给下一轮
      setSave(pending.current ? "dirty" : "saved");
      refreshSymbols(1500);
    } catch (e) {
      if (e instanceof ServerError && e.code === "file.changedOnDisk") {
        // 别处（git、另一个编辑器）改过这个文件：不覆盖，交给用户选留哪份
        // 写盘途中又打了字：pending 里是更新的那份（TS 以为上面置空后它一直是 null）
        const now = pending.current as { path: string; text: string | Text } | null;
        const mine = (now?.path === p.path ? now.text : p.text).toString();
        pending.current = null;
        setConflict({ path: p.path, mine });
        setSave("dirty");
        throw e;
      }
      // 没写进去：放回待写，下次保存或编译再试
      pending.current ??= p;
      setSave("error");
      toastError(t("workspace.saveFailed", { error: String(e).replace(/^Error:\s*/, "") }));
      throw e;
    } finally {
      saving.current--;
    }
  }, [api, refreshSymbols]);

  const openFile = useCallback(
    async (path: string, line?: number) => {
      const jump = line ? { line, n: ++gotoSeq.current } : null;
      if (path === activeRef.current) {
        if (jump) setGoto(jump);
        return;
      }
      await flush().catch(() => {});
      const kind = kindOf(path);
      let text: string | null = null;
      if (kind === "text") {
        // 扩展名看着像文本、内容却不是 UTF-8 的，退回按二进制看
        const f = await api.file(path).catch(() => null);
        if (f) {
          text = f.content;
          mtimes.current.set(path, f.mtime);
        }
      }
      activeRef.current = path;
      cursorLine.current = line ?? 1;
      setActive(path);
      setGoto(jump);
      setRecent((r) => [path, ...r.filter((p) => p !== path)].slice(0, 12));
      if (text === null) {
        setAttach({ kind: kind === "text" ? "binary" : kind, url: api.rawUrl(path) });
      } else {
        setAttach(null);
        docRef.current = text;
        setContent(text);
      }
    },
    [api, flush]
  );

  useEffect(() => {
    api
      .project()
      .then((p) => {
        setProject(p);
        const first = p.entry ?? p.files.find((f) => !f.dir && f.path.endsWith(".tex"))?.path;
        if (first) {
          void openFile(first);
          // 打开项目先出一次预览，和 Overleaf 的行为一致
          void api.build().catch(() => {});
        }
        refreshSymbols();
      })
      .catch((e) => {
        setLog(String(e));
        setTab("log");
        setShowLog(true);
        setJob("failed");
      });
  }, [api, openFile, refreshSymbols]);

  useEffect(() => {
    if (template === undefined) return;
    libraryApi
      .templates()
      .then((list) => setTpl(list.find((x) => x.id === template) ?? null))
      .catch(toastError);
  }, [template]);

  /** 用正在预览的模板新建项目，建好直接打开 */
  const useTpl = useCallback(async () => {
    if (!tpl || using) return;
    setUsing(true);
    try {
      onSwitch(await fromTemplate(tpl));
    } catch (e) {
      toastError(e);
      setUsing(false);
    }
  }, [tpl, using, onSwitch]);

  /** 归档状态变了（这里点了取消归档，或侧栏菜单里改的）：重新拿项目信息，只读随之切换 */
  const refreshProject = useCallback(() => void api.project().then(setProject).catch(() => {}), [api]);

  /** 文件树里改名、移动、删除、新建之后：刷新目录和大纲；打开着的文件（或它所在的文件夹）被改名就跟到新路径，
   *  被删了就换回主文件 */
  const onFsChange = useCallback(
    (from: string | null, to: string | null) => {
      const moved = (p: string) => !!from && (p === from || p.startsWith(`${from}/`));
      setRecent((r) => r.flatMap((p) => (!moved(p) ? [p] : to ? [to + p.slice(from!.length)] : [])));
      refreshSymbols();
      const cur = activeRef.current;
      void api
        .project()
        .then((p) => {
          setProject(p);
          if (!cur || !moved(cur)) return;
          // 先放掉旧路径，openFile 才不会当成「已经开着」直接返回
          activeRef.current = null;
          mtimes.current.delete(cur);
          const next = to ? to + cur.slice(from!.length) : (p.entry ?? p.files.find((f) => !f.dir && f.path.endsWith(".tex"))?.path);
          if (next) return openFile(next);
          setActive(null);
          setAttach(null);
          setContent("");
        })
        .catch(toastError);
    },
    [api, openFile, refreshSymbols]
  );

  const unarchive = useCallback(async () => {
    if (unarchiving) return;
    setUnarchiving(true);
    try {
      await libraryApi.archive(id, false);
      toast(t("home.unarchived", { name: project?.name ?? id }), "success");
      refreshProject();
    } catch (e) {
      toastError(e);
    } finally {
      setUnarchiving(false);
    }
  }, [id, project, unarchiving, refreshProject]);

  // 离开项目（回列表、切换项目）前把防抖中的编辑写掉
  useEffect(() => () => void flush().catch(() => {}), [flush]);

  // 有没写完的编辑时关窗口，浏览器会问一句
  useEffect(() => {
    const on = (e: BeforeUnloadEvent) => {
      if (pending.current) {
        void flush().catch(() => {});
        e.preventDefault();
      }
    };
    window.addEventListener("beforeunload", on);
    return () => window.removeEventListener("beforeunload", on);
  }, [flush]);

  const onChange = useCallback(
    (doc: Text) => {
      docRef.current = doc;
      // 全文只在真用得上时才拼：Markdown 预览要跟着刷新，.tex 等保存时再取
      if (isMarkdown(activeRef.current)) setContent(doc.toString());
      if (!activeRef.current || locked) return;
      setSave("dirty");
      // 正等用户决定冲突怎么办：手上的版本跟着改，先不写盘
      const c = conflictRef.current;
      if (c && c.path === activeRef.current) {
        setConflict({ ...c, mine: doc.toString() });
        return;
      }
      pending.current = { path: activeRef.current, text: doc };
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => void flush().catch(() => {}), 600);
    },
    [flush, locked]
  );

  /** 冲突时留磁盘上的：手上那份先记进历史，再载入磁盘版本 */
  const takeDisk = useCallback(async () => {
    const c = conflictRef.current;
    if (!c) return;
    try {
      await api.keepHistory(c.path, c.mine);
      const f = await api.file(c.path);
      mtimes.current.set(c.path, f.mtime);
      if (activeRef.current === c.path) replaceEditor(f.content);
      setConflict(null);
      setSave("saved");
      toast(t("workspace.external.tookDisk"), "success");
      refreshSymbols();
    } catch (e) {
      toastError(e);
    }
  }, [api, replaceEditor, refreshSymbols]);

  /** 冲突时留自己的：覆盖磁盘，磁盘上那份由服务端记进历史 */
  const keepMine = useCallback(async () => {
    const c = conflictRef.current;
    if (!c) return;
    saving.current++;
    try {
      const r = await api.save(c.path, c.mine, null, true);
      mtimes.current.set(c.path, r.mtime);
      setConflict(null);
      setSave("saved");
      toast(t("workspace.external.keptMine"), "success");
      refreshSymbols();
    } catch (e) {
      toastError(e);
    } finally {
      saving.current--;
    }
  }, [api, refreshSymbols]);

  // 轮询外部修改：当前文件被别的程序改了就载入（有没存的编辑时问用户），文件增删了刷新目录。
  // 窗口回到前台时立即看一次。模板预览是只读副本，不会被外部改
  useEffect(() => {
    if (preview) return;
    let live = true;
    let tree: string | null = null;
    let busy = false;
    const check = async () => {
      if (busy || document.hidden || saving.current) return;
      busy = true;
      try {
        const path = activeRef.current;
        const text = path != null && mtimes.current.has(path) ? path : null;
        const r = await api.stat(text);
        if (!live || saving.current) return;
        if (tree !== null && r.tree !== tree) void api.project().then((p) => live && setProject(p)).catch(() => {});
        tree = r.tree;
        if (!text || text !== activeRef.current || conflictRef.current) return;
        const known = mtimes.current.get(text);
        if (r.mtime == null || known == null || r.mtime === known) return;
        if (pending.current?.path === text) {
          window.clearTimeout(timer.current);
          const mine = pending.current.text.toString();
          pending.current = null;
          setConflict({ path: text, mine });
          return;
        }
        const f = await api.file(text);
        if (!live || activeRef.current !== text || pending.current || saving.current) return;
        mtimes.current.set(text, f.mtime);
        replaceEditor(f.content);
        toast(t("workspace.external.reloaded", { file: text }));
        refreshSymbols();
      } catch {
        /* 服务暂时连不上：下一轮再看 */
      } finally {
        busy = false;
      }
    };
    const timer2 = window.setInterval(() => void check(), 2500);
    const onFocus = () => void check();
    window.addEventListener("focus", onFocus);
    return () => {
      live = false;
      window.clearInterval(timer2);
      window.removeEventListener("focus", onFocus);
    };
  }, [api, preview, replaceEditor, refreshSymbols]);

  /** 恢复历史版本：手上的先写掉，再整篇换成那个版本（磁盘上现有的记进历史） */
  const restoreVersion = useCallback(
    async (text: string) => {
      const path = activeRef.current;
      if (!path) return;
      await flush().catch(() => {});
      saving.current++;
      try {
        const r = await api.save(path, text, mtimes.current.get(path), true);
        mtimes.current.set(path, r.mtime);
        replaceEditor(text);
        setSave("saved");
        toast(t("history.restored"), "success");
        refreshSymbols();
      } finally {
        saving.current--;
      }
    },
    [api, flush, replaceEditor, refreshSymbols]
  );

  /** 导出这个项目用到的补装宏包，存进项目根目录 */
  const exportSnapshot = useCallback(async () => {
    try {
      const r = await api.exportSnapshot();
      toast(t("workspace.snapshot.exported", { path: r.path, count: r.packages.length, size: fileSize(r.bytes, locale.code) }), "success");
      void api.project().then(setProject).catch(() => {});
    } catch (e) {
      toastError(e);
    }
  }, [api, locale]);

  useEffect(() => {
    const es = new EventSource("/api/events");
    es.onmessage = (e) => {
      const ev = JSON.parse(e.data) as JobEvent;
      // 事件流是全局的，只看当前项目
      if (ev.projectId !== id) return;
      switch (ev.type) {
        case "job.started":
          setJob("running");
          setProgress(NO_PROGRESS);
          setLog("");
          setReport(null);
          setMissing([]);
          setFixes([]);
          setWanted(null);
          setStale(null);
          setStopped(null);
          break;
        case "job.output":
          setLog((l) => (l ? `${l}\n${ev.text}` : ev.text));
          break;
        case "job.log":
          // 完整日志换掉实时拼出来的那份（两路输出的先后、引擎回退的说明以它为准）
          setLog(ev.text);
          break;
        case "job.issues":
          setReport(ev.report);
          break;
        case "job.progress":
          setProgress((p) =>
            ev.step === "pass"
              ? { pass: ev.pass, pages: 0, total: ev.pass > 1 && p.pages ? p.pages : p.total, tool: null }
              : ev.step === "pages"
                ? { ...p, pages: ev.pages, tool: null }
                : { ...p, tool: ev.step }
          );
          break;
        case "job.done":
          setJob("done");
          setMeta({ engine: ev.engine, ms: ev.ms });
          setBuilt((n) => n + 1);
          refreshSymbols();
          setPdfUrl(api.pdfUrl(ev.jobId));
          setShown(ev.jobId);
          break;
        case "job.failed":
          setJob("failed");
          setMeta({ engine: ev.engine, ms: ev.ms });
          setBuilt((n) => n + 1);
          setStopped(ev.reason === "timeout" || ev.reason === "memory" || ev.reason === "fileSize" ? ev.reason : null);
          setTab("issues");
          setShowLog(true);
          break;
        case "case.missing":
          setMissing(ev.files);
          setTab("issues");
          setShowLog(true);
          break;
        case "compat.found":
          setFixes(ev.fixes);
          setTab("issues");
          setShowLog(true);
          break;
        case "engine.missing":
          setWanted(ev);
          setTab("issues");
          setShowLog(true);
          break;
        case "tex.stale":
          setStale(ev);
          setTab("issues");
          setShowLog(true);
          break;
      }
    };
    return () => es.close();
  }, [id, api, refreshSymbols]);

  const buildNow = useCallback(async () => {
    try {
      await flush();
    } catch {
      return; // 保存失败已经提示过；不能拿旧稿去编
    }
    await api.build().catch(toastError);
  }, [api, flush]);

  /** 按钮、⌘↵、菜单发起的编译：正在编时不再发。后端会把重复请求排成「编完再整编一遍」，
   *  连点就白白多编一轮。换引擎、装好宏包等改了条件的仍走 buildNow，照常补编 */
  const running = job === "running";
  const requesting = useRef(false);
  const runningRef = useRef(running);
  runningRef.current = running;
  const buildByUser = useCallback(async () => {
    if (runningRef.current || requesting.current) return;
    requesting.current = true;
    try {
      await buildNow();
    } finally {
      requesting.current = false;
    }
  }, [buildNow]);

  /** SyncTeX 正向：光标所在行 → 预览；.md 没有 PDF 可查，定位到 Markdown 预览 */
  const syncForward = useCallback(() => {
    const path = activeRef.current;
    if (!path || attach) return;
    if (isMarkdown(path)) return void setMdLocate((n) => n + 1);
    if (!shown) return void toast(t("workspace.noPdfYet"));
    api
      .syncView(shown, path, cursorLine.current)
      .then((spot) => setMark({ ...spot, n: ++gotoSeq.current }))
      .catch(toastError);
  }, [api, shown, attach]);

  /** 导出正在显示的那份 PDF：存到项目根目录，与入口 .tex 同名。
   *  模板预览、已归档项目不往项目里写，直接下载（桌面壳存进「下载」文件夹） */
  const exporting = useRef(false);
  const exportPdf = useCallback(async () => {
    if (!shown) return void toast(t("workspace.noPdfYet"));
    if (locked) {
      const a = document.createElement("a");
      a.href = api.pdfUrl(shown);
      a.download = `${preview ? (tpl && templateName(tpl)) || "template" : (project?.name ?? id)}.pdf`;
      a.click();
      return;
    }
    if (exporting.current) return;
    exporting.current = true;
    try {
      const { path } = await api.exportPdf(shown);
      toast(t("workspace.exported", { path }), "success");
      // 新文件要出现在左侧目录里
      void api.project().then(setProject).catch(() => {});
    } catch (err) {
      toastError(err);
    } finally {
      exporting.current = false;
    }
  }, [api, shown, preview, locked, tpl, project, id]);

  useMenu({
    "file.quickOpen": () => setQuick(true),
    "file.save": () => void flush().catch(() => {}),
    "file.exportPdf": () => void exportPdf(),
    "build.run": () => void buildByUser(),
    "build.syncForward": syncForward,
    "build.log": () => setShowLog((s) => !s),
  });

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      if (!mod || e.altKey) return;
      const k = e.key.toLowerCase();
      if (k === "s") {
        e.preventDefault();
        void flush().catch(() => {});
      } else if (e.key === "Enter") {
        e.preventDefault();
        void buildByUser();
      } else if (k === "p" && !e.shiftKey) {
        e.preventDefault();
        setQuick((q) => !q);
      } else if (k === "j" && e.shiftKey) {
        e.preventDefault();
        syncForward();
      } else if (k === "j") {
        e.preventDefault();
        setShowLog((s) => !s);
      } else if (k === "e" && !e.shiftKey && (MAC ? e.metaKey : e.ctrlKey)) {
        // Mac 上 Ctrl+E 是编辑器的「到行尾」，只认 ⌘E
        e.preventDefault();
        void exportPdf();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [flush, buildByUser, syncForward, exportPdf]);

  const name = preview ? (tpl ? templateName(tpl) : "") : (project?.name ?? id);
  useEffect(() => {
    document.title = t(job === "running" ? "workspace.windowTitleBusy" : "workspace.windowTitle", { name });
    return () => {
      document.title = t("brand.windowTitle");
    };
  }, [name, job, locale]);

  const files = useMemo(() => (project?.files ?? []).filter((f) => !f.dir).map((f) => f.path), [project]);
  const fileSet = useMemo(() => new Set(files), [files]);

  /** 编辑 .md 时右栏换成 Markdown 预览 */
  const showMd = isMarkdown(active) && !attach;

  /** SyncTeX 反向：预览上的一点 → 源码 */
  const syncBack = (page: number, x: number, y: number) => {
    if (!shown) return;
    api
      .syncEdit(shown, page, x, y)
      .then(({ file, line }) =>
        fileSet.has(file) ? void openFile(file, line) : toast(t("workspace.notInProject", { file }))
      )
      .catch(toastError);
  };

  const openIssue = (i: Issue) => {
    if (i.file && i.line && fileSet.has(i.file)) void openFile(i.file, i.line);
  };

  const fixIssue = async (i: Issue) => {
    if (!i.file || !i.line || !fileSet.has(i.file)) return;
    await openFile(i.file, i.line);
    setFix({ issue: i, file: i.file, n: ++fixSeq.current });
  };

  const showInLog = (i: Issue) => {
    setTab("log");
    setShowLog(true);
    setLogFocus({ from: i.logRange[0], to: i.logRange[1], n: ++focusSeq.current });
  };

  const backToIssue = (i: Issue) => {
    setTab("issues");
    setIssueFocus({ id: i.id, n: ++focusSeq.current });
  };

  const openProblems = () => {
    setTab("issues");
    setShowLog(true);
  };

  const progressText =
    job !== "running"
      ? ""
      : progress.tool
        ? t(`workspace.status.progress.${progress.tool}`)
        : progress.pass
          ? [
              t("workspace.status.progress.pass", { n: progress.pass }),
              progress.pages
                ? progress.total && progress.pages <= progress.total
                  ? t("workspace.status.progress.pagesOf", { count: progress.pages, total: progress.total })
                  : t("workspace.status.progress.pages", { count: progress.pages })
                : "",
            ]
              .filter(Boolean)
              .join(" · ")
          : "";

  const issueCount = errors + warnings + missing.length + fixes.length + (wanted ? 1 : 0) + (stale ? 1 : 0);
  const statusText =
    job === "running"
      ? t("workspace.status.running")
      : job === "failed"
        ? errors
          ? t("workspace.status.errors", { count: errors })
          : t("workspace.status.failed")
        : job === "done"
          ? t("workspace.status.done")
          : t("workspace.status.idle");

  const cut = active ? active.lastIndexOf("/") + 1 : 0;

  return (
    <div className="app">
      {/* 键盘用户第一下 Tab 就能跳过顶栏直接进编辑区；平时看不见，聚焦才出来 */}
      <button
        className="skip-link"
        onClick={() => (document.querySelector<HTMLElement>(".editor-host .cm-content") ?? document.querySelector<HTMLElement>("main"))?.focus()}
      >
        {t("workspace.skipToEditor")}
      </button>
      <header className="topbar" data-tauri-drag-region>
        <button className="brand" onClick={onHome} title={t("workspace.home")}>
          <Mark busy={job === "running"} alert={job === "failed" || missing.length + fixes.length > 0 || !!wanted || !!stale} />
          {t("brand.primary")} {t("brand.secondary") && <span className="brand-en">{t("brand.secondary")}</span>}
        </button>
        <div className="crumbs" title={project?.root}>
          <span className="crumb-sep" aria-hidden="true">/</span>
          {locked ? (
            <span className="crumb-name">
              <span>{name}</span>
              <span className="crumb-tag">{t(preview ? "workspace.preview.tag" : "workspace.archived.tag")}</span>
            </span>
          ) : (
            <>
              <ProjectSwitch id={id} name={name} onSwitch={onSwitch} />
              {project?.folder && (
                <span className="crumb-tag" title={t("workspace.folderTitle", { path: project.root })}>
                  {t("home.tagFolder")}
                </span>
              )}
            </>
          )}
        </div>
        <div className="actions">
          {job === "failed" ? (
            <button className={`status ${job}`} onClick={openProblems} title={t("workspace.viewIssues")}>
              <span className="status-dot" />
              {statusText}
            </button>
          ) : (
            <span className={`status ${job}`}>
              {job === "running" ? <Spinner size={11} /> : <span className="status-dot" />}
              {statusText}
              {progressText && <span className="meta">{progressText}</span>}
              {job === "done" && meta && <span className="meta">{duration(meta.ms)}</span>}
            </span>
          )}
          {/* 读屏播报编译状态：失败时上面换成了按钮，播报区得一直挂着才念得出来 */}
          <span className="sr-only" role="status">
            {statusText}
          </span>
          <span className="topbar-sep" />
          <button
            className={`btn btn-ghost btn-sm${showLog ? " on" : ""}`}
            onClick={() => setShowLog((s) => !s)}
            title={t("workspace.logTitle")}
            aria-pressed={showLog}
          >
            <Icon name="terminal" size={15} />
            {t("workspace.log")}
            {errors > 0 ? (
              <CountBadge count={errors} label={t("workspace.status.errors", { count: errors })} />
            ) : warnings > 0 ? (
              <CountBadge count={warnings} warn label={t("workspace.status.warnings", { count: warnings })} />
            ) : null}
          </button>
          <button
            className="btn btn-ghost btn-sm"
            onClick={() => void exportPdf()}
            disabled={!shown}
            title={t(locked ? "workspace.preview.downloadTitle" : "workspace.exportTitle")}
          >
            <Icon name="download" size={14} />
            {t(locked ? "workspace.preview.download" : "workspace.export")}
          </button>
          {!locked && <EngineSwitch api={api} version={built} onChanged={() => void buildNow()} />}
          <button
            className={`btn btn-sm${locked ? "" : " btn-primary"}`}
            onClick={() => void buildByUser()}
            disabled={running}
            aria-busy={running}
            title={t("workspace.buildTitle")}
          >
            {job === "running" ? <Spinner size={12} /> : <Icon name="play" size={12} />}
            {t("workspace.build")}
            <Kbd>⌘↵</Kbd>
          </button>
          {preview && (
            <button
              className="btn btn-primary btn-sm"
              onClick={() => void useTpl()}
              disabled={!tpl || using}
              title={t("workspace.preview.useTitle")}
            >
              {using ? <Spinner size={12} /> : <Icon name="template" size={13} />}
              {t("workspace.preview.use")}
            </button>
          )}
          {archived && (
            <button
              className="btn btn-primary btn-sm"
              onClick={() => void unarchive()}
              disabled={unarchiving}
              title={t("workspace.archived.unarchiveTitle")}
            >
              {unarchiving ? <Spinner size={12} /> : <Icon name="unarchive" size={13} />}
              {t("workspace.archived.unarchive")}
            </button>
          )}
          <LocaleToggle />
          <ThemeToggle />
        </div>
      </header>

      <main
        ref={mainRef}
        style={{
          gridTemplateColumns: `${layout.tree}px ${GUTTER}px minmax(${PANE_MIN}px, ${layout.ratio}fr) ${GUTTER}px minmax(${PANE_MIN}px, ${1 - layout.ratio}fr)`,
        }}
      >
        <FileTree
          id={id}
          files={project?.files ?? []}
          entry={project?.entry ?? null}
          active={active}
          preview={preview ? name : undefined}
          archived={archived}
          onArchived={(pid) => pid === id && refreshProject()}
          onOpen={(p) => void openFile(p)}
          onSwitch={onSwitch}
          onRename={async (pid, next) => {
            if (pid !== id) {
              await libraryApi.rename(pid, next);
              return;
            }
            // 和顶栏改名一样：先把防抖中的编辑写进旧目录，改完切到新 id
            await flush();
            const r = await libraryApi.rename(id, next);
            if (r.id !== id) onSwitch(r.id);
          }}
          onQuickOpen={() => setQuick(true)}
          outline={symbols?.outline}
          here={here}
          onOutline={(file, line) => (fileSet.has(file) ? void openFile(file, line) : toast(t("workspace.notInProject", { file })))}
          onExportSnapshot={locked ? undefined : () => void exportSnapshot()}
          onBeforeFs={flush}
          onFsChange={onFsChange}
        />
        <Gutter
          title={t("workspace.gutterTree")}
          onReset={() => reset("tree")}
          onStart={() => {
            const t0 = layout.tree;
            const main = mainRef.current?.clientWidth ?? 0;
            // 目录栏再宽也得给编辑器和预览各留出最小宽度
            const hi = Math.min(TREE_MAX, main - 2 * PANE_MIN - 2 * GUTTER);
            return (dx) =>
              setLayout((l) => ({ ...l, tree: Math.round(Math.max(TREE_MIN, Math.min(hi, t0 + dx))) }));
          }}
        />
        <section className="editor-pane" ref={editorRef}>
          {active && (
            <div className="pane-bar">
              <span className="pane-title" title={active}>
                <Icon name={fileIcon(active)} size={15} />
                <span>
                  {cut > 0 && <span className="path-dir">{active.slice(0, cut)}</span>}
                  {active.slice(cut)}
                </span>
              </span>
              <span className="pane-tools">
                {attach ? (
                  <span className="dim">{t("workspace.readOnly")}</span>
                ) : locked ? (
                  <span
                    className="save-state"
                    title={t(preview ? "workspace.preview.readOnlyTitle" : "workspace.archived.readOnlyTitle")}
                  >
                    <Icon name="lock" size={12} /> {t(preview ? "workspace.preview.readOnly" : "workspace.archived.readOnly")}
                  </span>
                ) : (
                  <SaveState state={save} />
                )}
                {!attach && !preview && (
                  <button
                    className="btn btn-icon btn-sm btn-ghost"
                    onClick={() => setHistoryOpen(true)}
                    title={t("workspace.historyTitle")}
                    aria-label={t("workspace.historyTitle")}
                  >
                    <Icon name="history" size={14} />
                  </button>
                )}
                {attach && active.endsWith(".subocase") && !preview && <SnapshotImport path={active} api={api} onImported={() => void buildNow()} />}
                {/* .md 的预览本来就跟着光标走，不再放定位按钮 */}
                {!attach && !showMd && shown && (
                  <button
                    className="btn btn-icon btn-sm btn-ghost"
                    onClick={syncForward}
                    title={t("workspace.locate")}
                    aria-label={t("workspace.locateLabel")}
                  >
                    <Icon name="locate" size={14} />
                  </button>
                )}
              </span>
            </div>
          )}
          {conflict && (
            <div className="conflict-bar" role="alert">
              <Icon name="alert" size={14} />
              <span>{t("workspace.external.conflict", { file: conflict.path })}</span>
              <button className="btn btn-sm" onClick={() => void takeDisk()} title={t("workspace.external.takeDiskTitle")}>
                {t("workspace.external.takeDisk")}
              </button>
              <button className="btn btn-sm btn-primary" onClick={() => void keepMine()} title={t("workspace.external.keepMineTitle")}>
                {t("workspace.external.keepMine")}
              </button>
            </div>
          )}
          {active && attach && <Attachment key={attach.url} path={active} {...attach} />}
          {active && !attach && (
            <Editor
              key={`${active}:${locked}`}
              path={active}
              initial={docRef.current}
              readOnly={locked}
              onChange={onChange}
              goto={goto}
              onLocate={!showMd && shown ? syncForward : undefined}
              replace={replace}
              assist={assist}
              fix={fix && fix.file === active ? fix : null}
              onFixTaken={() => setFix(null)}
              onLine={(n) => {
                cursorLine.current = n;
                if (isMarkdown(active)) setMdLine(n);
                window.clearTimeout(hereTimer.current);
                hereTimer.current = window.setTimeout(() => setHere({ file: active, line: n }), 300);
              }}
            />
          )}
          {!active && (
            <div className="pane-empty">
              {project ? (
                <>
                  <Icon name="tex" size={32} />
                  <div>{t("workspace.pickFile")}</div>
                  <Shortcuts />
                </>
              ) : (
                <Spinner size={18} />
              )}
            </div>
          )}
        </section>
        <Gutter
          title={t("workspace.gutterSplit")}
          onReset={() => reset("ratio")}
          onStart={() => {
            const e0 = editorRef.current?.offsetWidth ?? 0;
            const rest = (mainRef.current?.clientWidth ?? 0) - layout.tree - 2 * GUTTER;
            return (dx) => {
              if (rest <= 2 * PANE_MIN) return;
              const px = Math.max(PANE_MIN, Math.min(rest - PANE_MIN, e0 + dx));
              setLayout((l) => ({ ...l, ratio: px / rest }));
            };
          }}
        />
        {showMd && active && (
          <MarkdownView
            path={active}
            text={content}
            rawUrl={api.rawUrl}
            line={mdLine}
            locate={mdLocate}
            onOpen={(p, line) => (fileSet.has(p) ? void openFile(p, line) : toast(t("workspace.notInProject", { file: p })))}
          />
        )}
        <Preview
          hidden={showMd}
          url={pdfUrl}
          mark={mark}
          onSync={syncBack}
          busy={job === "running"}
          notice={
            job === "failed" && pdfUrl ? (
              <>
                <Icon name="error" size={14} />
                <span>{t("workspace.failedNotice")}</span>
                <button className="btn btn-sm" onClick={openProblems}>
                  {t("workspace.viewIssues")}
                </button>
              </>
            ) : null
          }
          empty={
            job === "running" ? (
              <>
                <Spinner size={20} />
                <div className="preview-empty-title">{t("workspace.generating")}</div>
              </>
            ) : job === "failed" ? (
              <>
                <Icon name="error" size={30} />
                <div className="preview-empty-title">{t("workspace.notPassed")}</div>
                <button className="btn btn-sm" onClick={openProblems}>
                  {t("workspace.viewIssues")}
                </button>
              </>
            ) : (
              <>
                <Icon name="pdf" size={30} />
                <div className="preview-empty-title">{t("workspace.noPdf")}</div>
                <div>{tx("workspace.pressToBuild", { kbd: <Kbd>⌘↵</Kbd> })}</div>
              </>
            )
          }
        />
      </main>

      {showLog && (
        <Gutter
          axis="y"
          title={t("workspace.gutterLog")}
          onReset={() => reset("log")}
          onStart={() => {
            // 从实际显示高度起算：存的值可能因窗口变矮被 CSS 压过
            const h0 = logRef.current?.offsetHeight ?? layout.log;
            const hi = Math.max(LOG_MIN, window.innerHeight - LOG_ROOM);
            // 往上拖（dy 为负）日志变高
            return (dy) =>
              setLayout((l) => ({ ...l, log: Math.round(Math.max(LOG_MIN, Math.min(hi, h0 - dy))) }));
          }}
        />
      )}
      {showLog && (
        <div className="log" ref={logRef} style={{ height: layout.log, maxHeight: `calc(100vh - ${LOG_ROOM}px)` }}>
          <div className="pane-bar">
            <div className="tabs" role="tablist" onKeyDown={rovingKeys}>
              <button className={`tab${tab === "issues" ? " on" : ""}`} role="tab" aria-selected={tab === "issues"} tabIndex={tab === "issues" ? 0 : -1} onClick={() => setTab("issues")}>
                {t("workspace.tabIssues")}
                {issueCount > 0 && (
                  <CountBadge
                    count={issueCount}
                    warn={!(errors || missing.length || fixes.length || wanted || stale)}
                    label={t("workspace.status.issues", { count: issueCount })}
                  />
                )}
              </button>
              <button className={`tab${tab === "log" ? " on" : ""}`} role="tab" aria-selected={tab === "log"} tabIndex={tab === "log" ? 0 : -1} onClick={() => setTab("log")}>
                {t("workspace.tabLog")}
              </button>
            </div>
            <span className="pane-tools">
              {meta && job !== "running" && (
                <span className="dim">
                  {meta.engine} · {duration(meta.ms)}
                </span>
              )}
              <button
                className="btn btn-ghost btn-sm btn-icon"
                title={t("workspace.copyLog")}
                aria-label={t("workspace.copyLog")}
                disabled={!log}
                onClick={() =>
                  void navigator.clipboard?.writeText(log).then(
                    () => toast(t("workspace.logCopied"), "success"),
                    () => toast(t("common.copyFailed"), "error")
                  )
                }
              >
                <Icon name="copy" size={14} />
              </button>
              <button className="btn btn-ghost btn-sm btn-icon" onClick={() => setShowLog(false)} title={t("workspace.collapse")} aria-label={t("workspace.collapseLabel")}>
                <Icon name="x" size={14} />
              </button>
            </span>
          </div>
          {tab === "issues" ? (
            <div className="log-body">
              {fixes.length > 0 && (
                <CompatList fixes={fixes} apply={api.applyCompat} onFixed={() => void buildNow()} />
              )}
              {stopped && <StoppedNotice reason={stopped} />}
              {stale && <TexStaleCard stale={stale} onFixed={() => void buildNow()} />}
              {wanted && <EngineMissing want={wanted} onInstalled={() => void buildNow()} />}
              {/* 换了引擎试编时缺的文件多半是引擎不对造成的，先装引擎、重编后再看 */}
              {missing.length > 0 && !wanted && (
                <MissingList
                  files={missing}
                  project={preview ? undefined : id}
                  snapshots={preview ? [] : files.filter((f) => f.endsWith(".subocase"))}
                  importSnapshot={api.importSnapshot}
                  onInstalled={() => void buildNow()}
                />
              )}
              {/* 卡片已经说了为什么没编过，列表空了就不再显示「没有错误」 */}
              {!(stale && !issues.length) && (
                <IssueList
                  issues={issues}
                  boxTotal={report?.boxes ?? 0}
                  running={job === "running"}
                  failed={job === "failed"}
                  focusId={issueFocus}
                  canOpen={(i) => !!(i.file && i.line && fileSet.has(i.file))}
                  onOpen={openIssue}
                  onFix={assist ? (i) => void fixIssue(i) : undefined}
                  onShowLog={showInLog}
                />
              )}
            </div>
          ) : (
            <RawLog text={log} follow={job === "running"} issues={report?.issues ?? []} focus={logFocus} onIssue={backToIssue} />
          )}
        </div>
      )}

      {historyOpen && active && !attach && (
        <HistoryPanel
          path={active}
          current={docRef.current.toString()}
          list={() => api.history(active)}
          read={(vid) => api.historyVersion(active, vid)}
          onRestore={restoreVersion}
          readOnly={locked}
          onClose={() => setHistoryOpen(false)}
        />
      )}

      {quick && (
        <QuickOpen
          files={files}
          recent={recent}
          onClose={() => setQuick(false)}
          onPick={(p) => {
            setQuick(false);
            void openFile(p);
          }}
        />
      )}
    </div>
  );
}

function SaveState({ state }: { state: Save }) {
  if (state === "saving")
    return (
      <span className="save-state">
        <Spinner size={10} /> {t("workspace.save.saving")}
      </span>
    );
  if (state === "dirty")
    return (
      <span className="save-state" title={t("workspace.save.dirtyTitle")}>
        <span className="dot" /> {t("workspace.save.dirty")}
      </span>
    );
  if (state === "error")
    return (
      <span className="save-state" style={{ color: "var(--danger)" }} title={t("workspace.save.errorTitle")}>
        <Icon name="alert" size={13} /> {t("workspace.save.error")}
      </span>
    );
  return (
    <span className="save-state saved" title={t("workspace.save.savedTitle")}>
      <Icon name="check" size={13} /> {t("workspace.save.saved")}
    </span>
  );
}

function Shortcuts() {
  return (
    <div className="shortcut-list">
      <span>
        <Kbd>⌘↵</Kbd>
      </span>
      <span>{t("workspace.shortcuts.build")}</span>
      <span>
        <Kbd>⌘P</Kbd>
      </span>
      <span>{t("workspace.shortcuts.quickOpen")}</span>
      <span>
        <Kbd>⌘F</Kbd>
      </span>
      <span>{t("workspace.shortcuts.find")}</span>
      <span>
        <Kbd>⌘J</Kbd>
      </span>
      <span>{t("workspace.shortcuts.log")}</span>
      <span>
        <Kbd>⌘⇧J</Kbd>
      </span>
      <span>{t("workspace.shortcuts.sync")}</span>
      <span>
        <Kbd>⌘E</Kbd>
      </span>
      <span>{t("workspace.shortcuts.export")}</span>
    </div>
  );
}

/** 日志滚到底时跟着新输出走；用户往上翻了就不打扰 */
/** 外部程序的代号 → 显示名（专有名词，不随语言变） */
function toolName(id: string) {
  return id === "ghostscript" ? "Ghostscript (gs)" : id;
}

/** 自动安装过的包组合：装完重编还缺同样的包时不再自动装，免得来回打转 */
const autoTried = new Set<string>();

/** 旧模板的已知不兼容：说清原因，点「修复」才改文件，改完重编 */
function CompatList({
  fixes,
  apply,
  onFixed,
}: {
  fixes: CompatFix[];
  apply: (fix: CompatFix) => Promise<unknown>;
  onFixed: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const fixAll = async () => {
    setBusy(true);
    try {
      // 同一文件里的多处一次就改完，按文件去重
      const seen = new Set<string>();
      for (const f of fixes) {
        if (seen.has(f.file)) continue;
        seen.add(f.file);
        await apply(f);
      }
      toast(t("workspace.compat.fixed", { files: [...seen].join(", ") }), "success");
      onFixed();
    } catch (e) {
      toastError(e);
    }
    setBusy(false);
  };
  return (
    <div className="missing">
      <div className="missing-head">
        <Icon name="alert" size={14} />
        {t("workspace.compat.head")}
      </div>
      <ul>
        {fixes.map((f) => (
          <li key={`${f.file}:${f.line}`}>
            <code>{t("workspace.compat.where", { file: f.file, line: f.line })}</code>
            <span className="missing-arrow">→</span>
            <span>{t(`workspace.compat.${f.id}.why`)}</span>
          </li>
        ))}
      </ul>
      <div className="missing-hint">
        {busy ? (
          <span className="missing-busy">
            <Spinner size={12} /> {t("workspace.compat.fixing")}
          </span>
        ) : (
          <button className="btn btn-primary btn-sm" onClick={() => void fixAll()}>
            <Icon name="pencil" size={13} /> {t("workspace.compat.fix")}
          </button>
        )}
        <span className="dim">
          {[...new Set(fixes.map((f) => f.id))].map((id) => t(`workspace.compat.${id}.hint`)).join(" ")}{" "}
          {t("workspace.compat.hint")}
        </span>
      </div>
    </div>
  );
}

type EngineWanted = Extract<JobEvent, { type: "engine.missing" }>;
type TexStale = Extract<JobEvent, { type: "tex.stale" }>;
/** 「TeX 太旧」卡片已经说清的那几条原始报错，问题列表里不再重复 */
const STALE_MSG = /too old|l3-too-old|requested release|Mismatched LaTeX support files/;

/** 自动安装过的引擎：同一个引擎在一次运行里只自动装一回，装失败了不反复重试 */
const autoEngine = new Set<string>();

/** 项目要的引擎没装：内置 TeX 时一键装好重编（开了自动安装就直接装）；用户自己的 TeX 给出 tlmgr 命令 */
function EngineMissing({ want, onInstalled }: { want: EngineWanted; onInstalled: () => void }) {
  const locale = useLocale();
  const [busy, setBusy] = useState(false);
  const [auto, setAuto] = useState(false);
  const [failed, setFailed] = useState("");
  const program = programName(want.program);

  const install = async (byAuto = false) => {
    setBusy(true);
    setAuto(byAuto);
    setFailed("");
    try {
      const r = await libraryApi.installEngine(want.engine);
      toast(
        r.archive
          ? t("workspace.engineMissing.installedFromArchive", { name: want.name, url: r.archive })
          : t("workspace.engineMissing.installed", { name: want.name }),
        "success"
      );
      onInstalled();
    } catch (e) {
      setFailed(String(e instanceof Error ? e.message : e));
    }
    setBusy(false);
    setAuto(false);
  };

  useEffect(() => {
    if (!want.managed || autoEngine.has(want.engine)) return;
    let live = true;
    prefs()
      .then((p) => {
        if (!live || !p.autoInstall || autoEngine.has(want.engine)) return;
        autoEngine.add(want.engine);
        void install(true);
      })
      .catch(() => {});
    return () => {
      live = false;
    };
    // install 每次渲染都是新的，按引擎触发一次即可
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [want.engine, want.managed]);

  const cmd = `tlmgr install ${want.engine}`;
  return (
    <div className="missing">
      <div className="missing-head">
        <Icon name="alert" size={14} />
        {want.managed
          ? t("workspace.engineMissing.head", { program, size: fileSize(want.bytes, locale.code) })
          : t("workspace.engineMissing.headSystem", { program })}
      </div>
      <div className="missing-hint">
        {want.managed ? (
          <>
            {busy ? (
              <span className="missing-busy">
                <Spinner size={12} /> {auto ? t("workspace.engineMissing.autoInstalling", { name: want.name }) : t("workspace.engineMissing.installing", { name: want.name })}
              </span>
            ) : (
              <button className="btn btn-primary btn-sm" onClick={() => void install()}>
                <Icon name="download" size={13} /> {t("workspace.engineMissing.install", { name: want.name })}
              </button>
            )}
            {!busy && <span className="dim">{t("workspace.engineMissing.hint")}</span>}
          </>
        ) : (
          <>
            <code>{cmd}</code>
            <span className="dim">{t("workspace.engineMissing.hintSystem")}</span>
          </>
        )}
      </div>
      {failed && (
        <div className="missing-fail">
          <div>{t("workspace.engineMissing.failed")}</div>
          <pre>{failed}</pre>
        </div>
      )}
    </div>
  );
}

/** 编译被终止的原因：超时、吃内存、往外写爆了文件 */
function StoppedNotice({ reason }: { reason: "timeout" | "memory" | "fileSize" }) {
  useLocale();
  return (
    <div className="missing">
      <div className="missing-head">
        <Icon name="alert" size={14} />
        {t(`workspace.stopped.${reason}`)}
      </div>
      <div className="missing-hint">
        <span className="dim">{t(`workspace.stopped.${reason}Hint`)}</span>
      </div>
    </div>
  );
}

/** 打开的是项目里的 .subocase：顶栏给一个「导入」 */
function SnapshotImport({ path, api, onImported }: { path: string; api: ReturnType<typeof projectApi>; onImported: () => void }) {
  useLocale();
  const [busy, setBusy] = useState(false);
  return (
    <button
      className="btn btn-sm btn-primary"
      disabled={busy}
      title={t("workspace.snapshot.importTitle")}
      onClick={() => {
        setBusy(true);
        api
          .importSnapshot(path)
          .then((r) => {
            toast(importedText(r), "success");
            if (r.packages.length) onImported();
          })
          .catch(toastError)
          .finally(() => setBusy(false));
      }}
    >
      {busy ? <Spinner size={12} /> : <Icon name="package" size={13} />} {t("workspace.snapshot.import")}
    </button>
  );
}

function MissingList({
  files,
  project,
  snapshots,
  importSnapshot,
  onInstalled,
}: {
  files: MissingFile[];
  /** 连不上软件源时记进待取队列，记在这个项目名下 */
  project?: string;
  /** 项目里带着的字盘快照：离线时先试它 */
  snapshots: string[];
  importSnapshot: (path: string) => Promise<import("./api").SnapshotImported>;
  onInstalled: () => void;
}) {
  const pkgs = [...new Set(files.flatMap((f) => (f.package ? [f.package] : [])))];
  const cmd = `tlmgr install ${pkgs.join(" ")}`;
  const ctl = useInstall();
  /** idle → installing（点一下就装）→ 成功就重编；失败留下 tlmgr 的输出 */
  const step = ctl.busy ? "installing" : "idle";
  /** 这次是设置里的「自动安装」触发的，不是用户点的 */
  const [auto, setAuto] = useState(false);
  const [failLog, setFailLog] = useState("");
  /** 连不上软件源：已记进待取队列 */
  const [offline, setOffline] = useState(false);
  /** 查不到包名的（多半也是离线），服务端已经记进待取队列 */
  const queuedFiles = files.some((f) => f.kind === "file" && f.lookup === "unknown");
  const [importing, setImporting] = useState(false);
  /** 失败时说明原因：软件源已是新的年度版本（附带归档源也没成时的地址） */
  const [tooNew, setTooNew] = useState<{ year: number; archive: string | null } | null>(null);
  const progress = ctl.progress;
  const fontCount = files.filter((f) => f.kind === "font").length;
  const shipped = files.find((f) => f.kind === "font" && f.fontFile)?.fontFile;
  const otherFonts = files.some((f) => f.kind === "font" && !f.fontFile);
  const tools = files.filter((f) => f.kind === "tool");
  const local = files.find((f) => f.lookup === "project");
  const allLocal = files.every((f) => f.lookup === "project");
  const fileCount = files.length - fontCount - tools.length;
  const key = pkgs.join(" ");
  useEffect(() => {
    setFailLog("");
    setTooNew(null);
    setOffline(false);
  }, [key]);

  const install = async (byAuto = false) => {
    setAuto(byAuto);
    setFailLog("");
    setTooNew(null);
    setOffline(false);
    try {
      const r = await ctl.run(pkgs, byAuto, project);
      // 本机 tlmgr 太旧，服务端先替用户更新了它：改了用户自己的 TeX，得说一声
      if (r.tlmgrUpdated) toast(t("workspace.missing.tlmgrUpdated"));
      if (r.ok && r.archive) {
        toast(t("workspace.missing.installedFromArchive", { pkgs: key, url: r.archive }), "success");
        onInstalled();
      } else if (r.ok) {
        toast(t("workspace.missing.installed", { pkgs: key }), "success");
        onInstalled();
      } else if (r.cancelled) {
        toast(t("workspace.missing.installCancelled"));
      } else if (r.offline) {
        setOffline(true);
        setFailLog(r.log);
      } else {
        if (r.repoYear) setTooNew({ year: r.repoYear, archive: r.archive });
        setFailLog(r.log || t("workspace.missing.installFailed"));
      }
    } catch (e) {
      // 要电脑密码时自动安装不代劳，按钮还在，用户自己点
      if (e instanceof ServerError && e.code === "case.autoNeedsAdmin") toast(t("workspace.missing.autoNeedsAdmin"));
      else toastError(e);
    }
    setAuto(false);
  };

  /** 项目里带着快照：导入它（不用联网），导完重编 */
  const useSnapshot = async (path: string) => {
    setImporting(true);
    try {
      const r = await importSnapshot(path);
      toast(importedText(r), "success");
      if (r.packages.length) onInstalled();
    } catch (e) {
      toastError(e);
    }
    setImporting(false);
  };

  // 设置里开了自动安装：包名都查出来后直接装，装好自动重编
  const pending = files.some((f) => f.lookup === "pending");
  useEffect(() => {
    if (!pkgs.length || pending || autoTried.has(key)) return;
    let live = true;
    prefs()
      .then((p) => {
        if (!live || !p.autoInstall || autoTried.has(key)) return;
        autoTried.add(key);
        void install(true);
      })
      .catch(() => {});
    return () => {
      live = false;
    };
    // install 每次渲染都是新的，按包名组合触发一次即可
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, pending]);

  return (
    <div className="missing">
      <div className="missing-head">
        <Icon name="alert" size={14} />
        {allLocal
          ? t("workspace.missing.headProject", { count: files.length })
          : tools.length === files.length
            ? t("workspace.missing.headTools", { count: tools.length })
            : fontCount === 0 && tools.length === 0
              ? t("workspace.missing.head", { count: files.length })
              : fileCount === 0 && tools.length === 0
                ? t("workspace.missing.headFonts", { count: fontCount })
                : t("workspace.missing.headMixed", { count: files.length })}
      </div>
      <ul>
        {files.map((f) => (
          <li key={`${f.kind}:${f.file}`}>
            <code>{f.kind === "font" ? `"${f.file}"` : f.kind === "tool" ? toolName(f.file) : f.file}</code>
            <span className="missing-arrow">→</span>
            {f.kind === "font" &&
              (f.fontFile ? (
                <span>{tx("workspace.missing.fontShipped", { file: <code>{f.fontFile}</code> })}</span>
              ) : (
                <span className="dim">{t("workspace.missing.font")}</span>
              ))}
            {f.kind === "tool" && (
              <span className="dim">
                {tx("workspace.missing.ghostscript", {
                  files: <code>{(f.neededBy ?? []).join(", ")}</code>,
                })}
              </span>
            )}
            {f.kind === "file" && f.lookup === "found" && (
              <span>{tx("workspace.missing.found", { pkg: <code>{f.package}</code> })}</span>
            )}
            {f.kind === "file" && f.lookup === "pending" && <span className="dim">{t("workspace.missing.pending")}</span>}
            {f.kind === "file" && f.lookup === "absent" && <span className="dim">{t("workspace.missing.absent")}</span>}
            {f.kind === "file" && f.lookup === "unknown" && <span className="dim">{t("workspace.missing.unknown")}</span>}
            {f.kind === "file" && f.lookup === "project" && (
              <span>{tx("workspace.missing.project", { path: <code>{f.projectPath}</code> })}</span>
            )}
          </li>
        ))}
      </ul>
      {local && (
        <div className="missing-hint">
          <span className="dim">
            {tx("workspace.missing.projectHint", {
              path: <code>{local.projectPath}</code>,
              tree: <code>{"texmf/tex/latex/…"}</code>,
            })}
          </span>
        </div>
      )}
      {shipped && (
        <div className="missing-hint">
          <span className="dim">
            {tx("workspace.missing.fontShippedHint", {
              example: <code>{`\\setmainfont{${shipped}}`}</code>,
              file: <code>{shipped}</code>,
            })}
          </span>
        </div>
      )}
      {otherFonts && (
        <div className="missing-hint">
          <span className="dim">
            {tx("workspace.missing.fontHint", { dir: <code>fonts/</code>, opt: <code>[Path=./fonts/]</code> })}
          </span>
        </div>
      )}
      {tools.length > 0 && (
        <div className="missing-hint">
          <span className="dim">
            {tx(MAC ? "workspace.missing.ghostscriptHintMac" : "workspace.missing.ghostscriptHintLinux", {
              cmd: <code>{MAC ? "brew install ghostscript" : "sudo apt install ghostscript"}</code>,
            })}
          </span>
        </div>
      )}
      {pkgs.length > 0 && (
        <div className="missing-hint">
          <code>{cmd}</code>
          {step === "idle" && (
            <button className="btn btn-primary btn-sm" onClick={() => void install()}>
              <Icon name="download" size={13} /> {t("workspace.missing.install")}
            </button>
          )}
          {step === "installing" && (
            <span className="missing-busy">
              <Spinner size={12} /> {auto ? t("workspace.missing.autoInstalling") : t("workspace.missing.installing")}
            </span>
          )}
          <CancelInstall ctl={ctl} />
          {step !== "installing" && (
            <button
              className="btn btn-ghost btn-sm"
              title={t("workspace.missing.copyTitle")}
              onClick={() =>
                void navigator.clipboard?.writeText(cmd).then(
                  () => toast(t("workspace.missing.cmdCopied"), "success"),
                  () => toast(t("common.copyFailed"), "error")
                )
              }
            >
              <Icon name="copy" size={13} /> {t("common.copy")}
            </button>
          )}
          {step === "idle" && !failLog && <span className="dim">{t("workspace.missing.hint")}</span>}
        </div>
      )}
      {snapshots.length > 0 && pkgs.length > 0 && step === "idle" && (
        <div className="missing-hint">
          <span className="dim">{tx("workspace.missing.snapshotHint", { file: <code>{snapshots[0]}</code> })}</span>
          <button className="btn btn-sm" disabled={importing} onClick={() => void useSnapshot(snapshots[0])}>
            {importing ? <Spinner size={12} /> : <Icon name="package" size={13} />} {t("workspace.snapshot.import")}
          </button>
        </div>
      )}
      {(offline || queuedFiles) && (
        <div className="missing-hint">
          <span className="dim">{t(offline ? "workspace.missing.offlineQueued" : "workspace.missing.unknownQueued")}</span>
        </div>
      )}
      {step === "installing" && progress && <InstallMeter p={progress} />}
      {failLog && !offline && (
        <div className="missing-fail">
          {tooNew && (
            <div>
              {tooNew.archive
                ? t("workspace.missing.archiveFailed", { year: tooNew.year, url: tooNew.archive })
                : t("workspace.missing.repoTooNew", { year: tooNew.year })}
            </div>
          )}
          <div>{t("workspace.missing.installFailed")}</div>
          <pre>{failLog}</pre>
        </div>
      )}
    </div>
  );
}

/** 数字角标：颜色区分错误 / 警告，读屏和悬停提示念完整的一句（不光靠颜色和一个数） */
function CountBadge({ count, warn, label }: { count: number; warn?: boolean; label: string }) {
  return (
    <span className={`count-badge${warn ? " warn" : ""}`} title={label}>
      <span aria-hidden="true">{count}</span>
      <span className="sr-only">{label}</span>
    </span>
  );
}
