// 项目模板：首页「模板」页签的列表，以及首页、+ 菜单共用的取用逻辑。
// 列表里点模板是只读预览（不进项目库），点「使用」才新建项目。
// 模板入库只查形状不编译；缺的宏包、字体等到用模板新建项目、首次编译时再补。
// 每个模板属于一个分类；预置模板随应用带、只读，名字和简介在语言文件里。

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, TEMPLATE_CATEGORIES, type TemplateCategory, type TemplateStored, type TemplateSummary } from "./api";
import ContextMenu, { type MenuItem } from "./ContextMenu";
import Icon from "./Icon";
import { t, useLocale, type Key } from "./i18n";
import { ago, fileSize, rovingKeys, toast, toastError } from "./ui";

const CATEGORY_KEYS: Record<TemplateCategory, Key> = {
  paper: "templates.category.paper",
  thesis: "templates.category.thesis",
  report: "templates.category.report",
  resume: "templates.category.resume",
  slides: "templates.category.slides",
  poster: "templates.category.poster",
  other: "templates.category.other",
};

/** 预置模板的文案：id → 语言文件里的名字、简介。服务端新增预置模板时这里跟着加 */
const PRESET_KEYS: Record<string, { name: Key; desc: Key }> = {
  "builtin:article-zh": { name: "templates.preset.articleZh", desc: "templates.preset.articleZhDesc" },
  "builtin:article-zh-tw": { name: "templates.preset.articleZhTw", desc: "templates.preset.articleZhTwDesc" },
  "builtin:paper-en": { name: "templates.preset.paperEn", desc: "templates.preset.paperEnDesc" },
  "builtin:paper-ja": { name: "templates.preset.paperJa", desc: "templates.preset.paperJaDesc" },
  "builtin:paper-ko": { name: "templates.preset.paperKo", desc: "templates.preset.paperKoDesc" },
  "builtin:thesis-zh": { name: "templates.preset.thesisZh", desc: "templates.preset.thesisZhDesc" },
  "builtin:report-zh": { name: "templates.preset.reportZh", desc: "templates.preset.reportZhDesc" },
  "builtin:resume-zh": { name: "templates.preset.resumeZh", desc: "templates.preset.resumeZhDesc" },
  "builtin:resume-en": { name: "templates.preset.resumeEn", desc: "templates.preset.resumeEnDesc" },
  "builtin:slides-zh": { name: "templates.preset.slidesZh", desc: "templates.preset.slidesZhDesc" },
  "builtin:poster": { name: "templates.preset.poster", desc: "templates.preset.posterDesc" },
};

export const categoryLabel = (c: TemplateCategory) => t(CATEGORY_KEYS[c] ?? CATEGORY_KEYS.other);

/** 显示名：预置模板查语言文件，用户模板就是存时起的名字 */
export function templateName(tpl: TemplateSummary): string {
  const k = PRESET_KEYS[tpl.id];
  return k ? t(k.name) : tpl.name;
}

/** 预置模板的一句话简介；用户模板没有 */
export function templateDesc(tpl: TemplateSummary): string | null {
  const k = PRESET_KEYS[tpl.id];
  return k ? t(k.desc) : null;
}

/** 按分类分组，组内保持服务端的顺序（用户模板新的在前，预置在后）；空组不出现 */
export function byCategory(list: TemplateSummary[]): [TemplateCategory, TemplateSummary[]][] {
  return TEMPLATE_CATEGORIES.map((c) => [c, list.filter((x) => x.category === c)] as [TemplateCategory, TemplateSummary[]]).filter(
    ([, xs]) => xs.length
  );
}

/** 模板列表；取不到时当空列表，错误照常提示 */
export function useTemplates() {
  const [list, setList] = useState<TemplateSummary[] | null>(null);
  const reload = useCallback(
    () =>
      api
        .templates()
        .then(setList)
        .catch((e) => {
          setList((l) => l ?? []);
          toastError(e);
        }),
    []
  );
  useEffect(() => void reload(), [reload]);
  return { list, reload };
}

/** 存模板成功的提示：略过了编译产物、杂项就顺带说一声 */
export function savedToast(key: "saved" | "uploaded", r: TemplateStored) {
  const name = r.name;
  const msg = r.droppedCount
    ? t(key === "saved" ? "templates.savedDropped" : "templates.uploadedDropped", { name, count: r.droppedCount })
    : t(key === "saved" ? "templates.saved" : "templates.uploaded", { name });
  toast(msg, "success");
}

/** 用模板新建项目：项目名取模板的显示名，重名由服务端加后缀。返回项目 id */
export async function fromTemplate(tpl: TemplateSummary): Promise<string> {
  const { id } = await api.useTemplate(tpl.id, templateName(tpl));
  // 依赖不在入库时核对，提醒一句去哪里看缺什么；预置模板只用内置的宏包和系统字体，不必提醒
  if (!tpl.builtin && (tpl.packages.length || tpl.fonts.length)) toast(t("templates.needsHint"));
  return id;
}

type Props = {
  items: TemplateSummary[] | null;
  query: string;
  busy: boolean;
  onUse: (tpl: TemplateSummary) => void;
  /** 只读预览：能编译、下载 PDF，不新建项目 */
  onPreview: (tpl: TemplateSummary) => void;
  /** 删除、改分类之后重新拉列表 */
  onChanged: () => void;
};

export default function TemplateList({ items, query, busy, onUse, onPreview, onChanged }: Props) {
  const locale = useLocale();
  const [cat, setCat] = useState<TemplateCategory | "all">("all");
  /** 删除要点两次：第一次只「上膛」，3 秒内再点才删 */
  const [armed, setArmed] = useState<string | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; tpl: TemplateSummary } | null>(null);
  const timer = useRef<number>();
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const closeMenu = useCallback(() => setMenu(null), []);

  const remove = async (tpl: TemplateSummary) => {
    if (armed !== tpl.id) {
      setArmed(tpl.id);
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setArmed(null), 3000);
      return;
    }
    setArmed(null);
    try {
      await api.removeTemplate(tpl.id);
      toast(t("templates.removed", { name: tpl.name }));
      onChanged();
    } catch (e) {
      toastError(e);
    }
  };

  const recategorize = async (tpl: TemplateSummary, next: TemplateCategory) => {
    if (next === tpl.category) return;
    try {
      await api.setTemplateCategory(tpl.id, next);
      toast(t("templates.moved", { name: tpl.name, category: categoryLabel(next) }), "success");
      onChanged();
    } catch (e) {
      toastError(e);
    }
  };

  const categoryMenu = (tpl: TemplateSummary): MenuItem[] =>
    TEMPLATE_CATEGORIES.map((c) => ({
      icon: c === tpl.category ? "check" : undefined,
      label: categoryLabel(c),
      run: () => void recategorize(tpl, c),
    }));

  // 搜索看名字、简介和分类名；分类条的计数跟着搜索走
  const q = query.trim().toLowerCase();
  const matched = useMemo(
    () =>
      (items ?? []).filter(
        (x) => !q || `${templateName(x)} ${templateDesc(x) ?? ""} ${categoryLabel(x.category)}`.toLowerCase().includes(q)
      ),
    // 语言切换后名字变了，要重算
    [items, q, locale]
  );
  const counts = useMemo(() => {
    const m = new Map<TemplateCategory, number>();
    for (const x of matched) m.set(x.category, (m.get(x.category) ?? 0) + 1);
    return m;
  }, [matched]);
  // 当前分类被搜没了，就回到「全部」的样子显示
  const active = cat !== "all" && counts.get(cat) ? cat : "all";
  const shown = active === "all" ? matched : matched.filter((x) => x.category === active);
  const mine = shown.filter((x) => !x.builtin);
  const presets = shown.filter((x) => x.builtin);
  const hasMine = items?.some((x) => !x.builtin);

  const row = (tpl: TemplateSummary) => {
    const go = () => !busy && onPreview(tpl);
    const name = templateName(tpl);
    const desc = templateDesc(tpl);
    return (
      // 整行可点（鼠标），键盘和读屏走名字那个按钮：行里还有「使用」「删除」按钮，不能整行当按钮套按钮
      <div key={tpl.id} className={`project-row${tpl.builtin ? " builtin" : ""}`} title={t("templates.previewTitle")} onClick={go}>
        <span className={`project-icon${tpl.builtin ? " builtin" : ""}`}>
          <Icon name="template" size={18} />
        </span>
        <span className="project-main">
          <span className="project-title">
            <button
              type="button"
              className="project-name project-open"
              onClick={(e) => {
                e.stopPropagation();
                go();
              }}
            >
              {name}
            </button>
            <span className="tag muted">{categoryLabel(tpl.category)}</span>
            {!tpl.builtin && !!tpl.packages.length && (
              <span className="tag muted" title={t("templates.packagesTitle", { list: tpl.packages.join(", ") })}>
                {t("templates.packages", { count: tpl.packages.length })}
              </span>
            )}
            {!tpl.builtin && !!tpl.fonts.length && (
              <span className="tag muted" title={t("templates.fontsTitle", { list: tpl.fonts.join(", ") })}>
                {t("templates.fonts", { count: tpl.fonts.length })}
              </span>
            )}
          </span>
          <span className="project-sub">
            {desc ??
              t("templates.sub", {
                entry: tpl.entry,
                files: t("templates.files", { count: tpl.files }),
                size: fileSize(tpl.bytes, locale.code),
              })}
          </span>
        </span>
        {!tpl.builtin && <span className="project-time">{ago(tpl.created, locale.code)}</span>}
        <span className={`row-actions${armed === tpl.id ? " armed" : ""}`}>
          {!tpl.builtin && armed !== tpl.id && (
            <button
              className="btn btn-ghost btn-sm"
              title={t("templates.categoryTitle")}
              aria-haspopup="menu"
              onPointerDown={(e) => e.stopPropagation()}
              onClick={(e) => {
                e.stopPropagation();
                const r = e.currentTarget.getBoundingClientRect();
                setMenu(menu?.tpl.id === tpl.id ? null : { x: r.left, y: r.bottom + 4, tpl });
              }}
            >
              {t("templates.categoryButton")}
              <Icon name="chevronDown" size={12} />
            </button>
          )}
          {!tpl.builtin && (
            <button
              className={`btn btn-ghost btn-sm${armed === tpl.id ? " btn-danger-solid" : " btn-icon btn-danger"}`}
              title={t("templates.remove")}
              aria-label={t("templates.remove")}
              onClick={(e) => {
                e.stopPropagation();
                void remove(tpl);
              }}
            >
              <Icon name="trash" size={15} />
              {armed === tpl.id && t("templates.removeConfirm")}
            </button>
          )}
          {armed !== tpl.id && (
            <button
              className="btn btn-sm btn-primary"
              title={t("templates.use")}
              disabled={busy}
              onClick={(e) => {
                e.stopPropagation();
                onUse(tpl);
              }}
            >
              {t("templates.useShort")}
            </button>
          )}
        </span>
      </div>
    );
  };

  if (items === null) return null;

  return (
    <div className="tpl">
      <div className="tpl-cats" role="tablist" aria-label={t("templates.categoriesLabel")} onKeyDown={rovingKeys}>
        {(["all", ...TEMPLATE_CATEGORIES] as const)
          .filter((c) => c === "all" || counts.get(c))
          .map((c) => (
            <button
              key={c}
              role="tab"
              aria-selected={active === c}
              tabIndex={active === c ? 0 : -1}
              className={`tpl-cat${active === c ? " on" : ""}`}
              onClick={() => setCat(c)}
            >
              {c === "all" ? t("templates.category.all") : categoryLabel(c)}
              <span className="tpl-cat-count">{c === "all" ? matched.length : counts.get(c)}</span>
            </button>
          ))}
      </div>

      {shown.length === 0 && <div className="list-empty">{t("templates.noMatchAll", { query })}</div>}

      {mine.length > 0 && (
        <section className="tpl-group">
          <h2 className="tpl-heading">{t("templates.mine")}</h2>
          <div className="project-list">{mine.map(row)}</div>
        </section>
      )}

      {presets.length > 0 && (
        <section className="tpl-group">
          <h2 className="tpl-heading">
            {t("templates.builtin")}
            <span className="tpl-heading-note">{t("templates.builtinNote")}</span>
          </h2>
          <div className="project-list">{presets.map(row)}</div>
        </section>
      )}

      {!hasMine && !q && active === "all" && (
        <div className="tpl-hint">
          <Icon name="info" size={15} />
          <span>{t("templates.mineEmpty")}</span>
        </div>
      )}

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={categoryMenu(menu.tpl)}
          label={t("templates.categoryTitle")}
          onClose={closeMenu}
        />
      )}
    </div>
  );
}
