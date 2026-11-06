import { useEffect, useMemo, useState, type ReactNode } from "react";
import { api, type AboutInfo, type ThirdParty } from "./api";
import { writeText } from "./clipboard";
import { t, useLocale } from "./i18n";
import Icon from "./Icon";
import Mark from "./Mark";
import { Spinner, toast, toastError, useDialog } from "./ui";

/*
 * 「关于素帛」：菜单栏（桌面版）点开。除了版本，主要给排查问题用：
 * 用的哪个排版引擎、哪份 TeX、项目库在哪，一键复制成一段文字贴给别人。
 * 第三方许可清单是构建时生成的 /licenses.json（scripts/gen-licenses.mjs）。
 */

type Props = { onClose: () => void };

export default function About({ onClose }: Props) {
  const locale = useLocale();
  const [info, setInfo] = useState<AboutInfo | null>(null);
  const [licenses, setLicenses] = useState(false);

  useEffect(() => {
    api.about().then(setInfo).catch(toastError);
  }, []);

  // Esc 先从许可清单退回，再关
  const dialog = useDialog(() => (licenses ? setLicenses(false) : onClose()));

  return (
    <div className="overlay about-overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className={`about${licenses ? " about-lic" : ""}`} role="dialog" aria-modal="true" aria-label={t("about.title")} ref={dialog} tabIndex={-1}>
        {licenses ? (
          <Licenses bundledTex={!!info?.bundledTex} onBack={() => setLicenses(false)} />
        ) : (
          <>
            <button className="btn btn-ghost btn-sm btn-icon about-close" onClick={onClose} title={t("common.close")} aria-label={t("common.close")}>
              <Icon name="x" size={15} />
            </button>
            <div className="about-head">
              <div className="app-tile">
                <Mark size={34} />
              </div>
              <div className="about-name">
                {t("brand.primary")} {t("brand.secondary") && <span>{t("brand.secondary")}</span>}
              </div>
              <div className="about-tag">{t("brand.tagline")}</div>
              {info && <div className="about-version">{versionLine(info)}</div>}
            </div>

            {info ? (
              <dl className="about-env">
                {rows(info).map(([label, value, action]) => (
                  <div key={label}>
                    <dt>{label}</dt>
                    <dd>
                      <span className="about-value" title={value}>
                        {value}
                      </span>
                      {action}
                    </dd>
                  </div>
                ))}
              </dl>
            ) : (
              <div className="about-loading">
                <Spinner />
              </div>
            )}

            <div className="about-foot">
              <button className="btn btn-sm" disabled={!info} onClick={() => info && copyDiagnostics(info, locale.code)}>
                <Icon name="copy" size={13} />
                {t("about.copy")}
              </button>
              <button className="btn btn-sm" onClick={() => setLicenses(true)}>
                {t("about.licenses")}
              </button>
              <span style={{ flex: 1 }} />
              <span className="about-copyright">{t("about.copyright", { year: year(info) })}</span>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

function versionLine(info: AboutInfo) {
  const v = t("about.version", { version: info.version });
  if (!info.build) return v;
  return `${v} · ${t("about.build", { build: info.build })}${info.buildDate ? ` · ${info.buildDate}` : ""}`;
}

function year(info: AboutInfo | null) {
  return info?.buildDate?.slice(0, 4) ?? String(new Date().getFullYear());
}

function osName(os: string) {
  return os === "macos" ? t("about.os.macos") : os === "linux" ? t("about.os.linux") : os === "windows" ? t("about.os.windows") : os;
}

function texText(info: AboutInfo) {
  if (info.engine === "mock") return t("about.texNone");
  const where = info.bundledTex ? t("about.texBundled") : t("about.texSystem");
  return info.texVersion ? `${where} · ${t("about.texVersion", { version: info.texVersion })}` : where;
}

/** 环境信息：标签、值、可选的操作按钮 */
function rows(info: AboutInfo): [string, string, ReactNode?][] {
  const reveal = (
    <button
      className="btn btn-ghost btn-sm btn-icon"
      onClick={() => void api.revealDir("library").catch(toastError)}
      title={t("about.reveal")}
      aria-label={t("about.reveal")}
    >
      <Icon name="reveal" size={13} />
    </button>
  );
  // 没有 TeX 时引擎一行没什么可说的，只留 TeX 一行说清楚
  const engine: [string, string][] = info.engine === "mock" ? [] : [[t("about.engine"), info.engine]];
  return [
    ...engine,
    [t("about.tex"), texText(info)],
    [t("about.library"), info.home, reveal],
    [t("about.system"), `${osName(info.os)} · ${info.arch}`],
  ];
}

/** 一段纯文本，贴到问题反馈里；标签跟界面语言走 */
function copyDiagnostics(info: AboutInfo, locale: string) {
  const lines = [
    `${t("brand.windowTitle")} ${versionLine(info)}`,
    `${t("about.system")}: ${osName(info.os)} ${info.arch}`,
    `${t("about.engine")}: ${info.engine}`,
    `${t("about.tex")}: ${texText(info)}${info.texRoot ? ` (${info.texRoot})` : ""}`,
    `${t("about.library")}: ${info.home}`,
    `${t("about.locale")}: ${locale}`,
  ];
  writeText(lines.join("\n")).then(
    () => toast(t("about.copied"), "success"),
    () => toast(t("common.copyFailed"), "error")
  );
}

function Licenses({ bundledTex, onBack }: { bundledTex: boolean; onBack: () => void }) {
  const [list, setList] = useState<ThirdParty[] | null>(null);
  const [query, setQuery] = useState("");

  useEffect(() => {
    api
      .licenses()
      .then(setList)
      .catch(() => setList([]));
  }, []);

  const groups = useMemo(() => {
    const q = query.trim().toLowerCase();
    const hit = (p: ThirdParty) => !q || p.name.toLowerCase().includes(q) || p.license.toLowerCase().includes(q);
    const pick = (source: ThirdParty["source"]) => (list ?? []).filter((p) => p.source === source && hit(p));
    return [
      { title: t("about.npm"), items: pick("npm") },
      { title: t("about.cargo"), items: pick("cargo") },
    ];
  }, [list, query]);

  return (
    <>
      <div className="about-lic-head">
        <button className="btn btn-ghost btn-sm btn-icon" onClick={onBack} title={t("common.back")} aria-label={t("common.back")}>
          <Icon name="chevron" size={15} className="flip" />
        </button>
        <h2 className="about-lic-title">{t("about.licenses")}</h2>
        <input
          className="input about-lic-filter"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={t("about.filter")}
          aria-label={t("about.filter")}
        />
      </div>
      <div className="about-lic-body">
        <p className="about-lic-note">{t("about.licensesNote")}</p>
        {bundledTex && (
          <section className="about-lic-tex">
            <h3>{t("about.texLive")}</h3>
            <p className="about-lic-note">{t("about.texLiveNote")}</p>
          </section>
        )}
        {list === null ? (
          <Spinner />
        ) : list.length === 0 ? (
          <p className="about-lic-note">{t("about.licensesMissing")}</p>
        ) : (
          groups.map(
            (g) =>
              g.items.length > 0 && (
                <section key={g.title}>
                  <h3>
                    {g.title} <span>{g.items.length}</span>
                  </h3>
                  <ul>
                    {g.items.map((p) => (
                      <li key={`${p.name}@${p.version}`}>
                        <span className="about-lic-name">{p.name}</span>
                        <span className="about-lic-ver">{p.version}</span>
                        <span className="about-lic-id">{p.license || t("about.licenseUnknown")}</span>
                      </li>
                    ))}
                  </ul>
                </section>
              )
          )
        )}
      </div>
    </>
  );
}
