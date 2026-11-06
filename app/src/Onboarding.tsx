import { useEffect, useState, type ReactNode } from "react";
import type { OnboardingKind, ProjectSummary } from "./api";
import Icon, { type IconName } from "./Icon";
import Mark from "./Mark";
import { t, tx, useLocale, type Key } from "./i18n";
import { Kbd, LocaleSelect, ThemeSwitch, useDialog } from "./ui";

/*
 * 上手引导：首页上的一张卡片，左边是路线图（几站、走到哪了），右边是这一站的内容。
 *
 * 分两种：全新安装（fresh）、升级（upgrade）。升级将来用来介绍新功能，目前和全新安装
 * 走同一条路线，只有卡片顶上那行字不同。要给升级换内容，改 ROUTES.upgrade 即可。
 *
 * 有没有项目都能走：最后一站按有无项目给不同的出口。
 * 关掉（跳过或走完）都算看过，由调用方通知服务端记下版本。
 */

/** 设置里「重看引导」跳到这里：项目列表上打开引导 */
export const TOUR_HASH = "#/welcome";

type Stop = "welcome" | "start" | "write" | "tex" | "go";

const ROUTES: Record<OnboardingKind, Stop[]> = {
  fresh: ["welcome", "start", "write", "tex", "go"],
  upgrade: ["welcome", "start", "write", "tex", "go"],
};

const STOP_ICON: Record<Stop, IconName> = {
  welcome: "home",
  start: "folderOpen",
  write: "pencil",
  tex: "package",
  go: "play",
};

/** 最后一站的出口，由首页执行（它手里有选择框、上传框这些） */
export type TourAction = "folder" | "zip" | "blank" | "template" | "recent";

type Props = {
  kind: OnboardingKind;
  version: string;
  projects: ProjectSummary[] | null;
  /** 服务端探测到的排版引擎；mock 表示本机没有 TeX */
  engine: string;
  bundledTex: boolean;
  onClose: () => void;
  onAction: (a: TourAction) => void;
};

export default function Onboarding({ kind, version, projects, engine, bundledTex, onClose, onAction }: Props) {
  useLocale();
  const route = ROUTES[kind];
  const [at, setAt] = useState(0);
  const stop = route[at];
  const last = at === route.length - 1;
  const dialog = useDialog(onClose);
  const usable = projects?.filter((p) => !p.missing) ?? [];

  useEffect(() => {
    const on = (e: KeyboardEvent) => {
      // 输入框、下拉框里的方向键留给它们自己
      if (/^(INPUT|TEXTAREA|SELECT)$/.test((e.target as HTMLElement).tagName)) return;
      if (e.key === "ArrowRight") setAt((i) => Math.min(i + 1, route.length - 1));
      else if (e.key === "ArrowLeft") setAt((i) => Math.max(i - 1, 0));
    };
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, [onClose, route.length]);

  const body: Record<Stop, ReactNode> = {
    welcome: (
      <>
        <div className="tour-brand">
          <div className="app-tile">
            <Mark size={34} />
          </div>
          <div>
            <div className="side-name">
              {t("brand.primary")} {t("brand.secondary") && <span>{t("brand.secondary")}</span>}
            </div>
            <div className="side-tag">{t("brand.tagline")}</div>
          </div>
        </div>
        <p className="tour-lead">{t("onboarding.welcome.body")}</p>
        <div className="tour-prefs">
          <span className="tour-pref">
            <span className="tour-pref-label">{t("theme.label")}</span>
            <ThemeSwitch />
          </span>
          <span className="tour-pref">
            <span className="tour-pref-label">{t("language.label")}</span>
            <LocaleSelect />
          </span>
        </div>
        <p className="tour-note">{t("onboarding.welcome.prefsNote")}</p>
      </>
    ),
    start: (
      <>
        <p className="tour-lead">
          {usable.length ? t("onboarding.start.bodyHas", { count: usable.length }) : t("onboarding.start.bodyNone")}
        </p>
        <ul className="tour-list">
          <Item icon="folderOpen" head="onboarding.start.folder" desc="onboarding.start.folderDesc" />
          <Item icon="upload" head="onboarding.start.zip" desc="onboarding.start.zipDesc" />
          <Item icon="plus" head="onboarding.start.blank" desc="onboarding.start.blankDesc" />
          <Item icon="template" head="onboarding.start.template" desc="onboarding.start.templateDesc" />
        </ul>
      </>
    ),
    write: (
      <>
        <p className="tour-lead">{t("onboarding.write.body")}</p>
        <ul className="tour-keys">
          <Shortcut kbd="⌘↵" text="onboarding.write.build" />
          <Shortcut kbd="⌘S" text="onboarding.write.save" />
          <Shortcut kbd="⌘P" text="onboarding.write.quickOpen" />
          <Shortcut kbd="⌘⇧J" text="onboarding.write.sync" />
          <Shortcut kbd="⌘J" text="onboarding.write.log" />
          <Shortcut kbd="⌘E" text="onboarding.write.export" />
        </ul>
        <p className="tour-note">{t("onboarding.write.syncNote")}</p>
      </>
    ),
    tex: (
      <>
        <p className="tour-lead">{t("onboarding.tex.body")}</p>
        {engine && (
          <p className={`tour-engine${engine === "mock" ? " warn" : ""}`}>
            <span className="status-dot" />
            {engine === "mock"
              ? t("onboarding.tex.none")
              : tx(bundledTex ? "onboarding.tex.bundled" : "onboarding.tex.system", {
                  engine: <span className="mono">{engine}</span>,
                })}
          </p>
        )}
        <ul className="tour-list">
          <Item icon="alert" head="onboarding.tex.missing" desc="onboarding.tex.missingDesc" />
          <Item icon="sliders" head="onboarding.tex.settings" desc="onboarding.tex.settingsDesc" />
        </ul>
      </>
    ),
    go: (
      <>
        <p className="tour-lead">{usable.length ? t("onboarding.go.bodyHas") : t("onboarding.go.bodyNone")}</p>
        <div className="tour-actions">
          {usable.length ? (
            <>
              <button className="btn btn-primary btn-lg" onClick={() => onAction("recent")}>
                <Icon name="tex" size={16} />
                <span className="tour-ellipsis">{t("onboarding.go.recent", { name: usable[0].name })}</span>
              </button>
              <button className="btn btn-lg" onClick={onClose}>
                {t("onboarding.go.list")}
              </button>
            </>
          ) : (
            <>
              {/* 全新安装时还没有模板，主按钮给新建空白项目（它自带一份中文模板） */}
              <button className="btn btn-primary btn-lg" onClick={() => onAction("blank")}>
                <Icon name="plus" size={16} />
                {t("home.newBlank")}
              </button>
              <button className="btn btn-lg" onClick={() => onAction("folder")}>
                <Icon name="folderOpen" size={16} />
                {t("home.openFolder")}
              </button>
              <button className="btn btn-lg" onClick={() => onAction("zip")}>
                <Icon name="upload" size={16} />
                {t("home.importZip")}
              </button>
              <button className="btn btn-lg" onClick={() => onAction("template")}>
                <Icon name="template" size={16} />
                {t("templates.fromTemplate")}
              </button>
            </>
          )}
        </div>
        <p className="tour-note">{t("onboarding.go.replay")}</p>
      </>
    ),
  };

  return (
    <div className="overlay tour-overlay">
      <div className="tour" role="dialog" aria-modal="true" aria-labelledby="tour-title" ref={dialog} tabIndex={-1}>
        <nav className="tour-route" aria-label={t("onboarding.routeLabel")}>
          <div className="tour-eyebrow">
            {kind === "upgrade" ? t("onboarding.eyebrowUpgrade", { version }) : t("onboarding.eyebrowFresh")}
          </div>
          <ol>
            {route.map((s, i) => (
              <li key={s} className={i === at ? "on" : i < at ? "done" : ""}>
                <button onClick={() => setAt(i)} aria-current={i === at ? "step" : undefined}>
                  <span className="tour-dot">{i < at ? <Icon name="check" size={12} /> : i + 1}</span>
                  <span>{t(`onboarding.${s}.title`)}</span>
                </button>
              </li>
            ))}
          </ol>
        </nav>

        <section className="tour-main">
          <button className="btn btn-ghost btn-sm btn-icon tour-close" onClick={onClose} title={t("onboarding.skip")} aria-label={t("onboarding.skip")}>
            <Icon name="x" size={15} />
          </button>
          <div className="tour-head">
            <span className="tour-icon">
              <Icon name={STOP_ICON[stop]} size={18} />
            </span>
            <h2 id="tour-title">{t(`onboarding.${stop}.heading`)}</h2>
          </div>
          <div className="tour-body" key={stop}>
            {body[stop]}
          </div>
          <div className="tour-foot">
            <span className="tour-progress">{t("onboarding.progress", { at: at + 1, total: route.length })}</span>
            <span style={{ flex: 1 }} />
            {!last && (
              <button className="btn btn-ghost" onClick={onClose}>
                {t("onboarding.skip")}
              </button>
            )}
            {at > 0 && (
              <button className="btn" onClick={() => setAt(at - 1)}>
                {t("onboarding.prev")}
              </button>
            )}
            {last ? (
              <button className="btn btn-primary" onClick={onClose}>
                {t("onboarding.finish")}
              </button>
            ) : (
              <button className="btn btn-primary" autoFocus onClick={() => setAt(at + 1)}>
                {t("onboarding.next")}
                <Icon name="chevron" size={14} />
              </button>
            )}
          </div>
        </section>
      </div>
    </div>
  );
}

function Item({ icon, head, desc }: { icon: IconName; head: Key; desc: Key }) {
  return (
    <li>
      <Icon name={icon} size={16} />
      <span>
        <b>{t(head)}</b>
        <span className="dim">{t(desc)}</span>
      </span>
    </li>
  );
}

function Shortcut({ kbd, text }: { kbd: string; text: Key }) {
  return (
    <li>
      <Kbd>{kbd}</Kbd>
      <span>{t(text)}</span>
    </li>
  );
}
