import { useCallback, useEffect, useRef, useState } from "react";
import { api, fail, type CaseQueue, type SnapshotImported } from "./api";
import Icon from "./Icon";
import { CancelInstall, InstallMeter, useInstall } from "./Install";
import { t, useLocale } from "./i18n";
import { fileSize, Spinner, toast, toastError } from "./ui";

/*
 * 字盘的两件离线工具，放在设置页「宏包」顶上：
 * - 待取队列：离线时缺的宏包先记下，联网后一键补齐（docs/architecture.md §4.2）
 * - 字盘快照：把补装的宏包打成 .subocase 带到别的机器，导入即可编（§4.3）
 */

/** 导入快照的结果说成一句话 */
export function importedText(r: SnapshotImported): string {
  if (!r.packages.length) return t("case.snapshot.nothingNew", { count: r.skipped.length });
  return t("case.snapshot.imported", { count: r.packages.length, pkgs: r.packages.join(" ") });
}

export function QueuePanel({ onChanged }: { onChanged?: () => void }) {
  useLocale();
  const [queue, setQueue] = useState<CaseQueue | null>(null);
  const [failLog, setFailLog] = useState("");
  const [resolving, setResolving] = useState(false);
  const ctl = useInstall();

  const load = useCallback(() => void api.queue().then(setQueue).catch(() => {}), []);
  useEffect(() => {
    load();
    // 联网了重新看一眼：离线时进队列的，这时候可以补了
    window.addEventListener("online", load);
    return () => window.removeEventListener("online", load);
  }, [load]);

  if (!queue) return null;
  const pkgs = Object.keys(queue.packages);
  const files = Object.keys(queue.files);
  if (!pkgs.length && !files.length) return null;
  const projects = new Set([...Object.values(queue.packages), ...Object.values(queue.files)].flat()).size;

  const fill = async () => {
    setFailLog("");
    try {
      let want = pkgs;
      // 先给不知道包名的文件查包名：要联网
      if (files.length) {
        setResolving(true);
        const r = await api.resolveQueue().finally(() => setResolving(false));
        setQueue(r.queue);
        want = Object.keys(r.queue.packages);
        if (r.unresolved && !want.length) {
          toast(t("case.queue.stillOffline"), "error");
          return;
        }
      }
      if (!want.length) return load();
      const r = await ctl.run(want);
      if (r.ok) {
        toast(t("case.queue.filled", { pkgs: want.join(" ") }), "success");
        onChanged?.();
      } else if (r.cancelled) {
        toast(t("case.queue.cancelled"));
      } else if (r.offline) {
        toast(t("case.queue.stillOffline"), "error");
      } else {
        setFailLog(r.log || t("case.queue.failed"));
      }
    } catch (e) {
      toastError(e);
    }
    load();
  };

  const remove = (p: string[], f: string[]) => void api.removeFromQueue(p, f).then(setQueue).catch(toastError);
  const busy = ctl.busy || resolving;

  return (
    <div className="missing case-queue">
      <div className="missing-head">
        <Icon name="download" size={14} />
        {t("case.queue.head", { count: pkgs.length + files.length, projects })}
      </div>
      <ul>
        {pkgs.map((p) => (
          <li key={`p:${p}`}>
            <code>{p}</code>
            <span className="dim">{t("case.queue.package")}</span>
            {!busy && (
              <button className="btn btn-ghost btn-sm btn-icon" title={t("case.queue.remove")} aria-label={t("case.queue.remove")} onClick={() => remove([p], [])}>
                <Icon name="x" size={12} />
              </button>
            )}
          </li>
        ))}
        {files.map((f) => (
          <li key={`f:${f}`}>
            <code>{f}</code>
            <span className="dim">{t("case.queue.file")}</span>
            {!busy && (
              <button className="btn btn-ghost btn-sm btn-icon" title={t("case.queue.remove")} aria-label={t("case.queue.remove")} onClick={() => remove([], [f])}>
                <Icon name="x" size={12} />
              </button>
            )}
          </li>
        ))}
      </ul>
      <div className="missing-hint">
        {busy ? (
          <span className="missing-busy">
            <Spinner size={12} /> {resolving ? t("case.queue.resolving") : t("case.queue.filling")}
          </span>
        ) : (
          <button className="btn btn-primary btn-sm" onClick={() => void fill()}>
            <Icon name="download" size={13} /> {t("case.queue.fill")}
          </button>
        )}
        <CancelInstall ctl={ctl} />
        {!busy && <span className="dim">{t("case.queue.hint")}</span>}
      </div>
      {ctl.busy && ctl.progress && <InstallMeter p={ctl.progress} />}
      {failLog && (
        <div className="missing-fail">
          <div>{t("case.queue.failed")}</div>
          <pre>{failLog}</pre>
        </div>
      )}
    </div>
  );
}

/** 「导出全部」「导入快照」两个按钮 */
export function SnapshotTools({ hasUser, onImported }: { hasUser: boolean; onImported: () => void }) {
  const locale = useLocale();
  const [busy, setBusy] = useState<"export" | "import" | null>(null);
  const input = useRef<HTMLInputElement>(null);

  const exportAll = async () => {
    setBusy("export");
    try {
      const r = await fetch(api.snapshotUrl());
      if (!r.ok) await fail(r);
      const blob = await r.blob();
      const name = /filename="([^"]+)"/.exec(r.headers.get("content-disposition") ?? "")?.[1] ?? "subo-case.subocase";
      const a = document.createElement("a");
      a.href = URL.createObjectURL(blob);
      a.download = name;
      a.click();
      window.setTimeout(() => URL.revokeObjectURL(a.href), 10_000);
      toast(t("case.snapshot.exported", { name, size: fileSize(blob.size, locale.code) }), "success");
    } catch (e) {
      toastError(e);
    }
    setBusy(null);
  };

  const importFile = async (file: File) => {
    setBusy("import");
    try {
      const r = await api.importSnapshot(file);
      toast(importedText(r), "success");
      if (r.kept.length) toast(t("case.snapshot.kept", { count: r.kept.length }));
      onImported();
    } catch (e) {
      toastError(e);
    }
    setBusy(null);
  };

  return (
    <div className="snapshot-tools">
      <div className="snapshot-text">
        <strong>{t("case.snapshot.title")}</strong>
        <span className="dim">{t("case.snapshot.hint")}</span>
      </div>
      <span className="snapshot-actions">
        <button className="btn btn-sm" disabled={!hasUser || busy !== null} onClick={() => void exportAll()} title={hasUser ? undefined : t("case.snapshot.noUser")}>
          {busy === "export" ? <Spinner size={12} /> : <Icon name="download" size={13} />} {t("case.snapshot.exportAll")}
        </button>
        <button className="btn btn-sm" disabled={busy !== null} onClick={() => input.current?.click()}>
          {busy === "import" ? <Spinner size={12} /> : <Icon name="upload" size={13} />} {t("case.snapshot.import")}
        </button>
        <input
          ref={input}
          type="file"
          accept=".subocase"
          hidden
          onChange={(e) => {
            const f = e.target.files?.[0];
            e.target.value = "";
            if (f) void importFile(f);
          }}
        />
      </span>
    </div>
  );
}
