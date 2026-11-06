import { useCallback, useEffect, useRef, useState, type ClipboardEvent, type KeyboardEvent, type MouseEvent, type ReactNode } from "react";
import * as pdfjsLib from "pdfjs-dist/legacy/build/pdf.mjs";
import type { PDFDocumentProxy, RenderTask } from "pdfjs-dist/legacy/build/pdf.mjs";
import Icon from "./Icon";
import { t, useLocale } from "./i18n";
import type { Spot } from "./api";
import { searchPdf, type Hit } from "./pdfsearch";
import { addLayers, cleanCopied, linkTarget, resolveDest, type Dest } from "./pdflayers";

pdfjsLib.GlobalWorkerOptions.workerSrc = new URL(
  "pdfjs-dist/legacy/build/pdf.worker.min.mjs",
  import.meta.url
).toString();

type Props = {
  url: string | null;
  label?: string;
  /** 暂时让位给别的预览：保持挂载，PDF 不用重新加载 */
  hidden?: boolean;
  /** 正在出新预览：顶上走一条进度线，旧预览照常可看 */
  busy?: boolean;
  /** 盖在预览顶上的提示条，如「编译失败，显示的是上一版」 */
  notice?: ReactNode;
  /** 还没有预览时显示的内容 */
  empty?: ReactNode;
  /** SyncTeX 正向跳转：滚到这块区域并闪一下；n 每次递增，同一处再跳也生效 */
  mark?: (Spot & { n: number }) | null;
  /** 双击或 ⌘+点击预览：第几页、页内坐标（bp，原点左上） */
  onSync?: (page: number, x: number, y: number) => void;
};

/** fit-width 跟随栏宽；fit-page 整页放进视口；数字是相对 PDF 原始尺寸（72 dpi）的倍数 */
type Zoom = "fit-width" | "fit-page" | number;

const STEPS = [0.5, 0.67, 0.75, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2, 2.5, 3, 4];
const MIN = 0.25;
const MAX = 5;
/** 与 CSS 的 .preview-scroll 内边距、.pdf-pages 页间距一致 */
const PAD = 20;
const GAP = 16;
/**
 * 一张画布最多多少像素（同 pdf.js 的 maxCanvasPixels）。高倍缩放 × Retina 时一页能到几千万像素，
 * 附近几页一起画，WebKit 会拒绝分配、页面变白；超了就降低画布分辨率，交给 CSS 拉伸。
 */
const MAX_CANVAS_PIXELS = 16 * 1024 * 1024;

type Size = { w: number; h: number };
/** 一个锚点：视口内的位置，以及它落在第几页、页内的相对位置 */
type Anchor = { x: number; y: number; page: number; fx: number; fy: number };

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

const sizeCache = new WeakMap<PDFDocumentProxy, Promise<Size[]>>();

/**
 * 每页原始尺寸：论文里常有横排的页，不能只看第一页。
 * 按文档缓存、各页并行取：缩放、拖分栏时不必再逐页问一遍 worker。
 */
function pageSizes(doc: PDFDocumentProxy): Promise<Size[]> {
  let p = sizeCache.get(doc);
  if (!p) {
    p = Promise.all(
      Array.from({ length: doc.numPages }, async (_, i) => {
        const vp = (await doc.getPage(i + 1)).getViewport({ scale: 1 });
        return { w: vp.width, h: vp.height };
      })
    );
    sizeCache.set(doc, p);
    p.catch(() => sizeCache.delete(doc));
  }
  return p;
}

export default function Preview({ url, label, hidden, busy, notice, empty, mark, onSync }: Props) {
  useLocale();
  const scroller = useRef<HTMLDivElement>(null);
  /** 页面容器只归这里的命令式代码管，React 不往里放子节点，免得两边互删 */
  const pagesHost = useRef<HTMLDivElement>(null);
  const [doc, setDoc] = useState<PDFDocumentProxy | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [zoom, setZoom] = useState<Zoom>("fit-width");
  const [scale, setScale] = useState(1);
  const [view, setView] = useState({ w: 0, h: 0 });
  const [page, setPage] = useState(1);
  /** 缩放到与当前相同的倍率时也要重画（捏合时 CSS zoom 要收回来） */
  const [tick, setTick] = useState(0);
  /** 捏合缩放进行中的临时倍率（相对已渲染的 scale），停手后才真正重画 */
  const live = useRef(1);
  /** 下一次重画要对准的位置；缩放时是指针处，换预览时是视口左上 */
  const anchorAt = useRef<{ x: number; y: number } | null>(null);
  const scaleRef = useRef(1);
  const flash = useRef<HTMLDivElement>(null);
  const pageRef = useRef(1);
  /** 搜索：框开着没有、搜的字、命中列表、当前第几处 */
  const [finding, setFinding] = useState(false);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<Hit[]>([]);
  const [cur, setCur] = useState(0);
  const [searching, setSearching] = useState(false);
  const findInput = useRef<HTMLInputElement>(null);
  const hitsRef = useRef<{ hits: Hit[]; cur: number }>({ hits: [], cur: 0 });
  /** 点链接跳走之前的位置，可以退回去、再前进；按页内相对位置记，缩放、重新编译后照样对得上 */
  const trail = useRef<{ back: Anchor[]; fwd: Anchor[] }>({ back: [], fwd: [] });
  const [trailLen, setTrailLen] = useState({ back: 0, fwd: 0 });
  hitsRef.current = { hits, cur };

  // 1. 换预览：只加载文档，旧页面先留着，等新的画好再替换，避免闪白
  useEffect(() => {
    if (!url) return;
    let cancelled = false;
    const task = pdfjsLib.getDocument({
      url,
      // XeTeX 产出的 CID 字体需要 CMap 才能正确取字宽，缺了会整页错位
      cMapUrl: "/cmaps/",
      cMapPacked: true,
      standardFontDataUrl: "/standard_fonts/",
    });
    task.promise.then(
      (d) => {
        if (cancelled) return void d.destroy();
        setError(null);
        setDoc(d);
      },
      (e) => !cancelled && setError(String(e))
    );
    return () => {
      cancelled = true;
    };
  }, [url]);

  useEffect(() => () => void doc?.destroy(), [doc]);

  // 2. 跟踪视口尺寸（扣掉内边距）；拖动分栏时连续触发，稍等再重画
  useEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const measure = () => setView({ w: Math.max(0, el.clientWidth - 2 * PAD), h: Math.max(0, el.clientHeight - 2 * PAD) });
    // 先同步量一次：ResizeObserver 只在页面真正出帧时回调，后台标签页里可能迟迟不来
    measure();
    let t: number | undefined;
    const ro = new ResizeObserver(() => {
      window.clearTimeout(t);
      t = window.setTimeout(measure, 100);
    });
    ro.observe(el);
    return () => {
      ro.disconnect();
      window.clearTimeout(t);
    };
  }, []);

  /** 记下 (x, y) 处落在哪一页、页内什么位置；重排后靠它把同一处放回原地 */
  const captureAnchor = useCallback((x: number, y: number): Anchor | null => {
    const box = scroller.current;
    const host = pagesHost.current;
    if (!box || !host || !host.children.length) return null;
    const br = box.getBoundingClientRect();
    const slots = host.children;
    let i = 0;
    while (i < slots.length - 1 && slots[i].getBoundingClientRect().bottom < br.top + y) i++;
    const r = slots[i].getBoundingClientRect();
    return { x, y, page: i, fx: (br.left + x - r.left) / r.width, fy: (br.top + y - r.top) / r.height };
  }, []);

  // 3. 布局与渲染
  useEffect(() => {
    const host = pagesHost.current;
    const box = scroller.current;
    if (!doc || !host || !box || view.w <= 0) return;
    let cancelled = false;
    const tasks = new Map<number, RenderTask>();
    let io: IntersectionObserver | null = null;

    (async () => {
      const sizes = await pageSizes(doc);
      if (cancelled) return;
      const widest = Math.max(...sizes.map((s) => s.w));
      const s =
        zoom === "fit-width"
          ? clamp(view.w / widest, MIN, MAX)
          : zoom === "fit-page"
            ? clamp(Math.min(view.w / sizes[0].w, view.h / sizes[0].h), MIN, MAX)
            : zoom;
      const dpr = window.devicePixelRatio || 1;
      const px = sizes.map((z) => ({ w: Math.floor(z.w * s), h: Math.floor(z.h * s) }));

      // 新布局里每页的位置（与 CSS 的内边距、页间距一致），用来算该先画哪几页、滚到哪
      const contentW = Math.max(box.clientWidth, Math.max(...px.map((p) => p.w)) + 2 * PAD);
      const tops: number[] = [];
      let y = PAD;
      for (const p of px) {
        tops.push(y);
        y += p.h + GAP;
      }

      const a = anchorAt.current;
      anchorAt.current = null;
      const anchor = captureAnchor(a?.x ?? 0, a?.y ?? 0);
      let top = 0;
      let left = 0;
      if (anchor && anchor.page < px.length) {
        const p = px[anchor.page];
        top = tops[anchor.page] + anchor.fy * p.h - anchor.y;
        left = (contentW - p.w) / 2 + anchor.fx * p.w - anchor.x;
      }

      const slots = px.map((p, i) => {
        const el = document.createElement("div");
        el.className = "pdf-page";
        el.style.width = `${p.w}px`;
        el.style.height = `${p.h}px`;
        el.dataset.page = String(i + 1);
        // 原始尺寸（bp）：页面坐标与屏幕像素互换时用，不受捏合中的 CSS zoom 影响
        el.dataset.w = String(sizes[i].w);
        el.dataset.h = String(sizes[i].h);
        return el;
      });

      const draw = async (i: number) => {
        const slot = slots[i];
        if (slot.firstChild || tasks.has(i)) return;
        const pg = await doc.getPage(i + 1);
        if (cancelled) return;
        const vp = pg.getViewport({ scale: s });
        const k = Math.min(dpr, Math.sqrt(MAX_CANVAS_PIXELS / (vp.width * vp.height)));
        const canvas = document.createElement("canvas");
        canvas.width = Math.floor(vp.width * k);
        canvas.height = Math.floor(vp.height * k);
        const task = pg.render({
          canvasContext: canvas.getContext("2d")!,
          viewport: vp,
          transform: k === 1 ? undefined : [k, 0, 0, k, 0, 0],
        });
        tasks.set(i, task);
        try {
          await task.promise;
          if (!cancelled) {
            slot.replaceChildren(canvas);
            paintHits(slot, hitsRef.current.hits, hitsRef.current.cur);
            void addLayers(slot, doc, pg, vp, () => !cancelled && slot.firstChild === canvas);
          }
        } finally {
          tasks.delete(i);
        }
      };

      // 先画落在视口里的几页，画好再整体换上：不闪白，位置也不跳
      const visible = px.map((_, i) => i).filter((i) => tops[i] + px[i].h > top && tops[i] < top + box.clientHeight);
      await Promise.all(visible.map(draw));
      if (cancelled) return;

      host.style.zoom = "";
      live.current = 1;
      host.replaceChildren(...slots);
      box.scrollTop = top;
      box.scrollLeft = left;
      scaleRef.current = s;
      setScale(s);
      box.dispatchEvent(new Event("scroll"));

      // 其余页面滚到附近再画；离远了把画布扔掉，长文档的内存才压得住
      io = new IntersectionObserver(
        (entries) => {
          for (const e of entries) {
            const i = Number((e.target as HTMLElement).dataset.page) - 1;
            if (e.isIntersecting) void draw(i).catch(() => {});
            else {
              tasks.get(i)?.cancel();
              slots[i].replaceChildren();
            }
          }
        },
        { root: box, rootMargin: "150% 50%" }
      );
      slots.forEach((el) => io!.observe(el));
    })().catch((e) => {
      // 被新一轮渲染取消属于正常
      if (!cancelled && e?.name !== "RenderingCancelledException") setError(String(e));
    });

    return () => {
      cancelled = true;
      io?.disconnect();
      tasks.forEach((t) => t.cancel());
    };
  }, [doc, zoom, tick, view, captureAnchor]);

  // 当前页：视口中线落在哪页
  useEffect(() => {
    const box = scroller.current;
    const host = pagesHost.current;
    if (!box || !host) return;
    let raf = 0;
    const on = () => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => {
        const mid = box.getBoundingClientRect().top + box.clientHeight / 2;
        const slots = host.children;
        let i = 0;
        while (i < slots.length - 1 && slots[i].getBoundingClientRect().bottom < mid) i++;
        setPage(i + 1);
        pageRef.current = i + 1;
      });
    };
    box.addEventListener("scroll", on, { passive: true });
    return () => {
      box.removeEventListener("scroll", on);
      cancelAnimationFrame(raf);
    };
  }, []);

  /** 缩放到 next，对准视口内 (x, y)；不传就对准视口顶部中间 */
  const zoomTo = useCallback((next: Zoom, at?: { x: number; y: number }) => {
    const box = scroller.current;
    anchorAt.current = at ?? { x: (box?.clientWidth ?? 0) / 2, y: 0 };
    setZoom(next);
    setTick((n) => n + 1);
  }, []);

  // 捏合（触控板发的是带 ctrl 的 wheel）或 ⌘/Ctrl + 滚轮：先用 CSS zoom 立刻跟手，停手再按清晰度重画
  useEffect(() => {
    const box = scroller.current;
    const host = pagesHost.current;
    if (!box || !host) return;
    let commit: number | undefined;
    const on = (e: WheelEvent) => {
      if (!e.ctrlKey && !e.metaKey) return;
      e.preventDefault();
      if (!host.children.length) return;
      const br = box.getBoundingClientRect();
      const x = e.clientX - br.left;
      const y = e.clientY - br.top;
      const base = scaleRef.current;
      const target = clamp(base * live.current * Math.exp(-e.deltaY * 0.01), MIN, MAX);
      const a = captureAnchor(x, y);
      live.current = target / base;
      host.style.zoom = String(live.current);
      // CSS zoom 连页间距一起放大，按比例挪滚动位置就能让指针下那一点不动
      if (a) {
        const r = host.children[a.page].getBoundingClientRect();
        box.scrollTop += r.top + a.fy * r.height - (br.top + y);
        box.scrollLeft += r.left + a.fx * r.width - (br.left + x);
      }
      setScale(target);
      window.clearTimeout(commit);
      commit = window.setTimeout(() => zoomTo(Math.round(target * 100) / 100, { x, y }), 160);
    };
    box.addEventListener("wheel", on, { passive: false });
    return () => {
      box.removeEventListener("wheel", on);
      window.clearTimeout(commit);
    };
  }, [captureAnchor, zoomTo]);

  const step = (dir: 1 | -1) => {
    const cur = scale;
    const next = dir > 0 ? STEPS.find((z) => z > cur + 0.01) : [...STEPS].reverse().find((z) => z < cur - 0.01);
    if (next) zoomTo(next);
  };

  // SyncTeX 正向：把那块区域滚到视口上三分之一处，盖一层会褪掉的高亮
  useEffect(() => {
    const box = scroller.current;
    const slot = pagesHost.current?.children[(mark?.page ?? 0) - 1] as HTMLElement | undefined;
    const fl = flash.current;
    if (!mark || !box || !slot || !fl) return;
    const br = box.getBoundingClientRect();
    const r = slot.getBoundingClientRect();
    const k = r.width / Number(slot.dataset.w);
    // 以滚动内容为参照的位置，滚动途中高亮跟着页面走
    const left = r.left - br.left + box.scrollLeft + mark.x * k;
    const top = r.top - br.top + box.scrollTop + mark.y * k;
    const w = Math.max(mark.w * k, 24);
    const h = Math.max(mark.h * k, 12);
    const P = 3;
    Object.assign(fl.style, { left: `${left - P}px`, top: `${top - P}px`, width: `${w + 2 * P}px`, height: `${h + 2 * P}px` });
    fl.classList.remove("on");
    void fl.offsetWidth; // 重新触发动画
    fl.classList.add("on");
    const vis = top >= box.scrollTop && top + h <= box.scrollTop + box.clientHeight;
    const hvis = left >= box.scrollLeft && left + Math.min(w, box.clientWidth) <= box.scrollLeft + box.clientWidth;
    box.scrollTo({
      top: vis ? box.scrollTop : Math.max(0, top - box.clientHeight / 3),
      left: hvis ? box.scrollLeft : Math.max(0, left - PAD),
      behavior: "smooth",
    });
  }, [mark]);

  const sync = (e: MouseEvent) => {
    const slot = (e.target as Element).closest<HTMLElement>(".pdf-page");
    if (!slot || !onSync) return;
    e.preventDefault();
    const r = slot.getBoundingClientRect();
    const x = ((e.clientX - r.left) / r.width) * Number(slot.dataset.w);
    const y = ((e.clientY - r.top) / r.height) * Number(slot.dataset.h);
    onSync(Number(slot.dataset.page), x, y);
  };

  // 搜索：停手 150ms 再搜；换了新 PDF（重新编译）也按原来的字重搜一遍
  const jump = useRef(false);
  useEffect(() => {
    const q = finding ? query.trim() : "";
    if (!doc || !q) {
      setHits([]);
      setSearching(false);
      return;
    }
    let cancelled = false;
    setSearching(true);
    const timer = window.setTimeout(() => {
      searchPdf(doc, q).then(
        (h) => {
          if (cancelled) return;
          setSearching(false);
          setHits(h);
          if (jump.current) {
            // 新搜的字：从当前页往后找第一处，找不到再从头
            jump.current = false;
            const i = h.findIndex((x) => x.page >= pageRef.current);
            setCur(i < 0 ? 0 : i);
            if (h.length) reveal(h[i < 0 ? 0 : i]);
          } else setCur((c) => Math.min(c, Math.max(0, h.length - 1)));
        },
        () => !cancelled && setSearching(false)
      );
    }, 150);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [doc, query, finding]);

  // 命中或当前项变了：已经画出来的页重盖高亮，没画的页等画好时再盖
  useEffect(() => {
    const host = pagesHost.current;
    if (!host) return;
    for (const el of Array.from(host.children) as HTMLElement[]) {
      if (el.firstChild) paintHits(el, hits, cur);
    }
  }, [hits, cur]);

  /** 把一处命中滚进视口（已经在里面就不动） */
  const reveal = (hit: Hit) => {
    const box = scroller.current;
    const slot = pagesHost.current?.children[hit.page - 1] as HTMLElement | undefined;
    if (!box || !slot || !hit.rects.length) return;
    const br = box.getBoundingClientRect();
    const r = slot.getBoundingClientRect();
    const k = r.width / Number(slot.dataset.w);
    const rc = hit.rects[0];
    const top = r.top - br.top + box.scrollTop + rc.y * k;
    const left = r.left - br.left + box.scrollLeft + rc.x * k;
    const h = rc.h * k;
    const vis = top >= box.scrollTop + 40 && top + h <= box.scrollTop + box.clientHeight;
    const hvis = left >= box.scrollLeft && left + rc.w * k <= box.scrollLeft + box.clientWidth;
    box.scrollTo({
      top: vis ? box.scrollTop : Math.max(0, top - box.clientHeight / 3),
      left: hvis ? box.scrollLeft : Math.max(0, left - box.clientWidth / 2),
    });
  };

  const findStep = (dir: 1 | -1) => {
    if (!hits.length) return;
    const i = (cur + dir + hits.length) % hits.length;
    setCur(i);
    reveal(hits[i]);
  };

  const openFind = () => {
    setFinding(true);
    // 已经开着就选中原来的字，直接打字覆盖
    requestAnimationFrame(() => findInput.current?.select());
  };

  const closeFind = () => {
    setFinding(false);
    scroller.current?.focus();
  };

  const onKey = (e: KeyboardEvent) => {
    if ((e.metaKey || e.ctrlKey) && !e.altKey && e.key.toLowerCase() === "f" && doc) {
      // 拦下来，菜单的「查找」就不会落到编辑器上
      e.preventDefault();
      e.stopPropagation();
      openFind();
    }
    // 返回 / 前进：⌘[ ⌘]（同「预览」），或 Alt + 方向键（同浏览器）
    const back = ((e.metaKey || e.ctrlKey) && e.key === "[") || (e.altKey && e.key === "ArrowLeft");
    const fwd = ((e.metaKey || e.ctrlKey) && e.key === "]") || (e.altKey && e.key === "ArrowRight");
    if ((back || fwd) && !(e.target as Element).closest("input")) {
      e.preventDefault();
      e.stopPropagation();
      travel(back ? "back" : "fwd");
    }
  };

  const syncTrail = () => setTrailLen({ back: trail.current.back.length, fwd: trail.current.fwd.length });

  /** 把视口左上角放回某个锚点 */
  const restore = (a: Anchor) => {
    const box = scroller.current;
    const slot = pagesHost.current?.children[Math.min(a.page, (pagesHost.current?.children.length ?? 1) - 1)];
    if (!box || !slot) return;
    const br = box.getBoundingClientRect();
    const r = slot.getBoundingClientRect();
    box.scrollTop += r.top + a.fy * r.height - br.top;
    box.scrollLeft += r.left + a.fx * r.width - br.left;
  };

  /** 跳到链接目标：目标那一行贴着视口顶部（留一点边），横向不在视口里才挪 */
  const goDest = (d: Dest) => {
    const box = scroller.current;
    const slot = pagesHost.current?.children[d.page - 1] as HTMLElement | undefined;
    if (!box || !slot) return;
    const here = captureAnchor(0, 0);
    if (here) {
      trail.current.back.push(here);
      if (trail.current.back.length > 100) trail.current.back.shift();
      trail.current.fwd = [];
      syncTrail();
    }
    const br = box.getBoundingClientRect();
    const r = slot.getBoundingClientRect();
    const k = r.width / Number(slot.dataset.w);
    box.scrollTop += r.top - br.top + (d.y === null ? -PAD : d.y * k - 12);
    if (d.x !== null) {
      const x = r.left - br.left + d.x * k;
      if (x < 0 || x > box.clientWidth - 40) box.scrollLeft += x - PAD;
    }
  };

  const travel = (dir: "back" | "fwd") => {
    const from = trail.current[dir];
    const to = trail.current[dir === "back" ? "fwd" : "back"];
    const a = from.pop();
    if (!a) return;
    const here = captureAnchor(0, 0);
    if (here) to.push(here);
    restore(a);
    syncTrail();
  };

  /** 页内链接：外链交给默认行为（新窗口 → 系统浏览器），文档内的目标在这里跳 */
  const follow = (e: MouseEvent) => {
    const a = (e.target as Element).closest<HTMLAnchorElement>("a.pdf-link");
    if (!a || !doc) return false;
    const target = linkTarget(a);
    if (!target) return true;
    e.preventDefault();
    // 跳走后原来那页可能被回收，链接连同焦点一起没了；焦点放回预览，⌘[ 才能退回来
    scroller.current?.focus({ preventScroll: true });
    if (target.kind === "dest") {
      void resolveDest(doc, target.dest).then((d) => d && goDest(d));
    } else {
      const n = pageRef.current;
      const page = { NextPage: n + 1, PrevPage: n - 1, FirstPage: 1, LastPage: doc.numPages }[target.name];
      if (page && page >= 1 && page <= doc.numPages) goDest({ page, x: null, y: null });
      if (target.name === "GoBack") travel("back");
      if (target.name === "GoForward") travel("fwd");
    }
    return true;
  };

  const onPagesClick = (e: MouseEvent) => {
    if (e.metaKey || e.ctrlKey) return sync(e);
    follow(e);
  };

  // 复制：中文行尾的折行是排版产生的，去掉再放进剪贴板
  const onCopy = (e: ClipboardEvent<HTMLDivElement>) => {
    const sel = window.getSelection();
    const text = sel?.toString();
    if (!text) return;
    e.preventDefault();
    e.clipboardData.setData("text/plain", cleanCopied(text));
  };

  const goPage = (n: number) => {
    const el = pagesHost.current?.children[n - 1];
    const box = scroller.current;
    if (el && box) box.scrollTop += el.getBoundingClientRect().top - box.getBoundingClientRect().top - PAD;
  };

  return (
    <div className="preview" hidden={hidden} onKeyDown={onKey}>
      <div className="pane-bar">
        <span className="pane-title">{label ?? t("preview.label")}</span>
        {doc && (
          <span className="pdf-tools">
            <button
              className={`btn btn-icon btn-sm btn-ghost${finding ? " on" : ""}`}
              onClick={() => (finding ? closeFind() : openFind())}
              title={t("preview.findTitle")}
              aria-label={t("preview.find")}
              aria-pressed={finding}
            >
              <Icon name="search" size={14} />
            </button>
            {(trailLen.back > 0 || trailLen.fwd > 0) && (
              <>
                <button className="btn btn-icon btn-sm btn-ghost" onClick={() => travel("back")} disabled={!trailLen.back} title={t("preview.backTitle")} aria-label={t("preview.back")}>
                  <Icon name="back" size={14} />
                </button>
                <button className="btn btn-icon btn-sm btn-ghost" onClick={() => travel("fwd")} disabled={!trailLen.fwd} title={t("preview.forwardTitle")} aria-label={t("preview.forward")}>
                  <Icon name="forward" size={14} />
                </button>
              </>
            )}
            <PageBox page={page} total={doc.numPages} onGo={goPage} />
            <span className="tool-sep" />
            <button className="btn btn-icon btn-sm btn-ghost" onClick={() => step(-1)} title={t("preview.zoomOutTitle")} aria-label={t("preview.zoomOut")}>
              <Icon name="minus" size={14} />
            </button>
            <button className="zoom-value" onClick={() => zoomTo(1)} title={t("preview.zoomReset")}>
              {Math.round(scale * 100)}%
            </button>
            <button className="btn btn-icon btn-sm btn-ghost" onClick={() => step(1)} title={t("preview.zoomIn")} aria-label={t("preview.zoomIn")}>
              <Icon name="plus" size={14} />
            </button>
            <span className="tool-sep" />
            <button
              className={`btn btn-icon btn-sm btn-ghost${zoom === "fit-width" ? " on" : ""}`}
              onClick={() => zoomTo("fit-width")}
              title={t("preview.fitWidth")}
              aria-label={t("preview.fitWidth")}
              aria-pressed={zoom === "fit-width"}
            >
              <Icon name="fitWidth" size={15} />
            </button>
            <button
              className={`btn btn-icon btn-sm btn-ghost${zoom === "fit-page" ? " on" : ""}`}
              onClick={() => zoomTo("fit-page")}
              title={t("preview.fitPage")}
              aria-label={t("preview.fitPage")}
              aria-pressed={zoom === "fit-page"}
            >
              <Icon name="fitPage" size={15} />
            </button>
          </span>
        )}
      </div>
      <div className="preview-stage">
        {busy && <div className="progress-line" />}
        {notice && <div className="preview-notice">{notice}</div>}
        {finding && doc && (
          <div className="pdf-find" role="search">
            <Icon name="search" size={14} />
            <input
              ref={findInput}
              autoFocus
              value={query}
              placeholder={t("preview.findPlaceholder")}
              aria-label={t("preview.find")}
              onChange={(e) => {
                jump.current = true;
                setQuery(e.target.value);
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  findStep(e.shiftKey ? -1 : 1);
                }
                if (e.key === "Escape") {
                  e.preventDefault();
                  closeFind();
                }
              }}
            />
            <span className={`pdf-find-count${query.trim() && !searching && !hits.length ? " none" : ""}`}>
              {!query.trim()
                ? ""
                : searching
                  ? t("preview.findSearching")
                  : hits.length
                    ? t("preview.findCount", { cur: cur + 1, total: hits.length })
                    : t("preview.findNone")}
            </span>
            <button className="btn btn-icon btn-sm btn-ghost" onClick={() => findStep(-1)} disabled={!hits.length} title={t("preview.findPrev")} aria-label={t("preview.findPrev")}>
              <Icon name="chevronDown" size={14} className="up" />
            </button>
            <button className="btn btn-icon btn-sm btn-ghost" onClick={() => findStep(1)} disabled={!hits.length} title={t("preview.findNext")} aria-label={t("preview.findNext")}>
              <Icon name="chevronDown" size={14} />
            </button>
            <button className="btn btn-icon btn-sm btn-ghost" onClick={closeFind} title={t("preview.findClose")} aria-label={t("preview.findClose")}>
              <Icon name="x" size={14} />
            </button>
          </div>
        )}
        <div className="preview-scroll" ref={scroller} tabIndex={-1}>
          {!url && !doc && <div className="preview-empty">{empty ?? t("preview.empty")}</div>}
          {error && <div className="preview-error">{error}</div>}
          <div
            className="pdf-pages"
            ref={pagesHost}
            onDoubleClick={(e) => !(e.target as Element).closest("a.pdf-link") && sync(e)}
            onClick={onPagesClick}
            onCopy={onCopy}
            // 鼠标侧键「后退 / 前进」
            onMouseUp={(e) => (e.button === 3 || e.button === 4) && travel(e.button === 3 ? "back" : "fwd")}
          />
          <div className="sync-flash" ref={flash} onAnimationEnd={(e) => e.currentTarget.classList.remove("on")} />
        </div>
      </div>
    </div>
  );
}

/** 在一页上盖搜索高亮：先清掉旧的，按页面原始尺寸的百分比摆，缩放时跟着页面走 */
function paintHits(slot: HTMLElement, hits: Hit[], cur: number) {
  slot.querySelector(".pdf-hits")?.remove();
  const page = Number(slot.dataset.page);
  const W = Number(slot.dataset.w);
  const H = Number(slot.dataset.h);
  let layer: HTMLDivElement | null = null;
  hits.forEach((hit, i) => {
    if (hit.page !== page) return;
    layer ??= document.createElement("div");
    for (const r of hit.rects) {
      const el = document.createElement("div");
      el.className = i === cur ? "pdf-hit cur" : "pdf-hit";
      Object.assign(el.style, {
        left: `${(r.x / W) * 100}%`,
        top: `${(r.y / H) * 100}%`,
        width: `${(r.w / W) * 100}%`,
        height: `${(r.h / H) * 100}%`,
      });
      layer.append(el);
    }
  });
  if (layer) {
    (layer as HTMLDivElement).className = "pdf-hits";
    slot.append(layer);
  }
}

/** 「3 / 36」，点数字可以直接输入页码跳过去 */
function PageBox({ page, total, onGo }: { page: number; total: number; onGo: (n: number) => void }) {
  const [edit, setEdit] = useState<string | null>(null);
  if (edit !== null) {
    return (
      <input
        className="page-input"
        autoFocus
        value={edit}
        inputMode="numeric"
        aria-label={t("preview.gotoPage")}
        onChange={(e) => setEdit(e.target.value.replace(/\D/g, ""))}
        onBlur={() => setEdit(null)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            const n = Number(edit);
            if (n >= 1 && n <= total) onGo(n);
            setEdit(null);
          }
          if (e.key === "Escape") setEdit(null);
        }}
      />
    );
  }
  return (
    <button className="page-box" onClick={() => setEdit(String(page))} title={t("preview.pageTitle")}>
      <span className="num">{page}</span>
      <span className="of">/ {total}</span>
    </button>
  );
}
