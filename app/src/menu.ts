import { useEffect, useRef } from "react";
import { t } from "./i18n";
import type { Key } from "./i18n/types";

/*
 * 桌面版（macOS）的菜单栏。菜单文案跟界面语言走，所以由这里按当前语言拼好整份菜单，
 * 交给本地服务转给桌面壳去画（server/src/shell.rs）；浏览器里没有壳，服务端直接丢掉。
 * 点了菜单项，壳往窗口里派 `subo:menu` 事件，detail 是项的 id，用 useMenu 接。
 *
 * 快捷键照常由各页面的 keydown 处理：页面里 preventDefault 了的按键不会再触发菜单，
 * 没接的（如列表页的 ⌘N）才落到菜单上。所以这里的 accel 主要是给菜单显示用。
 */

export type MenuPage = "home" | "project" | "template" | "settings";

type MenuKey = Extract<Key, `menu.${string}`>;
type NativeKey = Extract<Key, `menu.native.${string}`>;

type Entry =
  | { kind: "menu"; label: string }
  | { kind: "item"; id: string; label: string; accel?: string; enabled?: boolean }
  | { kind: "native"; id: string; label: string }
  | { kind: "sep" };

/** 首页上的动作：在别的页面点了，先回首页再做 */
export const HOME_ACTIONS = new Set(["file.newProject", "file.openFolder", "file.fromTemplate"]);

function entries(page: MenuPage): Entry[] {
  const inProject = page === "project" || page === "template";
  // 菜单项的 id 就是文案键去掉 menu. 前缀，系统项再去掉 native.
  const item = (key: MenuKey, accel?: string, enabled = true): Entry => ({ kind: "item", id: key.slice(5), label: t(key), accel, enabled });
  const native = (key: NativeKey): Entry => ({ kind: "native", id: key.slice(12), label: t(key) });
  const sep: Entry = { kind: "sep" };
  return [
    { kind: "menu", label: t("brand.primary") },
    item("menu.app.about"),
    sep,
    item("menu.app.settings", "CmdOrCtrl+,"),
    sep,
    native("menu.native.services"),
    sep,
    native("menu.native.hide"),
    native("menu.native.hideOthers"),
    native("menu.native.showAll"),
    sep,
    native("menu.native.quit"),

    { kind: "menu", label: t("menu.file.title") },
    item("menu.file.newProject", "CmdOrCtrl+N"),
    item("menu.file.openFolder", "CmdOrCtrl+O"),
    item("menu.file.fromTemplate"),
    sep,
    item("menu.file.quickOpen", "CmdOrCtrl+P", inProject),
    item("menu.file.save", "CmdOrCtrl+S", page === "project"),
    item("menu.file.exportPdf", "CmdOrCtrl+E", inProject),
    sep,
    // 列表页上 ⌘W 把窗口藏起来（壳自己处理），其他页面回到项目列表
    page === "home"
      ? { kind: "item", id: "shell.hide", label: t("menu.file.closeWindow"), accel: "CmdOrCtrl+W" }
      : item("menu.file.close", "CmdOrCtrl+W"),

    { kind: "menu", label: t("menu.edit.title") },
    native("menu.native.undo"),
    native("menu.native.redo"),
    sep,
    native("menu.native.cut"),
    native("menu.native.copy"),
    native("menu.native.paste"),
    native("menu.native.selectAll"),
    sep,
    item("menu.edit.find", "CmdOrCtrl+F", inProject),
    item("menu.edit.format", "Alt+Shift+F", page === "project"),

    { kind: "menu", label: t("menu.build.title") },
    item("menu.build.run", "CmdOrCtrl+Enter", inProject),
    item("menu.build.syncForward", "CmdOrCtrl+Shift+J", inProject),
    item("menu.build.log", "CmdOrCtrl+J", inProject),

    { kind: "menu", label: t("menu.window.title") },
    native("menu.native.minimize"),
    native("menu.native.zoom"),
    sep,
    native("menu.native.fullscreen"),

    { kind: "menu", label: t("menu.help.title") },
    item("menu.help.tour"),
  ];
}

let last = "";

/** 按当前页面和语言把菜单交给壳；内容没变就不发 */
export function syncMenu(page: MenuPage) {
  const body = JSON.stringify(entries(page));
  if (body === last) return;
  last = body;
  fetch("/api/shell/menu", { method: "PUT", headers: { "content-type": "application/json" }, body }).catch(() => {
    last = "";
  });
}

/** 接菜单事件。handlers 每次渲染可以是新对象，不会反复挂监听 */
export function useMenu(handlers: Record<string, () => void>) {
  const ref = useRef(handlers);
  ref.current = handlers;
  useEffect(() => {
    const on = (e: Event) => ref.current[(e as CustomEvent<string>).detail]?.();
    window.addEventListener("subo:menu", on);
    return () => window.removeEventListener("subo:menu", on);
  }, []);
}

let deferred: string | null = null;

/** 在别的页面点了首页的动作：记下来，切到首页后由首页取走去做 */
export function deferToHome(id: string) {
  deferred = id;
}

export function takeDeferred(): string | null {
  const id = deferred;
  deferred = null;
  return id;
}
