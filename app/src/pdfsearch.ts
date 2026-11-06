import { Util, type PDFDocumentProxy } from "pdfjs-dist/legacy/build/pdf.mjs";

/** 页内一块区域，单位 bp，原点在页面左上（与 SyncTeX 的 Spot 同一套坐标） */
export type Rect = { x: number; y: number; w: number; h: number };
/** 一处命中：第几页（从 1 起），以及要盖高亮的几块（跨行、跨文字块时不止一块） */
export type Hit = { page: number; rects: Rect[] };

/** 一个文字块在页面上占的框，n 是它原文的长度（UTF-16），按字数比例切出命中的那一段 */
type Box = Rect & { n: number };
/**
 * 一页可搜的文字：去掉空白、归一化之后拼成一串，每个字记着出自第几块、块内第几个字。
 * TeX 排出来的 PDF 词间空格时有时无，忽略空白搜起来最稳；顺带也能跨行搜到。
 */
type PageText = { text: string; item: number[]; off: number[]; boxes: Box[] };

const cache = new WeakMap<PDFDocumentProxy, Promise<PageText[]>>();

/** 连字（ﬁ）、全角字母拆成普通写法，再统一小写，搜「fi」「ABC」都能命中 */
const fold = (s: string) => s.normalize("NFKC").toLowerCase().replace(/\s+/g, "");

async function extractPage(doc: PDFDocumentProxy, n: number): Promise<PageText> {
  const page = await doc.getPage(n);
  const vp = page.getViewport({ scale: 1 });
  const content = await page.getTextContent();
  const chars: string[] = [];
  const item: number[] = [];
  const off: number[] = [];
  const boxes: Box[] = [];
  for (const it of content.items) {
    if (!("str" in it) || !it.str) continue;
    const tx = Util.transform(vp.transform, it.transform);
    // 字号取变换矩阵的纵向长度；框从基线往上约一个字高
    const size = Math.hypot(tx[2], tx[3]) || it.height;
    const b = boxes.length;
    boxes.push({ x: tx[4], y: tx[5] - size * 0.85, w: it.width, h: size * 1.1, n: it.str.length });
    let k = 0;
    for (const c of it.str) {
      for (const ch of fold(c)) {
        chars.push(ch);
        item.push(b);
        off.push(k);
      }
      k += c.length;
    }
  }
  return { text: chars.join(""), item, off, boxes };
}

/** 整份文档的文字，按文档缓存：重复搜、翻来覆去搜都只抽一次 */
function extract(doc: PDFDocumentProxy): Promise<PageText[]> {
  let p = cache.get(doc);
  if (!p) {
    p = Promise.all(Array.from({ length: doc.numPages }, (_, i) => extractPage(doc, i + 1)));
    cache.set(doc, p);
    p.catch(() => cache.delete(doc));
  }
  return p;
}

/** 命中的 [a, b) 一段字，按出自哪个文字块分组，每组一块高亮 */
function rectsOf(pt: PageText, a: number, b: number): Rect[] {
  const out: Rect[] = [];
  let i = a;
  while (i < b) {
    const blk = pt.item[i];
    const k0 = pt.off[i];
    let j = i;
    while (j + 1 < b && pt.item[j + 1] === blk) j++;
    const k1 = pt.off[j] + 1;
    const box = pt.boxes[blk];
    const unit = box.w / Math.max(1, box.n);
    out.push({ x: box.x + unit * k0, y: box.y, w: Math.max(unit * (k1 - k0), 2), h: box.h });
    i = j + 1;
  }
  return out;
}

/** 在整份文档里找 query，按页序、页内先后返回每处命中；忽略大小写和空白 */
export async function searchPdf(doc: PDFDocumentProxy, query: string): Promise<Hit[]> {
  const q = fold(query);
  if (!q) return [];
  const pages = await extract(doc);
  const hits: Hit[] = [];
  pages.forEach((pt, p) => {
    for (let at = pt.text.indexOf(q); at >= 0; at = pt.text.indexOf(q, at + q.length)) {
      hits.push({ page: p + 1, rects: rectsOf(pt, at, at + q.length) });
    }
  });
  return hits;
}
