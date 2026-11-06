// 应用自己的右键 / 弹出菜单：文件树的项目菜单、编辑器的右键菜单都用它。
// 固定定位在点击处，贴边时往里收。系统 WebView 自带的右键菜单由 installContextMenuGuard 拦掉，
// 各平台（macOS WKWebView、Windows WebView2、Linux WebKitGTK）看到的都是这一份。

import { useEffect, useLayoutEffect, useRef, type KeyboardEvent } from "react";
import { createPortal } from "react-dom";
import Icon, { type IconName } from "./Icon";
import { keyLabel } from "./i18n";

export type MenuItem = {
  icon?: IconName;
  label: string;
  /** 快捷键提示，按 Mac 写法记，其他平台由 keyLabel 换成 Ctrl+ 等 */
  shortcut?: string;
  /** 不可用时的原因，挂在 title 上；true 表示只是灰掉、无需解释 */
  disabled?: string | boolean;
  /** 单选菜单（下拉框）里的当前项；给了就按单选项读，打开时焦点落在它上面 */
  checked?: boolean;
  run: () => void;
};

/** 分隔线 */
export const SEP = "-" as const;
export type MenuEntry = MenuItem | typeof SEP;

type Props = {
  x: number;
  y: number;
  items: MenuEntry[];
  /** 读屏用的菜单名 */
  label: string;
  /** 至少这么宽：从下拉按钮打开时和按钮对齐 */
  minWidth?: number;
  /** end：x 是菜单右边缘（从靠右的按钮往左展开） */
  align?: "start" | "end";
  onClose: () => void;
};

/** 离视口边缘至少留这么多 */
const EDGE = 8;

/** 去掉开头、结尾和连续的分隔线：条件项被过滤掉后不会留下空段 */
function tidy(items: MenuEntry[]): MenuEntry[] {
  const out: MenuEntry[] = [];
  for (const it of items) if (it !== SEP || (out.length && out[out.length - 1] !== SEP)) out.push(it);
  if (out[out.length - 1] === SEP) out.pop();
  return out;
}

export default function ContextMenu({ x, y, items, label, minWidth, align, onClose }: Props) {
  const box = useRef<HTMLDivElement>(null);
  const entries = tidy(items);
  // 有一项带图标，其余项留出同宽的空位对齐；都没图标就不留
  const iconCol = entries.some((it) => it !== SEP && it.icon);

  // 先按点击处放，量出尺寸后再收进视口
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    const { width, height } = el.getBoundingClientRect();
    const left = align === "end" ? x - width : x;
    el.style.left = `${Math.max(EDGE, Math.min(left, window.innerWidth - width - EDGE))}px`;
    el.style.top = `${Math.max(EDGE, Math.min(y, window.innerHeight - height - EDGE))}px`;
    // 当前项（打勾的）优先拿焦点；不让聚焦把外面的滚动区带着滚，滚动会收起菜单
    const first = el.querySelector<HTMLButtonElement>(".menu-item[aria-checked=true]") ?? el.querySelector<HTMLButtonElement>(".menu-item:not(:disabled)");
    first?.focus({ preventScroll: true });
  }, [x, y, align]);

  // 点外面、Esc、滚动、窗口变化都收起。pointerdown 用冒泡阶段，打开菜单的按钮自己拦下，好做开关
  useEffect(() => {
    const down = (e: PointerEvent) => !box.current?.contains(e.target as Node) && onClose();
    // Esc 只收菜单：捕获阶段拦下，别让设置页之类的「Esc 返回」也跟着触发
    const key = (e: globalThis.KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopImmediatePropagation();
      e.preventDefault();
      onClose();
    };
    window.addEventListener("pointerdown", down);
    window.addEventListener("keydown", key, true);
    window.addEventListener("resize", onClose);
    window.addEventListener("blur", onClose);
    window.addEventListener("scroll", onClose, true);
    return () => {
      window.removeEventListener("pointerdown", down);
      window.removeEventListener("keydown", key, true);
      window.removeEventListener("resize", onClose);
      window.removeEventListener("blur", onClose);
      window.removeEventListener("scroll", onClose, true);
    };
  }, [onClose]);

  // 上下键在可用项之间走
  const nav = (e: KeyboardEvent) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const all = [...(box.current?.querySelectorAll<HTMLButtonElement>(".menu-item:not(:disabled)") ?? [])];
    if (!all.length) return;
    const at = all.indexOf(document.activeElement as HTMLButtonElement);
    const step = e.key === "ArrowDown" ? 1 : -1;
    all[(at + step + all.length) % all.length].focus();
  };

  // 挂到 body 上：放在带滚动或 transform 的容器里时，fixed 定位会跟着容器走
  return createPortal(
    <div
      ref={box}
      className="menu menu-float"
      role="menu"
      aria-label={label}
      style={{ left: x, top: y, minWidth }}
      onKeyDown={nav}
      onContextMenu={(e) => e.preventDefault()}
      // 菜单里按下不抢编辑器的焦点和选区
      onMouseDown={(e) => e.preventDefault()}
    >
      {entries.map((it, i) =>
        it === SEP ? (
          <div key={`sep-${i}`} className="menu-sep" role="separator" />
        ) : (
          <button
            key={it.label}
            className="menu-item"
            role={it.checked === undefined ? "menuitem" : "menuitemradio"}
            aria-checked={it.checked}
            disabled={!!it.disabled}
            title={typeof it.disabled === "string" ? it.disabled : undefined}
            onClick={() => {
              onClose();
              it.run();
            }}
          >
            {it.icon ? <Icon name={it.icon} size={15} /> : iconCol && <span className="menu-icon-spacer" />}
            <span className="menu-label">{it.label}</span>
            {it.shortcut && <kbd className="menu-shortcut">{keyLabel(it.shortcut)}</kbd>}
          </button>
        )
      )}
    </div>,
    document.body
  );
}

/**
 * 全局拦掉 WebView 自带的右键菜单（查询、翻译、服务、自动填充……那一套）。
 * 需要菜单的地方自己处理 contextmenu 并 preventDefault；输入框保留系统菜单，方便复制粘贴。
 * 开发构建里按住 ⇧ 右键仍出系统菜单，好用「检查元素」。
 */
export function installContextMenuGuard() {
  const onMenu = (e: MouseEvent) => {
    if (e.defaultPrevented) return;
    if (import.meta.env.DEV && e.shiftKey) return;
    const el = e.target as HTMLElement | null;
    if (el?.closest("input, textarea, [contenteditable=true]:not(.cm-content)")) return;
    e.preventDefault();
  };
  window.addEventListener("contextmenu", onMenu);
  return () => window.removeEventListener("contextmenu", onMenu);
}
