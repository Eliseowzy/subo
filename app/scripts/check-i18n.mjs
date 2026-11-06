// 多语言守门：界面代码里不许直接写面向用户的文字，必须走 src/i18n 的 t()。
// 规则与例外见 docs/i18n.md。npm run build 会先跑它，失败即中止。
//
// 拦三类：
//   1. 任何含中日韩文字的字符串 / 模板字符串 / JSX 文本（基准语言是中文，硬编码最可能是它）
//   2. JSX 文本里的字母（<b>Open</b> 这种英文硬编码）；<Kbd>、<code> 里的按键名、代码除外
//   3. title / placeholder / aria-label / alt 这几个属性直接写字符串
// 例外：同一行或所在语句前的注释里写 i18n-ignore，并说明原因。
// src/i18n/ 目录本身不查。
//
// 另查宏包说明：src/i18n/packages/ 下各语言收录的宏包必须一致（以 zh-CN 为准），值不能为空。

import { readdirSync, readFileSync } from "node:fs";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";

const root = fileURLToPath(new URL("..", import.meta.url));
const src = join(root, "src");
const CJK = /[぀-ヿ㐀-䶿一-鿿豈-﫿＀-￯가-힯]/;
const LETTER = /\p{L}/u;
const ATTRS = new Set(["title", "placeholder", "aria-label", "alt", "label"]);
const VERBATIM_TAGS = new Set(["Kbd", "kbd", "code", "pre"]);

function* walk(dir) {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name);
    if (e.isDirectory()) {
      if (p !== join(src, "i18n")) yield* walk(p);
    } else if (/\.(ts|tsx)$/.test(e.name) && !e.name.endsWith(".d.ts")) yield p;
  }
}

function ignored(sf, node) {
  const text = sf.text;
  const { line } = sf.getLineAndCharacterOfPosition(node.getStart(sf));
  const lineText = text.slice(sf.getPositionOfLineAndCharacter(line, 0), text.indexOf("\n", node.getStart(sf)) >>> 0);
  if (lineText.includes("i18n-ignore")) return true;
  for (let n = node; n && n !== sf; n = n.parent) {
    const ranges = ts.getLeadingCommentRanges(text, n.pos) ?? [];
    if (ranges.some((r) => text.slice(r.pos, r.end).includes("i18n-ignore"))) return true;
  }
  return false;
}

function tagName(el) {
  const open = ts.isJsxElement(el) ? el.openingElement : el;
  return open.tagName?.getText();
}

const problems = [];

for (const file of walk(src)) {
  const sf = ts.createSourceFile(file, readFileSync(file, "utf8"), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const report = (node, why) => {
    if (ignored(sf, node)) return;
    const { line, character } = sf.getLineAndCharacterOfPosition(node.getStart(sf));
    const snippet = node.getText(sf).replace(/\s+/g, " ").slice(0, 60);
    problems.push(`${relative(root, file)}:${line + 1}:${character + 1}  ${why}  ${snippet}`);
  };

  const visit = (node) => {
    if (
      ts.isStringLiteral(node) ||
      ts.isNoSubstitutionTemplateLiteral(node) ||
      ts.isTemplateHead(node) ||
      ts.isTemplateMiddle(node) ||
      ts.isTemplateTail(node)
    ) {
      if (CJK.test(node.text)) report(node, "字符串里有中日韩文字");
      else if (
        ts.isStringLiteral(node) &&
        ts.isJsxAttribute(node.parent) &&
        ATTRS.has(node.parent.name.getText(sf)) &&
        LETTER.test(node.text)
      )
        report(node, `属性 ${node.parent.name.getText(sf)} 写死了文字`);
    } else if (ts.isJsxText(node)) {
      const parent = node.parent;
      const verbatim = (ts.isJsxElement(parent) || ts.isJsxSelfClosingElement(parent)) && VERBATIM_TAGS.has(tagName(parent));
      if (CJK.test(node.text) || (!verbatim && LETTER.test(node.text))) report(node, "JSX 里写死了文字");
    }
    ts.forEachChild(node, visit);
  };
  visit(sf);
}

const pkgDir = join(src, "i18n", "packages");
const tables = Object.fromEntries(
  readdirSync(pkgDir)
    .filter((f) => f.endsWith(".json"))
    .map((f) => [f.slice(0, -5), JSON.parse(readFileSync(join(pkgDir, f), "utf8"))]),
);
const pkgProblems = [];
const baseNames = Object.keys(tables["zh-CN"] ?? {});
for (const [code, table] of Object.entries(tables)) {
  const names = Object.keys(table);
  const missing = baseNames.filter((n) => !(n in table));
  const extra = names.filter((n) => !baseNames.includes(n));
  const empty = names.filter((n) => typeof table[n] !== "string" || !table[n].trim());
  if (missing.length) pkgProblems.push(`${code}.json 缺：${missing.join(" ")}`);
  if (extra.length) pkgProblems.push(`${code}.json 多出（zh-CN 没有）：${extra.join(" ")}`);
  if (empty.length) pkgProblems.push(`${code}.json 说明为空：${empty.join(" ")}`);
}
if (pkgProblems.length) {
  console.error(`check:i18n  内置语言的宏包说明没对齐（src/i18n/packages/，以 zh-CN 为准）：\n`);
  for (const p of pkgProblems) console.error("  " + p);
  process.exit(1);
}

if (problems.length) {
  console.error(`check:i18n  发现 ${problems.length} 处硬编码的界面文字，请改成 t("…")，见 docs/i18n.md：\n`);
  for (const p of problems) console.error("  " + p);
  process.exit(1);
}
console.log("check:i18n  ok");
