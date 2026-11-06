import { snippetCompletion, type Completion, type CompletionContext, type CompletionResult } from "@codemirror/autocomplete";
import type { BibSym, LabelSym } from "./api";
import { getLocale, t, type Key, type Locale } from "./i18n";

/** 常用命令；带参数的用 snippet，Tab 在占位之间跳。说明文字在语言文件 latex.commands.<命令名> */
const COMMANDS: (string | [string, string])[] = [
  ["section", "section{${}}"],
  ["subsection", "subsection{${}}"],
  ["subsubsection", "subsubsection{${}}"],
  ["chapter", "chapter{${}}"],
  ["paragraph", "paragraph{${}}"],
  ["textbf", "textbf{${}}"],
  ["textit", "textit{${}}"],
  ["emph", "emph{${}}"],
  ["underline", "underline{${}}"],
  ["texttt", "texttt{${}}"],
  ["textsc", "textsc{${}}"],
  ["textcolor", "textcolor{${color}}{${}}"],
  ["footnote", "footnote{${}}"],
  ["label", "label{${}}"],
  ["ref", "ref{${}}"],
  ["eqref", "eqref{${}}"],
  ["cref", "cref{${}}"],
  ["autoref", "autoref{${}}"],
  ["pageref", "pageref{${}}"],
  ["cite", "cite{${}}"],
  ["citep", "citep{${}}"],
  ["citet", "citet{${}}"],
  ["url", "url{${}}"],
  ["href", "href{${url}}{${text}}"],
  ["includegraphics", "includegraphics[width=${0.8}\\linewidth]{${}}"],
  ["input", "input{${}}"],
  ["include", "include{${}}"],
  ["usepackage", "usepackage{${}}"],
  ["documentclass", "documentclass{${article}}"],
  ["newcommand", "newcommand{\\${name}}{${}}"],
  ["renewcommand", "renewcommand{\\${name}}{${}}"],
  ["caption", "caption{${}}"],
  ["title", "title{${}}"],
  ["author", "author{${}}"],
  ["date", "date{${}}"],
  ["frac", "frac{${}}{${}}"],
  ["sqrt", "sqrt{${}}"],
  ["mathbf", "mathbf{${}}"],
  ["mathrm", "mathrm{${}}"],
  ["mathcal", "mathcal{${}}"],
  ["mathbb", "mathbb{${}}"],
  ["operatorname", "operatorname{${}}"],
  ["text", "text{${}}"],
  ["hspace", "hspace{${1em}}"],
  ["vspace", "vspace{${1em}}"],
  ["setlength", "setlength{\\${}}{${}}"],
  ["bibliography", "bibliography{${}}"],
  ["bibliographystyle", "bibliographystyle{${plain}}"],
  ["addbibresource", "addbibresource{${}}"],
  "maketitle", "tableofcontents", "item", "centering", "noindent", "newpage", "clearpage",
  "today", "LaTeX", "TeX", "linewidth", "textwidth", "printbibliography", "appendix",
  "hline", "toprule", "midrule", "bottomrule", "quad", "qquad", "cdot", "cdots", "ldots",
  "alpha", "beta", "gamma", "delta", "epsilon", "varepsilon", "theta", "lambda", "mu", "pi",
  "sigma", "phi", "omega", "infty", "partial", "nabla", "sum", "prod", "int", "lim",
  "leq", "geq", "neq", "approx", "equiv", "in", "subset", "cup", "cap", "times", "rightarrow",
  "Rightarrow", "leftarrow", "mapsto", "left", "right", "small", "footnotesize", "large", "Large",
];

/** 说明文字在语言文件 latex.envs.<环境名> */
const ENVS = [
  "equation", "equation*", "align", "align*", "itemize", "enumerate", "description", "figure", "table", "tabular", "center", "abstract",
  "document", "minipage", "verbatim", "quote", "theorem", "proof", "cases", "matrix", "pmatrix", "bmatrix", "frame",
];

/** 环境的骨架：图、表、列表顺手把常用的内层也写上 */
function envBody(name: string): string {
  switch (name) {
    case "figure":
      return "\\begin{figure}[htbp]\n\t\\centering\n\t\\includegraphics[width=0.8\\linewidth]{${}}\n\t\\caption{${}}\n\t\\label{fig:${}}\n\\end{figure}";
    case "table":
      return "\\begin{table}[htbp]\n\t\\centering\n\t\\caption{${}}\n\t\\begin{tabular}{${ll}}\n\t\t${}\n\t\\end{tabular}\n\\end{table}";
    case "tabular":
      return "\\begin{tabular}{${ll}}\n\t${}\n\\end{tabular}";
    case "itemize":
    case "enumerate":
      return `\\begin{${name}}\n\t\\item \${}\n\\end{${name}}`;
    case "frame":
      return `\\begin{frame}{\${${t("latex.frameTitle")}}}\n\t\${}\n\\end{frame}`;
    default:
      return `\\begin{${name}}\n\t\${}\n\\end{${name}}`;
  }
}

const BEGIN = "\\begin{".length;

/** 当前语言里有说明就用，没有（大多数命令）就不显示 */
function detail(group: "commands" | "envs", name: string): string | undefined {
  const table = getLocale().messages.latex[group] as Record<string, unknown>;
  return name in table ? t(`latex.${group}.${name}` as Key) : undefined;
}

/** 补全项按语言建一次缓存，切换语言后下次补全自动换成新的 */
let built: { locale: Locale; commands: Completion[]; envs: Completion[] } | null = null;
function options() {
  const locale = getLocale();
  if (built?.locale === locale) return built;
  const commands = COMMANDS.map((c) =>
    typeof c === "string"
      ? { label: `\\${c}`, type: "keyword" }
      : snippetCompletion(`\\${c[1]}`, { label: `\\${c[0]}`, detail: detail("commands", c[0]), type: "function" })
  );
  const envs = ENVS.map((name): Completion => {
    const snip = snippetCompletion(envBody(name), { label: name, detail: detail("envs", name), type: "class" });
    const apply = snip.apply as Exclude<Completion["apply"], string | undefined>;
    return { ...snip, apply: (view, c, from, to) => apply(view, c, from - BEGIN, to) };
  });
  return (built = { locale, commands, envs });
}

/** 整个项目的标签和文献条目：工作区打开项目、保存、编译后从服务端取（symbols.rs），补全时直接用 */
let project: { labels: LabelSym[]; bib: BibSym[] } = { labels: [], bib: [] };

export function setProjectSymbols(next: { labels: LabelSym[]; bib: BibSym[] }) {
  project = next;
}

/** \ref 系列补全：当前文件里刚打的（服务端还没看到）加上全项目的；别的文件里的标出文件名 */
function labels(doc: string): Completion[] {
  const out = new Map<string, Completion>();
  for (const m of doc.matchAll(/\\label\{([^}]+)\}/g)) out.set(m[1], { label: m[1], type: "constant" });
  for (const l of project.labels) if (!out.has(l.key)) out.set(l.key, { label: l.key, type: "constant", detail: l.file });
  return [...out.values()];
}

/** \cite 补全：键，旁边是作者和年份，选中时浮出标题 */
function citations(): Completion[] {
  const seen = new Set<string>();
  const out: Completion[] = [];
  for (const b of project.bib) {
    if (seen.has(b.key)) continue;
    seen.add(b.key);
    // 第一作者的姓：「Knuth, Donald」取逗号前，「Donald Knuth」取最后一个词
    const first = b.author?.split(/\s+and\s+/)[0]?.trim();
    const who = first?.includes(",") ? first.split(",")[0].trim() : first?.split(/\s+/).pop();
    const detail = [who, b.year].filter(Boolean).join(" ");
    out.push({ label: b.key, type: "variable", detail: detail || undefined, info: b.title });
  }
  return out;
}

export function latexCompletions(ctx: CompletionContext): CompletionResult | null {
  const before = ctx.state.doc.sliceString(Math.max(0, ctx.pos - 80), ctx.pos);

  // \begin{fig| → 环境骨架。过滤只看环境名，选中后连同已打的 \begin{ 一起换掉
  const env = before.match(/\\begin\{([\w*]*)$/);
  if (env) return { from: ctx.pos - env[1].length, options: options().envs, validFor: /^[\w*]*$/ };

  const ref = before.match(/\\(?:ref|eqref|cref|Cref|autoref|pageref|nameref|vref)\{([^}]*)$/);
  if (ref) {
    return { from: ctx.pos - ref[1].length, options: labels(ctx.state.doc.toString()), validFor: /^[^}]*$/ };
  }

  // \cite、\citep、\parencite[见][12]{a, b|：多个键用逗号隔开，只补当前这一个
  const cite = before.match(/\\(?:[a-zA-Z]*cite[a-zA-Z]*|nocite)\*?(?:\[[^\]]*\]){0,2}\{([^}]*)$/);
  if (cite) {
    const cur = cite[1].slice(cite[1].lastIndexOf(",") + 1).replace(/^\s+/, "");
    return { from: ctx.pos - cur.length, options: citations(), validFor: /^[^,}\s]*$/ };
  }

  const cmd = ctx.matchBefore(/\\[a-zA-Z]*/);
  if (!cmd || (cmd.from === cmd.to && !ctx.explicit)) return null;
  return { from: cmd.from, options: options().commands, validFor: /^\\[a-zA-Z]*$/ };
}
