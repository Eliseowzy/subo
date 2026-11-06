import { api } from "../api";
import { defaultThemePack, localePack } from "../appearance";
import { BUILTIN_LOCALES } from "../i18n";
import localeMd from "./locale/SKILL.md?raw";
import localeValidate from "./locale/validate.mjs?raw";
import themeMd from "./theme/SKILL.md?raw";
import themeValidate from "./theme/validate.mjs?raw";

/*
 * 「AI 定制技能」：Claude Skill 的 zip，用户下载后交给 Claude 等工具，按描述生成配色方案或语言包。
 * 配色、语言各一个技能，互不夹带：做配色用不着几百条文案，做翻译也用不着令牌表。
 * 说明和校验脚本是静态文件；参考资料（全部令牌的默认值、基准语言的完整键）下载时
 * 从当前版本现做，保证和用户手里的 Subo 一致。语言技能另附本机已装宏包的英文说明，
 * 当作翻译宏包说明的原文；本机服务查不到就不附，宏包说明本来就是可选的。
 */

export type SkillKind = "theme" | "locale";

/** 下载时的文件名，提示文案里也要用 */
export const SKILL_FILE: Record<SkillKind, string> = { theme: "subo-theme-skill.zip", locale: "subo-locale-skill.zip" };

export async function buildSkillZip(kind: SkillKind): Promise<Blob> {
  const json = (v: unknown) => JSON.stringify(v, null, 2) + "\n";
  const english = kind === "locale" ? await englishPackages() : null;
  const files: [string, string][] =
    kind === "theme"
      ? [
          ["SKILL.md", themeMd],
          ["scripts/validate.mjs", themeValidate],
          ["reference/default.subo-theme.json", json(defaultThemePack())],
        ]
      : [
          ["SKILL.md", localeMd],
          ["scripts/validate.mjs", localeValidate],
          ...BUILTIN_LOCALES.map((l): [string, string] => [`reference/${l.code}.subo-locale.json`, json(localePack(l))]),
          ...(english ? [["reference/packages.en.json", json(english)] as [string, string]] : []),
        ];
  const dir = `subo-${kind}`;
  return zip(files.map(([p, s]) => [`${dir}/${p}`, new TextEncoder().encode(s)]));
}

/** 本机已装宏包的英文说明（宏包名 → 说明），按名字排好 */
async function englishPackages(): Promise<Record<string, string> | null> {
  try {
    const r = await api.packages();
    if (!r.available || !r.packages.length) return null;
    return Object.fromEntries(r.packages.filter((p) => p.desc).map((p) => [p.name, p.desc]));
  } catch {
    return null;
  }
}

/* ---------- 最小的 zip 写入：只存不压（几十 KB 的文本，不值得引库） ---------- */

const CRC = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(b: Uint8Array) {
  let c = 0xffffffff;
  for (const x of b) c = CRC[(c ^ x) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function zip(entries: [string, Uint8Array][]): Blob {
  const enc = new TextEncoder();
  const parts: Uint8Array[] = [];
  const central: Uint8Array[] = [];
  let offset = 0;
  const now = new Date();
  const time = (now.getHours() << 11) | (now.getMinutes() << 5) | (now.getSeconds() >> 1);
  const date = ((now.getFullYear() - 1980) << 9) | ((now.getMonth() + 1) << 5) | now.getDate();
  for (const [name, data] of entries) {
    const n = enc.encode(name);
    const crc = crc32(data);
    // 0x0800：文件名是 UTF-8
    const head = (sig: number, extra: number) => {
      const h = new DataView(new ArrayBuffer(extra));
      h.setUint32(0, sig, true);
      return h;
    };
    const local = head(0x04034b50, 30 + n.length);
    local.setUint16(4, 20, true);
    local.setUint16(6, 0x0800, true);
    local.setUint16(8, 0, true);
    local.setUint16(10, time, true);
    local.setUint16(12, date, true);
    local.setUint32(14, crc, true);
    local.setUint32(18, data.length, true);
    local.setUint32(22, data.length, true);
    local.setUint16(26, n.length, true);
    const lb = new Uint8Array(local.buffer);
    lb.set(n, 30);
    parts.push(lb, data);

    const cd = head(0x02014b50, 46 + n.length);
    cd.setUint16(4, 20, true);
    cd.setUint16(6, 20, true);
    cd.setUint16(8, 0x0800, true);
    cd.setUint16(10, 0, true);
    cd.setUint16(12, time, true);
    cd.setUint16(14, date, true);
    cd.setUint32(16, crc, true);
    cd.setUint32(20, data.length, true);
    cd.setUint32(24, data.length, true);
    cd.setUint16(28, n.length, true);
    cd.setUint32(42, offset, true);
    const cb = new Uint8Array(cd.buffer);
    cb.set(n, 46);
    central.push(cb);
    offset += lb.length + data.length;
  }
  const size = central.reduce((s, c) => s + c.length, 0);
  const end = new DataView(new ArrayBuffer(22));
  end.setUint32(0, 0x06054b50, true);
  end.setUint16(8, entries.length, true);
  end.setUint16(10, entries.length, true);
  end.setUint32(12, size, true);
  end.setUint32(16, offset, true);
  return new Blob([...parts, ...central, new Uint8Array(end.buffer)] as BlobPart[], { type: "application/zip" });
}
