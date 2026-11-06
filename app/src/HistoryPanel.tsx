import { useEffect, useMemo, useState } from "react";
import type { HistoryVersion } from "./api";
import { lineDiff } from "./diff";
import Icon from "./Icon";
import { t, useLocale } from "./i18n";
import { fileSize, Spinner, toastError, useDialog } from "./ui";

type Props = {
  path: string;
  /** 编辑器里现在的内容：版本和它比 */
  current: string;
  list: () => Promise<{ versions: HistoryVersion[] }>;
  read: (id: number) => Promise<{ content: string }>;
  /** 恢复：调用方负责写盘、换编辑器内容 */
  onRestore: (content: string) => Promise<void>;
  readOnly: boolean;
  onClose: () => void;
};

/** 两份文本的差异只画改动附近这么多行，其余折起来 */
const CONTEXT = 3;

/** 「3 分钟前」：一天之内说相对时间，再早给日期；不到一分钟统一说「现在」，免得秒数跳个不停 */
function when(ms: number, now: number, code: string): string {
  const diff = (ms - now) / 1000;
  const rtf = new Intl.RelativeTimeFormat(code, { numeric: "auto" });
  const abs = Math.abs(diff);
  if (abs < 60) return rtf.format(0, "second");
  if (abs < 3600) return rtf.format(Math.round(diff / 60), "minute");
  if (abs < 86400) return rtf.format(Math.round(diff / 3600), "hour");
  return new Date(ms).toLocaleString(code, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

/** 列表第二行的确切时间：今天只给时分，更早带上日期 */
function stamp(ms: number, code: string): string {
  const d = new Date(ms);
  const today = d.toDateString() === new Date().toDateString();
  return d.toLocaleString(code, today ? { hour: "2-digit", minute: "2-digit" } : { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

/** 文件的本地历史：左边版本列表，右边这个版本和现在的差异（删掉的是版本里有、现在没有的） */
export default function HistoryPanel({ path, current, list, read, onRestore, readOnly, onClose }: Props) {
  const locale = useLocale();
  const [versions, setVersions] = useState<HistoryVersion[] | null>(null);
  const [sel, setSel] = useState<number | null>(null);
  const [text, setText] = useState<string | null>(null);
  const [restoring, setRestoring] = useState(false);
  /** 相对时间的基准：打开时定下，之后每分钟走一次，点来点去不会跟着变 */
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 60_000);
    return () => window.clearInterval(id);
  }, []);

  useEffect(() => {
    list()
      .then((r) => {
        setVersions(r.versions);
        setSel(r.versions[0]?.id ?? null);
      })
      .catch((e) => {
        toastError(e);
        setVersions([]);
      });
    // 打开时取一次
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (sel == null) return;
    let live = true;
    setText(null);
    read(sel)
      .then((r) => live && setText(r.content))
      .catch(toastError);
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sel]);

  const dialog = useDialog(onClose);

  // 从这个版本到现在：版本里有、现在没有的是「之后删的」，现在多出来的是「之后加的」
  const diff = useMemo(() => (text == null ? null : lineDiff(text, current)), [text, current]);
  const added = diff ? diff.filter((d) => d.kind === "add").length : 0;
  const removed = diff ? diff.filter((d) => d.kind === "del").length : 0;
  const changed = added + removed;

  /** 点开过的折叠段，按段首在 diff 里的下标记；换版本就清掉 */
  const [opened, setOpened] = useState<Set<number>>(new Set());
  useEffect(() => setOpened(new Set()), [sel]);

  /** 每行配上两边的行号；只留改动附近的行，大段相同的折成一行，点开才展开 */
  const rows = useMemo(() => {
    if (!diff) return null;
    let a = 0;
    let b = 0;
    const numbered = diff.map((d) => ({
      ...d,
      then: d.kind === "add" ? null : ++a,
      now: d.kind === "del" ? null : ++b,
    }));
    const keep = new Array(diff.length).fill(false);
    diff.forEach((d, i) => {
      if (d.kind === "same") return;
      for (let k = Math.max(0, i - CONTEXT); k <= Math.min(diff.length - 1, i + CONTEXT); k++) keep[k] = true;
    });
    const out: ((typeof numbered)[number] | { kind: "gap"; at: number; count: number })[] = [];
    let gapAt = -1;
    const flush = (i: number) => {
      if (gapAt < 0) return;
      if (opened.has(gapAt)) out.push(...numbered.slice(gapAt, i));
      else out.push({ kind: "gap", at: gapAt, count: i - gapAt });
      gapAt = -1;
    };
    numbered.forEach((d, i) => {
      if (keep[i]) {
        flush(i);
        out.push(d);
      } else if (gapAt < 0) gapAt = i;
    });
    flush(numbered.length);
    return out;
  }, [diff, opened]);

  const restore = async () => {
    if (text == null) return;
    setRestoring(true);
    try {
      await onRestore(text);
      onClose();
    } catch (e) {
      toastError(e);
    }
    setRestoring(false);
  };

  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="history" role="dialog" aria-modal="true" aria-label={t("history.label", { file: path })} ref={dialog} tabIndex={-1}>
        <div className="history-head">
          <Icon name="history" size={16} />
          <span className="history-title">{t("history.title", { file: path })}</span>
          <button className="btn btn-ghost btn-sm btn-icon" onClick={onClose} title={t("common.close")} aria-label={t("common.close")}>
            <Icon name="x" size={14} />
          </button>
        </div>
        {versions && versions.length > 0 && <p className="history-intro">{t("history.intro")}</p>}
        <div className="history-body">
          <ul className="history-list">
            {versions === null && (
              <li className="history-empty">
                <Spinner size={13} />
              </li>
            )}
            {versions?.length === 0 && <li className="history-empty">{t("history.empty")}</li>}
            {versions?.map((v, i) => (
              <li key={v.id}>
                <button className={`history-item${v.id === sel ? " on" : ""}`} onClick={() => setSel(v.id)}>
                  <span className="history-when">
                    {when(v.id, now, locale.code)}
                    {i === 0 && <span className="history-latest">{t("history.latest")}</span>}
                  </span>
                  <span className="history-meta dim">
                    {stamp(v.id, locale.code)} · {fileSize(v.size, locale.code)}
                  </span>
                </button>
              </li>
            ))}
          </ul>
          <div className="history-view">
            {sel == null ? (
              <div className="history-empty">{versions?.length === 0 ? t("history.emptyHint") : null}</div>
            ) : text == null ? (
              <div className="history-empty">
                <Spinner size={13} />
              </div>
            ) : (
              <>
                <div className="history-summary">
                  {diff == null ? (
                    <span className="dim">{t("history.tooBig")}</span>
                  ) : changed === 0 ? (
                    <span className="dim">{t("history.same")}</span>
                  ) : (
                    <>
                      <span className="dim">{t("history.since")}</span>
                      {added > 0 && <span className="diff-chip add">+ {t("history.added", { count: added })}</span>}
                      {removed > 0 && <span className="diff-chip del">− {t("history.removed", { count: removed })}</span>}
                    </>
                  )}
                </div>
                {diff != null && changed === 0 ? null : rows ? (
                  <div className="history-diff">
                    <div className="diff-cols">
                      <span className="diff-num">{t("history.colThen")}</span>
                      <span className="diff-num">{t("history.colNow")}</span>
                    </div>
                    {rows.map((r) =>
                      r.kind === "gap" ? (
                        <button
                          key={`g${r.at}`}
                          className="diff-gap"
                          onClick={() => setOpened((o) => new Set(o).add(r.at))}
                        >
                          {t("history.unchanged", { count: r.count })}
                        </button>
                      ) : (
                        <div key={`${r.then}-${r.now}`} className={`diff-row diff-${r.kind}`}>
                          <span className="diff-num">{r.then ?? ""}</span>
                          <span className="diff-num">{r.now ?? ""}</span>
                          <span className="diff-sign">{r.kind === "add" ? "+" : r.kind === "del" ? "−" : ""}</span>
                          <span className="diff-text">
                            {r.text || (r.kind === "same" ? " " : <span className="diff-blank">{t("history.blank")}</span>)}
                          </span>
                          {r.kind !== "same" && (
                            <span className="diff-tag">{t(r.kind === "add" ? "history.addTag" : "history.delTag")}</span>
                          )}
                        </div>
                      )
                    )}
                  </div>
                ) : (
                  <pre className="history-diff history-raw">{text}</pre>
                )}
              </>
            )}
          </div>
        </div>
        <div className="history-foot">
          <span className="dim">{!readOnly && diff != null && changed > 0 ? t("history.restoreHint") : null}</span>
          {!readOnly && (
            <button className="btn btn-primary btn-sm" disabled={text == null || restoring || (diff != null && changed === 0)} onClick={() => void restore()}>
              {restoring ? <Spinner size={12} /> : <Icon name="history" size={13} />} {t("history.restore")}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
