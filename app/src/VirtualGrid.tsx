import { Fragment, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";

/** 往上找第一个会滚动的祖先；找不到就是整页 */
function scrollParent(el: HTMLElement | null): HTMLElement | null {
  for (let p = el?.parentElement; p; p = p.parentElement) {
    const { overflowY } = getComputedStyle(p);
    if (overflowY === "auto" || overflowY === "scroll") return p;
  }
  return null;
}

/**
 * 卡片网格，按行虚拟化：只画视口上下几行，几千项也不卡（字体卡片滚到才加载字体）。
 * 列数照 `repeat(auto-fill, minmax(minCol, 1fr))` 的算法自己算；行高取已画过的行里最高的，
 * 每行至少这么高，卡片拉伸对齐。
 */
export default function VirtualGrid<T>({
  items,
  itemKey,
  render,
  className = "",
  minCol = 210,
  gap = 10,
  overscan = 3,
}: {
  items: T[];
  itemKey: (x: T) => string;
  render: (x: T) => ReactNode;
  className?: string;
  minCol?: number;
  gap?: number;
  overscan?: number;
}) {
  const box = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  const [rowH, setRowH] = useState(110);
  const [view, setView] = useState({ top: 0, height: 800 });

  // 宽度变了列数跟着变
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  // 滚动、窗口高度变化：记下网格顶端相对视口的位置
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const parent = scrollParent(el);
    const target: HTMLElement | Window = parent ?? window;
    let raf = 0;
    const update = () => {
      raf = 0;
      const top = el.getBoundingClientRect().top - (parent ? parent.getBoundingClientRect().top : 0);
      const height = parent ? parent.clientHeight : window.innerHeight;
      setView((v) => (v.top === top && v.height === height ? v : { top, height }));
    };
    const schedule = () => {
      if (!raf) raf = requestAnimationFrame(update);
    };
    update();
    target.addEventListener("scroll", schedule, { passive: true });
    window.addEventListener("resize", schedule);
    const ro = parent ? new ResizeObserver(schedule) : null;
    ro?.observe(parent!);
    return () => {
      if (raf) cancelAnimationFrame(raf);
      target.removeEventListener("scroll", schedule);
      window.removeEventListener("resize", schedule);
      ro?.disconnect();
    };
  }, []);

  // 量已画出的行，行高只增不减（避免来回跳）
  const measure = useRef<ResizeObserver | null>(null);
  if (!measure.current && typeof ResizeObserver !== "undefined") {
    measure.current = new ResizeObserver((entries) => {
      let max = 0;
      for (const e of entries) max = Math.max(max, (e.target as HTMLElement).offsetHeight);
      if (max) setRowH((h) => (max > h ? max : h));
    });
  }
  useEffect(() => () => measure.current?.disconnect(), []);

  const cols = Math.max(1, Math.floor((width + gap) / (minCol + gap)));
  const rows = Math.ceil(items.length / cols);
  const stride = rowH + gap;
  const first = Math.max(0, Math.floor(-view.top / stride) - overscan);
  const last = Math.min(rows - 1, Math.ceil((view.height - view.top) / stride) + overscan);

  const lines: ReactNode[] = [];
  for (let r = first; r <= last; r++) {
    const slice = items.slice(r * cols, r * cols + cols);
    lines.push(
      <Row key={r} observer={measure.current} className={`card-grid ${className}`} top={r * stride} minHeight={rowH} cols={cols}>
        {slice.map((x) => (
          <Fragment key={itemKey(x)}>{render(x)}</Fragment>
        ))}
      </Row>,
    );
  }

  return (
    <div ref={box} style={{ position: "relative", height: rows ? rows * stride - gap : 0 }}>
      {lines}
    </div>
  );
}

function Row({
  observer,
  className,
  top,
  minHeight,
  cols,
  children,
}: {
  observer: ResizeObserver | null;
  className: string;
  top: number;
  minHeight: number;
  cols: number;
  children: ReactNode;
}) {
  const el = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const node = el.current;
    if (!node || !observer) return;
    observer.observe(node);
    return () => observer.unobserve(node);
  }, [observer]);
  return (
    <div
      ref={el}
      className={className}
      style={{
        position: "absolute",
        left: 0,
        right: 0,
        top,
        minHeight,
        gridTemplateColumns: `repeat(${cols}, minmax(0, 1fr))`,
      }}
    >
      {children}
    </div>
  );
}
