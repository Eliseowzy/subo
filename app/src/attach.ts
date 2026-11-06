/** 附件：按扩展名分类，以及从字体文件里读出名字、字形数和字符覆盖 */

export type Kind = "text" | "image" | "font" | "pdf" | "binary";

const IMAGE = /\.(png|jpe?g|gif|bmp|webp|svg)$/i;
const FONT = /\.(otf|ttf|ttc|woff2?)$/i;
/** 浏览器显示不了、编辑器也打不开的：只给文件信息 */
const BINARY = /\.(pfb|pfm|afm|tfm|vf|eps|ps|zip|gz|tgz|xz|dvi|xdv|synctex|ico|tiff?|subocase)$/i;

export function kindOf(path: string): Kind {
  if (IMAGE.test(path)) return "image";
  if (FONT.test(path)) return "font";
  if (/\.pdf$/i.test(path)) return "pdf";
  if (BINARY.test(path)) return "binary";
  return "text";
}

export function formatSize(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

export type FontMeta = {
  family?: string;
  subfamily?: string;
  full?: string;
  postscript?: string;
  /** 简体/繁体中文名，没有就空着 */
  localFamily?: string;
  version?: string;
  glyphs?: number;
  /** .ttc 里有几款；只解析第一款 */
  faces?: number;
  /** 拿不到 cmap（如 woff2）时为 undefined，表示不知道 */
  covers?: (cp: number) => boolean;
  /** cmap 覆盖的码位区间，已排序 */
  ranges?: [number, number][];
};

type Tables = Map<string, DataView>;

const tag = (v: DataView, o: number) =>
  String.fromCharCode(v.getUint8(o), v.getUint8(o + 1), v.getUint8(o + 2), v.getUint8(o + 3));

/** 解析失败不算错：字体照样能预览，只是少几行信息 */
export async function readFontMeta(buf: ArrayBuffer): Promise<FontMeta> {
  try {
    const v = new DataView(buf);
    const sig = tag(v, 0);
    let tables: Tables;
    let faces: number | undefined;
    if (sig === "ttcf") {
      faces = v.getUint32(8);
      tables = sfntTables(v, v.getUint32(12));
    } else if (sig === "wOFF") {
      tables = await woffTables(v);
    } else if (sig === "wOF2") {
      return {}; // brotli 压缩加变换，不值得在这里解
    } else {
      tables = sfntTables(v, 0);
    }
    const meta: FontMeta = { faces, ...names(tables.get("name")) };
    const maxp = tables.get("maxp");
    if (maxp) meta.glyphs = maxp.getUint16(4);
    const cmap = tables.get("cmap");
    const ranges = cmap && coverage(cmap);
    if (ranges) {
      meta.ranges = ranges;
      meta.covers = (cp) => {
        let lo = 0;
        let hi = ranges.length - 1;
        while (lo <= hi) {
          const mid = (lo + hi) >> 1;
          if (cp < ranges[mid][0]) hi = mid - 1;
          else if (cp > ranges[mid][1]) lo = mid + 1;
          else return true;
        }
        return false;
      };
    }
    return meta;
  } catch {
    return {};
  }
}

function sfntTables(v: DataView, at: number): Tables {
  const out: Tables = new Map();
  const n = v.getUint16(at + 4);
  for (let i = 0; i < n; i++) {
    const r = at + 12 + i * 16;
    out.set(tag(v, r), new DataView(v.buffer, v.byteOffset + v.getUint32(r + 8), v.getUint32(r + 12)));
  }
  return out;
}

/** WOFF 1：每张表单独 zlib 压缩，只解我们要的三张 */
async function woffTables(v: DataView): Promise<Tables> {
  const out: Tables = new Map();
  const n = v.getUint16(12);
  for (let i = 0; i < n; i++) {
    const r = 44 + i * 20;
    const t = tag(v, r);
    if (t !== "name" && t !== "maxp" && t !== "cmap") continue;
    const off = v.getUint32(r + 4);
    const comp = v.getUint32(r + 8);
    const orig = v.getUint32(r + 12);
    const raw = new Uint8Array(v.buffer, v.byteOffset + off, comp);
    if (comp === orig) {
      out.set(t, new DataView(raw.buffer, raw.byteOffset, raw.byteLength));
    } else {
      const s = new Blob([raw.slice()]).stream().pipeThrough(new DecompressionStream("deflate"));
      out.set(t, new DataView(await new Response(s).arrayBuffer()));
    }
  }
  return out;
}

const ZH = new Set([0x0804, 0x0404, 0x0c04, 0x1004, 0x1404]);

function names(v: DataView | undefined): Partial<FontMeta> {
  if (!v) return {};
  const count = v.getUint16(2);
  const base = v.getUint16(4);
  // nameID → [英文名, 其他名, 中文名]
  const best = new Map<number, { en?: string; any?: string; zh?: string }>();
  for (let i = 0; i < count; i++) {
    const r = 6 + i * 12;
    const platform = v.getUint16(r);
    const lang = v.getUint16(r + 4);
    const id = v.getUint16(r + 6);
    const len = v.getUint16(r + 8);
    const off = base + v.getUint16(r + 10);
    let text = "";
    if (platform === 0 || platform === 3) {
      for (let k = 0; k + 1 < len; k += 2) text += String.fromCharCode(v.getUint16(off + k));
    } else if (platform === 1 && v.getUint16(r + 2) === 0) {
      for (let k = 0; k < len; k++) text += String.fromCharCode(v.getUint8(off + k));
    } else continue;
    const slot = best.get(id) ?? {};
    if (platform === 3 && ZH.has(lang)) slot.zh ??= text;
    else if ((platform === 3 && lang === 0x0409) || (platform === 1 && lang === 0)) slot.en ??= text;
    else slot.any ??= text;
    best.set(id, slot);
  }
  const pick = (id: number) => {
    const s = best.get(id);
    return s?.en ?? s?.any ?? s?.zh;
  };
  // 16/17 是排版用的家族名，比 1/2 更完整（1/2 只容得下四种字重）
  return {
    family: pick(16) ?? pick(1),
    subfamily: pick(17) ?? pick(2),
    full: pick(4),
    version: pick(5),
    postscript: pick(6),
    localFamily: best.get(16)?.zh ?? best.get(1)?.zh,
  };
}

/** 从 cmap 取出码位区间，format 12 优先（能覆盖 BMP 以外），否则 format 4 */
function coverage(v: DataView): [number, number][] | undefined {
  const n = v.getUint16(2);
  let f4: number | undefined;
  let f12: number | undefined;
  for (let i = 0; i < n; i++) {
    const r = 4 + i * 8;
    const platform = v.getUint16(r);
    const enc = v.getUint16(r + 2);
    const off = v.getUint32(r + 4);
    if (platform !== 0 && !(platform === 3 && (enc === 1 || enc === 10))) continue;
    const fmt = v.getUint16(off);
    if (fmt === 12) f12 ??= off;
    if (fmt === 4) f4 ??= off;
  }
  const ranges: [number, number][] = [];
  if (f12 !== undefined) {
    const groups = v.getUint32(f12 + 12);
    for (let i = 0; i < groups; i++) {
      const g = f12 + 16 + i * 12;
      ranges.push([v.getUint32(g), v.getUint32(g + 4)]);
    }
  } else if (f4 !== undefined) {
    const seg = v.getUint16(f4 + 6) / 2;
    const ends = f4 + 14;
    const starts = ends + seg * 2 + 2;
    for (let i = 0; i < seg; i++) {
      const s = v.getUint16(starts + i * 2);
      const e = v.getUint16(ends + i * 2);
      if (s !== 0xffff) ranges.push([s, e]);
    }
  } else return undefined;
  ranges.sort((a, b) => a[0] - b[0]);
  return ranges;
}

/** 私用区：图标字体（FontAwesome 等）把图标放在这里，普通码位上常常只有空字形 */
const PUA: [number, number][] = [
  [0xe000, 0xf8ff],
  [0xf0000, 0xffffd],
  [0x100000, 0x10fffd],
];

/** 列出前 limit 个有字形的可见码位；有私用区字形时只列私用区 */
export function codepoints(ranges: [number, number][], limit: number): number[] {
  const clip = (want: [number, number][]) =>
    ranges.flatMap(([s, e]) =>
      want.flatMap(([ws, we]): [number, number][] => (s <= we && e >= ws ? [[Math.max(s, ws), Math.min(e, we)]] : []))
    );
  const pua = clip(PUA);
  const pick = pua.length ? pua : ranges;
  const out: number[] = [];
  for (const [s, e] of pick) {
    for (let cp = s; cp <= e && out.length < limit; cp++) {
      if (/[\s\p{C}]/u.test(String.fromCodePoint(cp)) && !pua.length) continue;
      out.push(cp);
    }
    if (out.length >= limit) break;
  }
  return out;
}
