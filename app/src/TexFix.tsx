import { useEffect, useState, type ReactNode } from "react";
import { api, ServerError, type JobEvent, type TexStatus } from "./api";
import Icon from "./Icon";
import { clock } from "./Install";
import { t, useLocale, type Key } from "./i18n";
import { Spinner, toast } from "./ui";

type Stale = Extract<JobEvent, { type: "tex.stale" }>;

/** 更新 MiKTeX 没成的原因，对应服务端 tex.rs 的 `miktex_reason` */
export type Reason = "admin" | "tooOld" | "offline" | "noTool" | "failed";

const MIKTEX_DOWNLOAD = "https://miktex.org/download";

type Update =
  | { phase: "idle" }
  | { phase: "running"; since: number }
  | { phase: "failed"; reason: Reason; log: string };

/** 更新 MiKTeX 的状态机：卡片和设置页共用。成功时交回新的 TeX 状态 */
export function useMiktexUpdate(onDone: (tex: TexStatus) => void) {
  const [state, setState] = useState<Update>({ phase: "idle" });
  const run = async () => {
    setState({ phase: "running", since: Date.now() });
    try {
      const r = await api.updateMiktex();
      setState({ phase: "idle" });
      toast(t("tex.miktex.done"), "success");
      onDone(r.tex);
    } catch (e) {
      const code = e instanceof ServerError ? e.code ?? "" : "";
      const reason = (["admin", "tooOld", "offline", "noTool"] as const).find((r) => code === `tex.miktex.${r}`) ?? "failed";
      const log = e instanceof ServerError && typeof e.params.log === "string" ? e.params.log : String(e instanceof Error ? e.message : e);
      setState({ phase: "failed", reason, log });
    }
  };
  return { state, run };
}

/** 秒表：更新要几分钟，让人知道它还在动 */
export function Elapsed({ since }: { since: number }) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);
  return <span className="tabular">{clock(Math.max(0, Math.round((now - since) / 1000)))}</span>;
}

/** 正在更新：一行转圈加秒表 */
export function Updating({ since }: { since: number }) {
  useLocale();
  return (
    <div className="tex-fix-row tex-fix-status">
      <Spinner size={12} />
      {t("tex.miktex.updating")}
      <Elapsed since={since} />
    </div>
  );
}

/**
 * 更新没成之后的那一行按钮。先给对应原因的修法（去 MiKTeX Console、下载新版、重试）；
 * 用户去外面处理过（点了 Console 或下载）才出现「重新编译」，没处理就重编没有意义。
 * `lead` 是排在最前面的主操作（有内置 TeX 时是「改用内置」），有它时这里的按钮都降为次要
 */
export function FailedActions({
  reason,
  log,
  retry,
  recheck,
  lead,
}: {
  reason: Reason;
  log: string;
  retry: () => void;
  recheck: () => void;
  lead?: ReactNode;
}) {
  useLocale();
  const [away, setAway] = useState(false);
  const [showLog, setShowLog] = useState(false);
  const primary = (main: boolean) => `btn btn-sm${main && !lead ? " btn-primary" : ""}`;
  const openConsole = () => {
    setAway(true);
    void api.openMiktexConsole().catch((e) => toast(String(e instanceof Error ? e.message : e), "error"));
  };

  let fix: ReactNode;
  if (reason === "offline")
    fix = (
      <button className={primary(true)} onClick={retry}>
        {t("tex.miktex.retry")}
      </button>
    );
  else if (reason === "tooOld")
    fix = (
      <a className={primary(!away)} href={MIKTEX_DOWNLOAD} target="_blank" rel="noreferrer" onClick={() => setAway(true)}>
        {t("tex.miktex.download")}
      </a>
    );
  else
    fix = (
      <button className={primary(!away)} onClick={openConsole}>
        {t("tex.miktex.openConsole")}
      </button>
    );

  return (
    <>
      <div className="tex-fix-row tex-fix-actions">
        {lead}
        {away && (
          <button className={primary(true)} onClick={recheck}>
            {t("tex.miktex.recheck")}
          </button>
        )}
        {fix}
        {log && (
          <button className="tex-fix-link" aria-expanded={showLog} onClick={() => setShowLog((v) => !v)}>
            {t(showLog ? "tex.miktex.hideLog" : "tex.miktex.showLog")}
          </button>
        )}
      </div>
      {showLog && <pre className="tex-fix-log">{log}</pre>}
    </>
  );
}

/** 编译失败是因为 LaTeX 内核比宏包旧：说清是哪套 TeX、差在哪，给出当场能做的修法，修好自动重编 */
export function TexStaleCard({ stale, onFixed }: { stale: Stale; onFixed: () => void }) {
  useLocale();
  const [switching, setSwitching] = useState(false);
  const { state, run } = useMiktexUpdate(() => onFixed());
  const tex = stale.tex;
  const own = tex.own;
  const year = tex.bundledYear ?? "";
  const canSwitch = tex.hasBundled && !tex.bundled;
  const miktex = !tex.bundled && own?.kind === "miktex";
  const which = tex.bundled ? t("tex.name.bundled", { year }) : t(`tex.name.${own?.kind ?? "other"}` as Key);

  const useBundled = async () => {
    setSwitching(true);
    try {
      await api.saveSettings({ tex: "bundled" });
      toast(t("tex.stale.switched", { year }), "success");
      onFixed();
    } catch (e) {
      toast(String(e instanceof Error ? e.message : e), "error");
    }
    setSwitching(false);
  };

  /** 用户在 MiKTeX Console、官网安装包里更新完回来 */
  const recheck = async () => {
    await api.refreshTex().catch(() => null);
    onFixed();
  };

  // 一行事实：哪套 TeX · 内核日期 · 要求的日期
  const facts = [which, stale.kernel && t("tex.stale.kernel", { date: stale.kernel }), stale.want && t("tex.stale.want", { date: stale.want })]
    .filter(Boolean)
    .join(" · ");

  const switchBtn = canSwitch ? (
    <button className="btn btn-sm btn-primary" disabled={switching} onClick={() => void useBundled()}>
      {switching && <Spinner size={12} />}
      {t("tex.stale.useBundled", { year })}
    </button>
  ) : null;

  // 没有按钮可按时，用一句话说清只能在外面做什么
  let note = "";
  if (tex.bundled) note = t("tex.stale.bundledToo");
  else if (!canSwitch && !miktex) note = own?.kind === "distro" ? t("tex.stale.distroNote") : t("tex.stale.texliveNote", { year: tex.floor.slice(0, 4) });

  const failed = state.phase === "failed";
  return (
    <section className="tex-fix">
      <div className="tex-fix-title">
        <Icon name="alert" size={14} />
        {failed ? t("tex.miktex.failedTitle") : t("tex.stale.title")}
      </div>
      <p className="tex-fix-sub">{failed ? t(`tex.miktex.why.${state.reason}` as Key) : facts}</p>
      {note && <p className="tex-fix-sub">{note}</p>}

      {state.phase === "running" ? (
        <Updating since={state.since} />
      ) : failed ? (
        <FailedActions reason={state.reason} log={state.log} retry={() => void run()} recheck={() => void recheck()} lead={switchBtn} />
      ) : (
        (switchBtn || miktex) && (
          <div className="tex-fix-row tex-fix-actions">
            {switchBtn}
            {miktex && (
              <button className={`btn btn-sm${canSwitch ? "" : " btn-primary"}`} onClick={() => void run()}>
                {t("tex.miktex.update")}
              </button>
            )}
            <span className="tex-fix-hint">{t(canSwitch ? "tex.stale.switchNote" : "tex.stale.miktexNote")}</span>
          </div>
        )
      )}
    </section>
  );
}
