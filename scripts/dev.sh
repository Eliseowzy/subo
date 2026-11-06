#!/bin/bash
# 开发模式：构建前端 + 跑本地服务
#   ./scripts/dev.sh
# 项目在界面里打开：首次使用时引导页选「打开本地文件夹」，或新建 / 导入 zip
set -euo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"

# 自包含工具链（TeX Live + Rust）所在的工作区；用 SUBO_WS 指定，
# 不存在就退回系统 PATH 里的 cargo / xelatex
WS="${SUBO_WS:-}"
if [ -d "$WS/.texlive" ]; then
  export PATH="$WS/.texlive/bin/universal-darwin:$PATH"
fi
if [ -d "$WS/.toolchain" ]; then
  export RUSTUP_HOME="$WS/.toolchain/rustup"
  export CARGO_HOME="$WS/.toolchain/cargo"
  export PATH="$CARGO_HOME/bin:$PATH"
fi

command -v cargo >/dev/null || { echo "找不到 cargo：请设置 SUBO_WS 指向工具链工作区" >&2; exit 1; }

echo "== 构建前端"
(cd "$HERE/app" && npm run build --silent)

echo "== 构建服务"
(cd "$HERE/server" && cargo build --quiet)

echo "== 启动"
exec "$HERE/server/target/debug/subo-server" \
  --port 8787 --dist "$HERE/app/dist"
