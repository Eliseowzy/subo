import { useCallback, useEffect, useRef, useState } from "react";
import { api, type InstallProgress, type InstallResult } from "./api";
import Icon from "./Icon";
import { t, useLocale } from "./i18n";
import { fileSize, Spinner, toastError } from "./ui";

/** 装宏包：调用安装，装的时候轮询进度（大的字体包要下好几分钟，光转圈让人以为卡死了），
 *  下载阶段可以取消。缺字清单和设置页的待取队列共用 */
export function useInstall() {
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<InstallProgress | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const live = useRef(true);
  useEffect(() => () => void (live.current = false), []);

  useEffect(() => {
    if (!busy) return;
    let on = true;
    const tick = () =>
      api
        .installProgress()
        .then((r) => on && setProgress(r.progress))
        .catch(() => {});
    void tick();
    const timer = setInterval(tick, 800);
    return () => {
      on = false;
      clearInterval(timer);
    };
  }, [busy]);

  const run = useCallback(async (packages: string[], auto = false, project?: string): Promise<InstallResult> => {
    setBusy(true);
    try {
      return await api.installPackages(packages, auto, project);
    } finally {
      if (live.current) {
        setBusy(false);
        setProgress(null);
        setCancelling(false);
      }
    }
  }, []);

  const cancel = useCallback(async () => {
    setCancelling(true);
    try {
      await api.cancelInstall();
    } catch (e) {
      setCancelling(false);
      toastError(e);
    }
  }, []);

  /** 还在连软件源、下载，且不是走系统授权框：这时停下不会留下装了一半的包 */
  const canCancel = busy && !cancelling && !!progress?.cancellable && (progress.phase === "connect" || progress.phase === "download");
  return { busy, progress, run, cancel, canCancel, cancelling };
}

/** 安装中的取消按钮：能取消时可点，解包开始后变成「正在解包，不能中断」 */
export function CancelInstall({ ctl }: { ctl: ReturnType<typeof useInstall> }) {
  useLocale();
  if (!ctl.busy || !ctl.progress) return null;
  if (ctl.cancelling)
    return (
      <span className="dim">
        <Spinner size={11} /> {t("install.cancelling")}
      </span>
    );
  if (!ctl.canCancel) return ctl.progress.cancellable ? <span className="dim">{t("install.tooLate")}</span> : null;
  return (
    <button className="btn btn-ghost btn-sm" onClick={() => void ctl.cancel()} title={t("install.cancelTitle")}>
      <Icon name="x" size={13} /> {t("install.cancel")}
    </button>
  );
}

/** 进度条该走到哪（0~1）：下载按字节，逐个解包按 tlmgr 按字节估的时间、估不出来按包数；量不出为 null */
export function installShare(p: InstallProgress): number | null {
  if (p.phase === "finish") return 1;
  const dl = p.got != null && p.want ? p.got / p.want : null;
  const inst =
    p.phase === "install"
      ? p.spent != null && p.estimate
        ? p.spent / p.estimate
        : p.total
          ? (p.index - 1) / p.total
          : null
      : null;
  return dl == null && inst == null ? null : Math.max(dl ?? 0, inst ?? 0);
}

/** 实测下载速度（字节/秒）：下了两秒以上才算 */
const downloadRate = (p: InstallProgress) => (p.got && p.downloading >= 2 ? p.got / p.downloading : null);

/** 大约还剩几秒：下载时按实测速度推，解包时用 tlmgr 自己的估计 */
export function installLeft(p: InstallProgress): number | null {
  const rate = downloadRate(p);
  if (p.phase === "download" && rate && p.want && p.got != null && p.want > p.got) return Math.round((p.want - p.got) / rate);
  if (p.phase === "install" && p.spent != null && p.estimate) return p.estimate - p.spent;
  return null;
}

/** 秒数读成 m:ss（过一小时 h:mm:ss），跟 tlmgr 自己的写法一致 */
export function clock(secs: number): string {
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = String(secs % 60).padStart(2, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
}

/** 安装进度：一条按字节算的进度条，加上「在干什么、下了多少、用了多久、大约还剩多久」 */
export function InstallMeter({ p }: { p: InstallProgress }) {
  const locale = useLocale();
  const size = (n: number) => fileSize(n, locale.code);
  const share = installShare(p);
  const bytes = p.got != null ? (p.want ? `${size(Math.min(p.got, p.want))} / ${size(p.want)}` : size(p.got)) : null;
  const what =
    p.phase === "finish"
      ? t("workspace.missing.progress.finish")
      : p.phase === "connect"
        ? t("workspace.missing.progress.connect")
        : p.phase === "install" && p.total
          ? t("workspace.missing.progress.install", { index: p.index, total: p.total, pkg: p.package ?? "" })
          : p.total
            ? t("workspace.missing.progress.download", { count: p.total })
            : p.package
              ? t("workspace.missing.progress.fetch", { pkg: p.package })
              : t("workspace.missing.progress.downloadAny");
  const rate = downloadRate(p);
  const left = installLeft(p);
  return (
    <div className="install-meter" role="status" aria-live="polite">
      <div
        className={`install-bar${share == null ? " indeterminate" : ""}`}
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={share == null ? undefined : Math.round(share * 100)}
      >
        {share != null && <span style={{ transform: `scaleX(${Math.min(Math.max(share, 0.03), p.phase === "finish" ? 1 : 0.98)})` }} />}
      </div>
      <div className="install-meta">
        <span>
          {p.followup && <span className="dim">{t("workspace.missing.progress.followup")} · </span>}
          {what}
          {bytes && p.phase === "download" && <span className="dim"> · {bytes}</span>}
        </span>
        <span className="dim">
          {t("workspace.missing.progress.elapsed", { time: clock(p.elapsed) })}
          {rate && p.phase === "download" && <> · {t("workspace.missing.progress.speed", { speed: size(rate) })}</>}
          {left != null && left > 0 && <> · {t("workspace.missing.progress.left", { time: clock(left) })}</>}
        </span>
      </div>
    </div>
  );
}

