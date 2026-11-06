#!/bin/bash
# 从 macOS / Linux 上裁好的内置 TeX Live（desktop/texlive）做出 Windows 版，给交叉打的 Windows 安装包用。
#   ./scripts/texlive-windows.sh [源目录，默认 desktop/texlive] [目标目录，默认 desktop/texlive-windows]
#
# 宏包与源目录完全一样，只换二进制：用源里的 tlmgr 按已装宏包下载 Windows 的二进制（platform add windows），
# 再删掉本机平台的。格式文件（.fmt）直接沿用：web2c 的格式与平台无关，只要求引擎版本一致，下面会核对。
# 要联网（约 100 MB）；软件源用 SUBO_TL_REPO，默认清华镜像，年份必须和源目录一致。
set -euo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"
SRC="${1:-$HERE/desktop/texlive}"
DEST="${2:-$HERE/desktop/texlive-windows}"
REPO="${SUBO_TL_REPO:-https://mirrors.tuna.tsinghua.edu.cn/CTAN/systems/texlive/tlnet}"

case "$(uname -s)" in
  Darwin) PLAT=universal-darwin ;;
  Linux) PLAT="$(uname -m | sed 's/arm64/aarch64/')-linux" ;;
  *) echo "在 macOS 或 Linux 上跑" >&2; exit 1 ;;
esac
[ -x "$SRC/bin/$PLAT/tlmgr" ] || { echo "$SRC 不是裁好的内置 TeX Live（找不到 bin/$PLAT/tlmgr），先跑 scripts/texlive-trim.sh" >&2; exit 1; }

year="$(head -1 "$SRC/release-texlive.txt" | awk '{print $NF}')"
remote="$(curl -fsS -r 0-8000 "$REPO/tlpkg/texlive.tlpdb" | sed -n 's/^depend release\/\([0-9]*\).*/\1/p' | head -1)"
[ "$remote" = "$year" ] || { echo "软件源是 TeX Live ${remote:-?}，内置的是 $year：换 SUBO_TL_REPO（跨年后用 $year 的 tlnet-final 归档源）" >&2; exit 1; }

echo "== 复制 $SRC → $DEST"
rm -rf "$DEST"
mkdir -p "$DEST"
if [ "$PLAT" = universal-darwin ]; then ditto "$SRC" "$DEST"; else cp -a "$SRC/." "$DEST/"; fi
TLMGR="$DEST/bin/$PLAT/tlmgr"
export PATH="$DEST/bin/$PLAT:/usr/bin:/bin:/usr/sbin:/sbin"

echo "== 下载 Windows 二进制"
"$TLMGR" --repository "$REPO" platform add windows >/dev/null
# 一起装进来的 PDF 小工具（wintools）、dviout 预览器、tlshell 图形界面用不上；
# tlperl（Windows 上的 tlmgr 要它）、tlgs（EPS 转 PDF 用的 Ghostscript）留着
"$TLMGR" remove --force --no-depends collection-wintools wintools.windows dviout.windows tlshell.windows >/dev/null 2>&1

echo "== 核对引擎版本（格式文件沿用，版本必须一致）"
ver() { grep -aoE "$2" "$1" | head -1; }
for pair in "xetex:3\.141592653-2\.6-0\.[0-9]+" "pdftex:3\.141592653-2\.6-1\.40\.[0-9]+"; do
  e="${pair%%:*}" re="${pair#*:}"
  a="$("$DEST/bin/$PLAT/$e" --version | head -1 | grep -oE "$re")"
  b="$(ver "$DEST/bin/windows/$e.dll" "$re")"
  [ -n "$a" ] && [ "$a" = "$b" ] || { echo "$e 版本对不上：本机 $a，Windows ${b:-?}" >&2; exit 1; }
  echo "  $e $a"
done

echo "== 去掉本机平台"
# tlmgr 不肯删自己正在跑的平台，也不肯删别的平台的二进制包：直接改 tlpdb，删掉 <包>.<本机平台> 的条目
python3 - "$DEST/tlpkg/texlive.tlpdb" "$PLAT" <<'EOF'
import sys
path, plat = sys.argv[1], sys.argv[2]
blocks = open(path).read().split("\n\n")
def name(b):
    first = b.lstrip("\n").split("\n", 1)[0]
    return first[5:] if first.startswith("name ") else ""
keep = [b for b in blocks if not name(b).endswith("." + plat)]
text = "\n\n".join(keep)
text = "\n".join("depend setting_available_architectures:windows" if l.startswith("depend setting_available_architectures:") else l for l in text.split("\n"))
open(path, "w").write(text)
EOF
rm -rf "$DEST/bin/$PLAT" "$DEST"/tlpkg/installer/*/*."$PLAT" "$DEST"/tlpkg/tlpobj/*."$PLAT".tlpobj \
  "$DEST"/tlpkg/tltcl "$DEST"/tlpkg/dviout "$DEST"/tlpkg/texlive.tlpdb.main.* "$DEST"/texmf-var/web2c/tlmgr*.log \
  "$DEST"/texmf-var/fonts/conf/texlive-fontconfig.conf
# 本机版换掉的 rungs、epstopdf 是 sh 脚本；Windows 用 TeX Live 自己的 .exe（配 tlgs）

echo "== XeTeX 的字体配置"
# Windows 上的 XeTeX 靠 fontconfig 找字体，读 TEXMFSYSVAR/fonts/conf/fonts.conf。这份文件本该由 xetex 的
# 安装后脚本在 Windows 上生成，交叉做时不会跑，这里照它的样子生成：Windows 字体目录用 fontconfig 自己认的
# WINDOWSFONTDIR / WINDOWSUSERFONTDIR，TeX Live 的字体目录按配置文件所在位置写相对路径，装到哪里都对
CONF="$DEST/texmf-var/fonts/conf"
rm -rf "$CONF" "$DEST/texmf-var/fonts/cache"
mkdir -p "$CONF"
cp -R "$DEST/tlpkg/tlpostcode/xetex/conf/." "$CONF/"
python3 - "$CONF/fonts.conf" <<'PY'
import sys, re
path = sys.argv[1]
text = open(path).read()
dirs = ('<dir>WINDOWSFONTDIR</dir>\n<dir>WINDOWSUSERFONTDIR</dir>\n'
        '<dir prefix="relative">../../../texmf-dist/fonts/opentype</dir>\n'
        '<dir prefix="relative">../../../texmf-dist/fonts/truetype</dir>\n')
# 去掉注释外的原有 <dir>（c:/windows/fonts、c:/Program Files/texlive/YYYY/...），换成上面这几行
body = re.sub(r'<!--.*?-->', lambda m: m.group(0).replace('<dir>', '<dir-x>'), text, flags=re.S)
body = re.sub(r'^<dir>[^<]*</dir>\n', '', body, flags=re.M)
body = body.replace('<dir-x>', '<dir>')
body = body.replace('<fontconfig>\n', '<fontconfig>\n' + dirs, 1)
open(path, 'w').write(body)
PY
# 字体缓存写进用户层（桌面壳把 TEXMFVAR 设在 ~/.subo/texlive/<年份>）：安装目录可能不可写，重装也不丢
cat >> "$DEST/texmf.cnf" <<'EOF'
XE_FC_CACHEDIR = $TEXMFVAR/fonts/cache
EOF
[ -z "$(find "$DEST" -type l | head -1)" ] || { echo "还有符号链接，Windows 上放不了：" >&2; find "$DEST" -type l >&2; exit 1; }

echo "== 体积"
du -sh "$DEST" | sed 's/^/  /'
for d in bin texmf-dist texmf-var tlpkg; do du -sh "$DEST/$d" | sed 's/^/    /'; done
