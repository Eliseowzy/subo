// 工作区顶栏的项目切换：收起时显示当前项目名，点开列出所有项目（最近改过的在前），
// 项目多了顶上出搜索框。底下是「新建项目」，点了在原处换成新建菜单。

import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { api, type ProjectSummary } from "./api";
import Icon from "./Icon";
import { t, useLocale } from "./i18n";
import { NewProjectPanel } from "./NewProject";

/** 项目超过这么多才出搜索框 */
const SEARCH_MIN = 8;

type Props = {
  id: string;
  name: string;
  onSwitch: (id: string) => void;
};

export default function ProjectSwitch({ id, name, onSwitch }: Props) {
  useLocale();
  const [open, setOpen] = useState(false);
  const [projects, setProjects] = useState<ProjectSummary[] | null>(null);
  const [q, setQ] = useState("");
  const [sel, setSel] = useState(0);
  /** 列表换成了新建菜单 */
  const [creating, setCreating] = useState(false);
  const busy = useRef(false);
  const box = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const button = useRef<HTMLButtonElement>(null);

  const close = (refocus = false) => {
    // 正在新建、导入时不收，免得半路丢了进度
    if (busy.current) return;
    setOpen(false);
    setQ("");
    setCreating(false);
    if (refocus) button.current?.focus();
  };

  // 每次点开都重拿一遍：别处可能新建、改名、归档过
  useEffect(() => {
    if (!open) return;
    let live = true;
    api.projects().then(
      (ps) => live && setProjects(ps),
      () => live && setProjects([])
    );
    return () => {
      live = false;
    };
  }, [open]);

  // 点外面收起
  useEffect(() => {
    if (!open) return;
    const down = (e: PointerEvent) => !box.current?.contains(e.target as Node) && close();
    window.addEventListener("pointerdown", down);
    return () => window.removeEventListener("pointerdown", down);
  }, [open]);

  // 找不到的文件夹打不开，归档的不列；当前项目置顶，其余按最近修改
  const live = useMemo(
    () =>
      (projects ?? [])
        .filter((p) => !p.missing && !p.archived)
        .sort((a, b) => Number(b.id === id) - Number(a.id === id) || b.modified - a.modified),
    [projects, id]
  );
  const searchable = live.length > SEARCH_MIN;
  const shown = useMemo(() => {
    const needle = q.trim().toLowerCase();
    return needle ? live.filter((p) => p.name.toLowerCase().includes(needle) || p.id.toLowerCase().includes(needle)) : live;
  }, [live, q]);

  useEffect(() => setSel(0), [q, open]);
  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>(`[data-i="${sel}"]`)?.scrollIntoView({ block: "nearest" });
  }, [sel]);
  // 没有搜索框时把焦点放进列表，方向键能直接走
  useEffect(() => {
    if (open && projects && !searchable && !creating) listRef.current?.querySelector<HTMLElement>(`[data-i="0"]`)?.focus();
  }, [open, projects, searchable, creating]);

  const pick = (p: ProjectSummary | undefined) => {
    if (!p) return;
    close();
    if (p.id !== id) onSwitch(p.id);
  };

  const keys = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      if (creating) setCreating(false);
      else close(true);
    } else if (creating) {
      return;
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const n = shown.length;
      if (!n) return;
      const next = (sel + (e.key === "ArrowDown" ? 1 : n - 1)) % n;
      setSel(next);
      if (!searchable) listRef.current?.querySelector<HTMLElement>(`[data-i="${next}"]`)?.focus();
    } else if (e.key === "Enter" && searchable && (e.target as HTMLElement).tagName === "INPUT") {
      e.preventDefault();
      pick(shown[sel]);
    }
  };

  return (
    <div className="project-switch" ref={box} onKeyDown={open ? keys : undefined}>
      <button
        ref={button}
        className={`crumb-name crumb-switch${open ? " on" : ""}`}
        title={t("switcher.title")}
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={() => (open ? close() : setOpen(true))}
      >
        <span>{name}</span>
        <Icon name="chevronDown" size={14} />
      </button>
      {open && (
        <div className="menu switch-menu" role="dialog" aria-label={t("switcher.label")}>
          {creating ? (
            <NewProjectPanel
              onBusy={(b) => (busy.current = b)}
              onBack={() => setCreating(false)}
              onOpen={(pid) => {
                busy.current = false;
                close();
                onSwitch(pid);
              }}
            />
          ) : (
            <>
            {searchable && (
              <div className="switch-search">
                <Icon name="search" size={14} />
                <input
                  autoFocus
                  value={q}
                  onChange={(e) => setQ(e.target.value)}
                  placeholder={t("switcher.search", { count: live.length })}
                  aria-label={t("switcher.searchLabel")}
                />
              </div>
            )}
            <ul className="switch-list menu-scroll" role="listbox" ref={listRef} aria-label={t("switcher.label")}>
              {projects === null ? (
                <li className="menu-note">{t("switcher.loading")}</li>
              ) : shown.length === 0 ? (
                <li className="menu-note">{t("switcher.empty")}</li>
              ) : (
                shown.map((p, i) => (
                  <li key={p.id}>
                    <button
                      className={`menu-item switch-item${i === sel ? " sel" : ""}`}
                      data-i={i}
                      role="option"
                      aria-selected={p.id === id}
                      title={p.path || p.name}
                      tabIndex={searchable ? -1 : 0}
                      onMouseEnter={() => setSel(i)}
                      onClick={() => pick(p)}
                    >
                      <span className="tree-monogram" aria-hidden="true">
                        {[...p.name.trim()][0]?.toUpperCase() ?? "?"}
                      </span>
                      <span className="menu-label switch-name">{p.name}</span>
                      {p.folder && <span className="switch-tag">{t("home.tagFolder")}</span>}
                      {p.id === id ? <Icon name="check" size={14} /> : <span className="menu-icon-spacer" />}
                    </button>
                  </li>
                ))
              )}
            </ul>
              <div className="menu-sep" />
              <button className="menu-item" onClick={() => setCreating(true)}>
                <Icon name="plus" size={14} />
                <span className="menu-label">{t("switcher.create")}</span>
              </button>
            </>
          )}
        </div>
      )}
    </div>
  );
}
