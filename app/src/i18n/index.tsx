import { Fragment, useSyncExternalStore, type ReactNode } from "react";
import base from "./locales/zh-CN";
import type { Key, Locale, Msg, Vars } from "./types";

/*
 * 界面多语言。规范见 docs/i18n.md，要点：
 * - 每种语言一个文件放在 ./locales/，文件名就是语言代码（BCP 47，如 en、zh-CN、ja）。
 *   放进去即被收录，不用改这里。
 * - zh-CN 是基准：所有键先在那里定义，其他语言的类型由它推出，缺键 tsc 直接报错。
 * - 界面代码里不写任何面向用户的自然语言，一律 t("…")；npm run check:i18n 会拦。
 */

const files = import.meta.glob<{ default: Locale }>("./locales/*.ts", { eager: true });
/** 宏包说明的译文，文件名是语言代码；英文用 TeX Live 自带的原文，不需要文件 */
const pkgFiles = import.meta.glob<{ default: Record<string, string> }>("./packages/*.json", { eager: true });
const pkgTables = new Map(Object.entries(pkgFiles).map(([path, m]) => [path.replace(/^.*\/|\.json$/g, ""), m.default]));

/** 随软件发布的语言，按语言代码排序，基准语言放第一个 */
export const BUILTIN_LOCALES: Locale[] = Object.values(files)
  .map((m) => ({ ...m.default, packages: pkgTables.get(m.default.code) }))
  .sort((a, b) => (a.code === base.code ? -1 : b.code === base.code ? 1 : a.code.localeCompare(b.code)));
const builtin = new Map(BUILTIN_LOCALES.map((l) => [l.code, l]));

/** 当前可选的语言：内置的 + 用户导入的语言包（与内置同代码的包顶替内置那一项） */
let locales: Locale[] = BUILTIN_LOCALES;
let byCode = new Map(locales.map((l) => [l.code, l]));
const KEY = "subo.locale";

/** 把 b 叠到 a 上：b 里有的叶子覆盖，没有的沿用 a */
function overlay(a: unknown, b: unknown): unknown {
  if (!b || typeof b !== "object" || "other" in b) return b ?? a;
  if (!a || typeof a !== "object" || "other" in a) return b;
  const out: Record<string, unknown> = { ...(a as Record<string, unknown>) };
  for (const [k, v] of Object.entries(b)) out[k] = overlay(out[k], v);
  return out;
}

/**
 * 登记导入的语言包（整体替换上一次登记的）。包可以只翻译一部分：缺的键依次从
 * 同代码的内置语言、包指定的 fallback（默认 en）、基准语言补齐，所以登记后的 messages 是完整的。
 * 宏包说明不走 fallback：包里没写的用同代码内置语言的，再没有就显示英文原文（见 packageDesc）。
 */
export function registerLocales(packs: (Locale & { fallback?: string })[]) {
  const custom = packs.map((p) => {
    const fb = builtin.get(p.fallback ?? "") ?? builtin.get("en") ?? base;
    let messages = overlay(base.messages, fb.messages);
    const same = builtin.get(p.code);
    if (same) messages = overlay(messages, same.messages);
    const packages = same?.packages || p.packages ? { ...same?.packages, ...p.packages } : undefined;
    return { code: p.code, name: p.name, custom: true, messages: overlay(messages, p.messages) as Locale["messages"], packages };
  });
  const codes = new Set(custom.map((l) => l.code));
  locales = [...BUILTIN_LOCALES.filter((l) => !codes.has(l.code)), ...custom];
  byCode = new Map(locales.map((l) => [l.code, l]));
  // 当前语言的包被换掉或删掉：换成新登记的同代码语言，没有了就按系统语言重新挑
  const next = byCode.get(current.code) ?? byCode.get(initial()) ?? base;
  if (next !== current) {
    current = next;
    apply();
  }
  emit();
}

export function getLocales(): Locale[] {
  return locales;
}

function initial(): string {
  try {
    const saved = localStorage.getItem(KEY);
    if (saved && byCode.has(saved)) return saved;
  } catch {
    /* 读不到就按系统语言 */
  }
  const prefs = typeof navigator !== "undefined" ? navigator.languages ?? [navigator.language] : [];
  for (const p of prefs) {
    if (byCode.has(p)) return p;
    // 港澳和标明繁体的系统语言用繁体，不能按主语言落到简体
    if (/^zh-(Hant|HK|MO|TW)\b/i.test(p) && byCode.has("zh-TW")) return "zh-TW";
    const lang = p.split("-")[0];
    const hit = locales.find((l) => l.code === lang || l.code.split("-")[0] === lang);
    if (hit) return hit.code;
  }
  return base.code;
}

let current: Locale = byCode.get(initial()) ?? base;
const subs = new Set<() => void>();
const changeSubs = new Set<(code: string) => void>();
/** 语言列表变了（导入、删除语言包）也要让下拉框刷新：版本号随之递增 */
let version = 0;
function emit() {
  version++;
  subs.forEach((f) => f());
}

function apply() {
  document.documentElement.lang = current.code;
}
apply();

export function getLocale(): Locale {
  return current;
}

/** 用户切换语言。quiet：只是同步已保存的选择（启动时），不通知 onLocaleChange */
export function setLocale(code: string, quiet = false) {
  const next = byCode.get(code);
  if (!next || next === current) return;
  current = next;
  try {
    localStorage.setItem(KEY, code);
  } catch {
    /* 存不进去就只在本次生效 */
  }
  apply();
  emit();
  if (!quiet) changeSubs.forEach((f) => f(code));
}

/** 用户换了语言时回调，用来把选择存到本机服务 */
export function onLocaleChange(f: (code: string) => void) {
  changeSubs.add(f);
  return () => void changeSubs.delete(f);
}

/** 按点号路径取值；当前语言缺了（运行时兜底，正常 tsc 已拦住）退回基准 */
function lookup(root: unknown, key: string): Msg | undefined {
  let v = root;
  for (const k of key.split(".")) v = (v as Record<string, unknown> | undefined)?.[k];
  return typeof v === "string" || (v && typeof v === "object" && "other" in v) ? (v as Msg) : undefined;
}

const plurals = new Map<string, Intl.PluralRules>();
function pick(msg: Msg, vars?: Vars): string {
  if (typeof msg === "string") return msg;
  const n = Number(vars?.count ?? 0);
  let rules = plurals.get(current.code);
  if (!rules) plurals.set(current.code, (rules = new Intl.PluralRules(current.code)));
  const cat = rules.select(n) as keyof typeof msg;
  return msg[cat] ?? msg.other;
}

function raw(key: Key, vars?: Vars): string {
  const msg = lookup(current.messages, key) ?? lookup(base.messages, key);
  if (msg === undefined) {
    if (import.meta.env.DEV) console.warn(`[i18n] 缺少文案：${key}`);
    return key;
  }
  return keyLabel(pick(msg, vars));
}

const MAC_KEYS = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);

/** 快捷键一律按 Mac 的写法记（⌘ ⇧ ⌥ ↵）；其他平台显示成 Ctrl+ Shift+ Alt+ Enter。
 *  文案经 t() 自动换，界面里直接写的（Kbd、菜单的快捷键列）也走这里。
 *  「⌘ + 滚轮」这种自己带加号的只换成 Ctrl，不叠出两个加号 */
export function keyLabel(s: string): string {
  if (MAC_KEYS) return s;
  return s
    .replace(/⌘(?=\s*\+)/g, "Ctrl")
    .replace(/⌘/g, "Ctrl+")
    .replace(/⇧/g, "Shift+")
    .replace(/⌥/g, "Alt+")
    // ↵ 在 Windows 字体里很小、不好认
    .replace(/↵/g, "Enter");
}

/** 取一条文案，{name} 换成 vars.name。非组件代码（toast、工具函数）也直接用它 */
export function t(key: Key, vars?: Vars): string {
  return raw(key, vars).replace(/\{(\w+)\}/g, (m, k) => (vars && k in vars ? String(vars[k]) : m));
}

/** 运行时拼出来的键（如服务端错误码）是否存在；以基准语言为准，其他语言 tsc 已保证齐全 */
export function hasKey(key: string): key is Key {
  return lookup(base.messages, key) !== undefined;
}

/** 宏包的一句话说明：当前语言有译文就用译文，没有就用 TeX Live 数据库里的英文原文 */
export function packageDesc(name: string, english: string): string {
  return current.packages?.[name] || english;
}

/** 文案里要嵌组件时用：{kbd} 之类的占位换成传入的节点，其余照样是字符串 */
export function tx(key: Key, nodes: Record<string, ReactNode>): ReactNode {
  return raw(key, nodes as Vars)
    .split(/(\{\w+\})/)
    .map((part, i) => {
      const k = part.match(/^\{(\w+)\}$/)?.[1];
      return <Fragment key={i}>{k && k in nodes ? nodes[k] : part}</Fragment>;
    });
}

const subscribe = (cb: () => void) => {
  subs.add(cb);
  return () => void subs.delete(cb);
};

/** 组件里调用一次，切换语言时这个组件会重渲染；返回当前语言 */
export function useLocale(): Locale {
  return useSyncExternalStore(subscribe, getLocale);
}

/** 可选语言列表，导入或删除语言包后刷新 */
export function useLocales(): Locale[] {
  useSyncExternalStore(subscribe, () => version);
  return locales;
}

export type { Key, Locale } from "./types";
