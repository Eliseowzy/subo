import { useSyncExternalStore } from "react";
import { api } from "./api";
import { BUILTIN_LOCALES, getLocale, getLocales, onLocaleChange, registerLocales, setLocale, t, type Locale } from "./i18n";
import base from "./i18n/locales/zh-CN";

/*
 * 外观与语言包：用户自己做（或让 AI 做）的配色方案、界面语言，导入导出。格式说明见 skill/theme/SKILL.md、skill/locale/SKILL.md（也是给 AI 看的规范）。
 *
 * - 配色方案只能改 styles.css 顶部定义的设计令牌（--accent 之类），能改哪些直接从样式表里读，
 *   不另列清单；值不许带 url()、花括号、分号，进不了网、也跳不出变量声明。
 * - 语言包的键以基准语言 zh-CN 为准，可以只翻一部分，缺的由 i18n 用内置语言补齐。
 * - 包和当前选择都存在本机服务（~/.subo/appearance/），不放 localStorage：
 *   桌面壳每次启动端口不同，localStorage 跟着源走会丢。localStorage 只做首帧前的快取。
 */

export type ThemeMode = "system" | "light" | "dark";
type Tokens = Record<string, string>;

export type ThemePack = {
  format: "subo-theme";
  version: 1;
  id: string;
  name: string;
  author?: string;
  description?: string;
  /** 不分明暗的令牌（字体、圆角）；light / dark 只在对应模式下生效 */
  base?: Tokens;
  light?: Tokens;
  dark?: Tokens;
};

export type LocalePack = {
  format: "subo-locale";
  version: 1;
  code: string;
  name: string;
  author?: string;
  /** 缺的键先用哪种内置语言补，默认 en */
  fallback?: string;
  messages: Record<string, unknown>;
  /** 可选：宏包名 → 一句话说明。没写的宏包显示英文原文，不走 fallback */
  packages?: Record<string, string>;
};

type Bundle = { format: "subo-bundle"; version: 1; prefs?: Prefs; themes?: unknown[]; locales?: unknown[] };
type Prefs = { mode?: ThemeMode; theme?: string; locale?: string; editorSize?: string };

const SECTIONS = ["base", "light", "dark"] as const;
const MODE_KEY = "subo.theme";
const EDITOR_SIZE_KEY = "subo.editorSize";
const FORBIDDEN = /[;{}<>\\]|url\s*\(|@import|expression\s*\(/i;
const ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;
const CODE = /^[A-Za-z]{2,3}(-[A-Za-z0-9]{1,8})*$/;
/** TeX Live 的包名：字母数字开头，可含 - _ .（平台二进制包带点，界面不列，但不必拒） */
const PKG = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;
export const DEFAULT_THEME = "default";

/* ---------- 内置令牌：从 styles.css 里读，令牌增减不用改这里 ---------- */

let tokenCache: { base: Tokens; light: Tokens; dark: Tokens } | null = null;

/** 内置配色的全部令牌（不带 --）。只出现在 :root 的算 base，深色也改了的算 light / dark */
export function builtinTokens() {
  if (tokenCache) return tokenCache;
  const root: Tokens = {};
  const dark: Tokens = {};
  for (const sheet of Array.from(document.styleSheets)) {
    if (sheet.ownerNode instanceof HTMLElement && sheet.ownerNode.id === STYLE_ID) continue;
    let rules: CSSRuleList;
    try {
      rules = sheet.cssRules;
    } catch {
      continue;
    }
    for (const r of Array.from(rules)) {
      if (!(r instanceof CSSStyleRule)) continue;
      const into = r.selectorText === ":root" ? root : r.selectorText === ':root[data-theme="dark"]' ? dark : null;
      if (!into) continue;
      for (const prop of Array.from(r.style)) {
        if (prop.startsWith("--")) into[prop.slice(2)] = r.style.getPropertyValue(prop).trim();
      }
    }
  }
  const out = { base: {} as Tokens, light: {} as Tokens, dark };
  for (const [k, v] of Object.entries(root)) (k in dark ? out.light : out.base)[k] = v;
  // 样式表还没加载好时别缓存空结果
  if (Object.keys(root).length) tokenCache = out;
  return out;
}

function tokenNames(): Set<string> {
  const b = builtinTokens();
  return new Set([...Object.keys(b.base), ...Object.keys(b.light), ...Object.keys(b.dark)]);
}

/* ---------- 校验：导入时用，报告写给人看 ---------- */

export type Report = {
  /** 丢掉的未知令牌 / 未知文案键 */
  unknown: string[];
  /** 值不合法被丢掉的 */
  invalid: string[];
  /** 语言包：译文里出现了基准文案没有的占位符 */
  placeholders: string[];
  /** 语言包：已翻译 / 总键数 */
  coverage?: [number, number];
};

const fail = (key: Parameters<typeof t>[0], vars?: Record<string, string | number>): never => {
  throw new Error(t(key, vars));
};

const str = (v: unknown, max = 200) => (typeof v === "string" && v.trim() && v.length <= max ? v.trim() : undefined);

export function checkTheme(raw: unknown): { pack: ThemePack; report: Report } {
  const o = raw as Record<string, unknown>;
  if (o?.format !== "subo-theme") fail("appearance.errors.notPack");
  if (o.version !== 1) fail("appearance.errors.version", { version: String(o.version) });
  const id = str(o.id, 64);
  if (!id || !ID.test(id)) fail("appearance.errors.badId", { id: String(o.id) });
  if (id === DEFAULT_THEME) fail("appearance.errors.reservedId", { id: id! });
  const name = str(o.name, 80) ?? fail("appearance.errors.noName");
  const names = tokenNames();
  const report: Report = { unknown: [], invalid: [], placeholders: [] };
  const pack: ThemePack = { format: "subo-theme", version: 1, id: id!, name };
  if (str(o.author, 120)) pack.author = str(o.author, 120);
  if (str(o.description, 500)) pack.description = str(o.description, 500);
  let total = 0;
  for (const sec of SECTIONS) {
    const src = o[sec];
    if (src == null) continue;
    if (typeof src !== "object" || Array.isArray(src)) fail("appearance.errors.badSection", { section: sec });
    const out: Tokens = {};
    for (const [k0, v] of Object.entries(src as object)) {
      const k = k0.replace(/^--/, "");
      if (!names.has(k)) report.unknown.push(`${sec}.${k}`);
      else if (typeof v !== "string" || !v.trim() || v.length > 300 || FORBIDDEN.test(v)) report.invalid.push(`${sec}.${k}`);
      else out[k] = v.trim();
    }
    if (Object.keys(out).length) {
      pack[sec] = out;
      total += Object.keys(out).length;
    }
  }
  if (!total) fail("appearance.errors.emptyTheme");
  return { pack, report };
}

const PLURAL_FORMS = new Set(["zero", "one", "two", "few", "many", "other"]);
const holes = (s: string) => new Set(Array.from(s.matchAll(/\{(\w+)\}/g), (m) => m[1]));

function msgOk(v: unknown): v is string | Record<string, string> {
  if (typeof v === "string") return v.length <= 4000;
  if (!v || typeof v !== "object" || Array.isArray(v)) return false;
  const e = Object.entries(v);
  return "other" in v && e.every(([k, x]) => PLURAL_FORMS.has(k) && typeof x === "string" && x.length <= 4000);
}

const forms = (v: string | Record<string, string>) => (typeof v === "string" ? [v] : Object.values(v));

/** 按基准语言的结构把包里的文案挑出来：结构以外的、类型不对的都丢掉并记下 */
function pickMessages(ref: Record<string, unknown>, src: Record<string, unknown>, path: string, report: Report, count: number[]) {
  const out: Record<string, unknown> = {};
  for (const [k, refV] of Object.entries(ref)) {
    const p = path ? `${path}.${k}` : k;
    const leaf = typeof refV === "string" || (refV && typeof refV === "object" && "other" in refV);
    if (leaf) count[1]++;
    if (!(k in src)) {
      if (!leaf) count[1] += countLeaves(refV);
      continue;
    }
    const v = src[k];
    if (leaf) {
      if (!msgOk(v)) {
        report.invalid.push(p);
        continue;
      }
      const allowed = new Set(forms(refV as string | Record<string, string>).flatMap((s) => [...holes(s)]));
      allowed.add("count");
      const extra = forms(v).flatMap((s) => [...holes(s)]).filter((h) => !allowed.has(h));
      if (extra.length) report.placeholders.push(`${p} {${[...new Set(extra)].join("}, {")}}`);
      out[k] = v;
      count[0]++;
    } else if (v && typeof v === "object" && !Array.isArray(v)) {
      const sub = pickMessages(refV as Record<string, unknown>, v as Record<string, unknown>, p, report, count);
      if (Object.keys(sub).length) out[k] = sub;
    } else {
      report.invalid.push(p);
      count[1] += countLeaves(refV);
    }
  }
  for (const k of Object.keys(src)) if (!(k in ref)) report.unknown.push(path ? `${path}.${k}` : k);
  return out;
}

function countLeaves(v: unknown): number {
  if (typeof v === "string" || (v && typeof v === "object" && "other" in v)) return 1;
  return v && typeof v === "object" ? Object.values(v).reduce<number>((n, x) => n + countLeaves(x), 0) : 0;
}

export function checkLocale(raw: unknown): { pack: LocalePack; report: Report } {
  const o = raw as Record<string, unknown>;
  if (o?.format !== "subo-locale") fail("appearance.errors.notPack");
  if (o.version !== 1) fail("appearance.errors.version", { version: String(o.version) });
  const code = str(o.code, 64);
  if (!code || !CODE.test(code)) fail("appearance.errors.badCode", { code: String(o.code) });
  const name = str(o.name, 80) ?? fail("appearance.errors.noName");
  if (!o.messages || typeof o.messages !== "object" || Array.isArray(o.messages)) fail("appearance.errors.noMessages");
  const report: Report = { unknown: [], invalid: [], placeholders: [] };
  const count = [0, 0];
  const messages = pickMessages(base.messages as Record<string, unknown>, o.messages as Record<string, unknown>, "", report, count);
  if (!count[0]) fail("appearance.errors.emptyLocale");
  report.coverage = [count[0], count[1]];
  const pack: LocalePack = { format: "subo-locale", version: 1, code: code!, name, messages };
  if (str(o.author, 120)) pack.author = str(o.author, 120);
  const fb = str(o.fallback, 64);
  if (fb && BUILTIN_LOCALES.some((l) => l.code === fb)) pack.fallback = fb;
  const packages = pickPackages(o.packages, report);
  if (packages) pack.packages = packages;
  return { pack, report };
}

/** 宏包说明：可选，不计入覆盖率；名字或值不合法的丢掉并记下 */
function pickPackages(v: unknown, report: Report): Record<string, string> | undefined {
  if (v == null) return undefined;
  if (typeof v !== "object" || Array.isArray(v)) {
    report.invalid.push("packages");
    return undefined;
  }
  const out: Record<string, string> = {};
  for (const [name, desc] of Object.entries(v)) {
    const d = str(desc, 300);
    if (PKG.test(name) && d) out[name] = d;
    else report.invalid.push(`packages.${name}`);
  }
  return Object.keys(out).length ? out : undefined;
}

/** 语言包的翻译覆盖率，列表里显示 */
export function coverage(p: LocalePack): number {
  const r: Report = { unknown: [], invalid: [], placeholders: [] };
  const count = [0, 0];
  pickMessages(base.messages as Record<string, unknown>, p.messages, "", r, count);
  return count[1] ? count[0] / count[1] : 0;
}

/** 某个配色在浅色、深色下实际生效的全部令牌（内置值打底，包里的覆盖），var() 已展开，给预览用 */
export function resolvedTokens(id: string): { light: Tokens; dark: Tokens } {
  const b = builtinTokens();
  const p = id === DEFAULT_THEME ? undefined : findTheme(id);
  const resolve = (tk: Tokens) => {
    const get = (v: string, depth: number): string =>
      depth > 8 ? v : v.replace(/var\(--([\w-]+)(?:,\s*([^()]*))?\)/g, (m, k: string, fb?: string) => (k in tk ? get(tk[k], depth + 1) : fb ?? m));
    return Object.fromEntries(Object.entries(tk).map(([k, v]) => [k, get(v, 0)]));
  };
  return {
    light: resolve({ ...b.base, ...b.light, ...p?.base, ...p?.light }),
    dark: resolve({ ...b.base, ...b.light, ...b.dark, ...p?.base, ...p?.dark }),
  };
}

/** 导入的配色按 id 找；内置只有默认配色（不是包，就是 styles.css 本身） */
function findTheme(id: string): ThemePack | undefined {
  return state.themes.find((p) => p.id === id);
}

/* ---------- 状态：已装的包 + 当前选择 ---------- */

/** 编辑器字号（px），null 表示跟随配色方案里的 --editor-size */
type State = { themes: ThemePack[]; locales: LocalePack[]; mode: ThemeMode; theme: string; editorSize: number | null; dir: string; ready: boolean };

let state: State = { themes: [], locales: [], mode: readMode(), theme: DEFAULT_THEME, editorSize: readEditorSize(), dir: "", ready: false };
const subs = new Set<() => void>();
function set(patch: Partial<State>) {
  state = { ...state, ...patch };
  subs.forEach((f) => f());
}

export function useAppearance(): State {
  return useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => void subs.delete(cb);
    },
    () => state
  );
}

function readMode(): ThemeMode {
  try {
    const v = localStorage.getItem(MODE_KEY);
    return v === "light" || v === "dark" ? v : "system";
  } catch {
    return "system";
  }
}

/** index.html 里有同样逻辑的内联脚本，首帧就定好明暗，不闪 */
function applyMode(m: ThemeMode) {
  if (m === "system") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = m;
  try {
    localStorage.setItem(MODE_KEY, m);
  } catch {
    /* 存不进去就只在本次生效 */
  }
}
applyMode(state.mode);

function parseEditorSize(v: unknown): number | null {
  const n = typeof v === "string" ? Number(v) : NaN;
  return Number.isFinite(n) && n >= 8 && n <= 40 ? n : null;
}

function readEditorSize(): number | null {
  try {
    return parseEditorSize(localStorage.getItem(EDITOR_SIZE_KEY));
  } catch {
    return null;
  }
}

/** 写在 <html> 的行内样式上，压过 styles.css 和配色方案里的 --editor-size */
function applyEditorSize(px: number | null) {
  const root = document.documentElement.style;
  if (px == null) root.removeProperty("--editor-size");
  else root.setProperty("--editor-size", `${px}px`);
  try {
    if (px == null) localStorage.removeItem(EDITOR_SIZE_KEY);
    else localStorage.setItem(EDITOR_SIZE_KEY, String(px));
  } catch {
    /* 存不进去就只在本次生效 */
  }
}
applyEditorSize(state.editorSize);

const STYLE_ID = "subo-theme-pack";

/** 生成覆盖令牌的样式：选择器与 styles.css 的三处一一对应，放在最后，同优先级后者赢 */
export function themeCss(p: ThemePack): string {
  const decl = (tk?: Tokens) =>
    Object.entries(tk ?? {})
      .map(([k, v]) => `  --${k}: ${v};`)
      .join("\n");
  const parts = [`:root {\n${decl(p.base)}\n${decl(p.light)}\n}`];
  if (p.dark && Object.keys(p.dark).length) {
    parts.push(`@media (prefers-color-scheme: dark) {\n:root:not([data-theme="light"]) {\n${decl(p.dark)}\n}\n}`);
    parts.push(`:root[data-theme="dark"] {\n${decl(p.dark)}\n}`);
  }
  return parts.join("\n");
}

function applyTheme(id: string) {
  const pack = findTheme(id);
  let el = document.getElementById(STYLE_ID);
  if (!pack) {
    el?.remove();
    return;
  }
  if (!el) el = document.createElement("style");
  el.id = STYLE_ID;
  el.textContent = themeCss(pack);
  // 每次都挪到 <head> 最后，保证排在所有样式表之后
  document.head.appendChild(el);
}

/* ---------- 和本机服务交互 ---------- */

function savePrefs(patch: Record<string, string | null>) {
  void api.saveAppearancePrefs(patch).catch(() => {
    /* 存不上只影响下次启动，不打扰用户 */
  });
}

/** 重新读已装的包；服务端只把关格式，内容在这里再校验一遍，不合格的跳过 */
async function reload(): Promise<Prefs> {
  const r = await api.appearance();
  const themes = r.themes.flatMap((x) => {
    try {
      return [checkTheme(x).pack];
    } catch {
      return [];
    }
  });
  const locales = r.locales.flatMap((x) => {
    try {
      return [checkLocale(x).pack];
    } catch {
      return [];
    }
  });
  registerLocales(locales as unknown as Locale[]);
  set({ themes, locales, dir: r.dir });
  if (!findTheme(state.theme)) {
    set({ theme: DEFAULT_THEME });
    applyTheme(DEFAULT_THEME);
  }
  return r.prefs as Prefs;
}

/**
 * 启动时调一次，渲染前等它（本机服务很快；连不上最多等 1.5 秒就照常渲染）。
 * 服务端还没存过选择（老版本升上来）就把 localStorage 里的选择补存过去。
 */
export async function bootAppearance() {
  onLocaleChange((code) => savePrefs({ locale: code }));
  const timeout = new Promise<null>((r) => setTimeout(() => r(null), 1500));
  const prefs = await Promise.race([reload().catch(() => null), timeout]);
  set({ ready: true });
  if (!prefs) return;
  if (!prefs.mode && !prefs.locale && !prefs.theme && !prefs.editorSize) {
    const local = { mode: state.mode, locale: getLocale().code, editorSize: state.editorSize == null ? null : String(state.editorSize) };
    savePrefs(local);
    return;
  }
  if (prefs.mode === "system" || prefs.mode === "light" || prefs.mode === "dark") {
    set({ mode: prefs.mode });
    applyMode(prefs.mode);
  }
  if (prefs.theme && findTheme(prefs.theme)) {
    set({ theme: prefs.theme });
    applyTheme(prefs.theme);
  }
  const size = parseEditorSize(prefs.editorSize);
  set({ editorSize: size });
  applyEditorSize(size);
  if (prefs.locale) setLocale(prefs.locale, true);
}

export function setMode(m: ThemeMode) {
  set({ mode: m });
  applyMode(m);
  savePrefs({ mode: m });
}

export function setTheme(id: string) {
  set({ theme: id });
  applyTheme(id);
  savePrefs({ theme: id === DEFAULT_THEME ? null : id });
}

/** save=false 只在本机生效（拖滑块途中），松手时再存 */
export function setEditorSize(px: number | null, save = true) {
  set({ editorSize: px });
  applyEditorSize(px);
  if (save) savePrefs({ editorSize: px == null ? null : String(px) });
}

/** 当前配色方案的默认编辑器字号（px）；配色里写的不是 px 就按内置的 13.5 */
export function defaultEditorSize(theme: string): number {
  const n = parseFloat(resolvedTokens(theme).light["editor-size"] ?? "");
  return Number.isFinite(n) && n >= 8 && n <= 40 ? n : 13.5;
}

export type Imported = { kind: "theme" | "locale"; id: string; name: string; replaced: boolean; report: Report };

async function install(kind: "theme" | "locale", raw: unknown): Promise<Imported> {
  const { pack, report } = kind === "theme" ? checkTheme(raw) : checkLocale(raw);
  const id = "id" in pack ? pack.id : pack.code;
  const replaced = kind === "theme" ? state.themes.some((p) => p.id === id) : state.locales.some((p) => p.code === id);
  await api.putPack(kind, id, pack);
  return { kind, id, name: pack.name, replaced, report };
}

/** 导入一个文件：配色方案、语言包，或「全部导出」的备份。单个包导入后直接启用 */
export async function importFile(file: File): Promise<Imported[]> {
  if (file.size > 2 * 1024 * 1024) fail("appearance.errors.tooLarge");
  let raw: unknown;
  try {
    raw = JSON.parse(await file.text());
  } catch {
    fail("appearance.errors.notJson");
  }
  const format = (raw as { format?: unknown })?.format;
  if (format === "subo-bundle") {
    const b = raw as Bundle;
    const done: Imported[] = [];
    for (const x of b.themes ?? []) done.push(await install("theme", x));
    for (const x of b.locales ?? []) done.push(await install("locale", x));
    await reload();
    // 备份里记着当时的选择，一并恢复
    if (b.prefs?.mode === "system" || b.prefs?.mode === "light" || b.prefs?.mode === "dark") setMode(b.prefs.mode);
    if (b.prefs?.theme && findTheme(b.prefs.theme)) setTheme(b.prefs.theme);
    if (b.prefs?.locale && getLocales().some((l) => l.code === b.prefs!.locale)) setLocale(b.prefs.locale);
    return done;
  }
  const kind = format === "subo-theme" ? "theme" : format === "subo-locale" ? "locale" : fail("appearance.errors.notPack");
  const one = await install(kind, raw);
  await reload();
  if (kind === "theme") setTheme(one.id);
  else setLocale(one.id);
  return [one];
}

export async function removePack(kind: "theme" | "locale", id: string) {
  await api.removePack(kind, id);
  if (kind === "theme" && state.theme === id) setTheme(DEFAULT_THEME);
  await reload();
}

/* ---------- 导出：存成文件，作备份或作为自己定制的起点 ---------- */

/** 内置配色导出成一个完整的包，令牌全列出来，改起来方便 */
export function defaultThemePack(): ThemePack {
  const b = builtinTokens();
  return {
    format: "subo-theme",
    version: 1,
    id: DEFAULT_THEME,
    name: t("appearance.defaultTheme"),
    author: "Subo",
    base: b.base,
    light: b.light,
    dark: b.dark,
  };
}

export function localePack(l: Locale): LocalePack {
  const custom = state.locales.find((p) => p.code === l.code);
  if (l.custom && custom) return custom;
  const pack: LocalePack = { format: "subo-locale", version: 1, code: l.code, name: l.name, messages: l.messages as Record<string, unknown> };
  if (l.packages) pack.packages = l.packages;
  return pack;
}

export function saveFile(name: string, data: unknown, type = "application/json") {
  const blob = data instanceof Blob ? data : new Blob([JSON.stringify(data, null, 2) + "\n"], { type });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

export function exportTheme(id: string) {
  const p = id === DEFAULT_THEME ? defaultThemePack() : findTheme(id);
  if (p) saveFile(`${p.id}.subo-theme.json`, p);
}

export function exportLocale(code: string) {
  const l = getLocales().find((x) => x.code === code);
  if (l) saveFile(`${code}.subo-locale.json`, localePack(l));
}

/** 全部导出：某一类自己导入的包 + 相关的当前选择，一个文件，导入时整体恢复 */
export function exportAll(kind: "theme" | "locale") {
  const day = new Date().toISOString().slice(0, 10).replace(/-/g, "");
  const bundle: Bundle =
    kind === "theme"
      ? { format: "subo-bundle", version: 1, prefs: { mode: state.mode, theme: state.theme }, themes: state.themes }
      : { format: "subo-bundle", version: 1, prefs: { locale: getLocale().code }, locales: state.locales };
  saveFile(`subo-${kind}s-${day}.json`, bundle);
}
