/** 按行比较两份文本，给历史版本面板标出增删。先去掉相同的头尾，中间用最长公共子序列；
 *  中间部分太大（几千行对几千行）就放弃，返回 null，界面只显示原文 */

export type DiffLine = { kind: "same" | "add" | "del"; text: string };

/** 中间部分两边行数之积的上限：表格用 Uint32Array，四百万格约 16 MB */
const MAX_CELLS = 4_000_000;

export function lineDiff(before: string, after: string): DiffLine[] | null {
  const a = before.split("\n");
  const b = after.split("\n");
  let head = 0;
  while (head < a.length && head < b.length && a[head] === b[head]) head++;
  let tail = 0;
  while (tail < a.length - head && tail < b.length - head && a[a.length - 1 - tail] === b[b.length - 1 - tail]) tail++;
  const am = a.slice(head, a.length - tail);
  const bm = b.slice(head, b.length - tail);
  if ((am.length + 1) * (bm.length + 1) > MAX_CELLS) return null;

  // lcs[i][j]：am[i..] 与 bm[j..] 的最长公共子序列长度
  const w = bm.length + 1;
  const lcs = new Uint32Array((am.length + 1) * w);
  for (let i = am.length - 1; i >= 0; i--) {
    for (let j = bm.length - 1; j >= 0; j--) {
      lcs[i * w + j] = am[i] === bm[j] ? lcs[(i + 1) * w + j + 1] + 1 : Math.max(lcs[(i + 1) * w + j], lcs[i * w + j + 1]);
    }
  }
  const out: DiffLine[] = a.slice(0, head).map((text) => ({ kind: "same", text }));
  let i = 0;
  let j = 0;
  while (i < am.length && j < bm.length) {
    if (am[i] === bm[j]) {
      out.push({ kind: "same", text: am[i] });
      i++;
      j++;
    } else if (lcs[(i + 1) * w + j] >= lcs[i * w + j + 1]) {
      out.push({ kind: "del", text: am[i++] });
    } else {
      out.push({ kind: "add", text: bm[j++] });
    }
  }
  while (i < am.length) out.push({ kind: "del", text: am[i++] });
  while (j < bm.length) out.push({ kind: "add", text: bm[j++] });
  for (const text of a.slice(a.length - tail)) out.push({ kind: "same", text });
  return out;
}
