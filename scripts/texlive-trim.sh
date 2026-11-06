#!/bin/bash
# 从一份完整可用的 TeX Live 裁出随应用分发的精简版（macOS、Linux）。
#   ./scripts/texlive-trim.sh <源 TeX Live 根目录> [目标目录，默认 desktop/texlive]
#   SUBO_ARCH=x86_64 ./scripts/texlive-trim.sh <源> desktop/texlive-x64   # macOS 上裁 Intel 版（给 desktop.sh bundle x64）
#
# 源目录不动，所有操作都在复制出来的目标目录上做。裁剪按宏包走 tlmgr remove，
# 让 texlive.tlpdb 与实际文件一致：被裁掉的宏包以后能装回用户层（不可重定位的如 pdftex，
# tlmgr --usermode 不肯装，由服务端下载容器只取宏文件，见 server/src/case.rs）。
# 改了裁剪清单后跑 scripts/bundled-smoke.sh 验证常见文档仍能补装通过。
set -euo pipefail

SRC="${1:?用法: texlive-trim.sh <源 TeX Live 根目录> [目标目录]}"
HERE="$(cd "$(dirname "$0")/.." && pwd)"
DEST="${2:-$HERE/desktop/texlive}"
SRC="$(cd "$SRC" && pwd)"

OS="$(uname -s)"
# macOS 的程序是通用包，只留一种架构：默认本机，SUBO_ARCH 可指定 arm64 / x86_64
ARCH="${SUBO_ARCH:-$(uname -m)}"
if [ "$OS" = Darwin ] && [ "$ARCH" != arm64 ] && [ "$ARCH" != x86_64 ]; then
  echo "SUBO_ARCH 只能是 arm64 或 x86_64，收到：$ARCH" >&2
  exit 2
fi
case "$OS" in
  Darwin) PLAT=universal-darwin ;;
  Linux)
    case "$ARCH" in
      x86_64) PLAT=x86_64-linux ;;
      aarch64 | arm64) PLAT=aarch64-linux ;;
      *) echo "不支持的架构: $ARCH" >&2; exit 1 ;;
    esac ;;
  *) echo "目前只支持 macOS、Linux" >&2; exit 1 ;;
esac
[ -x "$SRC/bin/$PLAT/xetex" ] || { echo "$SRC 不像 TeX Live 根目录（找不到 bin/$PLAT/xetex）" >&2; exit 1; }

# xelatex / pdflatex 用不到的宏包。缺了的以后由缺字清单按需装回用户层。
DROP_PKGS=(
  # 其他引擎：LuaTeX、日文 pTeX 系走按需安装；pdfTeX 内置
  luatex luahbtex luaotfload ptex uptex euptex ptex-fonts uptex-fonts
  zhmetrics zhmetrics-uptex metafont mflogo xdvi ttfutils
  # 给 CJK 宏包排汉字用的 CJK Type1 字体：用到时缺字清单按需装
  arphic wadalab uhc cns fonts-tlwg
  # CJK 的 Adobe CMap：xdvipdfmx 嵌入 OpenType 字体时用不到（实测中文排版与复制文字都正常）
  adobemapping
  # LaTeX 开发版（tex/latex-dev，给 pdflatex-dev 等格式用）：内置只生成 xelatex、pdflatex 两个正式格式
  latex-base-dev l3kernel-dev latex-firstaid-dev
  # 源 TeX Live 用久了会按需补装进来的大字体包（roboto ~80 MB、lato ~45 MB…），不进安装包，用到时再装
  roboto lato xcharter mathdesign nimbus15 arev fpl fontools mwe
)
# libertine、sourcesans、newtx、tipa、fandol 都保留：真实项目常用（Overleaf 模板、arXiv 论文），
# 不带 ctex 的 xeCJK 文档在 macOS 上也默认找 Fandol。删了等于让用户一打开就得装

# 留在 bin/ 里的程序，其余删掉
KEEP_BINS=(
  xetex xelatex xdvipdfmx dvipdfmx
  pdftex pdflatex
  kpsewhich kpseaccess kpsereadlink kpsestat
  mktexlsr texhash fmtutil fmtutil-sys fmtutil-user mktexfmt
  updmap updmap-sys updmap-user
  tlmgr bibtex biber makeindex
)

echo "== 复制 $SRC → $DEST"
rm -rf "$DEST"
mkdir -p "$DEST"
# 保留符号链接与权限（xelatex → xetex、tlmgr → tlmgr.pl 都是链接）；装机日志、远端 tlpdb 缓存、备份、
# 源里旧的文件索引（下面重新生成）、LuaTeX 字体缓存（LuaTeX 不随包分发）不要
if [ "$OS" = Darwin ]; then ditto "$SRC" "$DEST"; else cp -a "$SRC/." "$DEST/"; fi
rm -rf "$DEST"/install-tl* "$DEST"/tlpkg/backups "$DEST"/tlpkg/texlive.tlpdb.main.* \
  "$DEST"/tlpkg/*-file-index.txt "$DEST"/texmf-var/luatex-cache \
  "$DEST"/tlpkg/translations "$DEST"/tlpkg/tltcl "$DEST"/tlpkg/installer/install-tl-gui.tcl \
  "$DEST"/texmf-var/web2c/*.log

TLMGR="$DEST/bin/$PLAT/tlmgr"
"$TLMGR" option autobackup 0 >/dev/null

echo "== 移除宏包"
# 只移除确实装了的：remove 遇到没装的包会整体报错
have="$("$TLMGR" info --only-installed --data name 2>/dev/null)"
installed=()
for p in "${DROP_PKGS[@]}"; do
  grep -qx "$p" <<<"$have" && installed+=("$p")
done
if [ ${#installed[@]} -gt 0 ]; then
  # --no-depends：不连带删依赖，但会一起删 <包>.<平台> 二进制包。
  # 别用 --no-depends-at-all：本版 tlmgr 下它会让 remove 什么都不做
  "$TLMGR" remove --force --no-depends "${installed[@]}" >/dev/null 2>&1
  left="$("$TLMGR" info --only-installed --data name 2>/dev/null)"
  for p in "${installed[@]}"; do
    grep -qx "$p" <<<"$left" && { echo "没删掉: $p" >&2; exit 1; }
  done
fi

echo "== 完整版文件索引"
# 缺字清单离线判断「缺的文件属于哪个包」靠它（server/src/case.rs 的 FILE_INDEX），要联网取软件源的 tlpdb
"$HERE/scripts/texlive-index.sh" "$TLMGR" "$DEST/tlpkg/subo-file-index.txt"
# dump-tlpdb --remote 会把软件源的 tlpdb 缓存成 tlpkg/texlive.tlpdb.main.<hash>（约 20 MB），
# 复制时删过一次，这里又生成了，再删
rm -f "$DEST"/tlpkg/texlive.tlpdb.main.*

echo "== 删除编译用不到的文件"
# afm 只给 afm2tfm / fontinst 用；source、doc 不参与编译
rm -rf "$DEST"/texmf-dist/fonts/afm "$DEST"/texmf-dist/fonts/source \
  "$DEST"/texmf-dist/doc "$DEST"/texmf-dist/source
find "$DEST/texmf-dist" -type d -empty -delete
# 其他引擎的格式文件；pdftex 目录下只留 pdflatex（etex、pdftex、mptopdf 用不到）
find "$DEST/texmf-var/web2c" -mindepth 1 -maxdepth 1 -type d ! -name xetex ! -name pdftex -exec rm -rf {} +

echo "== 精简 bin/"
BIN="$DEST/bin/$PLAT"
for f in "$BIN"/*; do
  name="$(basename "$f")"
  keep=0
  for k in "${KEEP_BINS[@]}"; do [ "$name" = "$k" ] && keep=1 && break; done
  [ $keep = 1 ] || rm -f "$f"
done
thin() {
  local f="$1"
  [ -L "$f" ] && return
  if lipo -info "$f" 2>/dev/null | grep -q "^Architectures in the fat file"; then
    lipo -thin "$ARCH" "$f" -output "$f.thin" && mv "$f.thin" "$f"
  fi
}
# macOS 的二进制是通用包，只留本机架构；Linux 的本来就是单架构
if [ "$OS" = Darwin ]; then
  for f in "$BIN"/* "$DEST"/tlpkg/installer/*/*; do [ -f "$f" ] && thin "$f"; done
fi
# xdvipdfmx 转 EPS 调的 rungs 原本是 texlua 脚本（已随 LuaTeX 删掉），换成找系统 gs 的 sh 脚本
rm -f "$BIN/rungs"
cp "$HERE/scripts/rungs.sh" "$BIN/rungs"
chmod 755 "$BIN/rungs"
# pdflatex 转 EPS 走 epstopdf 宏包 → 受限 shell-escape 调 repstopdf（texmf.cnf 的 shell_escape_commands 里有它）。
# TeX Live 的 epstopdf 是 Perl 脚本，链接已随 KEEP_BINS 删掉；换成同样找系统 gs 的 sh 脚本
rm -f "$BIN/epstopdf" "$BIN/repstopdf"
cp "$HERE/scripts/epstopdf.sh" "$BIN/epstopdf"
chmod 755 "$BIN/epstopdf"
ln -s epstopdf "$BIN/repstopdf"

echo "== 重建格式、字体映射与文件索引"
# 用户层（TEXMFHOME / TEXMFVAR / TEXMFCONFIG）按年份放在 ~/.subo/texlive/<年份>；桌面壳启动时还会用
# 环境变量按 SUBO_HOME 覆盖一遍，这里写的是直接在终端里跑包内程序时的默认值
YEAR="$(head -1 "$DEST/release-texlive.txt" | awk '{print $NF}')"
cat > "$DEST/texmf.cnf" <<EOF
% Subo 内置 TeX Live：包内只读，用户层在 ~/.subo/texlive/$YEAR
TEXMFLOCAL = \$SELFAUTOPARENT/texmf-local
TEXMFHOME = ~/.subo/texlive/$YEAR/texmf
TEXMFVAR = ~/.subo/texlive/$YEAR/texmf-var
TEXMFCONFIG = ~/.subo/texlive/$YEAR/texmf-config
EOF
export PATH="$BIN:/usr/bin:/bin:/usr/sbin:/sbin"
# 构建期把系统层当可写层用，不碰任何人的 ~/.subo；TEXMFHOME 也指到空目录，
# 否则 updmap 会把构建机用户层里的字体映射写进包里
EMPTY_HOME="$(mktemp -d)"
export TEXMFVAR="$DEST/texmf-var" TEXMFCONFIG="$DEST/texmf-config" TEXMFHOME="$EMPTY_HOME"
mktexlsr "$DEST/texmf-dist" "$DEST/texmf-var" >/dev/null 2>&1
for fmt in xelatex:xetex pdflatex:pdftex; do
  f="${fmt%%:*}" e="${fmt##*:}"
  fmtutil-sys --byfmt "$f" >/dev/null 2>&1 || { echo "$f 格式生成失败，见 $DEST/texmf-var/web2c/$e/$f.log" >&2; exit 1; }
done
find "$DEST/texmf-var/web2c/pdftex" -type f ! -name pdflatex.fmt -delete
# 宏包删过一轮，映射要按包里实际有的字体重新生成（pdftex.map 给 pdflatex，dvipdfmx 的给 xdvipdfmx）
updmap-sys >/dev/null 2>&1 || { echo "updmap-sys 失败" >&2; exit 1; }
rm -rf "$EMPTY_HOME"
rm -f "$DEST"/texmf-var/web2c/*.log "$DEST"/texmf-var/web2c/xetex/*.log
mktexlsr "$DEST/texmf-dist" "$DEST/texmf-var" >/dev/null 2>&1

echo "== 体积"
du -sh "$SRC" | sed 's/^/  源:   /'
du -sh "$DEST" | sed 's/^/  精简: /'
for d in bin texmf-dist texmf-var tlpkg; do du -sh "$DEST/$d" | sed 's/^/    /'; done
