#!/bin/bash
# 一键重启：停掉占着 8787 的旧服务，重新构建前后端再启动
#   ./scripts/restart.sh
# 旧进程不停，新前端会调到旧接口
set -euo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"
PORT=8787

pids="$(lsof -tiTCP:$PORT -sTCP:LISTEN || true)"
if [ -n "$pids" ]; then
  echo "== 停止旧服务 (pid $pids)"
  kill $pids
  for _ in $(seq 50); do
    lsof -tiTCP:$PORT -sTCP:LISTEN >/dev/null || break
    sleep 0.1
  done
  if lsof -tiTCP:$PORT -sTCP:LISTEN >/dev/null; then
    echo "端口 $PORT 仍被占用，放弃" >&2
    exit 1
  fi
fi

exec "$HERE/scripts/dev.sh"
