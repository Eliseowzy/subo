import { useEffect, useRef, useState } from "react";
import type { AiModel } from "./api";
import { PRESETS } from "./assist";
import Icon from "./Icon";
import { t, type Key } from "./i18n";

/*
 * ⌘K 的一行输入框：贴在选区（或光标）下面。写一句话回车，或点预置指令；Esc 关掉。
 * 结果不在这里显示——交给编辑器里的内联预览，这个框随即关掉。
 *
 * 选的模型能看图时，还能附图：粘贴、拖进框里或点按钮选。不能看图的模型下，框里没有任何和图有关的东西。
 */

/** 上次在框里挑的模型，同一次运行里记住 */
let lastModel: string | null = null;

export type AskPick = { instruction?: string; preset?: string; model?: string; images?: string[] };

/** 最多附几张（与服务端 ai.rs 的 IMAGES_MAX 一致） */
const IMAGES_MAX = 4;
/** 发出去前缩到长边这么长：公式、表格截图够看清，请求也不会太大 */
const IMAGE_EDGE = 1600;
/** 缩完 PNG 还比这大（照片之类）就改用 JPEG */
const PNG_MAX = 1.5 * 1024 * 1024;

/** 读图并缩小，返回 data URL；读不出来（不是图、格式不认）返回 null */
async function shrink(file: Blob): Promise<string | null> {
  let bmp: ImageBitmap;
  try {
    bmp = await createImageBitmap(file);
  } catch {
    return null;
  }
  const k = Math.min(1, IMAGE_EDGE / Math.max(bmp.width, bmp.height));
  const canvas = document.createElement("canvas");
  canvas.width = Math.max(1, Math.round(bmp.width * k));
  canvas.height = Math.max(1, Math.round(bmp.height * k));
  const g = canvas.getContext("2d");
  if (!g) return null;
  // 透明底的截图转 JPEG 会变黑，先铺白
  g.fillStyle = "#fff";
  g.fillRect(0, 0, canvas.width, canvas.height);
  g.drawImage(bmp, 0, 0, canvas.width, canvas.height);
  bmp.close();
  const png = canvas.toDataURL("image/png");
  return png.length <= PNG_MAX ? png : canvas.toDataURL("image/jpeg", 0.9);
}

/** 剪贴板、拖放、文件选择里的图片文件 */
const imagesOf = (list: DataTransferItemList | FileList | null) => {
  const files = list instanceof FileList ? Array.from(list) : Array.from(list ?? [], (x) => (x.kind === "file" ? x.getAsFile() : null));
  return files.filter((f): f is File => !!f && f.type.startsWith("image/"));
};

export default function AskBox({
  x,
  y,
  hasSelection,
  models,
  onSubmit,
  onClose,
}: {
  /** 视口坐标：框的左上角 */
  x: number;
  y: number;
  hasSelection: boolean;
  models: AiModel[];
  onSubmit: (pick: AskPick) => void;
  onClose: () => void;
}) {
  const [text, setText] = useState("");
  const [model, setModel] = useState(() =>
    lastModel && models.some((m) => m.id === lastModel) ? lastModel : (models[0]?.id ?? "")
  );
  const [images, setImages] = useState<string[]>([]);
  /** 附图相关的一句提示（太多、读不出来、换了不能看图的模型） */
  const [note, setNote] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);
  const seeing = models.find((m) => m.id === model)?.vision ?? false;
  const box = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const picker = useRef<HTMLInputElement>(null);

  const addImages = async (files: File[]) => {
    if (!files.length) return;
    const room = IMAGES_MAX - images.length;
    const read = await Promise.all(files.slice(0, Math.max(0, room)).map(shrink));
    const good = read.filter((u): u is string => !!u);
    setImages((cur) => [...cur, ...good].slice(0, IMAGES_MAX));
    setNote(files.length > room ? t("editor.ask.imagesMax", { max: IMAGES_MAX }) : good.length < read.length ? t("editor.ask.imageBad") : null);
    input.current?.focus();
  };

  const pickModel = (id: string) => {
    setModel(id);
    if (images.length && !models.find((m) => m.id === id)?.vision) {
      setImages([]);
      setNote(t("editor.ask.imagesDropped"));
    }
  };
  useEffect(() => input.current?.focus(), []);

  // 点到框外就关
  useEffect(() => {
    const down = (e: MouseEvent) => {
      if (box.current && !box.current.contains(e.target as Node)) onClose();
    };
    window.addEventListener("mousedown", down, true);
    return () => window.removeEventListener("mousedown", down, true);
  }, [onClose]);

  // 别伸出窗口右边、下边
  const [pos, setPos] = useState({ left: x, top: y });
  useEffect(() => {
    const r = box.current?.getBoundingClientRect();
    if (!r) return;
    const left = Math.max(8, Math.min(x, window.innerWidth - r.width - 8));
    const top = y + r.height > window.innerHeight - 8 ? Math.max(8, y - r.height - 28) : y;
    setPos({ left, top });
  }, [x, y]);

  const send = (pick: AskPick) => {
    lastModel = model || null;
    onSubmit({ ...pick, model: model || undefined, images: images.length ? images : undefined });
  };
  const ready = !!text.trim() || images.length > 0;

  return (
    <div
      ref={box}
      className={`ask-box${dragging ? " dragging" : ""}`}
      style={{ left: pos.left, top: pos.top }}
      onPaste={(e) => {
        if (!seeing) return;
        const files = imagesOf(e.clipboardData.items);
        if (!files.length) return;
        e.preventDefault();
        void addImages(files);
      }}
      onDragOver={(e) => {
        if (!seeing || !Array.from(e.dataTransfer.types).includes("Files")) return;
        e.preventDefault();
        setDragging(true);
      }}
      onDragLeave={(e) => {
        if (!box.current?.contains(e.relatedTarget as Node)) setDragging(false);
      }}
      onDrop={(e) => {
        if (!seeing) return;
        e.preventDefault();
        setDragging(false);
        void addImages(imagesOf(e.dataTransfer.files));
      }}
      role="dialog"
      aria-label={t("editor.ask.label")}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          onClose();
        }
      }}
    >
      <form
        className="ask-row"
        onSubmit={(e) => {
          e.preventDefault();
          if (ready) send({ instruction: text.trim() || undefined });
        }}
      >
        <Icon name="sparkle" size={15} className="ask-icon" />
        <input
          ref={input}
          className="ask-input"
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder={
            images.length ? t("editor.ask.placeholderImage") : hasSelection ? t("editor.ask.placeholderEdit") : t("editor.ask.placeholderInsert")
          }
          aria-label={t("editor.ask.label")}
        />
        {seeing && (
          <>
            <button
              type="button"
              className="btn btn-ghost btn-sm btn-icon"
              onClick={() => picker.current?.click()}
              disabled={images.length >= IMAGES_MAX}
              title={t("editor.ask.addImage")}
              aria-label={t("editor.ask.addImage")}
            >
              <Icon name="image" size={15} />
            </button>
            <input
              ref={picker}
              type="file"
              accept="image/png,image/jpeg,image/webp,image/gif"
              multiple
              hidden
              onChange={(e) => {
                void addImages(imagesOf(e.target.files));
                e.target.value = "";
              }}
            />
          </>
        )}
        {models.length > 1 && (
          <select className="ask-model" value={model} onChange={(e) => pickModel(e.target.value)} aria-label={t("editor.ask.model")}>
            {models.map((m) => (
              <option key={m.id} value={m.id}>
                {m.name}
              </option>
            ))}
          </select>
        )}
        <button className="btn btn-primary btn-sm" type="submit" disabled={!ready}>
          {t("editor.ask.send")}
        </button>
      </form>
      {images.length > 0 && (
        <div className="ask-images">
          {images.map((src, i) => (
            <div key={i} className="ask-thumb">
              <img src={src} alt={t("editor.ask.imageAlt", { n: i + 1 })} />
              <button
                type="button"
                className="ask-thumb-x"
                onClick={() => setImages((cur) => cur.filter((_, j) => j !== i))}
                title={t("editor.ask.removeImage")}
                aria-label={t("editor.ask.removeImage")}
              >
                <Icon name="x" size={11} />
              </button>
            </div>
          ))}
        </div>
      )}
      {note && <div className="ask-note">{note}</div>}
      {hasSelection && (
        <div className="ask-presets">
          {PRESETS.map((p) => (
            <button key={p} type="button" className="ask-chip" onClick={() => send({ preset: p, instruction: text.trim() || undefined })}>
              {t(`editor.ask.presets.${p}` as Key)}
            </button>
          ))}
        </div>
      )}
      <div className="ask-hint">
        {seeing
          ? hasSelection
            ? t("editor.ask.hintEditImage")
            : t("editor.ask.hintInsertImage")
          : hasSelection
            ? t("editor.ask.hintEdit")
            : t("editor.ask.hintInsert")}
      </div>
    </div>
  );
}
