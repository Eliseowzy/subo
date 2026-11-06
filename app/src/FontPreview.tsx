import { type CSSProperties, useEffect, useState } from "react";
import { api, type FontFamily } from "./api";
import Icon from "./Icon";
import { t } from "./i18n";
import { Spinner, useDialog } from "./ui";

type Face = { style: string; family?: string; failed?: boolean };

let seq = 0;

/** 常规体排最前，粗体、斜体往后；同档按样式名里的字号数字从小到大 */
function rank(style: string): [number, number, string] {
  const w = style.toLowerCase();
  const tier = (/bold|black|heavy|semibold|demi/.test(w) ? 2 : 0) + (/italic|oblique/.test(w) ? 1 : 0);
  return [tier, parseFloat(w) || 0, w];
}

function byRank(a: string, b: string) {
  const [x, y] = [rank(a), rank(b)];
  return x[0] - y[0] || x[1] - y[1] || x[2].localeCompare(y[2]);
}

/** 把每个样式的字体文件装进页面，返回能写进 font-family 的名字；关掉时卸掉 */
function useFaces(font: FontFamily): Face[] | null {
  const [faces, setFaces] = useState<Face[] | null>(null);
  useEffect(() => {
    const styles = [...(font.previews ?? [])].sort(byRank);
    let alive = true;
    const added: FontFace[] = [];
    void Promise.all(
      styles.map(async (style): Promise<Face> => {
        const family = `subo-preview-${++seq}`;
        try {
          const face = await new FontFace(family, `url("${api.fontFileUrl(font, style)}")`).load();
          document.fonts.add(face);
          added.push(face);
          return { style, family };
        } catch {
          return { style, failed: true };
        }
      })
    ).then((r) => alive && setFaces(r));
    return () => {
      alive = false;
      added.forEach((f) => document.fonts.delete(f));
    };
  }, [font]);
  return faces;
}

/** 点字体卡片弹出的预览：每个样式一行，文字和字号可以改 */
export default function FontPreview({ font, onCopy, onClose }: { font: FontFamily; onCopy: () => void; onClose: () => void }) {
  const [text, setText] = useState(() => t("settings.fonts.previewText"));
  const [size, setSize] = useState(32);
  const faces = useFaces(font);

  // 设置页在 window 上听 Esc 回项目列表；useDialog 先接住，只关预览
  const dialog = useDialog(onClose);

  // 系统字体里 .ttc 集合的那些拿不到文件，但浏览器按名字就能用
  const byName = !font.previews?.length && font.source === "system";
  const sample = text || " ";

  return (
    <div className="overlay pack-overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="pack-dialog font-preview" role="dialog" aria-modal="true" aria-label={t("settings.fonts.preview", { name: font.name })} ref={dialog} tabIndex={-1}>
        <div className="pack-dialog-head">
          <span className="pack-dialog-title">{font.name}</span>
          <button className="btn btn-sm" onClick={onCopy}>
            <Icon name="copy" size={13} /> {t("settings.fonts.copy")}
          </button>
          <button className="btn btn-ghost btn-sm btn-icon" onClick={onClose} title={t("common.close")} aria-label={t("common.close")}>
            <Icon name="x" size={14} />
          </button>
        </div>
        <div className="fp-controls">
          <input
            className="input fp-text"
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder={t("settings.fonts.previewPlaceholder")}
            aria-label={t("settings.fonts.previewPlaceholder")}
            autoFocus
          />
          <label className="fp-size">
            <span className="dim">{t("settings.fonts.previewSize", { size })}</span>
            <input
              type="range"
              min={12}
              max={96}
              value={size}
              style={{ "--pct": `${((size - 12) / (96 - 12)) * 100}%` } as CSSProperties}
              onChange={(e) => setSize(Number(e.target.value))}
            />
          </label>
        </div>
        <div className="pack-dialog-body fp-body">
          {byName ? (
            <div className="fp-row">
              <div className="fp-style dim">{font.styles.join(" · ")}</div>
              <div className="fp-sample" style={{ fontFamily: `"${font.name}"`, fontSize: size }}>
                {sample}
              </div>
            </div>
          ) : !font.previews?.length ? (
            <div className="set-empty">{t("settings.fonts.previewNone")}</div>
          ) : !faces ? (
            <div className="set-empty">
              <Spinner size={14} /> {t("settings.fonts.previewLoading")}
            </div>
          ) : (
            faces.map((f) => (
              <div className="fp-row" key={f.style}>
                <div className="fp-style dim">{f.style}</div>
                {f.failed ? (
                  <div className="fp-failed">{t("settings.fonts.previewFailed")}</div>
                ) : (
                  <div className="fp-sample" style={{ fontFamily: `"${f.family}", var(--font-ui)`, fontSize: size }}>
                    {sample}
                  </div>
                )}
              </div>
            ))
          )}
        </div>
      </div>
    </div>
  );
}
