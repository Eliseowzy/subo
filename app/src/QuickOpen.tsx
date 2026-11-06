import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import Icon, { fileIcon } from "./Icon";
import { t, useLocale } from "./i18n";
import { Kbd, useDialog } from "./ui";

type Props = {
  files: string[];
  /** 最近打开过的排在前面 */
  recent: string[];
  onPick: (path: string) => void;
  onClose: () => void;
};

/**
 * 子序列模糊匹配：字符按顺序出现即可。连续命中、命中在词首（/ . _ - 后）、命中在文件名里加分。
 * 返回命中的下标，用来加粗；不匹配返回 null。
 */
function match(path: string, q: string): { score: number; hits: number[] } | null {
  if (!q) return { score: 0, hits: [] };
  const p = path.toLowerCase();
  const base = p.lastIndexOf("/") + 1;
  const hits: number[] = [];
  let score = 0;
  let i = 0;
  for (const ch of q.toLowerCase()) {
    const at = p.indexOf(ch, i);
    if (at < 0) return null;
    hits.push(at);
    if (at === i && hits.length > 1) score += 4;
    if (at === 0 || "/._- ".includes(p[at - 1])) score += 3;
    if (at >= base) score += 2;
    i = at + 1;
  }
  // 同分时路径短的在前
  return { score: score - p.length * 0.01, hits };
}

function highlight(text: string, hits: number[], offset: number): ReactNode {
  const set = new Set(hits.map((h) => h - offset));
  return [...text].map((ch, i) => (set.has(i) ? <mark key={i}>{ch}</mark> : ch));
}

export default function QuickOpen({ files, recent, onPick, onClose }: Props) {
  useLocale();
  const [q, setQ] = useState("");
  const [sel, setSel] = useState(0);
  const list = useRef<HTMLUListElement>(null);
  const dialog = useDialog(onClose);

  const results = useMemo(() => {
    const rank = (p: string) => {
      const r = recent.indexOf(p);
      return r < 0 ? 0 : (recent.length - r) * 0.5;
    };
    return files
      .flatMap((path) => {
        const m = match(path, q.trim());
        return m ? [{ path, ...m, score: m.score + rank(path) }] : [];
      })
      .sort((a, b) => b.score - a.score)
      .slice(0, 60);
  }, [files, recent, q]);

  useEffect(() => setSel(0), [q]);

  useEffect(() => {
    list.current?.children[sel]?.scrollIntoView({ block: "nearest" });
  }, [sel]);

  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="palette" role="dialog" aria-modal="true" aria-label={t("quickOpen.label")} ref={dialog} tabIndex={-1}>
        <div className="palette-input">
          <Icon name="search" />
          <input
            autoFocus
            value={q}
            placeholder={t("quickOpen.placeholder")}
            aria-label={t("quickOpen.label")}
            role="combobox"
            aria-expanded={results.length > 0}
            aria-controls="quick-open-list"
            aria-activedescendant={results[sel] ? `quick-open-${sel}` : undefined}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown" || (e.ctrlKey && e.key === "n")) {
                e.preventDefault();
                setSel((s) => Math.min(results.length - 1, s + 1));
              } else if (e.key === "ArrowUp" || (e.ctrlKey && e.key === "p")) {
                e.preventDefault();
                setSel((s) => Math.max(0, s - 1));
              } else if (e.key === "Enter") {
                e.preventDefault();
                const hit = results[sel];
                if (hit) onPick(hit.path);
              } else if (e.key === "Escape") {
                e.preventDefault();
                onClose();
              }
            }}
          />
        </div>
        {results.length ? (
          <ul className="palette-list" ref={list} role="listbox" id="quick-open-list" aria-label={t("quickOpen.label")}>
            {results.map((r, i) => {
              const cut = r.path.lastIndexOf("/") + 1;
              return (
                <li
                  key={r.path}
                  id={`quick-open-${i}`}
                  role="option"
                  aria-selected={i === sel}
                  className={`palette-item${i === sel ? " on" : ""}`}
                  onMouseMove={() => setSel(i)}
                  onClick={() => onPick(r.path)}
                >
                  <Icon name={fileIcon(r.path)} />
                  <span className="palette-name">{highlight(r.path.slice(cut), r.hits, cut)}</span>
                  {cut > 0 && <span className="palette-dir">{highlight(r.path.slice(0, cut - 1), r.hits, 0)}</span>}
                </li>
              );
            })}
          </ul>
        ) : (
          <div className="palette-empty">{t("quickOpen.noMatch", { q })}</div>
        )}
        <div className="palette-foot">
          <span>
            <Kbd>↑</Kbd>
            <Kbd>↓</Kbd> {t("quickOpen.select")}
          </span>
          <span>
            <Kbd>↵</Kbd> {t("common.open")}
          </span>
          <span>
            <Kbd>esc</Kbd> {t("common.close")}
          </span>
        </div>
      </div>
    </div>
  );
}
