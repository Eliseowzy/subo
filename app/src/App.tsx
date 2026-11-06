import { useCallback, useEffect, useState } from "react";
import { api, previewId, type OnboardingInfo } from "./api";
import About from "./About";
import Home from "./Home";
import { TOUR_HASH } from "./Onboarding";
import { t, useLocale } from "./i18n";
import { deferToHome, HOME_ACTIONS, syncMenu, useMenu } from "./menu";
import SettingsPage, { isSettingsShortcut, SECTIONS, settingsHash, type Section } from "./Settings";
import Workspace from "./Workspace";
import { toast, Toaster } from "./ui";

type Route =
  | { page: "home"; tour?: boolean; templates?: boolean }
  | { page: "project"; id: string }
  | { page: "template"; id: string }
  | { page: "settings"; section: Section; from?: string };

/** `#/p/<id>` 打开项目，`#/t/<模板 id>` 只读预览模板，`#/templates` 是首页的模板页签，
 *  `#/settings/<分区>` 是设置（从项目里点进来的带 `?from=<项目 hash>`，返回回到那个项目），
 *  `#/welcome` 是项目列表上重看上手引导，其余都是项目列表。
 *  用 hash 路由，服务端只托管一个 index.html。 */
function parse(hash: string): Route {
  if (hash === TOUR_HASH) return { page: "home", tour: true };
  if (hash === TEMPLATES_HASH) return { page: "home", templates: true };
  const p = hash.match(/^#\/p\/(.+)$/);
  if (p) return { page: "project", id: decodeURIComponent(p[1]) };
  const tp = hash.match(/^#\/t\/(.+)$/);
  if (tp) return { page: "template", id: decodeURIComponent(tp[1]) };
  const s = hash.match(/^#\/settings(?:\/(\w+))?(?:\?from=(.+))?$/);
  if (s) {
    const section = SECTIONS.find((x) => x === s[1]) ?? "general";
    // 只认项目和模板预览，别的来源一律回项目列表
    const from = s[2] ? decodeURIComponent(s[2]) : undefined;
    return { page: "settings", section, from: from && /^#\/[pt]\//.test(from) ? from : undefined };
  }
  return { page: "home" };
}

const TEMPLATES_HASH = "#/templates";

export function templateHash(id: string) {
  return `#/t/${encodeURIComponent(id)}`;
}

export default function App() {
  const [route, setRoute] = useState(() => parse(location.hash));
  const locale = useLocale();
  /** 服务端判定的这次启动该弹的引导；关掉后清空，本次运行不再自动弹 */
  const [onboarding, setOnboarding] = useState<OnboardingInfo | null>(null);
  const [about, setAbout] = useState(false);

  useEffect(() => {
    api
      .onboarding()
      .then(setOnboarding)
      .catch(() => {});
  }, []);

  // 联网了：待取队列里还有离线时没装上的，提醒一句去哪里补齐
  useEffect(() => {
    const on = () =>
      void api
        .queue()
        .then((q) => {
          const n = Object.keys(q.packages).length + Object.keys(q.files).length;
          if (n) toast(t("case.queue.online", { count: n }));
        })
        .catch(() => {});
    window.addEventListener("online", on);
    return () => window.removeEventListener("online", on);
  }, []);

  const closeTour = useCallback(() => {
    setOnboarding((o) => o && { ...o, kind: null });
    // 记不下来也不拦着用：最多下次启动再弹一次
    void api.onboardingDone().catch(() => {});
    if (location.hash === TOUR_HASH) location.replace("#");
  }, []);

  // 自动弹的按服务端判定；从设置里点「重看」的按全新安装那一套走
  const tour =
    route.page !== "home"
      ? null
      : route.tour
        ? { kind: "fresh" as const, version: onboarding?.version ?? "", previous: onboarding?.previous ?? null }
        : onboarding;

  // 项目页自己管标题；列表页、设置页的标题随语言
  useEffect(() => {
    if (route.page === "home") document.title = t("brand.windowTitle");
    if (route.page === "settings") document.title = `${t("settings.title")} · ${t("brand.windowTitle")}`;
  }, [route, locale]);

  // 换页后焦点别丢在 body 上：落到新页面的主区域，读屏从正文开始念。
  // 页面自己聚焦了（编辑器、输入框）不动；有「跳到编辑区」的页面（工作区）也不动，第一下 Tab 留给它
  useEffect(() => {
    const id = requestAnimationFrame(() => {
      if (document.activeElement && document.activeElement !== document.body) return;
      if (document.querySelector(".skip-link")) return;
      const main = document.querySelector<HTMLElement>('main, [role="main"]');
      if (!main) return;
      if (!main.hasAttribute("tabindex")) main.tabIndex = -1;
      main.focus({ preventScroll: true });
    });
    return () => cancelAnimationFrame(id);
  }, [route]);

  useEffect(() => {
    const on = () => setRoute(parse(location.hash));
    window.addEventListener("hashchange", on);
    return () => window.removeEventListener("hashchange", on);
  }, []);

  // ⌘, 打开设置。项目页里不接：离开前要先落盘，那边的「回到列表」会处理
  useEffect(() => {
    if (route.page === "project" || route.page === "template") return;
    const on = (e: KeyboardEvent) => {
      if (!isSettingsShortcut(e)) return;
      e.preventDefault();
      location.hash = settingsHash(route.page === "settings" ? route.section : "general");
    };
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, [route]);

  const open = useCallback((next: string | null) => {
    location.hash = next ? `#/p/${encodeURIComponent(next)}` : "";
  }, []);

  useEffect(() => syncMenu(route.page), [route.page, locale]);

  // 菜单栏里不依赖页面状态的动作；项目里的（保存、编译……）由 Workspace 接，首页的由 Home 接
  const leave = () => {
    if (route.page === "project") open(null);
    else if (route.page === "template") location.hash = TEMPLATES_HASH;
    else if (route.page === "settings") location.hash = route.from ?? "";
  };
  const fromHere = route.page === "project" || route.page === "template" ? location.hash : undefined;
  useMenu({
    "app.about": () => setAbout(true),
    "app.settings": () => (location.hash = settingsHash(route.page === "settings" ? route.section : "general", fromHere)),
    "file.close": leave,
    "help.tour": () => (location.hash = TOUR_HASH),
    ...(route.page === "home"
      ? {}
      : Object.fromEntries(
          [...HOME_ACTIONS].map((id) => [
            id,
            () => {
              deferToHome(id);
              location.hash = "";
            },
          ])
        )),
  });

  // key 保证切换项目时整个工作区重建，不串状态
  return (
    <>
      {route.page === "project" ? (
        <Workspace key={route.id} id={route.id} onHome={() => open(null)} onSwitch={open} />
      ) : route.page === "template" ? (
        <Workspace
          key={`t:${route.id}`}
          id={previewId(route.id)}
          template={route.id}
          onHome={() => (location.hash = TEMPLATES_HASH)}
          onSwitch={open}
        />
      ) : route.page === "settings" ? (
        <SettingsPage
          section={route.section}
          onSection={(s) => location.replace(settingsHash(s, route.from))}
          backToProject={!!route.from}
          onBack={() => (route.from ? (location.hash = route.from) : open(null))}
        />
      ) : (
        <Home
          onOpen={open}
          onPreview={(tpl) => (location.hash = templateHash(tpl))}
          initialView={route.templates ? "templates" : undefined}
          onSettings={(s) => (location.hash = settingsHash(s))} tour={tour} onTourClose={closeTour} />
      )}
      {about && <About onClose={() => setAbout(false)} />}
      <Toaster />
    </>
  );
}
