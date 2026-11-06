import Dropdown from "./Dropdown";
import { useEffect, useRef, useState, useSyncExternalStore, type KeyboardEvent as ReactKeyboardEvent } from "react";
import Icon from "./Icon";
import { setMode, useAppearance, type ThemeMode } from "./appearance";
import { keyLabel, setLocale, t, useLocale, useLocales, type Key } from "./i18n";

/* ---------- 主题：跟随系统 / 浅色 / 深色；状态和持久化在 appearance.ts ---------- */

const ORDER: ThemeMode[] = ["system", "light", "dark"];
const THEME_LABEL: Record<ThemeMode, Key> = {
  system: "theme.system",
  light: "theme.light",
  dark: "theme.dark",
};

function useThemePref() {
  return [useAppearance().mode, setMode] as const;
}

const THEME_ICON = { system: "monitor", light: "sun", dark: "moon" } as const;

/** 顶栏里的单个按钮，点一下换到下一种 */
export function ThemeToggle() {
  useLocale();
  const [pref, setPref] = useThemePref();
  const next = ORDER[(ORDER.indexOf(pref) + 1) % ORDER.length];
  return (
    <button
      className="btn btn-icon btn-ghost on-dark"
      onClick={() => setPref(next)}
      title={t("theme.toggle", { current: t(THEME_LABEL[pref]), next: t(THEME_LABEL[next]) })}
      aria-label={t("theme.current", { current: t(THEME_LABEL[pref]) })}
    >
      <Icon name={THEME_ICON[pref]} />
    </button>
  );
}

/** 三段并排，当前选中的一目了然；首页左栏用 */
export function ThemeSwitch() {
  useLocale();
  const [pref, setPref] = useThemePref();
  return (
    <span className="seg" role="radiogroup" aria-label={t("theme.label")} onKeyDown={rovingKeys}>
      {ORDER.map((k) => (
        <button
          key={k}
          className={pref === k ? "on" : ""}
          role="radio"
          aria-checked={pref === k}
          tabIndex={pref === k ? 0 : -1}
          title={t(THEME_LABEL[k])}
          onClick={() => setPref(k)}
        >
          <Icon name={THEME_ICON[k]} size={13} />
          {t(THEME_LABEL[k])}
        </button>
      ))}
    </span>
  );
}

/** 当前实际生效的是不是深色，给 CodeMirror 之类需要知道的地方用 */
export function useDark(): boolean {
  const q = typeof matchMedia === "function" ? matchMedia("(prefers-color-scheme: dark)") : null;
  return useSyncExternalStore(
    (cb) => {
      q?.addEventListener("change", cb);
      const mo = new MutationObserver(cb);
      mo.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
      return () => {
        q?.removeEventListener("change", cb);
        mo.disconnect();
      };
    },
    () => {
      const t = document.documentElement.dataset.theme;
      return t ? t === "dark" : !!q?.matches;
    }
  );
}

/* ---------- 模态弹窗：焦点关在框里，Esc 关掉，关了把焦点还回去 ---------- */

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"]), [contenteditable="true"]';
/** 叠着开的弹窗只有最上面那个接键盘 */
const dialogs: HTMLElement[] = [];

/**
 * 给 role="dialog" 的框用：打开时焦点进框（已有 autoFocus 的不动），Tab 在框里转圈，
 * Esc 只关最上面一层且不再往外传（免得设置页之类同时把整页退掉），关掉后焦点回到打开它的那个按钮。
 * 框本身要带 tabIndex={-1}，框里一个可聚焦的都没有时焦点落在框上。
 */
export function useDialog<T extends HTMLElement = HTMLDivElement>(onEscape?: () => void) {
  const ref = useRef<T>(null);
  const esc = useRef(onEscape);
  esc.current = onEscape;
  // 打开前焦点在哪：要在第一次渲染时记，等到 effect 里 autoFocus 已经把焦点挪进框了
  const [back] = useState(() => (document.activeElement instanceof HTMLElement ? document.activeElement : null));
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    dialogs.push(el);
    if (!el.contains(document.activeElement)) (el.querySelector<HTMLElement>(FOCUSABLE) ?? el).focus();
    const onKey = (e: KeyboardEvent) => {
      if (dialogs[dialogs.length - 1] !== el) return;
      if (e.key === "Escape" && esc.current) {
        e.preventDefault();
        e.stopPropagation();
        esc.current();
        return;
      }
      if (e.key !== "Tab") return;
      const items = [...el.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((x) => x.getClientRects().length > 0);
      if (!items.length) return e.preventDefault();
      const [first, last] = [items[0], items[items.length - 1]];
      const cur = document.activeElement;
      if (!el.contains(cur) || (e.shiftKey && cur === first) || (!e.shiftKey && cur === last)) {
        e.preventDefault();
        (e.shiftKey ? last : first).focus();
      }
    };
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("keydown", onKey, true);
      dialogs.splice(dialogs.indexOf(el), 1);
      // 框真被卸掉了才还焦点（StrictMode 开发时会空跑一遍卸载再挂上，那时框还在）
      if (!el.isConnected && back?.isConnected && back !== document.body) back.focus({ preventScroll: true });
    };
  }, [back]);
  return ref;
}

/** 页签组（role="tablist"）、单选组（role="radiogroup"）的方向键：换到相邻一项并选中，Home/End 到头尾
 *  （WAI 页签 / 单选模式）。各项自己按选中与否给 tabIndex 0 / -1，Tab 键只停在选中的那一项上 */
export function rovingKeys(e: ReactKeyboardEvent<HTMLElement>) {
  const step = { ArrowLeft: -1, ArrowUp: -1, ArrowRight: 1, ArrowDown: 1 }[e.key];
  if (step === undefined && e.key !== "Home" && e.key !== "End") return;
  const items = [...e.currentTarget.querySelectorAll<HTMLElement>('[role="tab"]:not([disabled]), [role="radio"]:not([disabled])')];
  const at = items.indexOf(document.activeElement as HTMLElement);
  if (at < 0) return;
  e.preventDefault();
  const n = items.length;
  const next = e.key === "Home" ? 0 : e.key === "End" ? n - 1 : (at + step! + n) % n;
  items[next].focus();
  items[next].click();
}

/* ---------- 轻提示：右下角，成功/提示自动消失，错误要手动关 ---------- */

type Toast = { id: number; kind: "info" | "success" | "error"; text: string };
let toasts: Toast[] = [];
let seq = 0;
const subs = new Set<() => void>();
const emit = () => subs.forEach((f) => f());

export function toast(text: string, kind: Toast["kind"] = "info") {
  const id = ++seq;
  // 同样的话连着弹没有意义，只留最新一条
  toasts = [...toasts.filter((t) => t.text !== text), { id, kind, text }].slice(-4);
  emit();
  // 提示 3～5 秒自己走；错误要用户看完手动关
  if (kind !== "error") window.setTimeout(() => dismiss(id), kind === "success" ? 3000 : 4500);
}

export const toastError = (err: unknown) => toast(String(err).replace(/^Error:\s*/, ""), "error");

function dismiss(id: number) {
  toasts = toasts.filter((t) => t.id !== id);
  emit();
}

export function Toaster() {
  useLocale();
  const list = useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => void subs.delete(cb);
    },
    () => toasts
  );
  return (
    <div className="toaster" role="status" aria-live="polite">
      {list.map((it) => (
        // 错误抢先念（role=alert），其余跟着外层的 polite 排队
        <div key={it.id} className={`toast toast-${it.kind}`} role={it.kind === "error" ? "alert" : undefined}>
          <Icon name={it.kind === "error" ? "error" : it.kind === "success" ? "check" : "info"} />
          <span className="toast-text">{it.text}</span>
          <button className="toast-close" onClick={() => dismiss(it.id)} aria-label={t("common.close")}>
            <Icon name="x" size={14} />
          </button>
        </div>
      ))}
    </div>
  );
}

/* ---------- 截断的字：悬停时补上全文 ---------- */

/**
 * 被省略号截断（text-overflow: ellipsis）的字，鼠标移上去时把全文挂成 title；没截断就不挂，
 * 组件自己写了 title 的不碰。全局装一次，省得每个截断处各写一遍、漏写。
 */
export function installTruncationTitles() {
  document.addEventListener("pointerover", (e) => {
    let n = e.target instanceof HTMLElement ? e.target : null;
    for (let depth = 0; n && depth < 4; n = n.parentElement, depth++) {
      if (getComputedStyle(n).textOverflow !== "ellipsis") continue;
      const auto = "autoTitle" in n.dataset;
      if (n.title && !auto) return;
      if (n.scrollWidth > n.clientWidth + 1) {
        n.title = n.textContent?.trim() ?? "";
        n.dataset.autoTitle = "";
      } else if (auto) {
        n.removeAttribute("title");
        delete n.dataset.autoTitle;
      }
      return;
    }
  });
}

/* ---------- 小零件 ---------- */

export const MAC = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);
/** 快捷键按平台显示：Mac 上 ⌘，其他 Ctrl */
export const MOD = MAC ? "⌘" : "Ctrl+";

export function Kbd({ children }: { children: string }) {
  return <kbd className="kbd">{keyLabel(children)}</kbd>;
}

export function Spinner({ size = 14 }: { size?: number }) {
  return <span className="spinner" style={{ width: size, height: size }} aria-hidden="true" />;
}

/** 相对时间交给 Intl 按当前语言写，「3 分钟前」「3 minutes ago」都不用自己拼 */
export function ago(secs: number, locale: string): string {
  if (!secs) return "";
  const d = Date.now() / 1000 - secs;
  if (d < 60) return t("time.justNow");
  const rtf = new Intl.RelativeTimeFormat(locale, { numeric: "auto" });
  if (d < 3600) return rtf.format(-Math.floor(d / 60), "minute");
  if (d < 86400) return rtf.format(-Math.floor(d / 3600), "hour");
  if (d < 86400 * 30) return rtf.format(-Math.floor(d / 86400), "day");
  return new Date(secs * 1000).toLocaleDateString(locale);
}

/** 字节数读成 KB / MB，单位写法随语言 */
export function fileSize(bytes: number, locale: string): string {
  const mb = bytes / (1 << 20);
  const [n, unit] = mb >= 1 ? [mb, "megabyte"] : [Math.max(bytes / 1024, 0.1), "kilobyte"];
  return new Intl.NumberFormat(locale, { style: "unit", unit, maximumFractionDigits: 1 }).format(n);
}

/** 毫秒数读成人话：850 ms、5.9 s、1 分 12 秒（分秒的写法随语言） */
export function duration(ms: number): string {
  if (ms < 1000) return `${ms} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  const s = Math.round(ms / 1000);
  return t("time.minutesSeconds", { m: Math.floor(s / 60), s: s % 60 });
}

/* ---------- 界面语言：语言列表来自 i18n/locales/，这里不写死有哪几种 ---------- */

/** 首页左栏用：下拉框，语言再多也放得下 */
export function LocaleSelect() {
  const cur = useLocale();
  const all = useLocales();
  return (
    <Dropdown
      value={cur.code}
      options={all.map((l) => ({ value: l.code, label: l.name, lang: l.code }))}
      onChange={setLocale}
      label={t("language.label")}
    />
  );
}

/** 顶栏用：图标按钮的样子，底下压一个透明的原生下拉框，点开就是系统菜单 */
export function LocaleToggle() {
  const cur = useLocale();
  const all = useLocales();
  return (
    <label className="btn btn-icon btn-ghost on-dark locale-toggle" title={t("language.current", { name: cur.name })}>
      <Icon name="globe" />
      <select value={cur.code} onChange={(e) => setLocale(e.target.value)} aria-label={t("language.label")}>
        {all.map((l) => (
          <option key={l.code} value={l.code} lang={l.code}>
            {l.name}
          </option>
        ))}
      </select>
    </label>
  );
}
