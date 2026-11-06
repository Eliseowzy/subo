#!/bin/sh
# Subo 内置 TeX Live 里的 epstopdf / repstopdf：pdflatex 遇到 EPS 插图时，epstopdf 宏包经受限
# shell-escape 调 `repstopdf --outfile=fig-eps-converted-to.pdf fig.eps`。TeX Live 自带的是 Perl 脚本，
# 内置版不依赖 Perl，所以由这个脚本接手，直接调系统的 Ghostscript（同 scripts/rungs.sh）。
# 找不到 gs 就打一行和 rungs 相同格式的话，
# 服务端（server/src/case.rs 的 missing_tools）认这行，在缺字清单里说清楚缺什么。
#
# 只实现 epstopdf 宏包和日常手工转换用得到的部分；gs 参数与 epstopdf.pl 的默认值一致
# （PDF 1.5、prepress、嵌入全部字体、不自动旋转），页面按 %%BoundingBox 裁（-dEPSCrop）。
# 以 repstopdf 名字运行或带 --restricted 时是受限模式：输入、输出文件名要过 kpsewhich 的
# openin_any / openout_any 检查，不接受 --gsopt 等能改 gs 行为的选项。
#
# 编译带 -output-directory 时 TeX 会设 TEXMF_OUTPUT_DIRECTORY，转好的 PDF 写到那里而不是项目里，
# pdfTeX 照样找得到，用户的源码目录里不多出 *-eps-converted-to.pdf。

prog="${0##*/}"
die() { echo "$prog: $*" >&2; exit 1; }

restricted=0
[ "$prog" = repstopdf ] && restricted=1
outfile="" infile="" extra="" autorotate=None embed=1 compress=1 gray=0 pdfsettings="" gsopts=""
nl='
'
while [ $# -gt 0 ]; do
  a="$1"
  # Getopt::Long 也认单横线写法
  case "$a" in -[!-]?*) a="-$a" ;; esac
  case "$a" in
    --outfile=*) outfile="${a#--outfile=}" ;;
    --outfile) shift; [ $# -gt 0 ] || die "--outfile 缺参数"; outfile="$1" ;;
    --restricted) restricted=1 ;;
    --autorotate=*) autorotate="${a#--autorotate=}" ;;
    --embed) embed=1 ;;
    --noembed) embed=0 ;;
    --compress) compress=1 ;;
    --nocompress) compress=0 ;;
    --gray) gray=1 ;;
    --nogray) gray=0 ;;
    --pdfsettings=*) pdfsettings="${a#--pdfsettings=}" ;;
    --gsopt=*) gsopts="$gsopts${nl}${a#--gsopt=}" ;;
    --gsopts=*) for o in ${a#--gsopts=}; do gsopts="$gsopts${nl}$o"; done ;;
    --hires|--exact|--nohires|--noexact|--debug|--nodebug|--quiet|--noquiet|--safer|--gs) ;;
    --help) echo "usage: $prog [--outfile=out.pdf] [--restricted] file.eps [out.pdf]"; exit 0 ;;
    --version) echo "$prog (Subo, Ghostscript wrapper)"; exit 0 ;;
    --) shift; break ;;
    --*) die "Subo 内置版不支持这个选项: $1" ;;
    *) if [ -z "$infile" ]; then infile="$1"; elif [ -z "$extra" ]; then extra="$1"; else die "参数太多: $1"; fi ;;
  esac
  shift
done
for a; do if [ -z "$infile" ]; then infile="$a"; elif [ -z "$extra" ]; then extra="$a"; else die "参数太多: $a"; fi; done

[ -n "$infile" ] || die "缺输入文件"
if [ -n "$extra" ]; then
  [ -z "$outfile" ] || die "输出文件给了两次: $extra, --outfile=$outfile"
  case "$extra" in *.pdf|*.PDF) outfile="$extra" ;; *) die "输出文件要以 .pdf 结尾: $extra" ;; esac
fi
[ -n "$outfile" ] || outfile="${infile%.*}.pdf"
case "$autorotate" in None|All|PageByPage) ;; *) die "--autorotate 只能是 None、All、PageByPage" ;; esac
case "$pdfsettings" in ''|screen|ebook|printer|prepress|default) ;; /*) pdfsettings="${pdfsettings#/}" ;; *) die "--pdfsettings 取值不对" ;; esac
[ -z "$pdfsettings" ] && [ $embed = 1 ] && pdfsettings=prepress

if [ $restricted = 1 ]; then
  [ -z "$gsopts" ] || die "受限模式不接受 --gsopt / --gsopts"
  case "$outfile" in '|'*|%pipe*) die "受限模式不能输出到管道: $outfile" ;; esac
  kpsewhich -progname repstopdf -safe-in-name "$infile" >/dev/null 2>&1 || die "受限模式不允许读: $infile"
  kpsewhich -progname repstopdf -safe-out-name "$outfile" >/dev/null 2>&1 || die "受限模式不允许写: $outfile"
fi
[ -r "$infile" ] || die "读不到 $infile"

# 输出写进 TeX 的产物目录（相对路径才改），子目录按需建
case "$outfile" in
  /*) ;;
  *) if [ -n "$TEXMF_OUTPUT_DIRECTORY" ]; then
       outfile="$TEXMF_OUTPUT_DIRECTORY/$outfile"
       mkdir -p "$(dirname -- "$outfile")" || die "建不了目录: $(dirname -- "$outfile")"
     fi ;;
esac

gs=""
IFS=:
for d in $PATH /opt/homebrew/bin /usr/local/bin /opt/local/bin /usr/bin; do
  [ -n "$d" ] && [ -x "$d/gs" ] && { gs="$d/gs"; break; }
done
unset IFS
if [ -z "$gs" ]; then
  echo "subo-rungs: ghostscript not found: $infile" >&2
  exit 127
fi

# 以 - 开头的文件名会被 gs 当成选项；% 在 -sOutputFile 里有特殊含义
case "$infile" in -*) infile="./$infile" ;; esac
out=$(printf '%s' "$outfile" | sed 's/%/%%/g')
set -- -q -dSAFER -dNOPAUSE -dBATCH -dCompatibilityLevel=1.5 -sDEVICE=pdfwrite "-sOutputFile=$out"
[ -n "$pdfsettings" ] && set -- "$@" "-dPDFSETTINGS=/$pdfsettings"
[ $embed = 1 ] && set -- "$@" -dMaxSubsetPct=100 -dSubsetFonts=true -dEmbedAllFonts=true
[ $compress = 0 ] && set -- "$@" -dUseFlateCompression=false
[ $gray = 1 ] && set -- "$@" -sColorConversionStrategy=Gray -dProcessColorModel=/DeviceGray
set -- "$@" "-dAutoRotatePages=/$autorotate" -dEPSCrop
IFS="$nl"
for o in $gsopts; do [ -n "$o" ] && set -- "$@" "$o"; done
unset IFS
exec "$gs" "$@" "$infile"
