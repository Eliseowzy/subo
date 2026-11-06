#!/bin/sh
# Subo 内置 TeX Live 里的 rungs：xdvipdfmx 转 EPS / PS 插图时调 rungs，
# TeX Live 自带的 rungs 是 texlua 脚本，而内置版裁掉了 LuaTeX，所以由这个脚本接手。
# Ghostscript 不随 Subo 分发（AGPL、体积），
# 用户自己装的 gs 在 PATH 或常见位置里就直接用；找不到就打一行固定格式的话，
# 服务端（server/src/case.rs 的 missing_tools）认这行，在缺字清单里说清楚缺什么。
IFS=:
for d in $PATH /opt/homebrew/bin /usr/local/bin /opt/local/bin /usr/bin; do
  [ -n "$d" ] && [ -x "$d/gs" ] && exec "$d/gs" "$@"
done
unset IFS
last=""
for a in "$@"; do last="$a"; done
echo "subo-rungs: ghostscript not found: $last" >&2
exit 127
