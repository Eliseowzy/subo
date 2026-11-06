// 工作区顶栏的编译引擎切换：显示项目实际用的引擎和来由，点开可改成自动或某个已装的引擎
// （内置的 pdfLaTeX、XeLaTeX，加上设置里装了的 LuaLaTeX、pLaTeX……）。
// 选了就记在项目设置里（服务端 projprefs），改完立刻重编。tectonic、mock、模板预览不显示。

import { useEffect, useState, type MouseEvent } from "react";
import { api as libraryApi, type EngineInfo, type EngineMode, type Program, type projectApi } from "./api";
import ContextMenu, { SEP, type MenuEntry } from "./ContextMenu";
import Icon from "./Icon";
import { settingsHash } from "./Settings";
import { t, useLocale } from "./i18n";
import { toast } from "./ui";

/** 程序的写法是专名，各语言一样 */
const PROGRAM: Record<Program, string> = {
  pdflatex: "pdfLaTeX",
  xelatex: "XeLaTeX",
  lualatex: "LuaLaTeX",
  platex: "pLaTeX",
  uplatex: "upLaTeX",
};
export const programName = (p: string) => PROGRAM[p as Program] ?? p;
/** 菜单里的顺序：内置的在前 */
const ORDER = Object.keys(PROGRAM) as Program[];

/** 为什么用这个引擎，一句话 */
function why(info: EngineInfo): string {
  const program = programName(info.program);
  const r = info.reason;
  switch (info.source) {
    case "user":
      return t("engine.why.user");
    case "missing":
      return t("engine.why.missing", { program: programName(info.wanted ?? "") });
    case "detected":
      if (r?.kind === "magic") return t("engine.why.magic", { program });
      if (r?.kind === "latexmkrc") return t("engine.why.latexmkrc", { program });
      if (r?.kind === "arxiv") return t("engine.why.arxiv", { program });
      return t("engine.why.preamble", { program, detail: r?.detail ?? "" });
    default:
      return t("engine.why.default", { program });
  }
}

type Props = {
  api: ReturnType<typeof projectApi>;
  /** 每次编译结束加一：源码可能改了，重新问一次探测结果 */
  version: number;
  /** 引擎换了，要重编 */
  onChanged: () => void;
};

export default function EngineSwitch({ api, version, onChanged }: Props) {
  useLocale();
  const [info, setInfo] = useState<EngineInfo | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  /** 能选的程序：装了的引擎提供的；没问到时只给内置的两个 */
  const [programs, setPrograms] = useState<Program[]>(["pdflatex", "xelatex"]);

  useEffect(() => {
    let live = true;
    api.engine().then(
      (i) => live && setInfo(i),
      () => live && setInfo(null)
    );
    libraryApi.engines().then(
      (e) => live && setPrograms(e.engines.filter((x) => x.installed).flatMap((x) => x.programs)),
      () => {}
    );
    return () => {
      live = false;
    };
  }, [api, version]);

  if (!info?.switchable) return null;

  const choose = (mode: EngineMode) => {
    if (mode === info.mode) return;
    const before = info.program;
    api.setEngine(mode).then(
      (next) => {
        setInfo(next);
        if (next.program !== before) onChanged();
      },
      (e) => toast(t("engine.saveFailed", { error: String(e).replace(/^Error:\s*/, "") }), "error")
    );
  };
  const mark = (on: boolean) => (on ? "check" : undefined);
  const items: MenuEntry[] = [
    {
      icon: mark(info.mode === "auto"),
      label: info.mode === "auto" ? t("engine.autoNow", { program: programName(info.program) }) : t("engine.auto"),
      run: () => choose("auto"),
    },
    SEP,
    // 选定过、后来又卸了的引擎也列出来，免得勾选项凭空消失
    ...ORDER.filter((p) => programs.includes(p) || info.mode === p).map((p) => ({
      icon: mark(info.mode === p),
      label: programs.includes(p) ? programName(p) : t("engine.notInstalled", { program: programName(p) }),
      run: () => choose(p),
    })),
    SEP,
    { icon: "sliders", label: t("engine.manage"), run: () => void (location.hash = settingsHash("engines", location.hash)) },
  ];
  const open = (e: MouseEvent<HTMLButtonElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    setMenu({ x: r.left, y: r.bottom + 2 });
  };

  return (
    <>
      <button
        className={`btn btn-ghost btn-sm${info.source === "missing" ? " warn" : ""}`}
        onClick={open}
        title={t("engine.title", { program: programName(info.program), why: why(info) })}
        aria-haspopup="menu"
        aria-expanded={!!menu}
      >
        <span className="mono">{programName(info.program)}</span>
        <Icon name="chevronDown" size={12} />
      </button>
      {menu && <ContextMenu x={menu.x} y={menu.y} items={items} label={t("engine.menuLabel")} onClose={() => setMenu(null)} />}
    </>
  );
}
