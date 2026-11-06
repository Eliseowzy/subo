#!/bin/bash
# 生成完整版 TeX Live 的「文件名 → 包名」索引，随内置 TeX Live 分发（见 server/src/case.rs 的 FILE_INDEX）。
#   ./scripts/texlive-index.sh <tlmgr> <输出文件>
#
# 数据来自 tlmgr 当前软件源的 tlpdb（tlmgr dump-tlpdb --remote），与它要装的版本一致。
# 每行 `包名<TAB>文件名 文件名 …`，只收 tex/、fonts/、bibtex/、makeindex/ 下的运行文件，
# 平台二进制包（<包>.<平台>）不收。约 4 MB 文本，安装包压缩后约 0.6 MB。
set -euo pipefail

TLMGR="${1:?用法: texlive-index.sh <tlmgr> <输出文件>}"
OUT="${2:?用法: texlive-index.sh <tlmgr> <输出文件>}"

tmp="$OUT.tmp-$$"
"$TLMGR" dump-tlpdb --remote 2>/dev/null | awk '
  # 同名文件不止一个包有时（tools 与 latex-tools-dev 的 afterpage.sty、babel 与 antomega 的 hyphen.cfg），
  # 按 xelatex 的搜索顺序取：TEXINPUTS 是 tex/{xelatex,xetex,latex,generic,}//，其余 tex/ 子目录垫底
  function rank(p) {
    if (p ~ /^tex\/xelatex\//) return 0
    if (p ~ /^tex\/xetex\//)   return 1
    if (p ~ /^tex\/latex\//)   return 2
    if (p ~ /^tex\/generic\//) return 3
    if (p ~ /^tex\//)          return 4
    return 2
  }
  /^name /    { pkg = ($2 ~ /\./) ? "" : $2; run = 0; next }
  /^runfiles/ { run = 1; next }
  /^[^ ]/ || /^$/ { run = 0; next }
  run && pkg != "" {
    p = $1; sub(/^RELOC\//, "", p); sub(/^texmf-dist\//, "", p)
    if (p !~ /^(tex|fonts|bibtex|makeindex)\//) next
    n = split(p, a, "/"); b = a[n]
    if (b ~ /^\./) next
    r = rank(p)
    # 先到先得，后来的只有排得更靠前才顶掉（tlpdb 按包名排序，结果稳定）
    if (!(b in best) || r < rk[b]) { best[b] = pkg; rk[b] = r }
  }
  END {
    for (b in best) files[best[b]] = files[best[b]] " " b
    for (k in files) print k "\t" substr(files[k], 2)
  }
' | sort > "$tmp"

# 软件源连不上时 dump 出来是空的，别拿半截索引顶掉旧的
lines="$(wc -l < "$tmp" | tr -d ' ')"
if [ "$lines" -lt 1000 ]; then
  rm -f "$tmp"
  echo "文件索引只有 $lines 行，软件源可能连不上，没有写入 $OUT" >&2
  exit 1
fi
mv "$tmp" "$OUT"
echo "  文件索引: $lines 个宏包 → $OUT"
