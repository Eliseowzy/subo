#!/bin/bash
# 用随应用内置的精简 TeX Live 跑一遍桌面端的「编译 → 缺字清单 → 安装 → 重编」，
# 在终端里复现安装包上的行为，不用打包。
#   ./scripts/bundled-smoke.sh [额外的项目目录…]
#
# 默认编译 fixtures/smoke/ 下的文档，可以追加真实项目（会复制一份，原目录不动）；
# SUBO_SMOKE_ONLY=1 只跑参数里给的项目；SUBO_SMOKE_LOGS=目录 把每个项目最后一次编译的日志存下来。
# 用户层、项目库、缓存都放在临时目录，不碰 ~/.subo 和 ~/Documents/Subo；
# 补装宏包要联网，走主树配置的镜像。
# 内置 TeX 默认取 desktop/texlive（scripts/texlive-trim.sh 生成），可用 SUBO_TL 指到别处。
set -euo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"
TL="${SUBO_TL:-$HERE/desktop/texlive}"
case "$(uname -s)" in
  Darwin) PLAT=universal-darwin ;;
  Linux) PLAT="$(uname -m | sed 's/arm64/aarch64/')-linux" ;;
esac
[ -x "$TL/bin/$PLAT/xelatex" ] || { echo "找不到内置 TeX：$TL（先跑 scripts/texlive-trim.sh）" >&2; exit 1; }

WS="${SUBO_WS:-}"
if [ -d "$WS/.toolchain" ]; then
  export RUSTUP_HOME="$WS/.toolchain/rustup" CARGO_HOME="$WS/.toolchain/cargo"
  export PATH="$CARGO_HOME/bin:$PATH"
fi
(cd "$HERE/server" && cargo build --quiet)

TMP="$(mktemp -d "${TMPDIR:-/tmp}/subo-smoke.XXXXXX")"
trap 'kill $SERVER 2>/dev/null || true; rm -rf "$TMP"' EXIT
mkdir -p "$TMP/quires"
DEFAULTS=("$HERE"/fixtures/smoke/*/)
[ "${SUBO_SMOKE_ONLY:-0}" = 1 ] && DEFAULTS=()
for d in ${DEFAULTS[@]+"${DEFAULTS[@]}"} "$@"; do
  [ -d "$d" ] && cp -R "${d%/}" "$TMP/quires/"
done

# 与 desktop/src/platform.rs 的 choose_tex 一致：内置 bin 打头，用户层在 SUBO_HOME/texlive
LAYER="$TMP/subo/texlive"
PORT="${SUBO_SMOKE_PORT:-8799}"
# debug 构建比发行版慢得多，大论文（几十页、几十张图）单次编译可能超过默认 120 s；
# SUBO_SMOKE_TIMEOUT 放宽服务端单任务上限，验收大样本时用
TIMEOUT="${SUBO_SMOKE_TIMEOUT:-120}"
env PATH="$TL/bin/$PLAT:/usr/bin:/bin:/usr/sbin:/sbin" \
  SUBO_HOME="$TMP/subo" \
  TEXMFHOME="$LAYER/texmf" TEXMFVAR="$LAYER/texmf-var" TEXMFCONFIG="$LAYER/texmf-config" \
  "$HERE/server/target/debug/subo-server" --bundled-tex --home "$TMP/quires" --port "$PORT" \
  --timeout "$TIMEOUT" \
  >"$TMP/server.log" 2>&1 &
SERVER=$!

python3 - "$PORT" <<'PY'
import json, os, re, sys, threading, time, urllib.request, urllib.parse, queue

BASE = f"http://127.0.0.1:{sys.argv[1]}"
for _ in range(100):
    try:
        urllib.request.urlopen(BASE + "/api/health", timeout=1)
        break
    except Exception:
        time.sleep(0.1)

events = queue.Queue()
def listen():
    with urllib.request.urlopen(BASE + "/api/events") as r:
        for raw in r:
            line = raw.decode("utf-8", "replace").rstrip("\n")
            if line.startswith("data:"):
                events.put(json.loads(line[5:]))
threading.Thread(target=listen, daemon=True).start()
time.sleep(0.5)

def post(path, body=None):
    req = urllib.request.Request(BASE + path, method="POST",
                                 data=json.dumps(body).encode() if body is not None else b"",
                                 headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=900) as r:
            return r.status, json.loads(r.read() or b"null")
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"null")

def build(pid):
    """编译一次，等到结果和缺字清单（查包完成）都到齐"""
    post(f"/api/p/{urllib.parse.quote(pid)}/build")
    result, missing, log, pending = None, None, "", False
    deadline = time.time() + 300
    while time.time() < deadline:
        try:
            e = events.get(timeout=1)
        except queue.Empty:
            continue
        if e.get("projectId") != pid:
            continue
        t = e["type"]
        if t == "job.log":
            log = e["text"]
        elif t in ("job.done", "job.failed"):
            result = e
        elif t == "case.missing":
            pending = True
            if e.get("resolved"):
                missing = e["files"]
        if result and missing is not None:
            return result["type"] == "job.done", missing, log
        # 日志里认不出缺什么：不会再有 case.missing（成功时也可能有，比如 bibtex 缺 .bst）
        if result and missing is None and not pending:
            try:
                e = events.get(timeout=3)
                events.put(e)
            except queue.Empty:
                return result["type"] == "job.done", [], log
    return False, None, log

def engine(pid):
    """项目用哪个程序、为什么（自动探测）"""
    e = json.load(urllib.request.urlopen(f"{BASE}/api/p/{urllib.parse.quote(pid)}/engine"))
    why = e.get("reason") or {}
    why = why.get("detail") or why.get("kind") or ""
    want = f"，要 {e['wanted']}" if e.get("wanted") else ""
    return f"{e['program']}（{e['source']}{' ' + why if why else ''}{want}）"

projects = [p["id"] for p in json.load(urllib.request.urlopen(BASE + "/api/projects"))]
report = []
for pid in projects:
    eng = engine(pid)
    tried, status, note = set(), "fail", ""
    for _ in range(8):
        ok, missing, log = build(pid)
        if ok and not missing:
            # 参考文献、术语表没跑出来时 PDF 照样产出，只是显示成 [?] 或空白：看最后一遍的日志
            last = re.split(r"== \w+ 第", log)[-1].replace("\n", "")
            bad = [k for k in ("undefined references", "undefined citations", "Empty bibliography",
                               "No file main.bbl", "No file main.gls", "Please (re)run Biber")
                   if k in last]
            if bad:
                note = "引用/术语表未解析: " + ", ".join(bad)
            else:
                status = "ok"
            break
        if missing is None:
            note = "编译超时"
            break
        pkgs = sorted({m["package"] for m in missing if m.get("package")} - tried)
        if not pkgs:
            # -file-line-error 下报错行是 `./main.tex:9: …`，不以 ! 开头
            errs = [l for l in log.splitlines() if l.startswith("!") or ".tex:" in l and "rror" in l][:3]
            tools = [f'{m["file"]}（{", ".join(m.get("neededBy", []))} 要用）' for m in missing if m.get("kind") == "tool"]
            unfound = [m["file"] for m in missing if not m.get("package") and m.get("kind") != "tool"]
            note = "缺程序: " + "; ".join(tools) if tools else "装不上: " + ", ".join(unfound) if unfound else " | ".join(errs) or "原因不明，日志末尾:\n    " + "\n    ".join(log.splitlines()[-12:])
            break
        tried.update(pkgs)
        code, r = post("/api/case/install", {"packages": pkgs})
        ok_install = code == 200 and r and r.get("ok")
        print(f"  {pid}: 安装 {' '.join(pkgs)} → {'成功' if ok_install else '失败'}", flush=True)
        if not ok_install:
            note = f"安装 {' '.join(pkgs)} 失败: " + (r.get("log") or r.get("detail") or json.dumps(r, ensure_ascii=False))[-600:]
            break
    if os.environ.get("SUBO_SMOKE_LOGS"):
        with open(os.path.join(os.environ["SUBO_SMOKE_LOGS"], pid + ".log"), "w") as f:
            f.write(log)
    report.append((pid, status, sorted(tried), note))
    print(f"{'✓' if status == 'ok' else '✗'} {pid}  {eng}  装了: {' '.join(sorted(tried)) or '-'}  {note}", flush=True)

failed = [r for r in report if r[1] != "ok"]
print(f"\n{len(report) - len(failed)}/{len(report)} 通过")
sys.exit(1 if failed else 0)
PY
