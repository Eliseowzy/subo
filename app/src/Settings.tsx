import Dropdown from "./Dropdown";
import { FailedActions, Updating, useMiktexUpdate } from "./TexFix";
import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type DragEvent, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from "react";
import { api, type AiConfig, type AiInfo, type AiModel, type AiService, type AiTest, type EnginesInfo, type EngineStatus, type FontFamily, type InstalledPackage, type InstallProgress, type ProjectSummary, type Settings, type SettingsInfo, type TexChoice, type TexStatus, type TlmgrStatus, type UpdateInfo, failMessage } from "./api";
import { QueuePanel, SnapshotTools } from "./CaseTools";
import { clock, installLeft } from "./Install";
import { programName } from "./EngineSwitch";
import Editor from "./Editor";
import Icon, { type IconName } from "./Icon";
import { TOUR_HASH } from "./Onboarding";
import { coverage, DEFAULT_THEME, defaultEditorSize, exportAll, exportLocale, exportTheme, importFile, removePack, resolvedTokens, saveFile, setEditorSize, setTheme, useAppearance } from "./appearance";
import { BUILTIN_LOCALES, packageDesc, setLocale, t, useLocale, useLocales, type Key } from "./i18n";
import { buildSkillZip, SKILL_FILE } from "./skill";
import { remember } from "./prefs";
import VirtualGrid from "./VirtualGrid";
import FontPreview from "./FontPreview";
import { ago, fileSize, LocaleSelect, MAC, rovingKeys, Spinner, ThemeSwitch, toast, toastError, useDialog } from "./ui";

/*
 * 设置页：`#/settings/<分区>`。左边分区导航，右边是一行一项「说明 | 控件」。
 * 改了就存（开关、单选、失焦），不设全局「保存」按钮；结果用轻提示告诉用户。
 */

export const SECTIONS = ["general", "editor", "library", "tex", "mirror", "engines", "packages", "fonts", "ai", "update"] as const;
export type Section = (typeof SECTIONS)[number];

/** from：从哪个页面点进来的（当前 hash），设置页的「返回」回到那里 */
export function settingsHash(section: Section = "general", from?: string) {
  return `#/settings/${section}${from ? `?from=${encodeURIComponent(from)}` : ""}`;
}

const NAV: Record<Section, { label: Key; icon: IconName }> = {
  general: { label: "settings.nav.general", icon: "sliders" },
  editor: { label: "settings.nav.editor", icon: "pencil" },
  library: { label: "settings.nav.library", icon: "folder" },
  tex: { label: "settings.nav.tex", icon: "tex" },
  engines: { label: "settings.nav.engines", icon: "play" },
  mirror: { label: "settings.nav.mirror", icon: "globe" },
  packages: { label: "settings.nav.packages", icon: "package" },
  fonts: { label: "settings.nav.fonts", icon: "font" },
  ai: { label: "settings.nav.ai", icon: "sparkle" },
  update: { label: "settings.nav.update", icon: "refresh" },
};

type Save = (patch: Partial<Omit<Settings, "library">>) => Promise<Settings | null>;

export default function SettingsPage({
  section,
  onSection,
  backToProject,
  onBack,
}: {
  section: Section;
  onSection: (s: Section) => void;
  /** 从项目里点进来的：返回回到那个项目，而不是项目列表 */
  backToProject?: boolean;
  onBack: () => void;
}) {
  useLocale();
  const [info, setInfo] = useState<SettingsInfo | null>(null);

  const load = useCallback(
    () =>
      api
        .settings()
        .then((i) => {
          setInfo(i);
          remember(i.settings);
        })
        .catch((e) => toast(t("settings.loadFailed", { detail: String(e.message ?? e) }), "error")),
    []
  );
  useEffect(() => void load(), [load]);

  // Esc 回项目列表；输入框里按 Esc 只是退出输入
  useEffect(() => {
    const on = (e: KeyboardEvent) => {
      const tag = (e.target as HTMLElement).tagName;
      if (e.key === "Escape" && !/^(INPUT|TEXTAREA|SELECT)$/.test(tag)) onBack();
    };
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, [onBack]);

  const save: Save = useCallback(async (patch) => {
    try {
      const s = await api.saveSettings(patch);
      remember(s);
      setInfo((i) => i && { ...i, settings: s });
      return s;
    } catch (e) {
      toastError(e);
      return null;
    }
  }, []);

  return (
    <div className="app settings">
      <div className="drag-strip" data-tauri-drag-region />
      <div className="settings-split">
        <nav className="settings-nav" aria-label={t("settings.title")}>
          <button className="btn btn-ghost btn-sm settings-back" onClick={onBack}>
            <Icon name="chevron" size={14} className="flip" />
            {t(backToProject ? "settings.backToProject" : "settings.back")}
          </button>
          <h1>{t("settings.title")}</h1>
          <ul>
            {SECTIONS.map((s) => (
              <li key={s}>
                <button className={section === s ? "on" : ""} aria-current={section === s ? "page" : undefined} onClick={() => onSection(s)}>
                  <Icon name={NAV[s].icon} size={16} />
                  {t(NAV[s].label)}
                </button>
              </li>
            ))}
          </ul>
        </nav>
        <div className="settings-body" role="main">
          <div className={`settings-inner${section === "packages" || section === "fonts" || section === "mirror" ? " wide" : ""}`} key={section}>
            <h2>{t(NAV[section].label)}</h2>
            {section === "general" && <General />}
            {section === "editor" && <EditorPrefs />}
            {section === "packages" && <Packages bundled={info?.env.bundledTex ?? false} />}
            {section === "fonts" && <Fonts />}
            {section === "engines" && <Engines />}
            {section === "ai" && <Ai />}
            {!info && section !== "general" && section !== "editor" && section !== "packages" && section !== "fonts" && section !== "engines" && section !== "ai" && (
              <div className="settings-loading">
                <Spinner />
              </div>
            )}
            {info && section === "library" && (
              <>
                <Library info={info} onChanged={load} />
                <Archived />
              </>
            )}
            {info && section === "tex" && <Tex info={info} save={save} reload={load} />}
            {info && section === "mirror" && <Mirror info={info} save={save} />}
            {info && section === "update" && <Updates settings={info.settings} save={save} />}
          </div>
        </div>
      </div>
    </div>
  );
}

/**
 * 一项设置：左边名称和说明，右边控件。stack 时控件总在下一行（路径、列表这种宽的）；
 * wrap 时放得下就并排，窄了再换行（滑块这种不能缩的）
 */
function Row({ label, hint, children, stack, wrap }: { label: ReactNode; hint?: ReactNode; children?: ReactNode; stack?: boolean; wrap?: boolean }) {
  return (
    <div className={`set-row${stack ? " stack" : ""}${wrap ? " wrap" : ""}`}>
      <div className="set-label">
        <div className="set-name">{label}</div>
        {hint && <p>{hint}</p>}
      </div>
      {children && <div className="set-control">{children}</div>}
    </div>
  );
}

function Switch({ on, onChange, label }: { on: boolean; onChange: (v: boolean) => void; label: string }) {
  return (
    <button className={`switch${on ? " on" : ""}`} role="switch" aria-checked={on} aria-label={label} onClick={() => onChange(!on)}>
      <span />
    </button>
  );
}

function PathLine({ path, onReveal }: { path: string; onReveal?: () => void }) {
  return (
    <span className="set-path">
      <code title={path}>{path}</code>
      {onReveal && (
        <button className="btn btn-ghost btn-sm btn-icon" title={t("settings.reveal")} aria-label={t("settings.reveal")} onClick={onReveal}>
          <Icon name="reveal" size={15} />
        </button>
      )}
    </span>
  );
}

const reveal = (what: "library" | "subo") => () => void api.revealDir(what).catch(toastError);

/* ---------- 通用 ---------- */

function General() {
  const { themes, theme } = useAppearance();
  const [dialog, setDialog] = useState<PackKind | null>(null);
  const open = (k: PackKind) => (
    <button className="btn btn-ghost btn-sm pack-open" onClick={() => setDialog(k)}>
      <Icon name="sliders" size={14} /> {t("settings.packs.customize")}
    </button>
  );
  return (
    <>
      <div className="set-group">
        <Row label={t("theme.label")} hint={t("settings.general.themeHint")}>
          <ThemeSwitch />
        </Row>
        <Row label={t("settings.general.scheme")} hint={t("settings.general.schemeHint")}>
          {open("theme")}
          <Dropdown
            value={theme}
            options={[{ value: DEFAULT_THEME, label: t("appearance.defaultTheme") }, ...themes.map((p) => ({ value: p.id, label: p.name }))]}
            onChange={setTheme}
            label={t("settings.general.scheme")}
          />
        </Row>
        <Row label={t("language.label")} hint={t("settings.general.languageHint")}>
          {open("locale")}
          <LocaleSelect />
        </Row>
        <Row label={t("settings.general.tour")} hint={t("settings.general.tourHint")}>
          <button className="btn btn-sm" onClick={() => (location.hash = TOUR_HASH)}>
            {t("settings.general.tourReplay")}
          </button>
        </Row>
      </div>
      {dialog && <PackDialog kind={dialog} onClose={() => setDialog(null)} />}
    </>
  );
}

function PackDialog({ kind, onClose }: { kind: PackKind; onClose: () => void }) {
  const dialog = useDialog(onClose);
  return (
    <div className="overlay pack-overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="pack-dialog" role="dialog" aria-modal="true" aria-label={t(SECTION_TEXT[kind].title)} ref={dialog} tabIndex={-1}>
        <div className="pack-dialog-head">
          <span className="pack-dialog-title">{t(SECTION_TEXT[kind].title)}</span>
          <button className="btn btn-ghost btn-sm btn-icon" onClick={onClose} title={t("common.close")} aria-label={t("common.close")}>
            <Icon name="x" size={14} />
          </button>
        </div>
        {kind === "theme" ? <ThemePacks /> : <LocalePacks />}
      </div>
    </div>
  );
}

/* ---------- 编辑器 ---------- */

/** 预览用的一段 LaTeX：命令是代码不翻译，标题、注释、正文走语言文件 */
function previewSample() {
  return [
    "\\documentclass{article}",
    "\\usepackage{amsmath}",
    "",
    "\\begin{document}",
    `\\section{${t("settings.editor.sampleSection")}}`,
    `% ${t("settings.editor.sampleComment")}`,
    `${t("settings.editor.sampleBody")} $E = mc^2$.`,
    "\\begin{equation}",
    "  \\int_0^1 x^2 \\, dx = \\frac{1}{3}",
    "\\end{equation}",
    "\\end{document}",
  ].join("\n");
}

/** 滑块以配色方案的默认字号为中点，左右各 5px */
const SIZE_SPAN = 5;
const TICKS = Array.from({ length: SIZE_SPAN * 2 + 1 }, (_, i) => i);

/** 字号输入框：回车或失焦生效，Esc 取消，↑↓ 每次 0.5；超出滑块范围的夹到两端 */
function SizeInput({ value, min, max, onCommit }: { value: number; min: number; max: number; onCommit: (v: number) => void }) {
  const [draft, setDraft] = useState<string | null>(null);
  const commit = (raw: string) => {
    setDraft(null);
    const n = Math.round(Number(raw.replace(/px$/i, "").trim()) * 2) / 2;
    if (raw.trim() && Number.isFinite(n)) onCommit(Math.min(max, Math.max(min, n)));
  };
  return (
    <span className="size-input">
      <input
        className="input input-sm"
        inputMode="decimal"
        value={draft ?? String(value)}
        onChange={(e) => setDraft(e.target.value)}
        onFocus={(e) => e.target.select()}
        onBlur={(e) => commit(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") e.currentTarget.blur();
          else if (e.key === "Escape") {
            setDraft(null);
            e.currentTarget.blur();
          } else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
            e.preventDefault();
            const cur = Number(draft ?? value);
            commit(String((Number.isFinite(cur) ? cur : value) + (e.key === "ArrowUp" ? 0.5 : -0.5)));
          }
        }}
        aria-label={t("settings.editor.fontSizeInput")}
        title={t("settings.editor.fontSizeInput")}
      />
      <span className="size-input-unit">{t("settings.editor.unit")}</span>
    </span>
  );
}

function EditorPrefs() {
  const { editorSize, theme } = useAppearance();
  const locale = useLocale();
  const base = defaultEditorSize(theme);
  const min = base - SIZE_SPAN;
  const max = base + SIZE_SPAN;
  const value = Math.min(max, Math.max(min, editorSize ?? base));
  // 回到中点就当没改过，跟着配色方案走
  const pick = (v: number, save: boolean) => setEditorSize(v === base ? null : v, save);
  const reset = () => setEditorSize(null);
  const pct = ((value - base + SIZE_SPAN) / (SIZE_SPAN * 2)) * 100;
  return (
    <>
      <div className="set-group">
        <Row label={t("settings.editor.fontSize")} hint={t("settings.editor.fontSizeHint")} wrap>
          <span className="size-slider">
            <span className="size-slider-rail">
              <Icon name="font" size={12} className="size-slider-a" />
              <span className="size-slider-track">
                <input
                  type="range"
                  min={min}
                  max={max}
                  step={0.5}
                  value={value}
                  style={{ "--pct": `${pct}%` } as CSSProperties}
                  onChange={(e) => pick(Number(e.target.value), false)}
                  onPointerUp={(e) => pick(Number(e.currentTarget.value), true)}
                  onKeyUp={(e) => pick(Number(e.currentTarget.value), true)}
                  onDoubleClick={reset}
                  title={t("settings.editor.sliderTitle")}
                  aria-label={t("settings.editor.fontSize")}
                  aria-valuetext={t("settings.editor.fontSizePx", { n: value })}
                />
                <span className="size-slider-ticks" aria-hidden>
                  {TICKS.map((i) => (
                    <i key={i} className={i === SIZE_SPAN ? "major" : undefined} style={{ "--at": i / (SIZE_SPAN * 2) } as CSSProperties} />
                  ))}
                </span>
              </span>
              <Icon name="font" size={18} className="size-slider-a" />
            </span>
            <span className="size-slider-rail">
              <SizeInput value={value} min={min} max={max} onCommit={(v) => pick(v, true)} />
              <button className="btn btn-ghost btn-sm" onClick={reset} disabled={editorSize == null} style={{ visibility: editorSize == null ? "hidden" : undefined }}>
                {t("settings.editor.reset")}
              </button>
            </span>
          </span>
        </Row>
      </div>
      <h3 className="set-sub">{t("settings.editor.preview")}</h3>
      <div className="set-editor-preview">
        {/* 换语言时重建，示例文字跟着换 */}
        <Editor key={locale.code} path="preview.tex" initial={previewSample()} readOnly onChange={() => {}} />
      </div>
    </>
  );
}

type PackKind = "theme" | "locale";
type PackRow = { id: string; name: string; meta: string[]; builtin: boolean; active: boolean };

/** 配色方案：每项带浅色、深色两张缩略图，导入前后心里有数 */
function ThemePacks() {
  const { themes, theme } = useAppearance();
  const rows: PackRow[] = [
    { id: DEFAULT_THEME, name: t("appearance.defaultTheme"), meta: [], builtin: true, active: theme === DEFAULT_THEME },
    ...themes.map((p) => ({
      id: p.id,
      name: p.name,
      meta: [p.author ? t("settings.packs.by", { author: p.author }) : "", p.description ?? ""].filter(Boolean),
      builtin: false,
      active: theme === p.id,
    })),
  ];
  return <PackSection kind="theme" rows={rows} hasCustom={themes.length > 0} />;
}

function LocalePacks() {
  const locale = useLocale();
  const all = useLocales();
  const { locales } = useAppearance();
  const pct = (n: number) => n.toLocaleString(locale.code, { style: "percent", maximumFractionDigits: 1 });
  const rows: PackRow[] = all.map((l) => {
    const pack = l.custom ? locales.find((p) => p.code === l.code) : undefined;
    const meta = [l.code];
    if (pack?.author) meta.push(t("settings.packs.by", { author: pack.author }));
    if (pack) meta.push(t("settings.packs.coverage", { pct: pct(coverage(pack)) }));
    if (pack && BUILTIN_LOCALES.some((b) => b.code === l.code)) meta.push(t("settings.packs.overrides"));
    return { id: l.code, name: l.name, meta, builtin: !l.custom, active: locale.code === l.code };
  });
  return <PackSection kind="locale" rows={rows} hasCustom={locales.length > 0} />;
}

const SECTION_TEXT = {
  theme: {
    title: "settings.packs.themesTitle",
    intro: "settings.packs.themesIntro",
    exportAllTitle: "settings.packs.themesExportAllTitle",
    skillTitle: "settings.packs.themeSkillTitle",
    skillSaved: "settings.packs.themeSkillSaved",
  },
  locale: {
    title: "settings.packs.localesTitle",
    intro: "settings.packs.localesIntro",
    exportAllTitle: "settings.packs.localesExportAllTitle",
    skillTitle: "settings.packs.localeSkillTitle",
    skillSaved: "settings.packs.localeSkillSaved",
  },
} as const;

/** 一类包的列表：内置的只能导出（作为定制的起点），导入的还能删；不在用的可以直接切过去 */
function PackSection({ kind, rows, hasCustom }: { kind: PackKind; rows: PackRow[]; hasCustom: boolean }) {
  const locale = useLocale();
  const input = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const [armed, setArmed] = useState<string | null>(null);
  const timer = useRef<number>();
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const text = SECTION_TEXT[kind];

  const pct = (n: number) => n.toLocaleString(locale.code, { style: "percent", maximumFractionDigits: 1 });

  const onFile = async (file: File | undefined) => {
    if (!file) return;
    setBusy(true);
    try {
      const done = await importFile(file);
      if (done.length === 1) {
        const [d] = done;
        const msg = t(d.replaced ? "settings.packs.updated" : "settings.packs.imported", { name: d.name });
        const r = d.report;
        const notes = [
          r.coverage && r.coverage[0] < r.coverage[1] ? t("settings.packs.partial", { pct: pct(r.coverage[0] / r.coverage[1]) }) : "",
          r.unknown.length ? t("settings.packs.dropped", { count: r.unknown.length, list: r.unknown.slice(0, 3).join(", ") }) : "",
          r.invalid.length ? t("settings.packs.invalid", { count: r.invalid.length, list: r.invalid.slice(0, 3).join(", ") }) : "",
          r.placeholders.length ? t("settings.packs.placeholders", { count: r.placeholders.length, list: r.placeholders.slice(0, 2).join("; ") }) : "",
        ].filter(Boolean);
        toast([msg, ...notes].join(" · "), notes.length ? "info" : "success");
      } else {
        toast(t("settings.packs.restored", { count: done.length }), "success");
      }
    } catch (e) {
      toastError(e);
    }
    setBusy(false);
  };

  const remove = async (row: PackRow) => {
    if (armed !== row.id) {
      setArmed(row.id);
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setArmed(null), 3000);
      return;
    }
    setArmed(null);
    try {
      await removePack(kind, row.id);
      toast(t("settings.packs.removed", { name: row.name }));
    } catch (e) {
      toastError(e);
    }
  };

  const skill = async () => {
    saveFile(SKILL_FILE[kind], await buildSkillZip(kind));
    toast(t(text.skillSaved, { file: SKILL_FILE[kind] }), "success");
  };

  return (
    <div className="pack-dialog-body">
      <p className="set-intro">{t(text.intro)}</p>
      <div className="set-toolbar">
        <button className="btn btn-sm btn-primary" onClick={() => input.current?.click()} disabled={busy}>
          {busy ? <Spinner size={12} /> : <Icon name="upload" size={14} />}
          {t("settings.packs.import")}
        </button>
        <button className="btn btn-sm" onClick={() => exportAll(kind)} disabled={!hasCustom} title={t(text.exportAllTitle)}>
          <Icon name="download" size={14} /> {t("settings.packs.exportAll")}
        </button>
        <button className="btn btn-sm" onClick={skill} title={t(text.skillTitle)}>
          <Icon name="sliders" size={14} /> {t("settings.packs.skill")}
        </button>
        <input
          ref={input}
          type="file"
          accept=".json,application/json"
          hidden
          onChange={(e) => {
            void onFile(e.target.files?.[0]);
            e.target.value = "";
          }}
        />
      </div>
      <div className="set-group">
        {rows.map((r) => (
          <div key={r.id} className={`set-row pack-row${r.active ? " active" : ""}`}>
            {kind === "theme" ? <ThemePreview id={r.id} /> : <Icon name="globe" size={16} className="pack-icon" />}
            <div className="set-label">
              <div className="set-name">
                <span lang={kind === "locale" ? r.id : undefined}>{r.name}</span>
                {r.builtin && <span className="tag muted">{t("settings.packs.builtin")}</span>}
                {r.active && <span className="tag">{t("settings.packs.active")}</span>}
              </div>
              {r.meta.length > 0 && <p>{r.meta.join(" · ")}</p>}
            </div>
            <span className={`set-actions${armed === r.id ? " armed" : ""}`}>
              {!r.active && (
                <button className="btn btn-sm" onClick={() => (kind === "theme" ? setTheme(r.id) : setLocale(r.id))}>
                  {t("settings.packs.use")}
                </button>
              )}
              <button
                className="btn btn-ghost btn-sm btn-icon"
                title={t("settings.packs.export")}
                aria-label={t("settings.packs.export")}
                onClick={() => (kind === "theme" ? exportTheme(r.id) : exportLocale(r.id))}
              >
                <Icon name="download" size={15} />
              </button>
              {!r.builtin && (
                <button
                  className={`btn btn-ghost btn-sm${armed === r.id ? " btn-danger-solid" : " btn-icon btn-danger"}`}
                  title={t("settings.packs.remove")}
                  aria-label={t("settings.packs.remove")}
                  onClick={() => void remove(r)}
                >
                  <Icon name="trash" size={15} />
                  {armed === r.id && t("settings.packs.removeConfirm")}
                </button>
              )}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}

/** 配色缩略图：浅色、深色各一张小窗口——顶栏、侧栏、编辑区里几行着色的代码、一个强调色按钮 */
function ThemePreview({ id }: { id: string }) {
  const { themes } = useAppearance();
  // 导入的包变了要重算；内置令牌本身有缓存
  const tokens = useMemo(() => resolvedTokens(id), [id, themes]);
  return (
    <span className="theme-preview" role="img" aria-label={t("settings.packs.previewLabel")}>
      {(["light", "dark"] as const).map((m) => {
        const tk = tokens[m];
        const bar = (color: string, width: string) => <i style={{ background: color, width }} />;
        return (
          <span key={m} className="tp-window" title={t(m === "light" ? "settings.packs.previewLight" : "settings.packs.previewDark")} style={{ background: tk.bg, borderColor: tk["line-strong"] }}>
            <span className="tp-header" style={{ background: tk.header, borderColor: tk["header-edge"] }}>
              <i style={{ background: tk["header-dim"] }} />
            </span>
            <span className="tp-body">
              <span className="tp-panel" style={{ background: tk.sidebar, borderColor: tk.line }}>
                {bar(tk["ink-3"], "70%")}
                <i className="tp-sel" style={{ background: tk["accent-soft"] }} />
                {bar(tk["ink-3"], "55%")}
              </span>
              <span className="tp-editor" style={{ background: tk["editor-bg"] }}>
                <span>
                  {bar(tk["syn-command"], "34%")}
                  {bar(tk["syn-arg"], "40%")}
                </span>
                <span>
                  {bar(tk["ink"], "56%")}
                  {bar(tk["syn-math"], "22%")}
                </span>
                <span>{bar(tk["syn-comment"], "46%")}</span>
                <span className="tp-accent" style={{ background: tk.accent }} />
              </span>
            </span>
          </span>
        );
      })}
    </span>
  );
}

/* ---------- 已归档项目 ---------- */

/** 归档的项目从首页收起，统一在这里管理：放回常用列表、在访达里显示、把外部文件夹移出列表 */
function Archived() {
  const [projects, setProjects] = useState<ProjectSummary[] | null>(null);

  const load = useCallback(() => api.projects().then((ps) => setProjects(ps.filter((p) => p.archived)), toastError), []);
  useEffect(() => void load(), [load]);

  const act = (run: () => Promise<unknown>) => void run().catch(toastError);

  const unarchive = (p: ProjectSummary) =>
    act(() => api.archive(p.id, false).then(() => (toast(t("home.unarchived", { name: p.name }), "success"), load())));

  const forget = (p: ProjectSummary) =>
    act(() => api.forgetFolder(p.id).then(() => (toast(t("home.forgotten", { name: p.name })), load())));

  return (
    <>
      <h3 className="set-sub">{t("settings.library.archivedTitle")}</h3>
      <p className="set-intro">{t("settings.library.archivedIntro")}</p>
      {projects && projects.length === 0 ? (
        <div className="set-empty">{t("settings.library.archivedEmpty")}</div>
      ) : (
        <div className="set-group">
          {!projects && (
            <div className="settings-loading">
              <Spinner />
            </div>
          )}
          {projects?.map((p) => (
            <div key={p.id} className="set-row">
              <span className={`project-icon${p.folder ? " folder" : ""}`}>
                <Icon name={p.missing ? "alert" : p.folder ? "folder" : "tex"} size={16} />
              </span>
              <div className="set-label">
                <div className="set-name">
                  {p.name}
                  {p.folder && !p.missing && <span className="tag muted">{t("home.tagFolder")}</span>}
                  {p.missing && <span className="tag warn">{t("home.tagMissing")}</span>}
                </div>
                <p>{p.folder ? <code title={p.path}>{p.path}</code> : p.entry ? <code>{p.entry}</code> : t("home.noTexFile")}</p>
              </div>
              <span className="set-actions">
                <button className="btn btn-sm" onClick={() => unarchive(p)}>
                  <Icon name="unarchive" size={14} /> {t("home.unarchive")}
                </button>
                {!p.missing && (
                  <button
                    className="btn btn-ghost btn-sm btn-icon"
                    title={t("home.revealTitle", { path: p.path })}
                    aria-label={t("home.reveal")}
                    onClick={() => act(() => api.reveal(p.id))}
                  >
                    <Icon name="reveal" size={15} />
                  </button>
                )}
                {p.folder && (
                  <button
                    className="btn btn-ghost btn-sm btn-icon btn-danger"
                    title={t("home.forgetTitle")}
                    aria-label={t("home.forget")}
                    onClick={() => forget(p)}
                  >
                    <Icon name="x" size={15} />
                  </button>
                )}
              </span>
            </div>
          ))}
        </div>
      )}
    </>
  );
}

/* ---------- 项目库 ---------- */

function Library({ info, onChanged }: { info: SettingsInfo; onChanged: () => Promise<unknown> }) {
  const { env, settings } = info;
  /** 正在确认的新位置；reset 表示恢复默认 */
  const [draft, setDraft] = useState<{ path: string; typing: boolean; reset: boolean } | null>(null);
  const [move, setMove] = useState(true);
  const [busy, setBusy] = useState(false);

  const change = async () => {
    setBusy(true);
    try {
      const { path } = await api.pickFolder();
      if (path) setDraft({ path, typing: false, reset: false });
    } catch {
      // 系统选择框用不了，退回手填
      setDraft({ path: "", typing: true, reset: false });
    }
    setBusy(false);
  };

  const confirm = async () => {
    if (!draft || (!draft.reset && !draft.path.trim())) return;
    setBusy(true);
    try {
      const r = await api.setLibrary(draft.reset ? null : draft.path.trim(), move);
      const parts = [t("settings.library.changed", { path: r.library })];
      if (move && r.moved) parts.push(t("settings.library.moved", { count: r.moved }));
      if (r.left.length) parts.push(t("settings.library.left", { count: r.left.length, names: r.left.slice(0, 3).join(", ") }));
      toast(parts.join(" · "), r.left.length ? "info" : "success");
      setDraft(null);
      await onChanged();
    } catch (e) {
      toastError(e);
    }
    setBusy(false);
  };

  return (
    <div className="set-group">
      <Row label={t("settings.library.location")} hint={t("settings.library.locationHint")} stack>
        <PathLine path={env.library} onReveal={reveal("library")} />
        {env.libraryLocked ? (
          <p className="set-note">
            <Icon name="info" size={14} /> {t("settings.library.locked")}
          </p>
        ) : (
          !draft && (
            <span className="set-actions">
              <button className="btn btn-sm" onClick={() => void change()} disabled={busy}>
                <Icon name="folderOpen" size={14} /> {t("settings.library.change")}
              </button>
              {settings.library ? (
                <button className="btn btn-ghost btn-sm" onClick={() => setDraft({ path: env.defaultLibrary, typing: false, reset: true })} disabled={busy}>
                  {t("settings.library.reset")}
                </button>
              ) : (
                <span className="tag muted">{t("settings.library.isDefault")}</span>
              )}
            </span>
          )
        )}
        {draft && (
          <div className="set-confirm">
            <div className="set-name">{t("settings.library.confirmTitle")}</div>
            {draft.typing ? (
              <input
                className="input"
                autoFocus
                value={draft.path}
                onChange={(e) => setDraft({ ...draft, path: e.target.value })}
                onKeyDown={(e) => e.key === "Enter" && void confirm()}
                placeholder={t("settings.library.pathPlaceholder")}
                aria-label={t("settings.library.pathLabel")}
              />
            ) : (
              <code className="set-confirm-path">{draft.path}</code>
            )}
            <label className="check">
              <input type="checkbox" checked={move} onChange={(e) => setMove(e.target.checked)} />
              {t("settings.library.moveProjects")}
            </label>
            <p className="dim">{move ? t("settings.library.moveHint") : t("settings.library.noMoveHint")}</p>
            <span className="set-actions">
              <button className="btn btn-sm" onClick={() => setDraft(null)} disabled={busy}>
                {t("common.cancel")}
              </button>
              <button className="btn btn-primary btn-sm" onClick={() => void confirm()} disabled={busy || (!draft.reset && !draft.path.trim())}>
                {busy && <Spinner size={12} />}
                {t("settings.library.confirm")}
              </button>
            </span>
          </div>
        )}
      </Row>
    </div>
  );
}

/* ---------- 排版与安装 ---------- */

/** 用哪套 TeX：本机的还是内置的。本机的太旧时说明为什么用了内置的；MiKTeX 可以就地更新 */
function TexSource({ st, setSt, save, reload }: { st: TexStatus; setSt: (s: TexStatus) => void; save: Save; reload: () => void }) {
  useLocale();
  const { state, run } = useMiktexUpdate((next) => {
    setSt(next);
    reload();
  });
  const own = st.own;
  if (!own) return null;
  const kernel = own.kernel ?? t("settings.texSource.unknown");
  const year = st.bundledYear ?? "";
  const name = t(`tex.name.${own.kind}` as Key);

  let hint: string;
  if (!st.hasBundled) hint = t(own.old ? "settings.texSource.oldNoBundled" : "settings.texSource.ownOnly", { tex: name, kernel, floor: st.floor });
  else if (st.choice === "auto" && st.bundled) hint = t("settings.texSource.autoOld", { tex: name, kernel, floor: st.floor, year });
  else if (st.choice === "own" && own.old) hint = t("settings.texSource.ownOld", { tex: name, kernel, floor: st.floor });
  else hint = t("settings.texSource.hint");

  const change = async (v: string) => {
    const s = await save({ tex: v as TexChoice });
    if (!s) return;
    const next = await api.tex().catch(() => null);
    if (next) setSt(next);
    toast(t(next?.bundled ? "settings.texSource.nowBundled" : "settings.texSource.nowOwn", { year, tex: name }), "success");
    reload();
  };

  // 更新后内核够新，自动档会换回本机的
  const canUpdate = own.kind === "miktex" && own.old && st.choice !== "bundled";
  return (
    <>
      <Row label={t("settings.texSource.label")} hint={hint}>
        {st.switchable && (
          <Dropdown
            value={st.choice}
            options={[
              { value: "auto", label: t("settings.texSource.auto") },
              { value: "own", label: t("settings.texSource.own", { tex: name, kernel }) },
              { value: "bundled", label: t("settings.texSource.bundled", { year }) },
            ]}
            onChange={(v) => void change(v)}
            label={t("settings.texSource.label")}
          />
        )}
        {canUpdate && state.phase === "idle" && (
          <button className="btn btn-sm" onClick={() => void run()}>
            {t("tex.miktex.update")}
          </button>
        )}
      </Row>
      {canUpdate && state.phase === "running" && (
        <div className="set-follow">
          <Updating since={state.since} />
        </div>
      )}
      {canUpdate && state.phase === "failed" && (
        <div className="set-follow tex-fix">
          <div className="tex-fix-title">
            <Icon name="alert" size={14} />
            {t("tex.miktex.failedTitle")}
          </div>
          <p className="tex-fix-sub">{t(`tex.miktex.why.${state.reason}` as Key)}</p>
          <FailedActions
            reason={state.reason}
            log={state.log}
            retry={() => void run()}
            recheck={() =>
              void api.refreshTex().then((next) => {
                setSt(next);
                reload();
              }, toastError)
            }
          />
        </div>
      )}
    </>
  );
}

/** 本机 TeX Live 的 tlmgr 旧不旧：tlmgr 比软件源旧时不肯装宏包。进设置页就核对一次，旧了给「更新 tlmgr」 */
function TlmgrCheck() {
  useLocale();
  const [st, setSt] = useState<TlmgrStatus | null>(null);
  const [updating, setUpdating] = useState(false);
  const [log, setLog] = useState("");
  const check = useCallback(() => {
    setSt(null);
    api.tlmgrStatus().then(setSt, (e) => setSt({ state: "unknown", admin: false, detail: String(e) }));
  }, []);
  useEffect(check, [check]);
  if (st?.state === "na") return null;

  const update = async () => {
    setUpdating(true);
    setLog("");
    try {
      const r = await api.updateTlmgr();
      if (r.ok) {
        toast(t("settings.tex.tlmgr.updated"), "success");
        check();
      } else if (r.cancelled) {
        toast(t("settings.tex.tlmgr.cancelled"));
      } else {
        toast(t("settings.tex.tlmgr.failed"), "error");
        setLog(r.log);
      }
    } catch (e) {
      toastError(e);
    }
    setUpdating(false);
  };

  const admin = st?.admin ? ` ${t("settings.tex.tlmgr.adminNote")}` : "";
  const hint = !st
    ? t("settings.tex.tlmgr.checking")
    : st.state === "current"
      ? t("settings.tex.tlmgr.current")
      : st.state === "stale"
        ? t("settings.tex.tlmgr.stale", { local: st.local, remote: st.remote }) + admin
        : t("settings.tex.tlmgr.unknown");
  return (
    <>
      <Row label={t("settings.tex.tlmgr.label")} hint={hint}>
        {!st ? (
          <Spinner size={13} />
        ) : updating ? (
          <span className="dim">
            <Spinner size={12} /> {t("settings.tex.tlmgr.updating")}
          </span>
        ) : st.state === "stale" ? (
          <button className="btn btn-sm btn-primary" onClick={() => void update()}>
            {t("settings.tex.tlmgr.update")}
          </button>
        ) : (
          <button className="btn btn-ghost btn-sm" onClick={check}>
            {t("settings.tex.tlmgr.recheck")}
          </button>
        )}
      </Row>
      {log && (
        <div className="set-follow tex-fix">
          <div className="tex-fix-title">
            <Icon name="alert" size={14} />
            {t("settings.tex.tlmgr.log")}
          </div>
          <pre className="tex-fix-log">{log}</pre>
        </div>
      )}
    </>
  );
}

function Tex({ info, save, reload }: { info: SettingsInfo; save: Save; reload: () => void }) {
  const { env, settings } = info;
  const [texSt, setTexSt] = useState<TexStatus | null>(null);
  useEffect(() => void api.tex().then(setTexSt, () => setTexSt(null)), [env.bundledTex]);
  const tex = env.tex;
  const [min, max] = env.timeoutRange;
  const [timeout, setTimeoutText] = useState(String(settings.timeoutSecs ?? ""));
  useEffect(() => setTimeoutText(String(settings.timeoutSecs ?? "")), [settings.timeoutSecs]);
  const [memMin, memMax] = env.memoryRange;
  // 滑块不超过本机内存：比内存还大的上限等于不设
  const memTop = Math.max(memMin, Math.min(memMax, env.totalMemory ?? memMax));
  const memStops = stops(MEMORY_STOPS, memMin, memTop, 512);
  const [parMin, parMax] = env.parallelRange;
  const parStops = Array.from({ length: parMax - parMin + 1 }, (_, i) => parMin + i);
  const memNow = settings.memoryMb ?? env.defaultMemory;
  const parNow = settings.parallel ?? env.cpus;

  const commitTimeout = async () => {
    const raw = timeout.trim();
    if (raw === String(settings.timeoutSecs ?? "")) return;
    if (!raw) {
      await save({ timeoutSecs: null });
      return;
    }
    const n = Number(raw);
    const s = await save({ timeoutSecs: Number.isFinite(n) ? Math.round(n) : -1 });
    if (s) toast(t("settings.tex.timeoutSaved", { secs: s.timeoutSecs ?? env.defaultTimeout }), "success");
    else setTimeoutText(String(settings.timeoutSecs ?? ""));
  };

  const mock = env.engine === "mock";
  const own = texSt?.own;
  // 没有内置 TeX 时服务端不给 own：tlmgr 报得出版本就是 TeX Live。说清是哪套、装在哪
  const kind = own?.kind ?? (tex?.texVersion ? "texlive" : null);
  const root = tex?.texRoot;
  const systemHint = kind
    ? root
      ? t("settings.tex.systemAt", { tex: t(`tex.name.${kind}` as Key), path: root })
      : t("settings.tex.system", { tex: t(`tex.name.${kind}` as Key) })
    : root
      ? t("settings.tex.systemPlainAt", { path: root })
      : t("settings.tex.systemPlain");
  return (
    <div className="set-group">
      {texSt && <TexSource st={texSt} setSt={setTexSt} save={save} reload={reload} />}
      <Row
        label={t("settings.tex.engine")}
        hint={
          mock
            ? t("settings.tex.mock")
            : env.bundledTex
              ? t("settings.tex.bundled")
              : systemHint
        }
        stack={!mock}
      >
        {!mock && (
          <>
            <span className="set-engine">
              <span className="engine">
                <span className="status-dot" /> {env.engine}
              </span>
              {tex?.texVersion && <span className="tag muted">{t("settings.tex.version", { version: tex.texVersion })}</span>}
            </span>
          </>
        )}
      </Row>
      {/* 用的是本机自己的 TeX Live 才有这一项（不是 TeX Live 的服务端答 na，自己藏起来）：内置 TeX 补装走用户层，不查 tlmgr 的版本 */}
      {!env.bundledTex && !mock && <TlmgrCheck />}
      <Row label={t("settings.tex.autoInstall")} hint={t("settings.tex.autoInstallHint")}>
        <Switch
          on={settings.autoInstall}
          label={t("settings.tex.autoInstall")}
          onChange={(v) =>
            void save({ autoInstall: v }).then((s) => s && toast(v ? t("settings.tex.autoOn") : t("settings.tex.autoOff"), "success"))
          }
        />
      </Row>
      <Row label={t("settings.tex.timeout")} hint={t("settings.tex.timeoutHint", { secs: env.defaultTimeout })}>
        <span className="set-number">
          <input
            className="input"
            type="number"
            inputMode="numeric"
            min={min}
            max={max}
            step={10}
            value={timeout}
            placeholder={String(env.defaultTimeout)}
            aria-label={t("settings.tex.timeoutLabel")}
            onChange={(e) => setTimeoutText(e.target.value)}
            onBlur={() => void commitTimeout()}
            onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
          />
          <span className="dim">{t("settings.tex.seconds")}</span>
          {settings.timeoutSecs != null && (
            <button className="btn btn-ghost btn-sm" onClick={() => void save({ timeoutSecs: null })}>
              {t("settings.tex.reset")}
            </button>
          )}
        </span>
      </Row>
      <Row
        label={
          // 本机有几个核就标在标题旁：滑块的上限就是它
          <span className="set-name-with">
            {t("settings.tex.parallel")}
            <span className="tag muted">{t("settings.tex.machineCpus", { n: env.cpus })}</span>
          </span>
        }
        hint={t("settings.tex.parallelHint", { cpus: env.cpus })}
        wrap
      >
        <RangeSetting
          saved={settings.parallel}
          fallback={env.cpus}
          stops={parStops}
          min={parMin}
          max={parMax}
          unit={t("settings.tex.parallelUnit")}
          format={String}
          parse={(raw) => {
            const n = Number(raw.trim());
            return Number.isFinite(n) ? Math.round(n) : null;
          }}
          label={t("settings.tex.parallelLabel")}
          valueText={(n) => t("settings.tex.parallelValue", { n })}
          onSave={async (n) => {
            const s = await save({ parallel: n });
            if (s) toast(t("settings.tex.parallelSaved", { n: s.parallel ?? env.cpus }), "success");
            return !!s;
          }}
        />
      </Row>
      <Row
        label={
          <span className="set-name-with">
            {t("settings.tex.memory")}
            <span className="tag muted">
              {env.totalMemory != null ? t("settings.tex.machineMemory", { size: mbText(env.totalMemory) }) : t("settings.tex.machineMemoryUnknown")}
            </span>
          </span>
        }
        hint={t("settings.tex.memoryHint", { size: mbText(env.defaultMemory) })}
        wrap
      >
        <RangeSetting
          saved={settings.memoryMb}
          fallback={env.defaultMemory}
          stops={memStops}
          min={memMin}
          max={memTop}
          unit={t("settings.tex.gigabytes")}
          format={(mb) => gbNumber(mb)}
          parse={parseMemory}
          label={t("settings.tex.memoryLabel")}
          valueText={mbText}
          onSave={async (mb) => {
            const s = await save({ memoryMb: mb });
            if (s) toast(t("settings.tex.memorySaved", { size: mbText(s.memoryMb ?? env.defaultMemory) }), "success");
            return !!s;
          }}
        />
      </Row>
      {/* 只在用户自己调过时提醒：默认组合是按「不额外限制」给的，真正吃满的情况很少 */}
      {env.totalMemory != null && (settings.parallel != null || settings.memoryMb != null) && memNow * parNow > env.totalMemory && (
        <div className="set-follow set-warn">
          <Icon name="alert" size={14} />
          {t("settings.tex.overcommit", { n: parNow, each: mbText(memNow), sum: mbText(memNow * parNow), total: mbText(env.totalMemory) })}
        </div>
      )}
    </div>
  );
}

/* ---------- 并行编译数、内存上限这类有范围的数值 ---------- */

/** 内存滑块的停靠点（MB）：小处密、大处疏，常用的 4 GB、8 GB 都落在点上 */
const MEMORY_STOPS = [512, 1024, 1536, 2048, 3072, 4096, 6144, 8192, 12288, 16384, 24576, 32768, 49152, 65536];

/** 取 [min, max] 里的停靠点；max 不在点上就补上它（按 snap 向下取整） */
function stops(all: number[], min: number, max: number, snap: number): number[] {
  const in_ = all.filter((v) => v >= min && v <= max);
  const top = Math.max(min, Math.floor(max / snap) * snap);
  if (in_[in_.length - 1] !== top) in_.push(top);
  return in_;
}

/** MB 换成 GB 的数字，最多两位小数，去掉尾零 */
function gbNumber(mb: number): string {
  return String(Math.round((mb / 1024) * 100) / 100);
}

function mbText(mb: number): string {
  return mb < 1024 ? t("settings.tex.mbValue", { n: mb }) : t("settings.tex.gbValue", { n: gbNumber(mb) });
}

/** 内存输入：不带单位按 GB；也认 512M、512MB、6G、6GB */
function parseMemory(raw: string): number | null {
  const m = /^\s*(\d+(?:\.\d+)?)\s*(g|gb|gib|m|mb|mib)?\s*$/i.exec(raw);
  if (!m) return null;
  const n = Number(m[1]);
  return Math.round(m[2] && m[2][0].toLowerCase() === "m" ? n : n * 1024);
}

function nearest(list: number[], v: number): number {
  let best = 0;
  list.forEach((x, i) => {
    if (Math.abs(x - v) < Math.abs(list[best] - v)) best = i;
  });
  return best;
}

/**
 * 滑块 + 输入框。拖动、输入只改草稿，「应用」或回车才保存；
 * 「重置」丢掉草稿回到已保存的值，「恢复默认」清掉设置（null）
 */
function RangeSetting({
  saved,
  fallback,
  stops: list,
  min,
  max,
  unit,
  format,
  parse,
  label,
  valueText,
  onSave,
}: {
  saved: number | null;
  fallback: number;
  stops: number[];
  min: number;
  max: number;
  unit: string;
  format: (v: number) => string;
  parse: (raw: string) => number | null;
  label: string;
  valueText: (v: number) => string;
  onSave: (v: number | null) => Promise<boolean>;
}) {
  const current = saved ?? fallback;
  const [draft, setDraft] = useState(current);
  const [text, setText] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => setDraft(current), [current]);

  const clamp = (v: number) => Math.min(max, Math.max(min, v));
  // 单位占多宽：汉字一个字宽，拉丁字母约 0.65 个
  const unitEm = [...unit].reduce((w, c) => w + (c.charCodeAt(0) > 0xff ? 1 : 0.65), 0);
  const dirty = draft !== current;
  const field = useRef<HTMLInputElement>(null);
  /** 框里最长会出现几个字（「0.5」「10」） */
  const width = Math.max(format(min).length, format(max).length, ...list.map((v) => format(v).length));
  const idx = nearest(list, draft);
  const pct = list.length > 1 ? (idx / (list.length - 1)) * 100 : 100;
  const defIdx = nearest(list, fallback);

  const apply = async (v: number | null) => {
    setBusy(true);
    const ok = await onSave(v);
    setBusy(false);
    if (!ok) setDraft(current);
  };
  /** 输入框的字读成数；读不出就退回草稿 */
  const take = (raw: string): number | null => {
    setText(null);
    const n = parse(raw);
    if (n == null) return null;
    const v = clamp(n);
    setDraft(v);
    return v;
  };

  return (
    <span className="size-slider range-setting">
      <span className="size-slider-rail">
        <span className="dim range-end">{format(list[0])}</span>
        <span className="size-slider-track">
          <input
            type="range"
            min={0}
            max={list.length - 1}
            step={1}
            value={idx}
            disabled={busy}
            style={{ "--pct": `${pct}%` } as CSSProperties}
            onChange={(e) => setDraft(list[Number(e.target.value)])}
            onDoubleClick={() => setDraft(fallback)}
            title={t("settings.tex.sliderTitle")}
            aria-label={label}
            aria-valuetext={valueText(draft)}
          />
          <span className="size-slider-ticks" aria-hidden>
            {list.map((v, i) => (
              <i key={v} className={i === defIdx ? "major" : undefined} style={{ "--at": list.length > 1 ? i / (list.length - 1) : 1 } as CSSProperties} />
            ))}
          </span>
        </span>
        <span className="dim range-end">{format(list[list.length - 1])}</span>
      </span>
      <span className="size-slider-rail">
        {/* 框宽按最长的值固定，数字和单位一起居中：一位数、两位数切换时布局不动 */}
        <span
          className={`input input-sm range-input${busy ? " disabled" : ""}`}
          style={{ width: `calc(${width + 0.2}ch + ${unitEm}em + 28px)` }}
          onMouseDown={(e) => {
            if (e.target !== field.current) {
              e.preventDefault();
              field.current?.focus();
            }
          }}
        >
          <input
              ref={field}
              inputMode="decimal"
              value={text ?? format(draft)}
              disabled={busy}
              style={{ width: `${Math.max((text ?? format(draft)).length, 1) + 0.2}ch` }}
              onChange={(e) => setText(e.target.value)}
              onFocus={(e) => e.target.select()}
              onBlur={(e) => take(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  const v = take(e.currentTarget.value);
                  if (v != null && v !== current) void apply(v === fallback ? null : v);
                } else if (e.key === "Escape") {
                  setText(null);
                  setDraft(current);
                  e.currentTarget.blur();
                } else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
                  e.preventDefault();
                  setText(null);
                  const i = nearest(list, draft) + (e.key === "ArrowUp" ? 1 : -1);
                  setDraft(list[Math.min(list.length - 1, Math.max(0, i))]);
                }
              }}
              aria-label={label}
              title={t("settings.tex.inputTitle", { min: valueText(min), max: valueText(max) })}
          />
          {unit && <span className="range-unit">{unit}</span>}
        </span>
      </span>
      {/* 三种状态叠在同一格里，宽度取最宽的那种，切换时不挤动左边 */}
      <span className="range-actions">
        <span className={dirty ? undefined : "off"} aria-hidden={!dirty}>
          <button className="btn btn-primary btn-sm" disabled={busy || !dirty} onClick={() => void apply(draft === fallback ? null : draft)}>
            {t("settings.tex.apply")}
          </button>
          <button className="btn btn-ghost btn-sm" disabled={busy || !dirty} onClick={() => setDraft(current)}>
            {t("settings.tex.revert")}
          </button>
          {saved != null && <ResetButton busy={busy} on={dirty} apply={apply} />}
        </span>
        <span className={!dirty && saved != null ? undefined : "off"} aria-hidden={dirty || saved == null}>
          <span className="dim">{t("settings.tex.usingCustom")}</span>
          <ResetButton busy={busy} on={!dirty && saved != null} apply={apply} />
        </span>
        <span className={!dirty && saved == null ? undefined : "off"} aria-hidden={dirty || saved != null}>
          <span className="dim">{t("settings.tex.usingDefault")}</span>
        </span>
        {/* 只占位：最宽的组合 */}
        <span className="off" aria-hidden>
          <span className="btn btn-sm">{t("settings.tex.apply")}</span>
          <span className="btn btn-sm">{t("settings.tex.revert")}</span>
          <span className="btn btn-sm">{t("settings.tex.reset")}</span>
        </span>
      </span>
    </span>
  );
}

function ResetButton({ busy, on, apply }: { busy: boolean; on: boolean; apply: (v: null) => Promise<void> }) {
  return (
    <button className="btn btn-ghost btn-sm" disabled={busy || !on} tabIndex={on ? undefined : -1} onClick={() => void apply(null)}>
      {t("settings.tex.reset")}
    </button>
  );
}

/* ---------- 引擎 ---------- */

/** TeX Live 每年三月前后发新版，主软件源随即切过去，旧年份冻结。
 *  这次运行里已经改用过归档源就是确证；否则按日期推：过了次年四月一定已冻结 */
function frozen(info: EnginesInfo): boolean {
  if (info.archive) return true;
  if (info.year == null) return false;
  const now = new Date();
  const y = now.getFullYear();
  return y > info.year + 1 || (y === info.year + 1 && now.getMonth() >= 3);
}

function Engines() {
  const locale = useLocale();
  const [info, setInfo] = useState<EnginesInfo | null>(null);
  /** 正在装 / 卸的引擎 */
  const [busy, setBusy] = useState<string | null>(null);
  const [failed, setFailed] = useState<{ id: string; text: string } | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  const load = useCallback(() => api.engines().then(setInfo, toastError), []);
  useEffect(() => void load(), [load]);

  const run = async (e: EngineStatus, what: "install" | "uninstall" | "import") => {
    setBusy(e.id);
    setFailed(null);
    try {
      if (what === "uninstall") {
        await api.uninstallEngine(e.id);
        toast(t("settings.engines.uninstalled", { name: e.name }), "success");
      } else {
        const r = await api.installEngine(e.id, what === "import" ? "import" : undefined);
        toast(
          r.archive
            ? t("settings.engines.installedFromArchive", { name: e.name, url: r.archive })
            : t("settings.engines.installed", { name: e.name, size: fileSize(r.bytes, locale.code) }),
          "success"
        );
      }
    } catch (err) {
      setFailed({ id: e.id, text: String(err instanceof Error ? err.message : err) });
    }
    setBusy(null);
    await load();
  };

  /** 离线导入：选中的容器逐个放进导入区，凑齐了哪个引擎就装哪个 */
  const importFiles = async (files: FileList | null) => {
    if (!files?.length || !info) return;
    setFailed(null);
    const touched = new Set<string>();
    for (const f of Array.from(files)) {
      try {
        (await api.stageEngineFile(f)).engines.forEach((id) => touched.add(id));
      } catch (err) {
        toastError(err);
      }
    }
    const next = await api.engines().catch(() => null);
    if (!next) return;
    setInfo(next);
    for (const e of next.engines.filter((x) => touched.has(x.id) && !x.installed)) {
      const lack = e.containers.filter((c) => !next.staged.includes(c));
      if (lack.length) toast(t("settings.engines.importLacks", { name: e.name, files: lack.join(", ") }));
      else await run(e, "import");
    }
  };

  if (!info) return <div className="settings-loading"><Spinner /></div>;
  const optional = info.engines.filter((e) => !e.builtin);
  return (
    <>
      <p className="set-intro">{info.managed ? t("settings.engines.intro", { year: info.year ?? "" }) : t("settings.engines.system")}</p>
      {info.year != null && (
        <p className="set-intro engine-release">
          <span className="tag muted">{t("settings.engines.release", { year: info.year })}</span>
          {frozen(info) && <span className="tag warn">{t("settings.engines.frozenTag")}</span>}
          {frozen(info) && (
            <span className="dim">{t(info.managed ? "settings.engines.frozenManaged" : "settings.engines.frozenSystem")}</span>
          )}
        </p>
      )}
      <div className="set-group">
        {info.engines.map((e) => (
          <Row
            key={e.id}
            label={
              <>
                {e.name}
                <span className="engine-programs dim">{e.programs.map(programName).join(" · ")}</span>
              </>
            }
            hint={
              <>
                {t(`settings.engines.desc.${e.id}` as Key)}
                {failed?.id === e.id && <span className="engine-error">{failed.text}</span>}
              </>
            }
          >
            <span className="engine-actions">
              {e.builtin && info.managed ? (
                <span className="tag muted">{t("settings.engines.builtin")}</span>
              ) : e.installed ? (
                <span className="tag">{t("settings.engines.installedTag")}</span>
              ) : (
                <span className="tag muted">{t("settings.engines.notInstalled")}</span>
              )}
              {info.managed && !e.builtin && <span className="dim">{fileSize(e.bytes, locale.code)}</span>}
              {info.managed && !e.builtin && busy === e.id && <Spinner size={14} />}
              {info.managed && !e.builtin && busy !== e.id && (
                <button
                  className={`btn btn-sm ${e.installed ? "btn-ghost" : "btn-primary"}`}
                  disabled={!!busy}
                  onClick={() => void run(e, e.installed ? "uninstall" : "install")}
                >
                  <Icon name={e.installed ? "trash" : "download"} size={13} />
                  {e.installed ? t("settings.engines.uninstall") : t("settings.engines.install")}
                </button>
              )}
            </span>
          </Row>
        ))}
        {info.managed && (
          <Row
            label={t("settings.engines.import")}
            hint={
              <>
                {t("settings.engines.importHint")}
                {optional.map((e) => (
                  <span key={e.id} className="engine-files">
                    <b>{e.name}</b>
                    {e.containers.map((c) => (
                      <code key={c} className={info.staged.includes(c) ? "on" : undefined}>
                        {c}
                      </code>
                    ))}
                  </span>
                ))}
              </>
            }
            stack
          >
            <input
              ref={fileRef}
              type="file"
              accept=".xz"
              multiple
              hidden
              onChange={(ev) => {
                void importFiles(ev.target.files);
                ev.target.value = "";
              }}
            />
            <button className="btn btn-sm" disabled={!!busy} onClick={() => fileRef.current?.click()}>
              <Icon name="upload" size={13} /> {t("settings.engines.importPick")}
            </button>
          </Row>
        )}
      </div>
    </>
  );
}

/* ---------- 下载源 ---------- */

function Mirror({ info, save }: { info: SettingsInfo; save: Save }) {
  const locale = useLocale();
  const { settings, presets, env } = info;
  const [probe, setProbe] = useState<Record<string, number | null> | null>(null);
  const [probing, setProbing] = useState(false);
  const [custom, setCustom] = useState(settings.customMirror);
  const customRef = useRef<HTMLInputElement>(null);
  const own = env.tex?.tlmgrRepository ?? null;

  const name = (id: string) =>
    id === "tlmgr" ? t("settings.mirror.follow") : id === "custom" ? t("settings.mirror.custom") : t(`settings.mirror.presets.${id}` as Key);

  const pick = async (patch: Partial<Settings>, id: string) => {
    const s = await save(patch);
    if (s) toast(t("settings.mirror.changed", { name: name(id) }), "success");
  };

  const runProbe = async () => {
    setProbing(true);
    try {
      const r = await api.probeMirrors(settings.mirror === "custom" ? custom : undefined);
      setProbe(r);
      if (!Object.values(r).some((v) => v != null)) toast(t("settings.mirror.noneReachable"), "error");
    } catch (e) {
      toastError(e);
    }
    setProbing(false);
  };

  // 只在预置源里挑：自定义源和「跟随 TeX Live」都是用户刻意选的
  const fastest = useMemo(() => {
    if (!probe) return null;
    const ok = presets.flatMap((p) => (probe[p.id] != null ? [{ id: p.id, ms: probe[p.id]! }] : []));
    return ok.sort((a, b) => a.ms - b.ms)[0] ?? null;
  }, [probe, presets]);

  // 中文界面把国内镜像放前面
  const regions: ("cn" | "global")[] = locale.code.startsWith("zh") ? ["cn", "global"] : ["global", "cn"];

  const latency = (id: string) => {
    if (!probe || !(id in probe)) return null;
    const ms = probe[id];
    if (ms == null) return <span className="latency bad">{t("settings.mirror.unreachable")}</span>;
    return <span className={`latency${ms < 400 ? " good" : ms < 1500 ? "" : " slow"}`}>{t("settings.mirror.ms", { ms })}</span>;
  };

  // 预置源一张卡；「跟随 TeX Live」和自定义源各自独占一行（URL 长、要放输入框）
  const option = (id: string, url: string | null, onPick: () => void, extra?: ReactNode, span = false) => (
    <label key={id} className={`card mirror-card${settings.mirror === id ? " on" : ""}${span ? " span" : ""}`}>
      <span className="card-head">
        <input type="radio" name="mirror" checked={settings.mirror === id} onChange={onPick} />
        <span className="card-name mirror-name">{name(id)}</span>
        {latency(id)}
      </span>
      {url && (
        <code className="mirror-url" title={url}>
          {url}
        </code>
      )}
      {extra}
    </label>
  );

  return (
    <>
      <p className="set-intro">{t("settings.mirror.intro")}</p>
      <p className="set-intro dim">{t("settings.mirror.archiveHint")}</p>
      {env.tex?.archive && (
        <p className="set-intro">
          <span className="tag">{t("settings.mirror.archiveActive", { url: env.tex.archive })}</span>
        </p>
      )}
      <div className="set-toolbar">
        <button className="btn btn-sm" onClick={() => void runProbe()} disabled={probing}>
          {probing ? <Spinner size={12} /> : <Icon name="speed" size={14} />}
          {probing ? t("settings.mirror.probing") : t("settings.mirror.probe")}
        </button>
        {fastest && settings.mirror !== fastest.id && (
          <button className="btn btn-primary btn-sm" onClick={() => void pick({ mirror: fastest.id }, fastest.id)}>
            {t("settings.mirror.pickFastest")} · {name(fastest.id)}
          </button>
        )}
      </div>

      <div className="card-grid mirror-grid" role="radiogroup" aria-label={t("settings.nav.mirror")}>
        {option("tlmgr", own, () => void pick({ mirror: "tlmgr" }, "tlmgr"), !own && <span className="dim">{t("settings.mirror.followHint")}</span>, true)}
      </div>
      {regions.map((r) => (
        <div key={r}>
          <h3 className="set-sub">{r === "cn" ? t("settings.mirror.cn") : t("settings.mirror.global")}</h3>
          <div className="card-grid mirror-grid" role="radiogroup" aria-label={r === "cn" ? t("settings.mirror.cn") : t("settings.mirror.global")}>
            {presets.filter((p) => p.region === r).map((p) => option(p.id, p.url, () => void pick({ mirror: p.id }, p.id)))}
          </div>
        </div>
      ))}
      <h3 className="set-sub">{t("settings.mirror.other")}</h3>
      <div className="card-grid mirror-grid">
        {option(
          "custom",
          null,
          () => {
            // 已经填过地址就直接切过去；没填先让用户填
            if (settings.customMirror) void pick({ mirror: "custom" }, "custom");
            else customRef.current?.focus();
          },
          <>
            <span className="dim mirror-hint">{t("settings.mirror.customHint")}</span>
            <form
              className="mirror-custom"
              onSubmit={(e) => {
                e.preventDefault();
                if (custom.trim()) void pick({ mirror: "custom", customMirror: custom.trim() }, "custom");
              }}
            >
              <input
                ref={customRef}
                className="input"
                value={custom}
                onChange={(e) => setCustom(e.target.value)}
                placeholder={t("settings.mirror.customPlaceholder")}
                aria-label={t("settings.mirror.customLabel")}
              />
              <button
                className={`btn btn-sm${custom.trim() && custom.trim() !== settings.customMirror ? " btn-primary" : ""}`}
                type="submit"
                disabled={!custom.trim() || (custom.trim() === settings.customMirror && settings.mirror === "custom")}
              >
                {t("settings.mirror.save")}
              </button>
            </form>
          </>,
          true,
        )}
      </div>
    </>
  );
}

/* ---------- AI 辅助 ---------- */

/** 常见服务的 Base URL，选了就填好（都说 OpenAI 兼容协议）；显示名在语言文件 settings.ai.presets.<id> */
const AI_PRESETS = [
  { id: "openai", url: "https://api.openai.com/v1" },
  { id: "deepseek", url: "https://api.deepseek.com/v1" },
  { id: "qwen", url: "https://dashscope.aliyuncs.com/compatible-mode/v1" },
  { id: "moonshot", url: "https://api.moonshot.cn/v1" },
  { id: "zhipu", url: "https://open.bigmodel.cn/api/paas/v4" },
  { id: "openrouter", url: "https://openrouter.ai/api/v1" },
  { id: "anthropic", url: "https://api.anthropic.com/v1" },
  { id: "ollama", url: "http://127.0.0.1:11434/v1" },
  { id: "lmstudio", url: "http://127.0.0.1:1234/v1" },
] as const;

/** 服务、模型的标识由界面生成，服务端只校验格式 */
const aiId = (prefix: string) => `${prefix}-${Math.random().toString(36).slice(2, 10)}`;

const toSave = (c: AiConfig) => ({
  services: c.services.map(({ id, name, baseUrl }) => ({ id, name, baseUrl })),
  models: c.models,
  verify: c.verify,
});

type AiCommit = (next: AiConfig) => Promise<boolean>;

function Ai() {
  const [info, setInfo] = useState<AiInfo | null>(null);
  const [adding, setAdding] = useState(false);
  /** 各服务「测试连接」列出的模型 ID，给添加模型时补全 */
  const [fetched, setFetched] = useState<Record<string, string[]>>({});
  const onFetched = useCallback((svc: string, ids: string[]) => setFetched((cur) => ({ ...cur, [svc]: ids })), []);
  useEffect(() => void api.ai().then(setInfo, toastError), []);

  const commit: AiCommit = useCallback(async (next) => {
    try {
      setInfo(await api.saveAi(toSave(next)));
      return true;
    } catch (e) {
      toastError(e);
      return false;
    }
  }, []);

  if (!info) {
    return (
      <div className="settings-loading">
        <Spinner />
      </div>
    );
  }
  const { config } = info;
  return (
    <>
      <p className="set-intro">{t("settings.ai.intro")}</p>
      <p className="set-intro dim">{info.keychain ? t("settings.ai.keychainHint") : t("settings.ai.fileHint")}</p>

      {config.services.map((svc) => (
        <AiServiceCard key={svc.id} svc={svc} config={config} commit={commit} onInfo={setInfo} onFetched={onFetched} />
      ))}

      {adding ? (
        <AiAddService
          config={config}
          commit={commit}
          onInfo={setInfo}
          onDone={() => setAdding(false)}
        />
      ) : (
        <div className="set-toolbar">
          <button className={`btn btn-sm${config.services.length ? "" : " btn-primary"}`} onClick={() => setAdding(true)}>
            <Icon name="plus" size={14} /> {t("settings.ai.addService")}
          </button>
          {!config.services.length && <span className="dim">{t("settings.ai.empty")}</span>}
        </div>
      )}

      {config.services.length > 0 && (
        <>
          <h3 className="set-sub">{t("settings.ai.models")}</h3>
          <p className="set-intro dim">{t("settings.ai.modelsHint")}</p>
          <AiModelList config={config} commit={commit} onInfo={setInfo} fetched={fetched} />
        </>
      )}

      <h3 className="set-sub">{t("settings.ai.behavior")}</h3>
      <div className="set-group">
        <Row label={t("settings.ai.verify")} hint={t("settings.ai.verifyHint")}>
          <Switch on={config.verify} label={t("settings.ai.verify")} onChange={(v) => void commit({ ...config, verify: v })} />
        </Row>
      </div>
    </>
  );
}

function AiAddService({
  config,
  commit,
  onInfo,
  onDone,
}: {
  config: AiConfig;
  commit: AiCommit;
  onInfo: (i: AiInfo) => void;
  onDone: () => void;
}) {
  const [preset, setPreset] = useState("");
  const [name, setName] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const nameRef = useRef<HTMLInputElement>(null);

  const pickPreset = (id: string) => {
    setPreset(id);
    const p = AI_PRESETS.find((x) => x.id === id);
    if (p) {
      setName(t(`settings.ai.presets.${p.id}` as Key));
      setBaseUrl(p.url);
    }
    nameRef.current?.focus();
  };

  const submit = async () => {
    setBusy(true);
    const id = aiId("s");
    const ok = await commit({ ...config, services: [...config.services, { id, name, baseUrl, key: "none" }] });
    if (ok && key.trim()) {
      try {
        onInfo(await api.setAiKey(id, key.trim()));
      } catch (e) {
        toastError(e);
      }
    }
    setBusy(false);
    if (ok) {
      toast(t("settings.ai.added", { name: name.trim() }), "success");
      onDone();
    }
  };

  return (
    <form
      className="set-group ai-service"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <Row label={t("settings.ai.preset")} hint={t("settings.ai.presetHint")}>
        <Dropdown
          value={preset}
          options={[{ value: "", label: t("settings.ai.presetCustom") }, ...AI_PRESETS.map((p) => ({ value: p.id, label: t(`settings.ai.presets.${p.id}` as Key) }))]}
          onChange={pickPreset}
          label={t("settings.ai.preset")}
        />
      </Row>
      <Row label={t("settings.ai.name")}>
        <input ref={nameRef} className="input ai-field" value={name} onChange={(e) => setName(e.target.value)} aria-label={t("settings.ai.name")} autoFocus />
      </Row>
      <Row label={t("settings.ai.baseUrl")} hint={t("settings.ai.baseUrlHint")}>
        <input
          className="input ai-field"
          value={baseUrl}
          onChange={(e) => setBaseUrl(e.target.value)}
          placeholder={t("settings.ai.baseUrlPlaceholder")}
          aria-label={t("settings.ai.baseUrl")}
          spellCheck={false}
        />
      </Row>
      <Row label={t("settings.ai.key")} hint={t("settings.ai.keyNewHint")}>
        <input
          className="input ai-field"
          type="password"
          value={key}
          onChange={(e) => setKey(e.target.value)}
          placeholder={t("settings.ai.keyPlaceholder")}
          aria-label={t("settings.ai.key")}
          autoComplete="off"
        />
      </Row>
      <div className="ai-actions">
        <button type="button" className="btn btn-ghost btn-sm" onClick={onDone}>
          {t("common.cancel")}
        </button>
        <button type="submit" className="btn btn-primary btn-sm" disabled={busy || !name.trim() || !baseUrl.trim()}>
          {busy && <Spinner size={12} />}
          {t("settings.ai.add")}
        </button>
      </div>
    </form>
  );
}

function AiServiceCard({
  svc,
  config,
  commit,
  onInfo,
  onFetched,
}: {
  svc: AiService;
  config: AiConfig;
  commit: AiCommit;
  onInfo: (i: AiInfo) => void;
  onFetched: (svc: string, ids: string[]) => void;
}) {
  const [name, setName] = useState(svc.name);
  const [baseUrl, setBaseUrl] = useState(svc.baseUrl);
  useEffect(() => setName(svc.name), [svc.name]);
  useEffect(() => setBaseUrl(svc.baseUrl), [svc.baseUrl]);
  const [key, setKey] = useState("");
  const [test, setTest] = useState<{ busy: boolean; result?: AiTest; error?: string } | null>(null);
  const [armed, setArmed] = useState(false);
  const timer = useRef<number>();
  useEffect(() => () => window.clearTimeout(timer.current), []);

  const patch = (p: Partial<AiService>) => commit({ ...config, services: config.services.map((s) => (s.id === svc.id ? { ...s, ...p } : s)) });

  const commitField = async (field: "name" | "baseUrl", value: string, reset: () => void) => {
    if (value.trim() === svc[field]) return;
    if (!(await patch({ [field]: value }))) reset();
  };

  const saveKey = async (value: string) => {
    try {
      onInfo(await api.setAiKey(svc.id, value));
      setKey("");
      setTest(null);
      toast(value ? t("settings.ai.keySaved") : t("settings.ai.keyCleared"), "success");
    } catch (e) {
      toastError(e);
    }
  };

  const runTest = async () => {
    setTest({ busy: true });
    try {
      const result = await api.testAi(svc.id);
      setTest({ busy: false, result });
      onFetched(svc.id, result.models);
    } catch (e) {
      setTest({ busy: false, error: e instanceof Error ? e.message : String(e) });
    }
  };

  const remove = async () => {
    if (!armed) {
      setArmed(true);
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setArmed(false), 3000);
      return;
    }
    setArmed(false);
    const ok = await commit({
      ...config,
      services: config.services.filter((s) => s.id !== svc.id),
      models: config.models.filter((m) => m.service !== svc.id),
    });
    if (ok) toast(t("settings.ai.removed", { name: svc.name }), "success");
  };

  return (
    <div className="set-group ai-service">
      <div className="ai-service-head">
        <input
          className="input ai-service-name"
          value={name}
          onChange={(e) => setName(e.target.value)}
          onBlur={() => void commitField("name", name, () => setName(svc.name))}
          onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
          aria-label={t("settings.ai.name")}
        />
        <button className="btn btn-sm" onClick={() => void runTest()} disabled={test?.busy}>
          {test?.busy ? <Spinner size={12} /> : <Icon name="speed" size={14} />}
          {t("settings.ai.test")}
        </button>
        <button
          className={`btn btn-ghost btn-sm${armed ? " btn-danger-solid" : " btn-icon btn-danger"}`}
          title={t("settings.ai.remove")}
          aria-label={t("settings.ai.remove")}
          onClick={() => void remove()}
        >
          <Icon name="trash" size={15} />
          {armed && t("settings.ai.removeConfirm")}
        </button>
      </div>
      {test && !test.busy && (
        <div className={`ai-test${test.error ? " bad" : " good"}`} role="status">
          <Icon name={test.error ? "error" : "check"} size={14} />
          <span>
            {test.error ??
              (test.result!.via === "models"
                ? t("settings.ai.testOkModels", { ms: test.result!.ms, count: test.result!.models.length })
                : t("settings.ai.testOkChat", { ms: test.result!.ms }))}
          </span>
        </div>
      )}
      <Row label={t("settings.ai.baseUrl")}>
        <input
          className="input ai-field"
          value={baseUrl}
          onChange={(e) => setBaseUrl(e.target.value)}
          onBlur={() => void commitField("baseUrl", baseUrl, () => setBaseUrl(svc.baseUrl))}
          onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
          aria-label={t("settings.ai.baseUrl")}
          spellCheck={false}
        />
      </Row>
      <Row
        label={t("settings.ai.key")}
        hint={svc.key === "keychain" ? t("settings.ai.keyInKeychain") : svc.key === "file" ? t("settings.ai.keyInFile") : t("settings.ai.keyNone")}
      >
        <form
          className="ai-key"
          onSubmit={(e) => {
            e.preventDefault();
            if (key.trim()) void saveKey(key.trim());
          }}
        >
          <input
            className="input ai-field"
            type="password"
            value={key}
            onChange={(e) => setKey(e.target.value)}
            placeholder={svc.key === "none" ? t("settings.ai.keyPlaceholder") : t("settings.ai.keyReplace")}
            aria-label={t("settings.ai.key")}
            autoComplete="off"
          />
          <button className="btn btn-sm" type="submit" disabled={!key.trim()}>
            {t("settings.ai.keySave")}
          </button>
          {svc.key !== "none" && (
            <button className="btn btn-ghost btn-sm" type="button" onClick={() => void saveKey("")}>
              {t("settings.ai.keyClear")}
            </button>
          )}
        </form>
      </Row>
    </div>
  );
}

/** 换位置：把 from 处的挪到 to 处（to 是挪走之前的下标，可以等于长度，表示放到最后） */
function moved<T>(list: T[], from: number, to: number) {
  const next = list.slice();
  const [x] = next.splice(from, 1);
  next.splice(to > from ? to - 1 : to, 0, x);
  return next;
}

/** 全部模型一条列表：拖动（或在手柄上按上下方向键）排先后，第一个是首选，用不了就依次往后换 */
function AiModelList({ config, commit, onInfo, fetched }: { config: AiConfig; commit: AiCommit; onInfo: (i: AiInfo) => void; fetched: Record<string, string[]> }) {
  /** 刚添加的模型：它那一行挂上来后自动测一次识图 */
  const [fresh, setFresh] = useState<string | null>(null);
  const [drag, setDrag] = useState<{ id: string; over: number | null } | null>(null);
  const rows = useRef<Map<string, HTMLDivElement>>(new Map());
  const { models } = config;

  const move = (from: number, to: number) => {
    if (from === to || from + 1 === to) return;
    void commit({ ...config, models: moved(models, from, to) });
  };

  /** 鼠标在某行的上半截就放到它前面，下半截放到后面 */
  const overAt = (e: DragEvent<HTMLDivElement>, i: number) => {
    const r = e.currentTarget.getBoundingClientRect();
    return e.clientY < r.top + r.height / 2 ? i : i + 1;
  };

  return (
    <div className="set-group ai-model-list">
      {Object.entries(fetched).map(([svc, ids]) => (
        <datalist key={svc} id={`ai-models-${svc}`}>
          {ids.map((m) => (
            <option key={m} value={m} />
          ))}
        </datalist>
      ))}
      {models.length > 0 ? (
        <div className="ai-models" role="list" aria-label={t("settings.ai.models")}>
          <div className="ai-model ai-model-head dim" aria-hidden>
            <span />
            <span>{t("settings.ai.modelName")}</span>
            <span>{t("settings.ai.modelId")}</span>
            <span>{t("settings.ai.service")}</span>
            <span>{t("settings.ai.vision")}</span>
            <span />
          </div>
          {models.map((m, i) => (
            <AiModelRow
              key={m.id}
              m={m}
              index={i}
              config={config}
              commit={commit}
              onInfo={onInfo}
              autoTest={m.id === fresh}
              rowRef={(el) => (el ? rows.current.set(m.id, el) : rows.current.delete(m.id))}
              dragging={drag?.id === m.id}
              dropAt={drag && drag.over != null ? (drag.over === i ? "before" : drag.over === i + 1 && i === models.length - 1 ? "after" : null) : null}
              onGrab={(e) => {
                e.dataTransfer.effectAllowed = "move";
                // 拖着走的是整行，不只是手柄
                const row = rows.current.get(m.id);
                if (row) e.dataTransfer.setDragImage(row, 16, row.offsetHeight / 2);
                setDrag({ id: m.id, over: null });
              }}
              onOver={(e) => {
                if (!drag) return;
                e.preventDefault();
                const over = overAt(e, i);
                if (over !== drag.over) setDrag({ ...drag, over });
              }}
              onDrop={(e) => {
                if (!drag) return;
                e.preventDefault();
                move(models.findIndex((x) => x.id === drag.id), overAt(e, i));
                setDrag(null);
              }}
              onRelease={() => setDrag(null)}
              onStep={(d) => move(i, d < 0 ? i - 1 : i + 2)}
            />
          ))}
        </div>
      ) : (
        <p className="dim ai-models-empty">{t("settings.ai.modelsEmpty")}</p>
      )}
      <AiAddModel config={config} commit={commit} onAdded={setFresh} />
    </div>
  );
}

function AiModelRow({
  m,
  index,
  config,
  commit,
  onInfo,
  autoTest,
  rowRef,
  dragging,
  dropAt,
  onGrab,
  onOver,
  onDrop,
  onRelease,
  onStep,
}: {
  m: AiModel;
  index: number;
  config: AiConfig;
  commit: AiCommit;
  onInfo: (i: AiInfo) => void;
  /** 刚添加：挂上来就测一次识图，没通过不出声（多数文本模型本来就不能看图） */
  autoTest: boolean;
  rowRef: (el: HTMLDivElement | null) => void;
  dragging: boolean;
  /** 拖动中，放下的位置在这一行前面或后面时画一条线 */
  dropAt: "before" | "after" | null;
  onGrab: (e: DragEvent<HTMLButtonElement>) => void;
  onOver: (e: DragEvent<HTMLDivElement>) => void;
  onDrop: (e: DragEvent<HTMLDivElement>) => void;
  onRelease: () => void;
  /** 键盘：-1 上移一位，1 下移一位 */
  onStep: (d: -1 | 1) => void;
}) {
  const [name, setName] = useState(m.name);
  const [model, setModel] = useState(m.model);
  useEffect(() => setName(m.name), [m.name]);
  useEffect(() => setModel(m.model), [m.model]);
  const [probe, setProbe] = useState<{ busy: boolean; ms?: number; error?: string } | null>(null);
  const patch = (p: Partial<AiModel>) => commit({ ...config, models: config.models.map((x) => (x.id === m.id ? { ...x, ...p } : x)) });
  const blur = (e: ReactKeyboardEvent<HTMLInputElement>) => e.key === "Enter" && e.currentTarget.blur();
  const svc = config.services.find((s) => s.id === m.service);
  const testVision = async (quiet: boolean) => {
    setProbe({ busy: true });
    try {
      const r = await api.testVision(m.id);
      onInfo(r.info);
      setProbe({ busy: false, ms: r.ms });
    } catch (e) {
      setProbe(quiet ? null : { busy: false, error: e instanceof Error ? e.message : String(e) });
    }
  };
  // 通过的提示看一眼就够，几秒后收起；没通过的留着，等用户看完原因
  useEffect(() => {
    if (!probe || probe.busy || probe.error) return;
    const id = window.setTimeout(() => setProbe(null), 4000);
    return () => window.clearTimeout(id);
  }, [probe]);
  useEffect(() => {
    if (autoTest) void testVision(true);
    // 只在挂上来时测一次
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return (
    <div
      ref={rowRef}
      className={`ai-model${dragging ? " dragging" : ""}${dropAt ? ` drop-${dropAt}` : ""}`}
      role="listitem"
      onDragOver={onOver}
      onDrop={onDrop}
    >
      <button
        type="button"
        className="ai-grip"
        draggable
        onDragStart={onGrab}
        onDragEnd={onRelease}
        onKeyDown={(e) => {
          if (e.key === "ArrowUp" || e.key === "ArrowDown") {
            e.preventDefault();
            onStep(e.key === "ArrowUp" ? -1 : 1);
          }
        }}
        title={t("settings.ai.reorder")}
        aria-label={t("settings.ai.reorderItem", { n: index + 1, name: m.name })}
      >
        <Icon name="grip" size={14} />
        <span className="ai-rank">{index + 1}</span>
      </button>
      <input
        className="input input-sm"
        value={name}
        onChange={(e) => setName(e.target.value)}
        onBlur={() => name.trim() !== m.name && void patch({ name }).then((ok) => ok || setName(m.name))}
        onKeyDown={blur}
        placeholder={m.model}
        aria-label={t("settings.ai.modelName")}
      />
      <input
        className="input input-sm mono"
        value={model}
        list={`ai-models-${m.service}`}
        onChange={(e) => setModel(e.target.value)}
        // 换了模型，原来的「能看图」不作数（服务端会清掉），重新检测一次
        onBlur={() => model.trim() !== m.model && void patch({ model }).then((ok) => (ok ? void testVision(true) : setModel(m.model)))}
        onKeyDown={blur}
        aria-label={t("settings.ai.modelId")}
        spellCheck={false}
      />
      <span className="ai-model-svc dim" title={svc?.name}>
        {svc?.name}
      </span>
      {/* 勾上要先过检测：认出图里的数字才算能看图（服务端也只认检测结果）；取消直接生效 */}
      <label className="ai-vision" title={t("settings.ai.visionHint")}>
        {probe?.busy ? (
          <Spinner size={14} />
        ) : (
          <input
            type="checkbox"
            checked={m.vision}
            onChange={(e) => void (e.target.checked ? testVision(false) : patch({ vision: false }).then(() => setProbe(null)))}
            aria-label={t("settings.ai.vision")}
          />
        )}
        {probe?.busy && <span className="dim">{t("settings.ai.visionChecking")}</span>}
      </label>
      <button
        className="btn btn-ghost btn-sm btn-icon btn-danger"
        title={t("settings.ai.removeModel")}
        aria-label={t("settings.ai.removeModel")}
        onClick={() => void commit({ ...config, models: config.models.filter((x) => x.id !== m.id) })}
      >
        <Icon name="x" size={14} />
      </button>
      {probe && !probe.busy && (
        <div className={`ai-model-note${probe.error ? " bad" : " good"}`} role="status">
          <Icon name={probe.error ? "error" : "check"} size={13} />
          <span>{probe.error ?? t("settings.ai.visionOk", { ms: probe.ms! })}</span>
        </div>
      )}
    </div>
  );
}

function AiAddModel({ config, commit, onAdded }: { config: AiConfig; commit: AiCommit; onAdded: (id: string) => void }) {
  const [name, setName] = useState("");
  const [model, setModel] = useState("");
  // 默认挂到最后添加的那个服务下：通常刚加完服务就要给它添模型
  const [svc, setSvc] = useState(config.services[config.services.length - 1]?.id ?? "");
  useEffect(() => {
    if (!config.services.some((s) => s.id === svc)) setSvc(config.services[config.services.length - 1]?.id ?? "");
  }, [config.services, svc]);
  const submit = async () => {
    const id = aiId("m");
    const ok = await commit({ ...config, models: [...config.models, { id, service: svc, name, model, vision: false }] });
    if (ok) {
      setName("");
      setModel("");
      onAdded(id);
    }
  };
  return (
    <form
      className="ai-model ai-model-add"
      onSubmit={(e) => {
        e.preventDefault();
        if (model.trim() && svc) void submit();
      }}
    >
      <Icon name="plus" size={14} className="dim" />
      <input
        className="input input-sm"
        value={name}
        onChange={(e) => setName(e.target.value)}
        placeholder={t("settings.ai.modelNamePlaceholder")}
        aria-label={t("settings.ai.modelName")}
      />
      <input
        className="input input-sm mono"
        value={model}
        list={`ai-models-${svc}`}
        onChange={(e) => setModel(e.target.value)}
        placeholder={t("settings.ai.modelIdPlaceholder")}
        aria-label={t("settings.ai.modelId")}
        spellCheck={false}
      />
      <Dropdown value={svc} options={config.services.map((s) => ({ value: s.id, label: s.name }))} onChange={setSvc} label={t("settings.ai.service")} />
      <button className="btn btn-sm" type="submit" disabled={!model.trim() || !svc}>
        {t("settings.ai.addModel")}
      </button>
    </form>
  );
}

/* ---------- 列表页共用：搜索 + 来源筛选 ---------- */

function useFiltered<T extends { source: string }>(items: T[] | null, query: string, filter: string, text: (x: T) => string) {
  return useMemo(() => {
    if (!items) return null;
    const q = query.trim().toLowerCase();
    return items.filter((x) => (filter === "all" || x.source === filter) && (!q || text(x).toLowerCase().includes(q)));
  }, [items, query, filter, text]);
}

function ListHead({
  query,
  setQuery,
  placeholder,
  filters,
  filter,
  setFilter,
  children,
}: {
  query: string;
  setQuery: (q: string) => void;
  placeholder: string;
  filters: { id: string; label: string; count: number }[];
  filter: string;
  setFilter: (f: string) => void;
  children?: ReactNode;
}) {
  return (
    <div className="set-toolbar">
      <label className="search">
        <Icon name="search" size={15} />
        <input
          className="input"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => e.key === "Escape" && setQuery("")}
          placeholder={placeholder}
          aria-label={placeholder}
        />
      </label>
      <span className="seg" role="radiogroup" aria-label={t("common.filter")} onKeyDown={rovingKeys}>
        {filters
          .filter((f) => f.id === "all" || f.count > 0)
          .map((f) => (
            <button
              key={f.id}
              className={filter === f.id ? "on" : ""}
              role="radio"
              aria-checked={filter === f.id}
              tabIndex={filter === f.id ? 0 : -1}
              onClick={() => setFilter(f.id)}
            >
              {f.label} <span className="seg-count">{f.count}</span>
            </button>
          ))}
      </span>
      {children}
    </div>
  );
}

function count<T extends { source: string }>(items: T[] | null, source: string) {
  return items?.filter((x) => x.source === source).length ?? 0;
}

/** tlmgr 输出的最后一行有字的，失败时塞进提示 */
const lastLine = (log: string) => log.split("\n").map((l) => l.trim()).filter(Boolean).pop() ?? "";

/**
 * 重装卡片上的总进度（0~1）：下载占前 85%，逐个解包占到 97%，刷新索引、登记字体停在 97% 等它收尾。
 * 不像缺字清单那样下载完就算满格：重装下完还要解包、刷新好几秒，满格停着像卡住了。量不出为 null
 */
function reinstallShare(p: InstallProgress): number | null {
  if (p.phase === "finish") return 0.97;
  if (p.phase === "install") {
    const inst = p.spent != null && p.estimate ? p.spent / p.estimate : p.total ? (p.index - 1) / p.total : 0;
    return 0.85 + Math.min(inst, 1) * 0.12;
  }
  return p.got != null && p.want ? Math.min(p.got / p.want, 1) * 0.85 : null;
}

/**
 * 补装宏包的卸载、重装。卸载点两下：第一下变成「确认卸载」，3 秒内再点才卸。
 * 同一时间只做一件（服务端也只许一次安装），busy 是正在处理的那一项。
 * 重装要从下载源重新下载，大的字体包要几分钟：装的时候轮询进度显示在卡片上，下载阶段可以取消。
 */
function useUserPackages(reload: () => void) {
  const [busy, setBusy] = useState<string | null>(null);
  const [op, setOp] = useState<"remove" | "reinstall">("remove");
  const [armed, setArmed] = useState<string | null>(null);
  const [progress, setProgress] = useState<InstallProgress | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const timer = useRef<number>();
  useEffect(() => () => window.clearTimeout(timer.current), []);

  const reinstalling = busy !== null && op === "reinstall";
  useEffect(() => {
    if (!reinstalling) return;
    let on = true;
    const tick = () =>
      api
        .installProgress()
        .then((r) => on && setProgress(r.progress))
        .catch(() => {});
    void tick();
    const poll = setInterval(tick, 800);
    return () => {
      on = false;
      clearInterval(poll);
      setProgress(null);
      setCancelling(false);
    };
  }, [reinstalling]);

  const cancel = async () => {
    setCancelling(true);
    try {
      await api.cancelInstall();
    } catch (e) {
      setCancelling(false);
      toastError(e);
    }
  };
  /** 还在连软件源、下载：这时停下不会留下装了一半的包 */
  const canCancel = reinstalling && !cancelling && !!progress?.cancellable && (progress.phase === "connect" || progress.phase === "download");

  const remove = async (key: string, packages: string[], name: string) => {
    if (armed !== key) {
      setArmed(key);
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setArmed(null), 3000);
      return;
    }
    setArmed(null);
    setOp("remove");
    setBusy(key);
    try {
      await api.removePackages(packages);
      toast(t("settings.packages.removed", { name }), "success");
      reload();
    } catch (e) {
      toastError(e);
    }
    setBusy(null);
  };

  const reinstall = async (key: string, packages: string[], name: string) => {
    setArmed(null);
    setOp("reinstall");
    setBusy(key);
    try {
      const r = await api.reinstallPackages(packages);
      if (r.cancelled) {
        toast(t("settings.packages.reinstallCancelled", { name }));
      } else if (r.ok) {
        toast(t("settings.packages.reinstalled", { name }), "success");
        reload();
      } else {
        toast(t("settings.packages.reinstallFailed", { name, detail: lastLine(r.log) }), "error");
      }
    } catch (e) {
      toastError(e);
    }
    setBusy(null);
  };

  /** 卡片自己的样式：正在重装、卸载时整张卡换成处理中的样子；重装量得出进度时底边那条线按实际进度走 */
  const share = reinstalling && progress ? reinstallShare(progress) : null;
  const cardClass = (key: string) => (busy === key ? `card working ${op}${share != null ? " measured" : ""}` : "card");
  const cardStyle = (key: string) =>
    busy === key && share != null ? ({ "--share": Math.max(share, 0.03) } as CSSProperties) : undefined;

  return { busy, op, armed, progress, share, cancelling, canCancel, cancel, remove, reinstall, cardClass, cardStyle };
}

/** 重装时状态里的小进度环：量得出进度就按进度画满，量不出退回转圈 */
function ProgressRing({ share }: { share: number | null }) {
  if (share == null) return <Spinner size={12} />;
  const r = 5;
  const c = 2 * Math.PI * r;
  return (
    <svg className="progress-ring" width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
      <circle cx="6" cy="6" r={r} />
      <circle cx="6" cy="6" r={r} strokeDasharray={c} strokeDashoffset={c * (1 - Math.min(share, 1))} />
    </svg>
  );
}

/** 卡片底部的体积；量不到就不显示 */
function CardSize({ bytes }: { bytes: number | null | undefined }) {
  const locale = useLocale();
  return bytes ? <span className="card-size">{fileSize(bytes, locale.code)}</span> : null;
}

/** 重装中的卡片：底部那行换成「在干什么 · 下了多少」和「约剩多久」 */
function ReinstallMeter({ p }: { p: InstallProgress | null }) {
  const locale = useLocale();
  const size = (n: number) => fileSize(n, locale.code);
  // 连软件源时就已经知道要下多少；第一个字节到之前一直说「准备下载 多少」
  const what = !p
    ? t("settings.packages.progress.start")
    : p.phase === "finish"
      ? t("settings.packages.progress.finish")
      : p.phase === "install"
        ? p.total > 1
          ? t("settings.packages.progress.installOf", { index: p.index, total: p.total })
          : t("settings.packages.progress.install")
        : p.got && p.want
          ? t("settings.packages.progress.download", { got: size(Math.min(p.got, p.want)), want: size(p.want) })
          : p.got
            ? t("settings.packages.progress.downloadGot", { got: size(p.got) })
            : p.want
              ? t("settings.packages.progress.prepare", { want: size(p.want) })
              : p.phase === "connect"
                ? t("settings.packages.progress.connect")
                : t("settings.packages.progress.downloadAny");
  const left = p ? installLeft(p) : null;
  return (
    <div className="card-meta card-progress" aria-live="polite">
      <span className="card-progress-what">{what}</span>
      <span className="card-size">
        {left != null && left > 0 ? t("settings.packages.progress.left", { time: clock(left) }) : p ? clock(p.elapsed) : null}
      </span>
    </div>
  );
}

function UserPackageActions({
  id,
  packages,
  name,
  ctl,
  removeTitle,
  reinstall = true,
  children,
}: {
  id: string;
  packages: string[];
  name: string;
  ctl: ReturnType<typeof useUserPackages>;
  removeTitle: string;
  reinstall?: boolean;
  /** 排在前面的其他按钮（字体页的复制） */
  children?: ReactNode;
}) {
  const locale = useLocale();
  const { busy, armed } = ctl;
  const on = busy === id || armed === id;
  return (
    <span className={`card-actions${on ? " on" : ""}`}>
      {busy === id && ctl.op === "reinstall" ? (
        <>
          <span className="card-status" role="status">
            {ctl.cancelling ? <Spinner size={12} /> : <ProgressRing share={ctl.share} />}
            {ctl.cancelling
              ? t("install.cancelling")
              : ctl.share != null
                ? new Intl.NumberFormat(locale.code, { style: "percent" }).format(ctl.share)
                : t("settings.packages.reinstalling")}
          </span>
          {ctl.canCancel && (
            <button
              className="btn btn-ghost btn-sm btn-icon"
              title={t("install.cancelTitle")}
              aria-label={t("install.cancel")}
              onClick={() => void ctl.cancel()}
            >
              <Icon name="x" size={14} />
            </button>
          )}
        </>
      ) : busy === id ? (
        <span className="card-status" role="status">
          <Spinner size={12} />
          {t("settings.packages.removing")}
        </span>
      ) : (
        <>
          {armed !== id && children}
          {reinstall && armed !== id && (
            <button
              className="btn btn-ghost btn-sm btn-icon"
              title={t("settings.packages.reinstall")}
              aria-label={t("settings.packages.reinstall")}
              disabled={busy !== null}
              onClick={() => void ctl.reinstall(id, packages, name)}
            >
              <Icon name="refresh" size={14} />
            </button>
          )}
          <button
            className={`btn btn-ghost btn-sm${armed === id ? " btn-danger-solid" : " btn-icon btn-danger"}`}
            title={removeTitle}
            aria-label={removeTitle}
            disabled={busy !== null}
            onClick={() => void ctl.remove(id, packages, name)}
          >
            <Icon name="trash" size={14} />
            {armed === id && t("settings.packages.removeConfirm")}
          </button>
        </>
      )}
    </span>
  );
}

/* ---------- 已装宏包 ---------- */

function Packages({ bundled }: { bundled: boolean }) {
  const locale = useLocale();
  const [data, setData] = useState<{ available: boolean; packages: InstalledPackage[] } | null>(null);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState("all");
  const load = useCallback(() => {
    api.packages().then(setData, (e) => {
      toastError(e);
      setData((d) => d ?? { available: false, packages: [] });
    });
  }, []);
  useEffect(load, [load]);
  const ctl = useUserPackages(load);
  // 说明换成当前语言的译文（依赖 locale：切换语言要重算），英文原文留着：搜索两种都认，悬停看原文
  const items = useMemo(() => data?.packages.map((p) => ({ ...p, text: packageDesc(p.name, p.desc) })) ?? null, [data, locale]);
  const shown = useFiltered(items, query, filter, searchText);

  return (
    <>
      <p className="set-intro">{t("settings.packages.intro")}</p>
      <QueuePanel onChanged={load} />
      {bundled && data?.available && <SnapshotTools hasUser={count(items, "user") > 0} onImported={load} />}
      {data && !data.available ? (
        <div className="set-empty">{t("settings.packages.unavailable")}</div>
      ) : (
        <>
          <ListHead
            query={query}
            setQuery={setQuery}
            placeholder={t("settings.packages.search")}
            filter={filter}
            setFilter={setFilter}
            filters={[
              { id: "all", label: t("settings.packages.all"), count: items?.length ?? 0 },
              { id: "tex", label: t("settings.packages.tex"), count: count(items, "tex") },
              { id: "user", label: t("settings.packages.user"), count: count(items, "user") },
            ]}
          />
          {shown?.length === 0 && <div className="set-empty">{t("settings.packages.noMatch", { query })}</div>}
          {!shown ? (
            <div className="card-grid pkg-grid">
              <CardSkeleton />
            </div>
          ) : (
            <VirtualGrid
              items={shown}
              itemKey={(p) => p.name}
              className="pkg-grid"
              render={(p) => (
              <div key={p.name} className={ctl.cardClass(p.name)} style={ctl.cardStyle(p.name)} aria-busy={ctl.busy === p.name} title={p.desc || undefined}>
                <div className="card-head">
                  <code className="card-name">{p.name}</code>
                  {p.source === "user" && (
                    <span className="tag" title={t("settings.packages.userTitle")}>
                      {t("settings.packages.user")}
                    </span>
                  )}
                </div>
                <div className="card-desc">{p.text}</div>
                {ctl.busy === p.name && ctl.op === "reinstall" ? (
                  <ReinstallMeter p={ctl.progress} />
                ) : (
                  (p.version || !!p.bytes) && (
                    <div className="card-meta">
                      {p.version && <span className="card-version">{p.version}</span>}
                      <CardSize bytes={p.bytes} />
                    </div>
                  )
                )}
                {p.source === "user" && (
                  <UserPackageActions id={p.name} packages={[p.name]} name={p.name} ctl={ctl} removeTitle={t("settings.packages.remove")} />
                )}
              </div>
              )}
            />
          )}
        </>
      )}
    </>
  );
}

const searchText = (p: InstalledPackage & { text: string }) => `${p.name} ${p.text} ${p.desc}`;

/* ---------- 字体 ---------- */

const FONT_SOURCE: Record<FontFamily["source"], Key> = {
  tex: "settings.fonts.tex",
  user: "settings.fonts.user",
  system: "settings.fonts.system",
};

function Fonts() {
  const [items, setItems] = useState<FontFamily[] | null>(null);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState("all");
  const [scanning, setScanning] = useState(false);

  const load = useCallback(async (rescan: boolean) => {
    setScanning(true);
    try {
      setItems(await api.fonts(rescan));
    } catch (e) {
      toastError(e);
      setItems((x) => x ?? []);
    }
    setScanning(false);
  }, []);
  useEffect(() => void load(false), [load]);
  // 卸载后服务端已重扫字体，直接取清单就是新的
  const ctl = useUserPackages(useCallback(() => void load(false), [load]));

  const shown = useFiltered(items, query, filter, (f) => f.name);

  const [previewing, setPreviewing] = useState<FontFamily | null>(null);
  const closePreview = useCallback(() => setPreviewing(null), []);

  const copy = (name: string) => {
    const cmd = `\\setmainfont{${name}}`;
    void navigator.clipboard?.writeText(cmd).then(
      () => toast(t("settings.fonts.copied", { cmd }), "success"),
      () => toast(t("common.copyFailed"), "error")
    );
  };

  return (
    <>
      <p className="set-intro">{t("settings.fonts.intro")}</p>
      <ListHead
        query={query}
        setQuery={setQuery}
        placeholder={t("settings.fonts.search")}
        filter={filter}
        setFilter={setFilter}
        filters={[
          { id: "all", label: t("settings.fonts.all"), count: items?.length ?? 0 },
          { id: "system", label: t("settings.fonts.system"), count: count(items, "system") },
          { id: "tex", label: t("settings.fonts.tex"), count: count(items, "tex") },
          { id: "user", label: t("settings.fonts.user"), count: count(items, "user") },
        ]}
      >
        <button className="btn btn-ghost btn-sm" onClick={() => void load(true)} disabled={scanning}>
          {scanning ? <Spinner size={12} /> : <Icon name="refresh" size={14} />}
          {t("settings.fonts.rescan")}
        </button>
      </ListHead>
      {shown?.length === 0 && <div className="set-empty">{t("settings.fonts.noMatch", { query })}</div>}
      {!shown ? (
        <div className="card-grid font-grid">
          <CardSkeleton />
        </div>
      ) : (
        <VirtualGrid
          items={shown}
          itemKey={(f) => `${f.source}:${f.name}`}
          className="font-grid"
          render={(f) => (
          <div
            key={`${f.source}:${f.name}`}
            className={ctl.cardClass(`${f.source}:${f.name}`)}
            style={ctl.cardStyle(`${f.source}:${f.name}`)}
            aria-busy={ctl.busy === `${f.source}:${f.name}`}
            // 整张卡可点（鼠标）；键盘和读屏走字体名那个按钮。卡片上的复制、卸载按钮各管各的，不打开预览
            onClick={(e) => !(e.target as Element).closest("button") && setPreviewing(f)}
          >
            <div className="card-head">
              <button
                type="button"
                className="card-name card-open"
                title={f.name}
                aria-label={t("settings.fonts.preview", { name: f.name })}
                onClick={() => setPreviewing(f)}
              >
                {f.name}
              </button>
              <span className={`tag${f.source === "system" ? " muted" : ""}`}>{t(FONT_SOURCE[f.source])}</span>
            </div>
            {/* 系统字体浏览器也能用，顺手预览；TeX Live 的字体没装进系统，预览不了，留空占位保持卡片等高 */}
            <div className="fc-sample" style={f.source === "system" ? { fontFamily: `"${f.name}", var(--font-ui)` } : undefined}>
              {f.source === "system" ? t("settings.fonts.sample") : "\u00a0"}
            </div>
            <div className="card-meta" title={f.styles.join(", ")}>
              <span className="card-version">{t("settings.fonts.styles", { count: f.styles.length })}</span>
              <CardSize bytes={f.bytes} />
            </div>
            {f.packages?.length ? (
              // 用户层字体：复制旁边多一个卸载，卸的是它所属的宏包；重装到宏包页做
              <UserPackageActions
                id={`${f.source}:${f.name}`}
                packages={f.packages}
                name={f.packages.join(", ")}
                ctl={ctl}
                reinstall={false}
                removeTitle={t("settings.fonts.remove", { packages: f.packages.join(", ") })}
              >
                <button className="btn btn-ghost btn-sm btn-icon" data-tip={t("settings.fonts.copyTip")} aria-label={t("settings.fonts.copy")} onClick={() => copy(f.name)}>
                  <Icon name="copy" size={14} />
                </button>
              </UserPackageActions>
            ) : (
              <button className="btn btn-ghost btn-sm btn-icon card-action" data-tip={t("settings.fonts.copyTip")} aria-label={t("settings.fonts.copy")} onClick={() => copy(f.name)}>
                <Icon name="copy" size={14} />
              </button>
            )}
          </div>
          )}
        />
      )}
      {previewing && <FontPreview font={previewing} onCopy={() => copy(previewing.name)} onClose={closePreview} />}
    </>
  );
}

function CardSkeleton() {
  return (
    <>
      {[0, 1, 2, 3, 4, 5].map((i) => (
        <div key={i} className="card" aria-hidden="true">
          <span className="skeleton" style={{ width: `${40 + (i % 3) * 15}%` }} />
          <span className="skeleton" style={{ width: "80%", height: 10, marginTop: 10 }} />
        </div>
      ))}
    </>
  );
}

/** ⌘, / Ctrl+, 打开设置，跟大多数桌面软件一样 */
export function isSettingsShortcut(e: KeyboardEvent) {
  return e.key === "," && (MAC ? e.metaKey : e.ctrlKey) && !e.shiftKey && !e.altKey;
}

/* ---------- 软件更新 ---------- */

function Updates({ settings, save }: { settings: Settings; save: Save }) {
  const locale = useLocale().code;
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [restarting, setRestarting] = useState(false);
  const [source, setSource] = useState(settings.updateSource);

  const load = useCallback(() => api.update().then(setInfo), []);
  useEffect(() => void load().catch(toastError), [load]);

  // 检查、下载中每秒看一次进度
  const phase = info?.status.phase;
  useEffect(() => {
    if (phase !== "checking" && phase !== "downloading") return;
    const id = setInterval(() => void load().catch(() => {}), 1000);
    return () => clearInterval(id);
  }, [phase, load]);

  if (!info) {
    return (
      <div className="settings-loading">
        <Spinner />
      </div>
    );
  }
  const { status, current, supported, defaultSource } = info;
  const release = status.release;

  // 失败会记进状态，直接显示在状态行里，不再另弹提示
  const check = () => {
    setInfo({ ...info, status: { ...status, phase: "checking" } });
    void api.checkUpdate().then(setInfo, () => void load().catch(toastError));
  };
  const download = () => void api.downloadUpdate().then(setInfo, toastError);
  const apply = () => {
    setRestarting(true);
    void api.applyUpdate().catch((e) => {
      setRestarting(false);
      toastError(e);
    });
  };

  const percent = status.total ? Math.min(100, Math.floor((status.received / status.total) * 100)) : null;
  const line = (() => {
    if (!supported) return t("settings.update.unsupported");
    switch (status.phase) {
      case "idle":
        return t("settings.update.idle");
      case "checking":
        return t("settings.update.checking");
      case "latest":
        return t("settings.update.latest", { when: ago(status.checkedAt ?? 0, locale) });
      case "available":
        return t("settings.update.available", { version: release?.version ?? "" });
      case "downloading":
        return percent === null
          ? t("settings.update.downloadingSize", { version: release?.version ?? "", size: fileSize(status.received, locale) })
          : t("settings.update.downloading", { version: release?.version ?? "", percent });
      case "ready":
        return t("settings.update.ready", { version: release?.version ?? "" });
      case "failed":
        return t("settings.update.failed", { detail: failMessage(status.error) ?? "" });
    }
  })();

  const action = (() => {
    if (!supported) return null;
    if (status.phase === "ready") {
      return (
        <button className="btn btn-primary btn-sm" onClick={apply} disabled={restarting}>
          {restarting && <Spinner size={12} />}
          {t(restarting ? "settings.update.restarting" : "settings.update.restart")}
        </button>
      );
    }
    if (status.phase === "downloading") {
      return (
        <div className="update-progress" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent ?? undefined}>
          <span style={{ width: `${percent ?? 0}%` }} />
        </div>
      );
    }
    if (release && (status.phase === "available" || status.phase === "failed")) {
      return (
        <button className="btn btn-primary btn-sm" onClick={download}>
          <Icon name="download" size={14} />
          {t("settings.update.download")}
        </button>
      );
    }
    return (
      <button className="btn btn-sm" onClick={check} disabled={status.phase === "checking"}>
        {status.phase === "checking" ? <Spinner size={12} /> : <Icon name="refresh" size={14} />}
        {t("settings.update.check")}
      </button>
    );
  })();

  const showNotes = release && status.phase !== "latest" && status.phase !== "idle";
  const saveSource = async (value: string) => {
    const s = await save({ updateSource: value });
    if (!s) return;
    setSource(s.updateSource);
    toast(t(value ? "settings.update.sourceSaved" : "settings.update.sourceReset"), "success");
  };

  return (
    <>
      <div className="set-group">
        <Row label={t("settings.update.current", { version: current })} hint={line}>
          {action}
        </Row>
        {showNotes && (
          <div className="set-follow update-notes">
            <div className="update-notes-head">
              {t("settings.update.notes", { version: release.version })}
              {release.date && <span className="dim">{new Date(release.date).toLocaleDateString(locale)}</span>}
            </div>
            {release.notes && <div className="update-notes-body">{release.notes}</div>}
          </div>
        )}
      </div>
      {supported && (
        <>
          <h3 className="set-sub">{t("settings.update.options")}</h3>
          <div className="set-group">
            <Row label={t("settings.update.auto")} hint={t("settings.update.autoHint")}>
              <Switch
                on={settings.updateCheck}
                label={t("settings.update.auto")}
                onChange={(v) => void save({ updateCheck: v }).then((s) => s && toast(t(v ? "settings.update.autoOn" : "settings.update.autoOff"), "success"))}
              />
            </Row>
            <Row label={t("settings.update.autoDownload")} hint={t("settings.update.autoDownloadHint")}>
              <Switch
                on={settings.updateCheck && settings.updateDownload}
                label={t("settings.update.autoDownload")}
                onChange={(v) =>
                  void save(v ? { updateCheck: true, updateDownload: true } : { updateDownload: false }).then(
                    (s) => s && toast(t(v ? "settings.update.autoDownloadOn" : "settings.update.autoDownloadOff"), "success"),
                  )
                }
              />
            </Row>
            <Row label={t("settings.update.source")} hint={t("settings.update.sourceHint")} stack>
              <form
                className="update-source"
                onSubmit={(e) => {
                  e.preventDefault();
                  void saveSource(source.trim());
                }}
              >
                <input
                  className="input"
                  value={source}
                  onChange={(e) => setSource(e.target.value)}
                  placeholder={defaultSource ?? ""}
                  aria-label={t("settings.update.source")}
                  spellCheck={false}
                />
                <button className={`btn btn-sm${source.trim() !== settings.updateSource ? " btn-primary" : ""}`} type="submit" disabled={source.trim() === settings.updateSource}>
                  {t("settings.update.sourceSave")}
                </button>
                {settings.updateSource && (
                  <button className="btn btn-ghost btn-sm" type="button" onClick={() => void saveSource("")}>
                    {t("settings.update.sourceDefault")}
                  </button>
                )}
              </form>
            </Row>
          </div>
        </>
      )}
    </>
  );
}
