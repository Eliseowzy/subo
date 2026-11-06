// 新建项目：文件树「当前项目」栏头的 + 菜单，和顶栏项目切换里的「新建项目」共用同一份内容。
// 不回首页也能新建空白项目、从模板建、导入 zip、打开本地文件夹。

import { useEffect, useId, useRef, useState, type FormEvent } from "react";
import { api, NAME_MAX, type TemplateSummary } from "./api";
import { ARCHIVE_ACCEPT, archiveBase, isArchive } from "./archive";
import Icon from "./Icon";
import { t } from "./i18n";
import { nameLength } from "./Rename";
import { byCategory, categoryLabel, fromTemplate, templateName } from "./Templates";
import { Spinner, toast, toastError } from "./ui";

/** list：四个入口；name：填新项目名；path：系统选择框用不了时手填路径；templates：挑模板 */
type Mode = "list" | "name" | "path" | "templates";

export default function NewProject({ onOpen }: { onOpen: (id: string) => void }) {
  const [open, setOpen] = useState(false);
  const busy = useRef(false);
  const box = useRef<HTMLDivElement>(null);

  // 正在新建、导入时不收，免得半路丢了进度
  const close = () => !busy.current && setOpen(false);

  // 点菜单外面或按 Esc 收起
  useEffect(() => {
    if (!open) return;
    const down = (e: PointerEvent) => !box.current?.contains(e.target as Node) && close();
    const key = (e: KeyboardEvent) => e.key === "Escape" && close();
    window.addEventListener("pointerdown", down);
    window.addEventListener("keydown", key);
    return () => {
      window.removeEventListener("pointerdown", down);
      window.removeEventListener("keydown", key);
    };
  });

  return (
    <div className="new-project" ref={box}>
      <button
        className={`btn btn-ghost btn-sm btn-icon${open ? " on" : ""}`}
        onClick={() => (open ? close() : setOpen(true))}
        title={t("tree.newTitle")}
        aria-label={t("tree.newTitle")}
        aria-expanded={open}
      >
        <Icon name="plus" size={14} />
      </button>
      {open && (
        <div className="menu" role="menu">
          <NewProjectPanel
            onBusy={(b) => (busy.current = b)}
            onOpen={(id) => {
              busy.current = false;
              setOpen(false);
              onOpen(id);
            }}
          />
        </div>
      )}
    </div>
  );
}

type PanelProps = {
  onOpen: (id: string) => void;
  /** 新建、导入进行中：外层据此别收起菜单 */
  onBusy?: (busy: boolean) => void;
  /** 在四个入口那一层给个「返回」；不传就没有 */
  onBack?: () => void;
};

/** 新建项目的菜单内容：四个入口，以及填名字、填路径、挑模板这几步。外层负责弹出和收起 */
export function NewProjectPanel({ onOpen, onBusy, onBack }: PanelProps) {
  const [mode, setMode] = useState<Mode>("list");
  const [text, setText] = useState("");
  const [busy, setBusyState] = useState<string | null>(null);
  const [templates, setTemplates] = useState<TemplateSummary[] | null>(null);
  const fieldId = useId();
  const zip = useRef<HTMLInputElement>(null);

  const setBusy = (b: string | null) => {
    setBusyState(b);
    onBusy?.(b !== null);
  };

  const back = () => {
    setMode("list");
    setText("");
  };

  const run = async (label: string, work: () => Promise<string | null>) => {
    setBusy(label);
    try {
      const id = await work();
      setBusy(null);
      if (id) onOpen(id);
    } catch (e) {
      setBusy(null);
      toastError(e);
    }
  };

  const create = (e: FormEvent) => {
    e.preventDefault();
    if (nameLength(text) > NAME_MAX) return;
    void run(t("home.creating"), async () => (await api.create(text.trim() || t("home.untitled"))).id);
  };

  const pickFolder = () =>
    void run(t("home.picking"), async () => {
      let picked: string | null;
      try {
        picked = (await api.pickFolder()).path;
      } catch {
        // 系统选择框用不了，退回手动填路径
        setMode("path");
        return null;
      }
      return picked ? (await api.openFolder(picked)).id : null;
    });

  const openPath = (e: FormEvent) => {
    e.preventDefault();
    if (!text.trim()) return;
    void run(t("home.opening"), async () => (await api.openFolder(text.trim())).id);
  };

  const importArchive = (file: File | undefined) => {
    if (!file) return;
    if (!isArchive(file.name)) return void toast(t("home.zipOnly"), "error");
    const base = archiveBase(file.name);
    void run(t("home.importing", { file: file.name }), async () => {
      const { id } = await api.importArchive(file, base);
      if (nameLength(base) > NAME_MAX) toast(t("home.importedTruncated", { max: NAME_MAX, id }));
      return id;
    });
  };

  const showTemplates = () => {
    setTemplates(null);
    setMode("templates");
    api.templates().then(setTemplates, (e) => {
      setTemplates([]);
      toastError(e);
    });
  };

  const tooLong = nameLength(text) > NAME_MAX;

  return (
    <>
      <input
        ref={zip}
        type="file"
        accept={ARCHIVE_ACCEPT}
        hidden
        onChange={(e) => {
          importArchive(e.target.files?.[0]);
          e.target.value = "";
        }}
      />
      {busy ? (
        <div className="menu-busy">
          <Spinner size={13} /> {busy}
        </div>
      ) : mode === "name" ? (
        <form className="menu-form" onSubmit={create}>
          <label htmlFor={fieldId}>{t("home.newNameLabel")}</label>
          <input
            id={fieldId}
            className="field"
            autoFocus
            value={text}
            placeholder={t("home.newNamePlaceholder")}
            onChange={(e) => setText(e.target.value)}
          />
          {tooLong && <div className="menu-error">{t("tree.nameTooLong", { max: NAME_MAX })}</div>}
          <div className="menu-actions">
            <button type="button" className="btn btn-ghost btn-sm" onClick={back}>
              {t("common.cancel")}
            </button>
            <button type="submit" className="btn btn-primary btn-sm" disabled={tooLong}>
              {t("common.create")}
            </button>
          </div>
        </form>
      ) : mode === "templates" ? (
        <>
          {templates === null ? (
            <div className="menu-busy">
              <Spinner size={13} />
            </div>
          ) : templates.length === 0 ? (
            <div className="menu-note">{t("templates.menuEmpty")}</div>
          ) : (
            // 按分类分组；组里自己的模板在前、预置在后
            <div className="menu-scroll">
              {byCategory(templates).map(([cat, list]) => (
                <div key={cat} role="group" aria-label={categoryLabel(cat)}>
                  <div className="menu-heading">{categoryLabel(cat)}</div>
                  {list.map((tpl) => (
                    <button
                      key={tpl.id}
                      role="menuitem"
                      className="menu-item"
                      onClick={() => void run(t("templates.creating"), () => fromTemplate(tpl))}
                    >
                      <Icon name="template" size={15} />
                      <span className="menu-label">{templateName(tpl)}</span>
                    </button>
                  ))}
                </div>
              ))}
            </div>
          )}
          <div className="menu-actions">
            <button type="button" className="btn btn-ghost btn-sm" onClick={back}>
              {t("common.back")}
            </button>
          </div>
        </>
      ) : mode === "path" ? (
        <form className="menu-form" onSubmit={openPath}>
          <label htmlFor={fieldId}>{t("tree.pathLabel")}</label>
          <input
            id={fieldId}
            className="field"
            autoFocus
            value={text}
            placeholder={t("home.pathPlaceholder")}
            onChange={(e) => setText(e.target.value)}
          />
          <div className="menu-actions">
            <button type="button" className="btn btn-ghost btn-sm" onClick={back}>
              {t("common.cancel")}
            </button>
            <button type="submit" className="btn btn-primary btn-sm" disabled={!text.trim()}>
              {t("common.open")}
            </button>
          </div>
        </form>
      ) : (
        <>
          <button role="menuitem" className="menu-item" onClick={() => setMode("name")}>
            <Icon name="tex" size={15} />
            <span>{t("home.newBlank")}</span>
          </button>
          <button role="menuitem" className="menu-item" onClick={showTemplates}>
            <Icon name="template" size={15} />
            <span>{t("templates.fromTemplate")}</span>
          </button>
          <button role="menuitem" className="menu-item" onClick={() => zip.current?.click()}>
            <Icon name="zip" size={15} />
            <span>{t("home.importZip")}</span>
          </button>
          <button role="menuitem" className="menu-item" onClick={pickFolder}>
            <Icon name="folder" size={15} />
            <span>{t("home.openFolder")}</span>
          </button>
          {onBack && (
            <div className="menu-actions">
              <button type="button" className="btn btn-ghost btn-sm" onClick={onBack}>
                {t("common.back")}
              </button>
            </div>
          )}
        </>
      )}
    </>
  );
}
