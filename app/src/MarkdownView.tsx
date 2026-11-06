// Markdown 预览：编辑 .md 时右栏显示排好的样子，随打随看。
// 按顶层块切开渲染，每块记下源码起始行：光标走到哪里预览跟到哪里，双击预览回到源码。

import { useDeferredValue, useEffect, useMemo, useRef, type MouseEvent } from "react";
import { marked, type Tokens, type TokensList } from "marked";
import DOMPurify from "dompurify";
import { t, useLocale } from "./i18n";

type Props = {
  path: string;
  text: string;
  /** 项目内文件的原始地址，给相对路径的图片用 */
  rawUrl: (path: string) => string;
  /** 点了指向项目内文件的链接 / 双击某块：打开源码并跳到该行 */
  onOpen: (path: string, line?: number) => void;
  /** 光标所在行；预览跟过去 */
  line?: number;
  /** 「定位」按钮：每按一次加一，强制滚到光标所在处并闪一下 */
  locate?: number;
};

/** 把 a/b/../c 规整成 a/c；跳出项目根的返回 null */
function resolve(from: string, rel: string): string | null {
  const parts = from.split("/").slice(0, -1);
  for (const seg of rel.split("/")) {
    if (seg === "" || seg === ".") continue;
    if (seg === "..") {
      if (!parts.length) return null;
      parts.pop();
    } else parts.push(seg);
  }
  return parts.join("/");
}

const EXTERNAL = /^[a-z][a-z0-9+.-]*:/i;

const lines = (s: string) => (s.match(/\n/g) ?? []).length;

/** 切块渲染：每个顶层块包一层带 data-line 的 div。
 *  列表常常一口气十几行，再记下每一项的起始行（items），定位才能落到具体某一项 */
function render(text: string): { html: string; items: number[][] } {
  const tokens = marked.lexer(text, { gfm: true });
  let line = 1;
  let html = "";
  const items: number[][] = [];
  for (const tok of tokens) {
    const start = line;
    line += lines(tok.raw);
    if (tok.type === "space") continue;
    const one = [tok] as unknown as TokensList;
    one.links = tokens.links;
    html += `<div class="md-block" data-line="${start}">${marked.parser(one, { gfm: true })}</div>`;
    const at: number[] = [];
    if (tok.type === "list") {
      let l = start;
      for (const it of (tok as Tokens.List).items) {
        at.push(l);
        l += lines(it.raw);
      }
    }
    items.push(at);
  }
  return { html, items };
}

/** 光标行落在哪一块（列表细到某一项）。块按文档顺序排列，行号单调递增 */
function blockAt(el: HTMLElement, line: number): HTMLElement | null {
  let hit: HTMLElement | null = null;
  for (const b of el.querySelectorAll<HTMLElement>("[data-line]")) {
    if (Number(b.dataset.line) <= line) hit = b;
    else break;
  }
  return hit;
}

export default function MarkdownView({ path, text, rawUrl, onOpen, line, locate }: Props) {
  useLocale();
  const deferred = useDeferredValue(text);
  const box = useRef<HTMLDivElement>(null);
  const body = useRef<HTMLDivElement>(null);

  // 导入的项目里 README 可能夹带 <script>、onerror：一律过一遍净化
  const out = useMemo(() => {
    const r = render(deferred);
    return { html: DOMPurify.sanitize(r.html), items: r.items };
  }, [deferred]);
  const html = out.html;

  // 净化后再改写：相对图片指向项目文件，外链新窗口打开
  useEffect(() => {
    const el = body.current;
    if (!el) return;
    el.innerHTML = html;
    // 列表项补上各自的起始行；净化前写进去会被当成用户内容，所以放在这里
    el.querySelectorAll<HTMLElement>(":scope > .md-block").forEach((b, i) => {
      const at = out.items[i];
      if (!at?.length) return;
      b.querySelectorAll<HTMLElement>(":scope > ul > li, :scope > ol > li").forEach((li, j) => {
        if (at[j] !== undefined) li.dataset.line = String(at[j]);
      });
    });
    el.querySelectorAll("img").forEach((img) => {
      const src = img.getAttribute("src") ?? "";
      if (!src || EXTERNAL.test(src) || src.startsWith("//")) return;
      const p = resolve(path, decodeURI(src.split(/[?#]/)[0]));
      if (p) img.src = rawUrl(p);
    });
    el.querySelectorAll("a[href]").forEach((a) => {
      const href = a.getAttribute("href") ?? "";
      if (EXTERNAL.test(href)) {
        a.setAttribute("target", "_blank");
        a.setAttribute("rel", "noopener noreferrer");
        a.setAttribute("title", href);
      }
    });
  }, [html, out, path, rawUrl]);

  // 光标行变了：把对应块滚进视野（已在视野里就不动，免得打字时预览乱跳）
  useEffect(() => {
    const el = body.current;
    const sc = box.current;
    if (!el || !sc || !line) return;
    const hit = blockAt(el, line);
    if (!hit) return;
    const r = hit.getBoundingClientRect();
    const br = sc.getBoundingClientRect();
    if (r.top < br.top || r.top > br.bottom - 40) {
      sc.scrollTop += r.top - br.top - br.height / 3;
    }
    el.querySelectorAll(".md-on").forEach((b) => b.classList.remove("md-on"));
    hit.classList.add("md-on");
  }, [line, html]);

  // 按了「定位」：不管在不在视野里都滚到中间，再闪一下
  useEffect(() => {
    const el = body.current;
    const sc = box.current;
    if (!el || !sc || !locate || !line) return;
    const hit = blockAt(el, line);
    if (!hit) return;
    const r = hit.getBoundingClientRect();
    const br = sc.getBoundingClientRect();
    sc.scrollTo({ top: sc.scrollTop + r.top - br.top - (br.height - r.height) / 2, behavior: "smooth" });
    hit.classList.remove("md-flash");
    void hit.offsetWidth; // 重新触发动画
    hit.classList.add("md-flash");
    // 只要 locate 变化时触发；line 用最新值即可
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [locate]);

  const onClick = (e: MouseEvent) => {
    const a = (e.target as HTMLElement).closest("a");
    if (!a) return;
    const href = a.getAttribute("href") ?? "";
    if (EXTERNAL.test(href)) return; // 交给浏览器（target=_blank）
    e.preventDefault();
    if (href.startsWith("#")) {
      const id = decodeURIComponent(href.slice(1));
      const target = body.current?.querySelector(`[id="${CSS.escape(id)}"]`) ?? findHeading(id);
      target?.scrollIntoView({ block: "start", behavior: "smooth" });
      return;
    }
    const p = resolve(path, decodeURI(href.split(/[?#]/)[0]));
    if (p) onOpen(p);
  };

  /** marked 不给标题加 id：按 GitHub 的规则现算 */
  const findHeading = (id: string) =>
    [...(body.current?.querySelectorAll("h1,h2,h3,h4,h5,h6") ?? [])].find((h) => slug(h.textContent ?? "") === id);

  const onDoubleClick = (e: MouseEvent) => {
    const b = (e.target as HTMLElement).closest<HTMLElement>("[data-line]");
    if (b) onOpen(path, Number(b.dataset.line));
  };

  return (
    <div className="preview md-preview">
      <div className="pane-bar">
        <span className="pane-title">{t("mdPreview.label")}</span>
      </div>
      <div className="md-scroll" ref={box} title={t("mdPreview.hint")}>
        {deferred.trim() ? (
          <div className="md-body" ref={body} onClick={onClick} onDoubleClick={onDoubleClick} />
        ) : (
          <div className="preview-empty">{t("mdPreview.empty")}</div>
        )}
      </div>
    </div>
  );
}

function slug(s: string) {
  return s
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s_-]/gu, "")
    .replace(/\s/g, "-");
}

export const isMarkdown = (path: string | null) => !!path && /\.(md|markdown|mdown|mkd)$/i.test(path);
