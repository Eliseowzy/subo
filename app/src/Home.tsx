import { useCallback, useEffect, useMemo, useRef, useState, type DragEvent, type FormEvent } from "react";
import { api, NAME_MAX, type OnboardingInfo, type ProjectSummary, type TemplateSummary } from "./api";
import { ARCHIVE_ACCEPT, archiveBase, isArchive } from "./archive";
import ContextMenu, { SEP, type MenuEntry } from "./ContextMenu";
import Icon from "./Icon";
import Mark from "./Mark";
import Onboarding, { type TourAction } from "./Onboarding";
import TypeCase from "./TypeCase";
import RenameInput, { NameHint, nameLength } from "./Rename";
import TemplateList, { fromTemplate, savedToast, useTemplates } from "./Templates";
import { t, useLocale } from "./i18n";
import { HOME_ACTIONS, takeDeferred, useMenu } from "./menu";
import type { Section } from "./Settings";
import { ago, Kbd, Spinner, rovingKeys, toast, toastError } from "./ui";

/** 主目录缩成 ~，路径短一截好读 */
function tidy(path: string, home: string): string {
  const user = home.match(/^(\/Users\/[^/]+|\/home\/[^/]+)/)?.[1];
  return user && path.startsWith(user) ? `~${path.slice(user.length)}` : path;
}

/** 拖进来的是文件（不是页面里的文字、链接）才响应 */
const hasFiles = (e: DragEvent) => [...e.dataTransfer.types].includes("Files");

type HomeProps = {
  onOpen: (id: string) => void;
  /** 只读预览模板 */
  onPreview: (templateId: string) => void;
  /** 进来时显示哪个页签；从模板预览回来时是模板页 */
  initialView?: "templates";
  onSettings: (s?: Section) => void;
  /** 要弹的上手引导；kind 为 null 不弹 */
  tour: OnboardingInfo | null;
  onTourClose: () => void;
};

export default function Home({ onOpen, onPreview, initialView, onSettings, tour, onTourClose }: HomeProps) {
  const locale = useLocale();
  const [projects, setProjects] = useState<ProjectSummary[] | null>(null);
  const [home, setHome] = useState("");
  /** 服务端探测到的排版引擎；mock 表示本机没有 TeX */
  const [engine, setEngine] = useState("");
  const [bundledTex, setBundledTex] = useState(false);
  const [query, setQuery] = useState("");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);
  const [renaming, setRenaming] = useState<string | null>(null);
  /** 左栏「新建空白项目」展开成名字输入框 */
  const [creating, setCreating] = useState(false);
  /** 选择框弹不出来（非 macOS）或用户想手动填时，改用路径输入框 */
  const [typing, setTyping] = useState(false);
  const [path, setPath] = useState("");
  /** 右栏看的是项目还是模板；已归档的项目在「设置 - 项目」里管理 */
  const [view, setView] = useState<"projects" | "templates">(initialView ?? "projects");
  // 首页没卸载时 hash 变成 #/templates（如浏览器后退）也切过去
  useEffect(() => {
    if (initialView) setView(initialView);
  }, [initialView]);
  const templates = useTemplates();
  const fileInput = useRef<HTMLInputElement>(null);
  const tplInput = useRef<HTMLInputElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  /** dragenter/leave 在子元素间来回触发，计数才知道是不是真离开了窗口 */
  const dragDepth = useRef(0);

  const reload = useCallback(
    () =>
      api
        .projects()
        .then(setProjects)
        .catch((e) => {
          setProjects((p) => p ?? []);
          toastError(e);
        }),
    []
  );

  useEffect(() => void reload(), [reload]);

  // 后台查到新版本、或已经下好：设置按钮上点个提示，点进去直接到「软件更新」。
  // 定时检查在启动半分钟后才跑，过一分钟再看一次
  const [update, setUpdate] = useState<string | null>(null);
  useEffect(() => {
    const look = () =>
      void api
        .update()
        .then((u) => setUpdate(u.status.phase === "available" || u.status.phase === "ready" ? (u.status.release?.version ?? null) : null))
        .catch(() => {});
    look();
    const id = setTimeout(look, 60_000);
    return () => clearTimeout(id);
  }, []);

  useEffect(() => {
    fetch("/api/health")
      .then((r) => r.json())
      .then((h: { home?: string; engine?: string; bundledTex?: boolean }) => {
        setHome(h.home ?? "");
        setEngine(h.engine ?? "");
        setBundledTex(!!h.bundledTex);
      })
      .catch(() => {});
  }, []);

  // 「/」聚焦搜索，和大多数列表页一致
  useEffect(() => {
    const on = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement;
      if (e.key === "/" && !/^(INPUT|TEXTAREA)$/.test(t.tagName)) {
        e.preventDefault();
        searchRef.current?.focus();
      }
    };
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, []);

  const fail = (err: unknown) => {
    toastError(err);
    setBusy(null);
  };

  const archive = useCallback(
    async (p: ProjectSummary, archived: boolean) => {
      await api.archive(p.id, archived);
      toast(t(archived ? "home.archived" : "home.unarchived", { name: p.name }), "success");
      await reload();
    },
    [reload]
  );

  const rename = useCallback(
    async (id: string, next: string) => {
      try {
        await api.rename(id, next);
      } catch (err) {
        toastError(err);
        throw err;
      }
      setRenaming(null);
      toast(t("home.renamed", { name: next }), "success");
      await reload();
    },
    [reload]
  );

  const create = useCallback(
    async (e: FormEvent) => {
      e.preventDefault();
      if (nameLength(name) > NAME_MAX) return;
      setBusy(t("home.creating"));
      try {
        const { id } = await api.create(name.trim() || t("home.untitled"));
        onOpen(id);
      } catch (err) {
        fail(err);
      }
    },
    [name, onOpen]
  );

  const openPath = useCallback(
    async (p: string) => {
      setBusy(t("home.opening"));
      try {
        const { id } = await api.openFolder(p);
        onOpen(id);
      } catch (err) {
        fail(err);
      }
    },
    [onOpen]
  );

  const pickFolder = useCallback(async () => {
    setBusy(t("home.picking"));
    let picked: string | null;
    try {
      picked = (await api.pickFolder()).path;
    } catch {
      // 系统选择框用不了，退回手动填路径
      setBusy(null);
      setTyping(true);
      return;
    }
    if (!picked) {
      setBusy(null);
      return;
    }
    await openPath(picked);
  }, [openPath]);

  const upload = useCallback(
    async (file: File | undefined) => {
      if (!file) return;
      if (!isArchive(file.name)) {
        toast(t("home.zipOnly"), "error");
        return;
      }
      setBusy(t("home.importing", { file: file.name }));
      const base = archiveBase(file.name);
      try {
        const { id } = await api.importArchive(file, base);
        if (nameLength(base) <= NAME_MAX) return onOpen(id);
        // 文件名超长，服务端截短了：留在列表里说清楚，别让用户进了项目才发现名字不对
        setBusy(null);
        toast(t("home.importedTruncated", { max: NAME_MAX, id }));
        await reload();
      } catch (err) {
        fail(err);
      }
    },
    [onOpen, reload]
  );

  /** 上传压缩包作为模板：服务端只查形状，不编译 */
  const uploadTemplate = useCallback(
    async (file: File | undefined) => {
      if (!file) return;
      if (!isArchive(file.name)) {
        toast(t("home.zipOnly"), "error");
        return;
      }
      setBusy(t("templates.uploading", { file: file.name }));
      const name = [...archiveBase(file.name)].slice(0, NAME_MAX).join("");
      try {
        savedToast("uploaded", await api.importTemplate(file, name));
        await templates.reload();
      } catch (err) {
        toastError(err);
      }
      setBusy(null);
    },
    [templates]
  );

  const pickTemplate = useCallback(
    async (tpl: TemplateSummary) => {
      setBusy(t("templates.creating"));
      try {
        onOpen(await fromTemplate(tpl));
      } catch (err) {
        fail(err);
      }
    },
    [onOpen]
  );

  /** 引导最后一站的出口：先关引导（记为看过），再做首页上对应的事 */
  const tourAction = (a: TourAction) => {
    onTourClose();
    if (a === "folder") void pickFolder();
    else if (a === "zip") fileInput.current?.click();
    else if (a === "template") {
      setQuery("");
      setView("templates");
    } else if (a === "blank") {
      setName("");
      setCreating(true);
    } else {
      const first = projects?.find((p) => !p.missing && !p.archived);
      if (first) onOpen(first.id);
    }
  };

  /** 菜单栏上的「新建项目」「打开文件夹」「从模板新建」；在别的页面点的，切回来后补做 */
  const menuAction = (id: string) => {
    if (busy) return;
    if (id === "file.openFolder") void pickFolder();
    else if (id === "file.newProject") {
      setName("");
      setCreating(true);
    } else if (id === "file.fromTemplate") {
      setQuery("");
      setView("templates");
    }
  };
  useMenu(Object.fromEntries([...HOME_ACTIONS].map((id) => [id, () => menuAction(id)])));
  useEffect(() => {
    const id = takeDeferred();
    if (id) menuAction(id);
    // 只在进首页时取一次
  }, []);

  // 项目行「⋯」菜单
  const [menu, setMenu] = useState<{ id: string; x: number; y: number } | null>(null);
  const menuProject = menu && projects?.find((p) => p.id === menu.id);
  const rowMenu = (p: ProjectSummary): MenuEntry[] => [
    ...(p.folder ? [] : [{ icon: "pencil" as const, label: t("home.rename"), run: () => setRenaming(p.id) }]),
    ...(p.missing
      ? []
      : [{ icon: "reveal" as const, label: t("home.reveal"), run: () => void api.reveal(p.id).catch(toastError) }]),
    { icon: "archive", label: t("home.archive"), run: () => void archive(p, true).catch(toastError) },
    ...(p.folder
      ? [
          SEP,
          {
            icon: "x" as const,
            label: t("home.forget"),
            run: () =>
              void api
                .forgetFolder(p.id)
                .then(() => {
                  toast(t("home.forgotten", { name: p.name }));
                  return reload();
                })
                .catch(toastError),
          },
        ]
      : []),
  ];

  const dropProps = {
    onDragEnter: (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      dragDepth.current++;
      setDragging(true);
    },
    onDragOver: (e: DragEvent) => {
      if (hasFiles(e)) e.preventDefault();
    },
    onDragLeave: () => {
      dragDepth.current = Math.max(0, dragDepth.current - 1);
      if (!dragDepth.current) setDragging(false);
    },
    onDrop: (e: DragEvent) => {
      e.preventDefault();
      dragDepth.current = 0;
      setDragging(false);
      const file = e.dataTransfer.files[0];
      void (view === "templates" ? uploadTemplate(file) : upload(file));
    },
  };

  /** 常用列表不含已归档的项目（归档的在「设置 - 项目」里管理） */
  const shown = useMemo(() => projects?.filter((p) => !p.archived) ?? null, [projects]);
  const archivedCount = projects?.filter((p) => p.archived).length ?? 0;

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!shown || !q) return shown;
    return shown.filter((p) => `${p.name} ${p.path} ${p.entry ?? ""}`.toLowerCase().includes(q));
  }, [shown, query]);

  const tabCount = {
    projects: projects && projects.length - archivedCount,
    templates: templates.list?.length,
  };
  const tabs = ["projects", "templates"] as const;
  const tabLabel = { projects: t("home.recent"), templates: t("templates.tab") };
  const count = tabCount[view];

  const pathForm = typing && (
    <form
      className="path-form"
      onSubmit={(e) => {
        e.preventDefault();
        if (path.trim()) void openPath(path.trim());
      }}
    >
      <input
        className="input"
        autoFocus
        value={path}
        onChange={(e) => setPath(e.target.value)}
        onKeyDown={(e) => e.key === "Escape" && setTyping(false)}
        placeholder={t("home.pathPlaceholder")}
        aria-label={t("tree.pathLabel")}
        disabled={!!busy}
      />
      <button className="btn btn-primary" type="submit" disabled={!!busy || !path.trim()}>
        {t("common.open")}
      </button>
      <button className="btn" type="button" onClick={() => setTyping(false)} disabled={!!busy}>
        {t("common.cancel")}
      </button>
    </form>
  );

  const zipInput = (
    <input
      ref={fileInput}
      type="file"
      accept={ARCHIVE_ACCEPT}
      hidden
      onChange={(e) => {
        void upload(e.target.files?.[0]);
        e.target.value = "";
      }}
    />
  );

  const tplZipInput = (
    <input
      ref={tplInput}
      type="file"
      accept={ARCHIVE_ACCEPT}
      hidden
      onChange={(e) => {
        void uploadTemplate(e.target.files?.[0]);
        e.target.value = "";
      }}
    />
  );

  const busyNote = busy && (
    <div className="busy-note">
      <Spinner /> {busy}
    </div>
  );

  const dropOverlay = dragging && (
    <div className="drop-overlay">
      <div className="drop-card">
        <Icon name="upload" size={30} />
        {view === "templates" ? t("templates.dropTitle") : t("home.dropTitle")}
        <span className="dim">{view === "templates" ? t("templates.dropHint") : t("home.dropHint")}</span>
      </div>
    </div>
  );

  // 左栏：身份、三句话讲清楚是什么、起步操作、本机状态。有没有项目都一样，布局不跳
  const side = (
    <aside className="home-side">
      <div className="side-body">
        <div className="side-brand">
          <div className="app-tile">
            <Mark size={34} busy={!!busy} />
          </div>
          <div>
            <div className="side-name">
              {t("brand.primary")} {t("brand.secondary") && <span>{t("brand.secondary")}</span>}
            </div>
            <div className="side-tag">{t("brand.tagline")}</div>
          </div>
        </div>

        <ul className="side-points">
          <li>
            <Icon name="folder" size={16} />
            <span>
              <b>{t("home.pointDisk")}</b>
            </span>
          </li>
          <li>
            <Icon name="check" size={16} />
            <span>
              <b>{t("home.pointOffline")}</b>
            </span>
          </li>
          <li>
            <Icon name="play" size={14} />
            <span>
              <b>{t("home.pointBuild")}</b>
            </span>
          </li>
        </ul>

        <div className="side-actions">
          <button className="btn btn-primary btn-lg" onClick={pickFolder} disabled={!!busy} title={t("home.openFolderTitle")}>
            <Icon name="folderOpen" size={16} />
            {t("home.openFolder")}
          </button>
          <button className="btn btn-lg" onClick={() => fileInput.current?.click()} disabled={!!busy} title={t("home.importZipTitle")}>
            <Icon name="upload" size={16} />
            {t("home.importZip")}
          </button>
          {creating ? (
            <form className="side-create" onSubmit={create}>
              <input
                className="input"
                autoFocus
                value={name}
                onChange={(e) => setName(e.target.value)}
                onKeyDown={(e) => e.key === "Escape" && setCreating(false)}
                placeholder={t("home.newNamePlaceholder")}
                disabled={!!busy}
                aria-label={t("home.newNameLabel")}
              />
              <span className="side-create-row">
                <NameHint value={name} />
                <span style={{ flex: 1 }} />
                <button className="btn btn-sm" type="button" onClick={() => setCreating(false)}>
                  {t("common.cancel")}
                </button>
                <button className="btn btn-primary btn-sm" type="submit" disabled={!!busy || nameLength(name) > NAME_MAX}>
                  {t("common.create")}
                </button>
              </span>
            </form>
          ) : (
            <button
              className="btn btn-lg"
              onClick={() => {
                setName("");
                setCreating(true);
              }}
              disabled={!!busy}
              title={t("home.newBlankTitle")}
            >
              <Icon name="plus" size={16} />
              {t("home.newBlank")}
            </button>
          )}
          <button
            className="btn btn-lg"
            onClick={() => {
              setQuery("");
              setView("templates");
            }}
            disabled={!!busy}
            title={t("templates.fromTemplateTitle")}
          >
            <Icon name="template" size={16} />
            {t("templates.fromTemplate")}
          </button>
          {zipInput}
          {tplZipInput}
        </div>

        {/* 字盘符号画垫在起步按钮和底栏之间，把左栏下半截撑满；矮窗口里藏掉 */}
        <div className="side-art">
          <TypeCase />
        </div>

        {/* 本机状态只留一行：引擎是否就绪；外观、语言、项目库位置、安装源都在设置里 */}
        <div className="side-foot">
          {engine && (
            <button
              className={`engine-chip${engine === "mock" ? " warn" : ""}`}
              onClick={() => onSettings("tex")}
              title={engine === "mock" ? t("home.noTexTitle") : t("home.engineTitle", { engine })}
            >
              <span className="status-dot" />
              {engine === "mock" ? t("home.noTex") : <span className="mono">{engine}</span>}
              {engine !== "mock" && bundledTex && <span className="engine-tag">{t("home.bundledTex")}</span>}
            </button>
          )}
          <button
            className="btn btn-ghost btn-sm side-settings"
            onClick={() => onSettings(update ? "update" : undefined)}
            title={update ? t("home.updateTitle", { version: update }) : t("home.settingsTitle")}
          >
            <Icon name="sliders" size={15} />
            {t("home.settings")}
            {update && <span className="update-dot" aria-label={t("home.updateTitle", { version: update })} />}
          </button>
        </div>
      </div>
    </aside>
  );

  return (
    <div className="app home" {...dropProps}>
      {/* macOS 桌面版隐藏了原生标题栏，顶上这条用来拖动窗口；别处不显示 */}
      <div className="drag-strip" data-tauri-drag-region />

      <div className="home-split">
        {side}
        <main className="home-body">
          <div className="home-inner">
            <div className="home-head">
              <h1 className="sr-only">{t("brand.windowTitle")}</h1>
              <div
                className="home-tabs"
                role="tablist"
                onKeyDown={rovingKeys}
              >
                {tabs.map((v) => (
                  <button
                    key={v}
                    role="tab"
                    aria-selected={view === v}
                    tabIndex={view === v ? 0 : -1}
                    onClick={() => {
                      setQuery("");
                      setView(v);
                    }}
                  >
                    {tabLabel[v]}
                    {!!tabCount[v] && <span className="home-tab-count">{tabCount[v]}</span>}
                  </button>
                ))}
              </div>
              {view === "templates" && (
                <button
                  className="btn btn-sm"
                  onClick={() => tplInput.current?.click()}
                  disabled={!!busy}
                  title={t("templates.uploadTitle")}
                >
                  <Icon name="upload" size={14} />
                  {t("templates.upload")}
                </button>
              )}
              {!!count && (
                <label className="search">
                  <Icon name="search" size={15} />
                  <input
                    ref={searchRef}
                    className="input"
                    value={query}
                    onChange={(e) => setQuery(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Escape") {
                        setQuery("");
                        e.currentTarget.blur();
                      }
                      // 搜到唯一一个时回车直接打开
                      if (e.key === "Enter" && view !== "templates" && filtered?.length === 1 && !filtered[0].missing)
                        onOpen(filtered[0].id);
                    }}
                    placeholder={view === "templates" ? t("templates.search") : t("home.search")}
                    aria-label={view === "templates" ? t("templates.search") : t("home.search")}
                  />
                  {!query && <Kbd>/</Kbd>}
                </label>
              )}
            </div>

            {pathForm}
            {busyNote}

            {view === "templates" ? (
              <TemplateList
                items={templates.list}
                query={query}
                busy={!!busy}
                onUse={pickTemplate}
                onPreview={(tpl) => onPreview(tpl.id)}
                onChanged={templates.reload}
              />
            ) : shown?.length === 0 ? (
              <div className="home-empty">
                <Icon name="upload" size={28} />
                <div className="home-empty-title">{t("home.emptyTitle")}</div>
                <div>{t("home.emptyBody")}</div>
              </div>
            ) : (
            <div className="project-list">
              {shown === null &&
                [0, 1, 2].map((i) => (
                  <div key={i} className="project-row" aria-hidden="true">
                    <span className="project-icon folder" />
                    <span className="project-main">
                      <span className="skeleton" style={{ width: `${30 + i * 12}%` }} />
                      <span className="skeleton" style={{ width: `${50 - i * 8}%`, height: 10, marginTop: 6 }} />
                    </span>
                  </div>
                ))}
              {!!shown?.length && filtered?.length === 0 && <div className="list-empty">{t("home.noMatch", { query })}</div>}
              {filtered?.map((p) => {
                const go = () => !p.missing && renaming !== p.id && onOpen(p.id);
                return (
                  // 整行可点（鼠标），键盘和读屏走名字那个按钮：行里还有「更多」按钮，不能整行当按钮套按钮
                  <div
                    key={p.id}
                    className={`project-row${p.missing ? " gone" : ""}${renaming === p.id ? " renaming" : ""}`}
                    title={renaming === p.id ? undefined : p.missing ? t("home.missingTitle") : p.path}
                    onClick={go}
                  >
                    <span className={`project-icon${p.folder ? " folder" : ""}`}>
                      <Icon name={p.missing ? "alert" : p.folder ? "folder" : "tex"} size={18} />
                    </span>
                    <span className="project-main">
                      <span className="project-title">
                        {renaming === p.id ? (
                          <RenameInput
                            initial={p.name}
                            onCommit={(next) => rename(p.id, next)}
                            onCancel={() => setRenaming(null)}
                            confirm
                          />
                        ) : (
                          <button
                            type="button"
                            className="project-name project-open"
                            aria-disabled={p.missing || undefined}
                            onClick={(e) => {
                              e.stopPropagation();
                              go();
                            }}
                          >
                            {p.name}
                          </button>
                        )}
                        {p.folder && !p.missing && <span className="tag muted">{t("home.tagFolder")}</span>}
                        {p.missing && <span className="tag warn">{t("home.tagMissing")}</span>}
                      </span>
                      <span className="project-sub">
                        {p.folder ? <code>{tidy(p.path, home)}</code> : p.entry ? <code>{p.entry}</code> : t("home.noTexFile")}
                      </span>
                    </span>
                    <span className="project-time">{ago(p.modified, locale.code)}</span>
                    {renaming !== p.id && (
                      <span className={`row-actions${menu?.id === p.id ? " armed" : ""}`}>
                        <button
                          className={`btn btn-ghost btn-sm btn-icon${menu?.id === p.id ? " on" : ""}`}
                          title={t("home.more")}
                          aria-label={t("home.more")}
                          aria-haspopup="menu"
                          aria-expanded={menu?.id === p.id}
                          // 菜单开着时按下按钮：别让菜单的「点外面就收」先收掉，交给 onClick 收
                          onPointerDown={(e) => menu?.id === p.id && e.stopPropagation()}
                          onClick={(e) => {
                            e.stopPropagation();
                            if (menu?.id === p.id) return setMenu(null);
                            const r = e.currentTarget.getBoundingClientRect();
                            setMenu({ id: p.id, x: r.right, y: r.bottom + 4 });
                          }}
                        >
                          <Icon name="more" size={16} />
                        </button>
                      </span>
                    )}
                  </div>
                );
              })}
            </div>
            )}
          </div>
        </main>
      </div>
      {menuProject && menu && (
        <ContextMenu x={menu.x} y={menu.y} align="end" label={t("home.more")} items={rowMenu(menuProject)} onClose={() => setMenu(null)} />
      )}
      {dropOverlay}
      {tour?.kind && (
        <Onboarding
          kind={tour.kind}
          version={tour.version}
          projects={projects}
          engine={engine}
          bundledTex={bundledTex}
          onClose={onTourClose}
          onAction={tourAction}
        />
      )}
    </div>
  );
}
