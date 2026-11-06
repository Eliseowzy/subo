import { useCallback, useEffect, useState, type KeyboardEvent as ReactKeyboardEvent, type PointerEvent as ReactPointerEvent } from "react";
import { t } from "./i18n";

/** 目录栏宽度用像素；编辑器与预览按比例分剩下的宽度，窗口缩放时两边一起伸缩；日志高度用像素 */
export type Layout = { tree: number; ratio: number; log: number };

const KEY = "subo.layout.v1";
const DEFAULT: Layout = { tree: 220, ratio: 0.48, log: 200 };
export const TREE_MIN = 140;
export const TREE_MAX = 480;
/** 编辑器、预览各自至少留这么宽 */
export const PANE_MIN = 260;
export const LOG_MIN = 80;
/** 日志再高也要给顶栏和编辑区留出这么多 */
export const LOG_ROOM = 220;
/** 与 .gutter 的宽度一致 */
export const GUTTER = 5;

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

function load(): Layout {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return DEFAULT;
    const v = JSON.parse(raw) as Partial<Layout>;
    return {
      tree: clamp(Number(v.tree) || DEFAULT.tree, TREE_MIN, TREE_MAX),
      ratio: clamp(Number(v.ratio) || DEFAULT.ratio, 0.15, 0.85),
      log: Math.max(LOG_MIN, Number(v.log) || DEFAULT.log),
    };
  } catch {
    return DEFAULT;
  }
}

/** 布局只是本机偏好：存不进去（隐私模式等）就用默认值，不影响使用 */
export function useLayout() {
  const [layout, setLayout] = useState<Layout>(load);
  useEffect(() => {
    try {
      localStorage.setItem(KEY, JSON.stringify(layout));
    } catch {
      /* 忽略 */
    }
  }, [layout]);
  const reset = useCallback((part: keyof Layout) => {
    setLayout((l) => ({ ...l, [part]: DEFAULT[part] }));
  }, []);
  return { layout, setLayout, reset };
}

type GutterProps = {
  /** 按下时记录起点，返回值随拖动距离 dx 计算新布局 */
  onStart: () => (dx: number) => void;
  onReset: () => void;
  title: string;
  /** x：左右拖、调宽度（默认）；y：上下拖、调高度，回调收到的是纵向位移 */
  axis?: "x" | "y";
};

/** 开始拖分栏时在 window 上广播：preventDefault 拦住了焦点转移，原地改名之类的编辑状态靠它收起 */
export const LAYOUT_DRAG = "subo:layout-drag";

/** 键盘挪拖动条：一下 16px，按住 Shift 一下 64px */
const KEY_STEP = 16;

/** 两栏之间的拖动条：指针捕获保证拖出条外也不断；拖动期间禁止选中文字。
 *  拖不了的时候用键盘：Tab 到条上，方向键挪，回车恢复默认（同双击） */
export function Gutter({ onStart, onReset, title, axis = "x" }: GutterProps) {
  const [active, setActive] = useState(false);

  const down = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    window.dispatchEvent(new Event(LAYOUT_DRAG));
    const el = e.currentTarget;
    el.setPointerCapture(e.pointerId);
    const pos = (ev: { clientX: number; clientY: number }) => (axis === "x" ? ev.clientX : ev.clientY);
    const p0 = pos(e);
    const apply = onStart();
    const cls = axis === "x" ? "resizing" : "resizing-y";
    setActive(true);
    document.body.classList.add(cls);

    const move = (ev: PointerEvent) => apply(pos(ev) - p0);
    const up = () => {
      setActive(false);
      document.body.classList.remove(cls);
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      el.removeEventListener("pointercancel", up);
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
    el.addEventListener("pointercancel", up);
  };

  const key = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Enter") {
      e.preventDefault();
      onReset();
      return;
    }
    const dir = axis === "x" ? { ArrowLeft: -1, ArrowRight: 1 }[e.key] : { ArrowUp: -1, ArrowDown: 1 }[e.key];
    if (!dir) return;
    e.preventDefault();
    onStart()(dir * (e.shiftKey ? KEY_STEP * 4 : KEY_STEP));
  };

  return (
    <div
      className={`gutter${axis === "y" ? " gutter-y" : ""}${active ? " active" : ""}`}
      role="separator"
      tabIndex={0}
      aria-orientation={axis === "x" ? "vertical" : "horizontal"}
      aria-label={title}
      title={t("split.gutterTitle", { title })}
      onPointerDown={down}
      onDoubleClick={onReset}
      onKeyDown={key}
    />
  );
}
