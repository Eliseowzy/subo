import { useEffect, useMemo, useState } from "react";
import { fail } from "./api";
import { codepoints, formatSize, readFontMeta, type FontMeta, type Kind } from "./attach";
import Icon from "./Icon";
import { t, useLocale } from "./i18n";
import Preview from "./Preview";
import { Spinner, toast } from "./ui";

type Props = { path: string; kind: Exclude<Kind, "text">; url: string };

/** 编辑栏里的附件预览：图片、字体、PDF；其余二进制只给文件信息 */
export default function Attachment({ path, kind, url }: Props) {
  useLocale();
  if (kind === "pdf") return <Preview url={url} label={t("attach.pdfLabel")} />;
  return <Loaded path={path} kind={kind} url={url} />;
}

function Loaded({ path, kind, url }: Props) {
  const [blob, setBlob] = useState<Blob | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setBlob(null);
    setError(null);
    fetch(url, { cache: "no-store" })
      .then(async (r) => {
        if (!r.ok) await fail(r);
        // 旧版服务没有附件接口时会退回前端页面，别把 HTML 当图片/字体去解
        if (r.headers.get("content-type")?.startsWith("text/html")) {
          throw new Error(t("attach.staleServer"));
        }
        return r.blob();
      })
      .then((b) => !cancelled && setBlob(b))
      .catch((e) => !cancelled && setError(e instanceof Error ? e.message : String(e)));
    return () => {
      cancelled = true;
    };
  }, [url]);

  const name = path.slice(path.lastIndexOf("/") + 1);
  if (error) return <div className="attach-body"><div className="preview-error">{error}</div></div>;
  if (!blob) return <div className="attach-body"><div className="attach-loading"><Spinner size={18} /></div></div>;
  if (kind === "image") return <ImageView blob={blob} />;
  if (kind === "font") return <FontView blob={blob} path={path} />;
  return (
    <div className="attach-body">
      <div className="attach-info">
        <Icon name="binary" size={36} />
        <div className="attach-title">{name}</div>
        <div>{t("attach.noPreview", { size: formatSize(blob.size) })}</div>
        <a className="btn btn-sm" href={url} download={name} style={{ textDecoration: "none" }}>
          <Icon name="download" size={14} /> {t("common.download")}
        </a>
      </div>
    </div>
  );
}

function useObjectUrl(blob: Blob) {
  const src = useMemo(() => URL.createObjectURL(blob), [blob]);
  useEffect(() => () => URL.revokeObjectURL(src), [src]);
  return src;
}

function ImageView({ blob }: { blob: Blob }) {
  useLocale();
  const src = useObjectUrl(blob);
  const [fit, setFit] = useState(true);
  const [dims, setDims] = useState<[number, number] | null>(null);
  const [broken, setBroken] = useState(false);

  return (
    <>
      <div className="attach-bar">
        <span className="seg">
          <button className={fit ? "on" : ""} onClick={() => setFit(true)} title={t("attach.fitTitle")}>
            {t("attach.fit")}
          </button>
          <button className={fit ? "" : "on"} onClick={() => setFit(false)} title={t("attach.actualTitle")}>
            {t("attach.actual")}
          </button>
        </span>
        <span className="dim">
          {dims && `${dims[0]} × ${dims[1]} · `}
          {formatSize(blob.size)}
        </span>
      </div>
      <div className={`attach-body image-stage${fit ? " fit" : ""}`}>
        {broken ? (
          <div className="preview-error">{t("attach.badImage")}</div>
        ) : (
          <img
            src={src}
            alt=""
            onLoad={(e) => setDims([e.currentTarget.naturalWidth, e.currentTarget.naturalHeight])}
            onError={() => setBroken(true)}
          />
        )}
      </div>
    </>
  );
}

const SIZES = [12, 18, 28, 44];
// 试排样张是测字体覆盖面的内容，不随界面语言变：有中文字形就排中文
const SAMPLE_ZH = "永和九年，岁在癸丑。素帛 Subo"; // i18n-ignore
const SAMPLE_EN = "The quick brown fox jumps over the lazy dog.";
const CHARSET = [
  "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
  "abcdefghijklmnopqrstuvwxyz",
  "0123456789 ,.;:!?'\"()[]{}&@#%*+-=/\\",
];
const HAN = 0x6c38; // 「永」，用来判断有没有中文字形
/** 图标字体一次列出的字形数上限 */
const GRID_MAX = 800;

let faceSeq = 0;

const copy = (text: string) =>
  navigator.clipboard?.writeText(text).then(
    () => toast(t("attach.copied", { text: text.length > 40 ? text.slice(0, 40) + "…" : text }), "success"),
    () => toast(t("common.copyFailed"), "error")
  );

function FontView({ blob, path }: { blob: Blob; path: string }) {
  const locale = useLocale();
  const [meta, setMeta] = useState<FontMeta | null>(null);
  const [family, setFamily] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [text, setText] = useState("");

  useEffect(() => {
    let cancelled = false;
    let face: FontFace | null = null;
    (async () => {
      const buf = await blob.arrayBuffer();
      const m = await readFontMeta(buf);
      if (cancelled) return;
      setMeta(m);
      // 每次一个新名字，同名文件换了内容也不会拿到旧字体
      const name = `subo-preview-${++faceSeq}`;
      face = new FontFace(name, buf);
      await face.load();
      if (cancelled) return;
      document.fonts.add(face);
      setFamily(name);
    })().catch((e) => !cancelled && setError(t("attach.fontFailed", { error: e?.message ?? e })));
    return () => {
      cancelled = true;
      if (face) document.fonts.delete(face);
    };
  }, [blob]);

  const hasHan = meta?.covers?.(HAN);
  // 连字母都没有的多半是图标/符号字体（如 FontAwesome），试排文字毫无意义，改列字形表
  const symbolic = meta?.covers !== undefined && !meta.covers(0x61) && !meta.covers(HAN);
  const grid = useMemo(
    () => (symbolic && meta?.ranges ? codepoints(meta.ranges, GRID_MAX) : []),
    [symbolic, meta]
  );
  const sample = text || (hasHan === false ? SAMPLE_EN : `${SAMPLE_ZH} ${SAMPLE_EN}`);
  const file = path.slice(path.lastIndexOf("/") + 1);
  const dir = path.includes("/") ? path.slice(0, path.lastIndexOf("/") + 1) : "./";
  // 符号字体不该当正文字体，单独定义一个字体命令再配合 \\symbol 用
  const snippet = symbolic
    ? `\\newfontfamily\\symbolfont{${file}}[Path=${dir}]`
    : `\\setmainfont{${file}}[Path=${dir}]`;

  /** 字体里没有的字会由浏览器拿别的字体顶上，标灰免得误以为是它自己的 */
  const mark = (s: string) =>
    meta?.covers
      ? [...s].map((ch, i) =>
          /\s/.test(ch) || meta.covers!(ch.codePointAt(0)!) ? ch : (
            <span key={i} className="glyph-missing" title={t("attach.glyphMissing")}>
              {ch}
            </span>
          )
        )
      : s;

  return (
    <>
      <div className="attach-bar">
        {symbolic ? (
          <span className="dim">{t("attach.symbolInfo", { count: meta?.glyphs?.toLocaleString(locale.code) ?? "?" })}</span>
        ) : (
          <input
            className="input font-sample-input"
            placeholder={t("attach.samplePlaceholder")}
            aria-label={t("attach.samplePlaceholder")}
            value={text}
            onChange={(e) => setText(e.target.value)}
          />
        )}
        <span className="dim">{formatSize(blob.size)}</span>
      </div>
      <div className="attach-body font-view">
        {error && <div className="preview-error">{error}</div>}
        <dl className="font-meta">
          {meta?.family && (
            <>
              <dt>{t("attach.family")}</dt>
              <dd>
                {meta.family}
                {meta.localFamily && meta.localFamily !== meta.family && ` · ${meta.localFamily}`}
              </dd>
            </>
          )}
          {meta?.subfamily && (
            <>
              <dt>{t("attach.subfamily")}</dt>
              <dd>{meta.subfamily}</dd>
            </>
          )}
          {meta?.postscript && (
            <>
              <dt>{t("attach.postscript")}</dt>
              <dd><code>{meta.postscript}</code></dd>
            </>
          )}
          {meta?.version && (
            <>
              <dt>{t("attach.version")}</dt>
              <dd>{meta.version}</dd>
            </>
          )}
          {meta?.glyphs !== undefined && (
            <>
              <dt>{t("attach.glyphs")}</dt>
              <dd>
                {t("attach.glyphCount", { count: meta.glyphs.toLocaleString(locale.code) })}
                {hasHan !== undefined && (
                  <span className={`badge${hasHan ? " on" : ""}`}>{t(hasHan ? "attach.hasHan" : "attach.noHan")}</span>
                )}
                {meta.faces && meta.faces > 1 && <span className="dim">{t("attach.faces", { count: meta.faces })}</span>}
              </dd>
            </>
          )}
          <dt>fontspec</dt> {/* i18n-ignore：宏包名，不翻译 */}
          <dd>
            <code className="font-snippet">{snippet}</code>
            <button className="btn btn-sm" onClick={() => void copy(snippet)}>
              <Icon name="copy" size={13} /> {t("common.copy")}
            </button>
          </dd>
        </dl>
        {family && symbolic && (
          <div className="glyph-grid" style={{ fontFamily: `"${family}"` }}>
            {grid.map((cp) => {
              const hex = cp.toString(16).toUpperCase().padStart(4, "0");
              return (
                <button key={cp} className="glyph-cell" title={t("attach.glyphTitle", { hex, cmd: `\\symbol{"${hex}}` })}
                  onClick={() => void copy(`\\symbol{"${hex}}`)}>
                  <span className="glyph-char">{String.fromCodePoint(cp)}</span>
                  <span className="glyph-code">{hex}</span>
                </button>
              );
            })}
            {grid.length >= GRID_MAX && <div className="dim">{t("attach.gridLimit", { max: GRID_MAX })}</div>}
          </div>
        )}
        {family && !symbolic && (
          <div style={{ fontFamily: `"${family}"` }}>
            {SIZES.map((px) => (
              <div key={px} className="font-sample">
                <span className="font-size-tag">{px}</span>
                <div style={{ fontSize: px }}>{mark(sample)}</div>
              </div>
            ))}
            <div className="font-charset">
              {CHARSET.map((line) => (
                <div key={line}>{mark(line)}</div>
              ))}
            </div>
          </div>
        )}
      </div>
    </>
  );
}
