import { t } from "./i18n";
import { TextLayer, type PDFDocumentProxy, type PDFPageProxy } from "pdfjs-dist/legacy/build/pdf.mjs";

type PageViewport = ReturnType<PDFPageProxy["getViewport"]>;

/** 链接要去的地方：页内某处（bp，原点左上；没给坐标就是页顶/原横向位置），或文档自带的翻页动作 */
export type Dest = { page: number; x: number | null; y: number | null };
type Target = { kind: "dest"; dest: string | unknown[] } | { kind: "action"; name: string };

/** 页内链接元素 → 它指向哪；放在旁边而不是塞进 DOM 属性，显式目标里有对象引用 */
const targets = new WeakMap<Element, Target>();

export const linkTarget = (el: Element) => targets.get(el);

/**
 * 给画好的一页盖上文字层（透明文字，用来拖选、复制）和链接层。
 * 都按页面原始尺寸的百分比或 --scale-factor 摆放，捏合时随 CSS zoom 一起缩放。
 * alive() 为假说明这页已经被换掉或扔掉，别再往里塞东西。
 */
export async function addLayers(
  slot: HTMLElement,
  doc: PDFDocumentProxy,
  page: PDFPageProxy,
  vp: PageViewport,
  alive: () => boolean
) {
  slot.style.setProperty("--scale-factor", String(vp.scale));
  const [text, links] = await Promise.allSettled([textLayer(page, vp), linkLayer(doc, page)]);
  if (!alive()) return;
  // 文字层在链接层下面：链接上照样能点，链接外拖选文字
  if (text.status === "fulfilled") slot.append(text.value);
  if (links.status === "fulfilled" && links.value) slot.append(links.value);
}

async function textLayer(page: PDFPageProxy, vp: PageViewport) {
  const div = document.createElement("div");
  div.className = "textLayer";
  const layer = new TextLayer({ textContentSource: page.streamTextContent({ includeMarkedContent: true }), container: div, viewport: vp });
  await layer.render();
  // 拖选途中把「内容末尾」垫到整层底下：指针扫过空白处时选区不会跳到页首
  const end = document.createElement("div");
  end.className = "endOfContent";
  div.append(end);
  div.addEventListener("mousedown", () => {
    div.classList.add("selecting");
    window.addEventListener("mouseup", () => div.classList.remove("selecting"), { once: true });
  });
  return div;
}

type LinkAnnot = {
  subtype: string;
  rect: number[];
  url?: string;
  dest?: string | unknown[];
  action?: string;
};

async function linkLayer(doc: PDFDocumentProxy, page: PDFPageProxy) {
  const annots = (await page.getAnnotations({ intent: "display" })) as LinkAnnot[];
  const vp = page.getViewport({ scale: 1 });
  let layer: HTMLDivElement | null = null;
  for (const a of annots) {
    if (a.subtype !== "Link") continue;
    const target: Target | null = a.dest ? { kind: "dest", dest: a.dest } : a.action ? { kind: "action", name: a.action } : null;
    if (!a.url && !target) continue;
    const [x1, y1, x2, y2] = vp.convertToViewportRectangle(a.rect);
    const el = document.createElement("a");
    el.className = "pdf-link";
    Object.assign(el.style, {
      left: `${(Math.min(x1, x2) / vp.width) * 100}%`,
      top: `${(Math.min(y1, y2) / vp.height) * 100}%`,
      width: `${(Math.abs(x2 - x1) / vp.width) * 100}%`,
      height: `${(Math.abs(y2 - y1) / vp.height) * 100}%`,
    });
    if (a.url) {
      // 外链交给系统浏览器：桌面壳拦下新窗口请求转给系统去开，浏览器里就是新标签页
      el.href = a.url;
      el.target = "_blank";
      el.rel = "noopener noreferrer";
      el.title = a.url;
    } else {
      el.href = "#";
      targets.set(el, target!);
      if (target!.kind === "dest") {
        void resolveDest(doc, target!.dest).then((d) => d && (el.title = t("preview.linkPage", { page: d.page })));
      }
    }
    layer ??= document.createElement("div");
    layer.append(el);
  }
  if (layer) layer.className = "pdf-links";
  return layer;
}

/** 把命名目标或显式目标换成「第几页、页内哪一点」 */
export async function resolveDest(doc: PDFDocumentProxy, dest: string | unknown[]): Promise<Dest | null> {
  const explicit = typeof dest === "string" ? await doc.getDestination(dest) : dest;
  if (!Array.isArray(explicit) || !explicit.length) return null;
  const [ref, kind, ...args] = explicit as [unknown, { name?: string } | undefined, ...(number | null)[]];
  let index: number;
  if (typeof ref === "number") index = ref;
  else if (ref && typeof ref === "object") index = await doc.getPageIndex(ref as Parameters<PDFDocumentProxy["getPageIndex"]>[0]);
  else return null;
  if (index < 0 || index >= doc.numPages) return null;
  const pg = await doc.getPage(index + 1);
  const vp = pg.getViewport({ scale: 1 });
  let left: number | null = null;
  let top: number | null = null;
  switch (kind?.name) {
    case "XYZ":
      left = args[0] ?? null;
      top = args[1] ?? null;
      break;
    case "FitH":
    case "FitBH":
      top = args[0] ?? null;
      break;
    case "FitV":
    case "FitBV":
      left = args[0] ?? null;
      break;
    case "FitR":
      left = args[0] ?? null;
      top = args[3] ?? null;
      break;
  }
  // PDF 用户空间原点在左下，换成页面左上为原点；缺的那一维保持 null
  const [px, py] = vp.convertToViewportPoint(left ?? 0, top ?? 0);
  return {
    page: index + 1,
    x: left === null ? null : Math.max(0, px),
    y: top === null ? null : Math.max(0, py),
  };
}

const CJK = "\\p{Script=Han}\\p{Script=Hiragana}\\p{Script=Katakana}\\p{Script=Hangul}\\u3000-\\u303f\\uff00-\\uffef";
const CJK_BREAK = new RegExp(`([${CJK}])\\s*\\n\\s*(?=[${CJK}])`, "gu");

/** 复制出来的字：中日韩文字之间的折行是排版折出来的，不是原文换行，去掉 */
export function cleanCopied(s: string) {
  return s.replace(/\0/g, "").replace(CJK_BREAK, "$1");
}
