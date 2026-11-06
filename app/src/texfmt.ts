/*
 * .tex 排版整理（格式化）。只动不影响编译结果的空白：
 *   - 按环境、跨行的 {…}、\[…\] 的嵌套重排行首缩进；document 环境不缩进；
 *     列表环境里 \item 之后的续行多缩一级
 *   - 去掉行尾空白，连续多个空行并成一个（TeX 里它们本来就等价），文件末尾留一个换行
 * 不拆行、不合并行，所以不会改动段落和间距。verbatim 一类的环境原样保留。
 */

/** 环境里的内容原样保留（空白有意义） */
const VERBATIM = new Set([
  "verbatim", "verbatim*", "Verbatim", "Verbatim*", "BVerbatim", "LVerbatim", "lstlisting",
  "minted", "comment", "alltt", "filecontents", "filecontents*", "luacode", "luacode*", "pycode",
]);
/** 这些环境的内容不缩进 */
const FLAT = new Set(["document"]);
const LISTS = new Set(["itemize", "enumerate", "description", "list", "compactitem", "compactenum", "inparaenum", "asparaitem", "asparaenum"]);

type Frame = { kind: "env" | "brace" | "display"; name: string; step: number; item: boolean };

type Tok =
  | { t: "open"; kind: Frame["kind"]; name: string }
  | { t: "close"; kind: Frame["kind"]; name: string }
  | { t: "item" }
  | { t: "text" };

/** 一行切成影响嵌套的记号；% 之后是注释，不看。遇到 verbatim 环境的开头就停 */
function tokens(s: string): Tok[] {
  const out: Tok[] = [];
  let i = 0;
  const envName = (at: number): [string, number] | null => {
    const m = /^\s*\{([^{}]*)\}/.exec(s.slice(at));
    return m ? [m[1].trim(), at + m[0].length] : null;
  };
  while (i < s.length) {
    const c = s[i];
    if (c === " " || c === "\t") {
      i++;
    } else if (c === "%") {
      break;
    } else if (c === "{") {
      out.push({ t: "open", kind: "brace", name: "" });
      i++;
    } else if (c === "}") {
      out.push({ t: "close", kind: "brace", name: "" });
      i++;
    } else if (c === "\\") {
      const m = /^\\([A-Za-z@]+\*?|.?)/.exec(s.slice(i))!;
      const cmd = m[1];
      i += m[0].length;
      if (cmd === "begin" || cmd === "end") {
        const env = envName(i);
        if (!env) {
          out.push({ t: "text" });
          continue;
        }
        i = env[1];
        out.push({ t: cmd === "begin" ? "open" : "close", kind: "env", name: env[0] });
        if (cmd === "begin" && VERBATIM.has(env[0])) break;
      } else if (cmd === "[" || cmd === "]") {
        out.push({ t: cmd === "[" ? "open" : "close", kind: "display", name: "" });
      } else if (cmd === "item") {
        out.push({ t: "item" });
      } else if (cmd === "verb" || cmd === "verb*") {
        // \verb|…|：跳到配对的分隔符
        const end = s.indexOf(s[i], i + 1);
        i = end < 0 ? s.length : end + 1;
        out.push({ t: "text" });
      } else {
        out.push({ t: "text" });
      }
    } else {
      out.push({ t: "text" });
      i++;
    }
  }
  return out;
}

/** 去行尾空白；但行尾是 “\ ”（控制空格）时留一个空格 */
function trimEnd(s: string): string {
  const t = s.replace(/[ \t]+$/, "");
  if (t.length === s.length) return t;
  const slashes = /\\*$/.exec(t)![0].length;
  return slashes % 2 ? t + " " : t;
}

/**
 * 逐行给出整理后的内容：string 是这一行的新内容，null 表示删掉这一行（多余的空行）。
 * 返回数组与输入行一一对应，调用方据此只改变了的行，光标和撤销都不乱。
 */
export function formatTexLines(lines: string[], unit = "  "): (string | null)[] {
  const out: (string | null)[] = [];
  const stack: Frame[] = [];
  let raw: string | null = null; // 正在 verbatim 环境里：它的名字
  let prevBlank = true; // 文件开头的空行也去掉

  const depth = (skipItem: boolean) =>
    stack.reduce((n, f, k) => n + f.step + (f.item && !(skipItem && k === stack.length - 1) ? 1 : 0), 0);

  const close = (kind: Frame["kind"], name: string) => {
    for (let k = stack.length - 1; k >= 0; k--) {
      if (stack[k].kind === kind && stack[k].name === name) {
        stack.length = k;
        return;
      }
    }
    // 对不上的闭合（多半是宏定义里拆开写的环境）不管
  };

  for (const line of lines) {
    if (raw !== null) {
      out.push(line);
      if (line.includes(`\\end{${raw}}`)) {
        raw = null;
        prevBlank = false;
      }
      continue;
    }
    const body = trimEnd(line.replace(/^[ \t]+/, ""));
    if (!body) {
      out.push(prevBlank ? null : "");
      prevBlank = true;
      continue;
    }
    prevBlank = false;

    const toks = tokens(body);
    let indent: number | null = null;
    for (const tok of toks) {
      if (tok.t === "close") {
        close(tok.kind, tok.name);
        continue;
      }
      if (indent === null) indent = depth(tok.t === "item" && LISTS.has(stack[stack.length - 1]?.name));
      if (tok.t === "open") {
        stack.push({ kind: tok.kind, name: tok.name, step: tok.kind === "env" && FLAT.has(tok.name) ? 0 : 1, item: false });
        if (tok.kind === "env" && VERBATIM.has(tok.name)) {
          stack.pop();
          raw = tok.name;
        }
      } else if (tok.t === "item") {
        const top = stack[stack.length - 1];
        if (top && LISTS.has(top.name)) top.item = true;
      }
    }
    // 整行只有闭合记号（或注释）：按闭合后的层级
    if (indent === null) indent = depth(false);
    out.push(unit.repeat(indent) + body);
  }

  // 末尾的空行只留文件最后那个（即末尾的换行）
  let last = out.length - 1;
  if (raw === null && last >= 0 && !out[last]) {
    out[last] = "";
    for (let k = last - 1; k >= 0 && !out[k]; k--) out[k] = null;
  }
  return out;
}

/** 整段文本整理（测试用；编辑器里走 formatTexLines 按行改） */
export function formatTex(text: string, unit = "  "): string {
  const kept = formatTexLines(text.split(/\r?\n/), unit).filter((l): l is string => l !== null);
  if (kept[kept.length - 1] !== "") kept.push("");
  return kept.join("\n");
}
