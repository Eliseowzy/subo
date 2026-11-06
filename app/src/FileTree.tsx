import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent, type MouseEvent } from "react";
import { api, projectApi, type Entry, type Heading, type ProjectSummary } from "./api";
import { kindOf } from "./attach";
import Icon, { fileIcon } from "./Icon";
import { t, useLocale } from "./i18n";
import NewProject from "./NewProject";
import ContextMenu, { SEP, type MenuEntry } from "./ContextMenu";
import { savedToast } from "./Templates";
import RenameInput from "./Rename";
import { Kbd, toast, toastError } from "./ui";
import { writeText } from "./clipboard";
import { Gutter } from "./Split";

type Props = {
  /** 当前打开的项目 */
  id: string;
  files: Entry[];
  /** 主文件，树里标一下 */
  entry: string | null;
  active: string | null;
  /** 模板预览时给模板的显示名：侧栏只列这一个，不能改名、不能新建 */
  preview?: string;
  /** 当前项目已归档：侧栏只列它自己，不和常用项目混在一起 */
  archived?: boolean;
  onOpen: (path: string) => void;
  onSwitch: (id: string) => void;
  /** 改项目名；当前项目改名后由上层切到新 id */
  onRename: (id: string, name: string) => Promise<void>;
  onQuickOpen: () => void;
  /** 菜单里归档、取消归档了某个项目；当前项目要据此切换只读 */
  onArchived?: (id: string, archived: boolean) => void;
  /** 当前项目的文档大纲（跨 \input 按文档顺序） */
  outline?: Heading[];
  /** 正在看的位置：大纲里高亮它所在的那一节 */
  here?: { file: string; line: number } | null;
  onOutline?: (file: string, line: number) => void;
  /** 当前项目菜单里的「导出字盘快照」；不传则不出这一项 */
  onExportSnapshot?: () => void;
  /** 文件操作之前：把没存的编辑写掉，免得改名、删除后又写回旧路径 */
  onBeforeFs?: () => Promise<void>;
  /** 文件操作做完：from 是改名 / 删除前的路径（新建、副本为 null），to 是新路径（删除为 null）。
   *  上层据此刷新目录，打开着的文件跟着换 */
  onFsChange?: (from: string | null, to: string | null) => void;
};

/** 文件名的字数上限：多数文件系统是 255 */
const FILE_MAX = 255;

const parentOf = (path: string) => path.slice(0, Math.max(0, path.lastIndexOf("/")));
const join = (dir: string, name: string) => (dir ? `${dir}/${name}` : name);
const baseName = (path: string) => path.slice(path.lastIndexOf("/") + 1);
/** 改名时只选中主干，不选扩展名；点开头的文件、文件夹全选 */
const stemRange = (name: string, dir: boolean): [number, number] => {
  const dot = name.lastIndexOf(".");
  return [0, !dir && dot > 0 ? dot : name.length];
};

/** 回车、空格与点击同效，键盘也能走文件树 */
const press = (run: () => void) => ({
  onClick: run,
  onKeyDown: (e: KeyboardEvent) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      run();
    }
  },
  tabIndex: 0,
  role: "treeitem",
});

type Node = { name: string; path: string; dir: boolean; children: Node[] };

/** 平铺的路径列表 → 嵌套树；目录在前，同类按名排序；点开头的隐藏项不显示 */
function build(files: Entry[]): Node[] {
  const root: Node = { name: "", path: "", dir: true, children: [] };
  const dirs = new Map<string, Node>([["", root]]);
  const ensure = (path: string): Node => {
    const hit = dirs.get(path);
    if (hit) return hit;
    const cut = path.lastIndexOf("/");
    const node: Node = { name: path.slice(cut + 1), path, dir: true, children: [] };
    ensure(cut < 0 ? "" : path.slice(0, cut)).children.push(node);
    dirs.set(path, node);
    return node;
  };
  for (const f of files) {
    if (f.path.split("/").some((p) => p.startsWith("."))) continue;
    if (f.dir) {
      ensure(f.path);
    } else {
      const cut = f.path.lastIndexOf("/");
      ensure(cut < 0 ? "" : f.path.slice(0, cut)).children.push({
        name: f.path.slice(cut + 1),
        path: f.path,
        dir: false,
        children: [],
      });
    }
  }
  const sort = (n: Node) => {
    n.children.sort((a, b) => Number(b.dir) - Number(a.dir) || a.name.localeCompare(b.name));
    n.children.forEach(sort);
  };
  sort(root);
  return root.children;
}

/** 当前文件的所有上级目录，初始展开它们 */
function ancestors(path: string | null): string[] {
  if (!path) return [];
  const parts = path.split("/").slice(0, -1);
  return parts.map((_, i) => parts.slice(0, i + 1).join("/"));
}

export default function FileTree({
  id,
  files,
  entry,
  active,
  preview,
  archived,
  onOpen,
  onSwitch,
  onRename,
  onQuickOpen,
  onArchived,
  outline,
  here,
  onOutline,
  onExportSnapshot,
  onBeforeFs,
  onFsChange,
}: Props) {
  useLocale();
  const [projects, setProjects] = useState<ProjectSummary[]>([]);
  const [open, setOpen] = useState(true);
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());
  const [menu, setMenu] = useState<{ id: string; x: number; y: number } | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  const tree = useMemo(() => build(files), [files]);
  const papi = useMemo(() => projectApi(id), [id]);
  /** 模板预览、已归档：只能看，不能动文件 */
  const locked = preview !== undefined || !!archived;
  const [fileMenu, setFileMenu] = useState<{ path: string; dir: boolean; x: number; y: number } | null>(null);
  const [fileRenaming, setFileRenaming] = useState<string | null>(null);
  /** 正在新建：在 parent 目录（项目根为空串）顶上出一行输入框 */
  const [creating, setCreating] = useState<{ parent: string; dir: boolean } | null>(null);

  const reload = useCallback(() => api.projects().then(setProjects).catch(() => {}), []);
  // 当前项目归档状态一变（顶栏取消归档）就重拿列表，侧栏跟着换
  useEffect(() => void reload(), [reload, archived]);
  const closeMenu = useCallback(() => setMenu(null), []);
  const closeFileMenu = useCallback(() => setFileMenu(null), []);

  /** 右键在鼠标处开；三个点和键盘（菜单键 / ⇧F10）从按钮或行的左下角开 */
  const openMenu = (e: MouseEvent, pid: string) => {
    e.preventDefault();
    e.stopPropagation();
    const fromPointer = e.type === "contextmenu" && (e.clientX || e.clientY);
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    setFileMenu(null);
    setMenu({ id: pid, x: fromPointer ? e.clientX : r.left, y: fromPointer ? e.clientY : r.bottom + 2 });
  };

  const openFileMenu = (e: MouseEvent, n: Node) => {
    e.preventDefault();
    e.stopPropagation();
    const fromPointer = e.clientX || e.clientY;
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    setMenu(null);
    setFileMenu({ path: n.path, dir: n.dir, x: fromPointer ? e.clientX : r.left, y: fromPointer ? e.clientY : r.bottom + 2 });
  };

  /** 新建：先展开目标目录，在它顶上出输入框 */
  const startCreate = (parent: string, dir: boolean) => {
    setOpen(true);
    if (parent) setExpanded((s) => new Set([...s, ...ancestors(`${parent}/x`)]));
    setFileRenaming(null);
    setCreating({ parent, dir });
  };

  // 出错时提示一下，再抛给输入框，让它留着等用户改
  const loud = async <T,>(work: Promise<T>): Promise<T> => {
    try {
      return await work;
    } catch (e) {
      toastError(e);
      throw e;
    }
  };

  const commitCreate = async (name: string) => {
    if (!creating) return;
    const path = join(creating.parent, name);
    await loud(papi.newFile(path, creating.dir));
    const dir = creating.dir;
    setCreating(null);
    onFsChange?.(null, path);
    if (dir) setExpanded((s) => new Set([...s, path]));
    else onOpen(path);
  };

  const commitRename = async (from: string, name: string) => {
    const to = join(parentOf(from), name);
    await onBeforeFs?.().catch(() => {});
    await loud(papi.renameFile(from, to));
    setFileRenaming(null);
    // 改的是文件夹：展开状态跟着换路径
    setExpanded((s) => new Set([...s].map((p) => (p === from || p.startsWith(`${from}/`) ? to + p.slice(from.length) : p))));
    onFsChange?.(from, to);
    toast(t("tree.fileMenu.renamed", { name }), "success");
  };

  const fileItems = (path: string, dir: boolean): MenuEntry[] => {
    const name = baseName(path);
    const here = dir ? path : parentOf(path);
    return [
      ...(dir ? [] : [{ label: t("tree.fileMenu.open"), run: () => onOpen(path) }, SEP]),
      ...(locked
        ? []
        : [
            { icon: "plus" as const, label: t("tree.fileMenu.newFile"), run: () => startCreate(here, false) },
            { icon: "folder" as const, label: t("tree.fileMenu.newFolder"), run: () => startCreate(here, true) },
            SEP,
            {
              icon: "pencil" as const,
              label: t("tree.fileMenu.rename"),
              run: () => {
                setCreating(null);
                setFileRenaming(path);
              },
            },
            {
              icon: "copy" as const,
              label: t("tree.fileMenu.duplicate"),
              run: () =>
                void (onBeforeFs?.() ?? Promise.resolve())
                  .catch(() => {})
                  .then(() => papi.duplicateFile(path, t("tree.fileMenu.copySuffix")))
                  .then((r) => onFsChange?.(null, r.path))
                  .catch(toastError),
            },
          ]),
      SEP,
      {
        label: t("tree.fileMenu.copyPath"),
        run: () =>
          void writeText(path)
            .then(() => toast(t("tree.fileMenu.pathCopied", { path }), "success"))
            .catch(toastError),
      },
      ...(preview !== undefined
        ? []
        : [{ icon: "reveal" as const, label: t("tree.fileMenu.reveal"), run: () => void api.reveal(id, path).catch(toastError) }]),
      ...(locked
        ? []
        : [
            SEP,
            {
              icon: "trash" as const,
              label: t("tree.fileMenu.delete"),
              run: () =>
                void (onBeforeFs?.() ?? Promise.resolve())
                  .catch(() => {})
                  .then(() => papi.deleteFile(path))
                  .then(() => {
                    onFsChange?.(path, null);
                    toast(t("tree.fileMenu.trashed", { name }), "success");
                  })
                  .catch(toastError),
            },
          ]),
    ];
  };

  /** 新建时的输入行 */
  const draft = (depth: number) =>
    creating && (
      <li key="__draft">
        <div className="tree-item tree-file" style={{ paddingLeft: 8 + depth * 16 }}>
          <span className="caret-spacer" />
          <Icon name={creating.dir ? "folder" : "file"} size={15} />
          <RenameInput
            initial=""
            className="tree-rename"
            max={FILE_MAX}
            onCommit={commitCreate}
            onCancel={() => setCreating(null)}
          />
        </div>
      </li>
    );

  const menuItems = (p: ProjectSummary): MenuEntry[] => [
    ...(p.id === id && !locked
      ? [
          { icon: "plus" as const, label: t("tree.fileMenu.newFile"), run: () => startCreate("", false) },
          { icon: "folder" as const, label: t("tree.fileMenu.newFolder"), run: () => startCreate("", true) },
          SEP,
        ]
      : []),
    {
      icon: "pencil",
      label: t("tree.rename"),
      disabled: p.folder ? t("server.rename.folder") : undefined,
      run: () => setRenaming(p.id),
    },
    {
      icon: "reveal",
      label: t("tree.reveal"),
      run: () => void api.reveal(p.id).catch(toastError),
    },
    {
      icon: "template",
      label: t("templates.save"),
      // 只查形状不编译，存的是磁盘上的版本：编辑器里没保存的改动不在里面
      run: () =>
        void api
          .saveTemplate(p.id, p.name)
          .then((r) => savedToast("saved", r))
          .catch(toastError),
    },
    ...(onExportSnapshot && p.id === id
      ? [{ icon: "package" as const, label: t("tree.exportSnapshot"), run: onExportSnapshot }]
      : []),
    {
      icon: p.archived ? "unarchive" : "archive",
      label: p.archived ? t("home.unarchive") : t("home.archive"),
      run: () =>
        void api
          .archive(p.id, !p.archived)
          .then(() => toast(t(p.archived ? "home.unarchived" : "home.archived", { name: p.name }), "success"))
          .then(() => onArchived?.(p.id, !p.archived))
          .then(reload)
          .catch(toastError),
    },
  ];

  const rename = async (pid: string, next: string) => {
    try {
      await onRename(pid, next);
    } catch (e) {
      toastError(e);
      throw e;
    }
    setRenaming(null);
    toast(t("tree.renamed", { name: next }), "success");
    await reload();
  };

  // 打开的文件所在的目录要展开，不然高亮项藏在折叠里
  useEffect(() => {
    const need = ancestors(active);
    if (need.length) setExpanded((s) => new Set([...s, ...need]));
  }, [active]);

  const toggle = (path: string) =>
    setExpanded((s) => {
      const next = new Set(s);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  /** 行里的名字；正在改名时换成输入框 */
  const nameOf = (n: Node) =>
    fileRenaming === n.path ? (
      <RenameInput
        initial={n.name}
        className="tree-rename"
        max={FILE_MAX}
        select={stemRange(n.name, n.dir)}
        onCommit={(next) => commitRename(n.path, next)}
        onCancel={() => setFileRenaming(null)}
      />
    ) : (
      <span className="tree-name">{n.name}</span>
    );

  const render = (nodes: Node[], depth: number) =>
    nodes.map((n) => {
      const pad = { paddingLeft: 8 + depth * 16 };
      if (n.dir) {
        const isOpen = expanded.has(n.path);
        return (
          <li key={n.path}>
            <div
              className="tree-item tree-dir"
              style={pad}
              title={n.path}
              aria-expanded={isOpen}
              {...press(() => toggle(n.path))}
              onContextMenu={(e) => openFileMenu(e, n)}
            >
              <Icon name="chevron" className={`caret${isOpen ? " open" : ""}`} size={14} />
              <Icon name={isOpen ? "folderOpen" : "folder"} size={15} />
              {nameOf(n)}
            </div>
            {isOpen && (
              <ul role="group" className="tree-group" style={{ "--d": depth } as CSSProperties}>
                {creating?.parent === n.path && draft(depth + 1)}
                {render(n.children, depth + 1)}
              </ul>
            )}
          </li>
        );
      }
      // 浏览器也显示不了的二进制：灰显，点开只有文件信息
      const binary = kindOf(n.name) === "binary";
      const cls = ["tree-item", "tree-file"];
      if (n.path === active) cls.push("active");
      if (binary) cls.push("binary");
      return (
        <li key={n.path}>
          <div
            className={cls.join(" ")}
            style={pad}
            title={binary ? t("tree.noPreview", { path: n.path }) : n.path}
            aria-selected={n.path === active}
            {...press(() => onOpen(n.path))}
            onContextMenu={(e) => openFileMenu(e, n)}
          >
            <span className="caret-spacer" />
            <Icon name={fileIcon(n.name)} size={15} />
            {nameOf(n)}
            {n.path === entry && (
              <span className="tree-entry" title={t("tree.entryTitle")}>
                {t("tree.entry")}
              </span>
            )}
          </div>
        </li>
      );
    });

  // 列表还没回来时也要先把当前项目显示出来
  const self = { id, name: id, path: "", entry: null, modified: 0, folder: false, missing: false, archived: !!archived };
  // 侧栏只列当前项目，切换项目走顶栏的下拉
  const mine = projects.find((p) => p.id === id) ?? self;
  const list =
    preview !== undefined
      ? [{ ...self, name: preview }]
      : archived
        ? [{ ...mine, archived: true }]
        : [mine];

  return (
    <aside className="tree">
      <div className="tree-scroll">
        <div className="tree-section">
          {preview !== undefined ? (
            <span>{t("tree.preview")}</span>
          ) : archived ? (
            <span>{t("tree.archived")}</span>
          ) : (
            <>
              <span>{t("tree.current")}</span>
              <span className="tree-section-tools">
                <NewProject onOpen={onSwitch} />
              </span>
            </>
          )}
        </div>
        <ul role="tree">
          {list.map((p) => {
            const current = p.id === id;
            return (
              <li key={p.id}>
                <div
                  className={`tree-item tree-project${current ? " current" : ""}`}
                  style={{ paddingLeft: 8 }}
                  title={p.path || p.name}
                  aria-expanded={current ? open : undefined}
                  {...press(() => setOpen((o) => !o))}
                  onContextMenu={(e) => (preview !== undefined ? e.preventDefault() : openMenu(e, p.id))}
                >
                  <Icon name="chevron" className={`caret${current && open ? " open" : ""}`} size={14} />
                  <span className="tree-monogram" aria-hidden="true">
                    {[...p.name.trim()][0]?.toUpperCase() ?? "?"}
                  </span>
                  {renaming === p.id ? (
                    <RenameInput
                      initial={p.name}
                      className="tree-rename"
                      onCommit={(next) => rename(p.id, next)}
                      onCancel={() => setRenaming(null)}
                    />
                  ) : (
                    <>
                      <span className="tree-name">{p.name}</span>
                      {preview === undefined && (
                        <button
                          className={`tree-more${menu?.id === p.id ? " on" : ""}`}
                          title={t("tree.more")}
                          aria-label={t("tree.more")}
                          aria-haspopup="menu"
                          aria-expanded={menu?.id === p.id}
                          tabIndex={-1}
                          onPointerDown={(e) => e.stopPropagation()}
                          onClick={(e) => (menu?.id === p.id ? (e.stopPropagation(), closeMenu()) : openMenu(e, p.id))}
                          onKeyDown={(e) => e.stopPropagation()}
                        >
                          <Icon name="more" size={16} />
                        </button>
                      )}
                    </>
                  )}
                </div>
                {current && open && (
                  <ul role="group" className="tree-group tree-files">
                    {creating?.parent === "" && draft(1)}
                    {render(tree, 1)}
                  </ul>
                )}
              </li>
            );
          })}
        </ul>
      </div>
      {outline && outline.length > 0 && (
        <Outline items={outline} here={here ?? null} onPick={(h) => onOutline?.(h.file, h.line)} />
      )}
      {menu && (() => {
        const p = list.find((x) => x.id === menu.id);
        return p && (
          <ContextMenu x={menu.x} y={menu.y} items={menuItems(p)} label={t("tree.menuLabel")} onClose={closeMenu} />
        );
      })()}
      {fileMenu && (
        <ContextMenu
          x={fileMenu.x}
          y={fileMenu.y}
          items={fileItems(fileMenu.path, fileMenu.dir)}
          label={t("tree.fileMenu.label")}
          onClose={closeFileMenu}
        />
      )}
      <div className="tree-foot">
        <button className="btn btn-ghost btn-sm" onClick={onQuickOpen} title={t("tree.findTitle")}>
          <Icon name="search" size={14} />
          <span className="tree-find-label">{t("tree.find")}</span>
          <Kbd>⌘P</Kbd>
        </button>
      </div>
    </aside>
  );
}

const OUTLINE_KEY = "subo.outline.v1";
const OUTLINE_DEFAULT = 260;
const OUTLINE_MIN = 80;
/** 大纲再高也给上面的项目列表留这么多 */
const OUTLINE_ROOM = 160;

/** 大纲的展开状态和高度只是本机偏好，读写失败就用默认值 */
function loadOutline(): { open: boolean; height: number } {
  try {
    const v = JSON.parse(localStorage.getItem(OUTLINE_KEY) ?? "{}") as { open?: boolean; height?: number };
    return { open: v.open !== false, height: Math.max(OUTLINE_MIN, Number(v.height) || OUTLINE_DEFAULT) };
  } catch {
    return { open: true, height: OUTLINE_DEFAULT };
  }
}

/** 文档大纲：钉在侧栏底部、自己滚动，顶上的拖动条调高度；按层级缩进，点一下跳到那一行；当前位置所在的那一节高亮 */
function Outline({
  items,
  here,
  onPick,
}: {
  items: Heading[];
  here: { file: string; line: number } | null;
  onPick: (h: Heading) => void;
}) {
  useLocale();
  const top = Math.min(...items.map((h) => h.level));
  // 当前文件里、行号不超过光标的最后一个标题
  const current = useMemo(() => {
    if (!here) return -1;
    let hit = -1;
    items.forEach((h, i) => {
      if (h.file === here.file && h.line <= here.line) hit = i;
    });
    return hit;
  }, [items, here]);
  const [pref, setPref] = useState(loadOutline);
  const { open, height } = pref;
  useEffect(() => {
    try {
      localStorage.setItem(OUTLINE_KEY, JSON.stringify(pref));
    } catch {
      /* 忽略 */
    }
  }, [pref]);
  const boxRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLUListElement>(null);

  // 光标走到别的小节时，把高亮的那一行滚进可视区
  useEffect(() => {
    if (current >= 0) listRef.current?.querySelector(".outline-item.active")?.scrollIntoView({ block: "nearest" });
  }, [current, open]);

  return (
    <>
      {open && (
        <Gutter
          axis="y"
          title={t("tree.gutterOutline")}
          onReset={() => setPref((p) => ({ ...p, height: OUTLINE_DEFAULT }))}
          onStart={() => {
            // 从实际显示高度起算：侧栏变矮时 CSS 会把存的值压下去
            const box = boxRef.current;
            const h0 = box?.offsetHeight ?? height;
            const hi = Math.max(OUTLINE_MIN, (box?.parentElement?.clientHeight ?? window.innerHeight) - OUTLINE_ROOM);
            // 往上拖（dy 为负）大纲变高
            return (dy) => setPref((p) => ({ ...p, height: Math.round(Math.max(OUTLINE_MIN, Math.min(hi, h0 - dy))) }));
          }}
        />
      )}
      <div
        className={`outline${open ? " open" : ""}`}
        ref={boxRef}
        style={open ? { height, maxHeight: `calc(100% - ${OUTLINE_ROOM}px)` } : undefined}
      >
        <div
          className="tree-section outline-head"
          {...press(() => setPref((p) => ({ ...p, open: !p.open })))}
          role="button"
          aria-expanded={open}
        >
          <span>
            <Icon name="chevron" className={`caret${open ? " open" : ""}`} size={13} /> {t("tree.outline")}
          </span>
        </div>
        {open && (
          <ul className="outline-list" ref={listRef}>
            {items.map((h, i) => (
              <li key={`${h.file}:${h.line}:${i}`}>
                <div
                  className={`tree-item outline-item level-${h.level}${i === current ? " active" : ""}`}
                  style={{ paddingLeft: 12 + Math.min(h.level - top, 4) * 12 }}
                  title={`${h.title} — ${h.file}:${h.line}`}
                  {...press(() => onPick(h))}
                >
                  <span className="tree-name">{h.title}</span>
                </div>
              </li>
            ))}
          </ul>
        )}
      </div>
    </>
  );
}
