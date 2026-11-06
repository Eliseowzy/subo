#!/usr/bin/env bash
# 把打好的包发布到 Cloudflare R2：应用内更新包、给新用户下载的安装包、版本清单 latest.json。
# 先用 scripts/desktop.sh 打包（macOS 要签名、公证），再运行：
#
#   ./scripts/release-r2.sh --notes 更新说明.md --dry-run   # 只看会传什么、清单长什么样
#   ./scripts/release-r2.sh --notes 更新说明.md
#
# R2 上的布局（地址前缀默认 https://dl.subo.ink）：
#   <版本>/…              每个版本的包，传上去就不再动
#   Subo_latest_<架构>.dmg 官网「下载」按钮用的固定地址，每次覆盖
#   latest.json           版本清单，最后才覆盖：包没传完之前客户端看不到新版本
#
# 用 npx wrangler 上传，先 `npx wrangler login` 登录一次（或设 CLOUDFLARE_API_TOKEN）。
# 环境变量：SUBO_R2_BUCKET（默认 subo）、SUBO_UPDATE_BASE（默认 https://dl.subo.ink）
set -euo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"
cd "$HERE/desktop"

NOTES=""
DRY=0
while [ $# -gt 0 ]; do
  case "$1" in
    --notes) NOTES="${2:?--notes 后面要跟更新说明文件}"; shift 2 ;;
    --dry-run) DRY=1; shift ;;
    *) echo "不认识的参数：$1（可选 --notes 文件、--dry-run）" >&2; exit 2 ;;
  esac
done
[ -z "$NOTES" ] || [ -f "$NOTES" ] || { echo "找不到更新说明文件：$NOTES" >&2; exit 1; }

BUCKET="${SUBO_R2_BUCKET:-subo}"
BASE="${SUBO_UPDATE_BASE:-https://dl.subo.ink}"
BASE="${BASE%/}"
VERSION="$(sed -n 's/^  "version": "\(.*\)",$/\1/p' tauri.conf.json)"
[ -n "$VERSION" ] || { echo "读不出 tauri.conf.json 里的版本号" >&2; exit 1; }

# 应用内更新包：清单里的平台键、文件。键名是 updater 插件认的 <系统>-<架构>，
# Windows 轻量版另用 windows-x86_64-lite（desktop/src/update.rs）
KEYS=()
FILES=()
add() { [ -f "$2" ] && { KEYS+=("$1"); FILES+=("$2"); }; return 0; }
add darwin-aarch64 "target/dist/bundle/updater/Subo_${VERSION}_aarch64.app.tar.gz"
add darwin-x86_64 "target/x86_64-apple-darwin/dist/bundle/updater/Subo_${VERSION}_x64.app.tar.gz"
add windows-x86_64 "target/x86_64-pc-windows-gnu/dist/bundle/nsis/Subo_${VERSION}_x64-setup-tex.exe"
add windows-x86_64-lite "target/x86_64-pc-windows-gnu/dist/bundle/nsis/Subo_${VERSION}_x64-setup-lite.exe"
[ ${#KEYS[@]} -gt 0 ] || { echo "没找到 $VERSION 的更新包。先用 scripts/desktop.sh 打包（要有更新签名私钥）" >&2; exit 1; }
for f in "${FILES[@]}"; do
  [ -f "$f.sig" ] || { echo "$f 没有签名文件 $f.sig。重新打包，或手动：npx tauri signer sign -f ~/.tauri/subo-updater.key --app-version $VERSION $f" >&2; exit 1; }
done

# 给新用户直接下载的安装包：dmg、各种 Windows 安装程序（含离线版）。名字 → 固定地址的别名（空就不起别名）
DIRECT=()
ALIAS=()
direct() { [ -f "$1" ] && { DIRECT+=("$1"); ALIAS+=("$2"); }; return 0; }
direct "target/dist/bundle/dmg/Subo_${VERSION}_aarch64.dmg" Subo_latest_aarch64.dmg
direct "target/x86_64-apple-darwin/dist/bundle/dmg/Subo_${VERSION}_x64.dmg" Subo_latest_x64.dmg
for v in tex lite tex-offline lite-offline; do
  direct "target/x86_64-pc-windows-gnu/dist/bundle/nsis/Subo_${VERSION}_x64-setup-$v.exe" "Subo_latest_x64-setup-$v.exe"
done

# 版本清单：插件要的格式 { version, notes, pub_date, platforms: { 键: { url, signature } } }
MANIFEST="$(mktemp -d)/latest.json"
PAIRS=()
for i in "${!KEYS[@]}"; do
  PAIRS+=("${KEYS[$i]}" "$BASE/$VERSION/$(basename "${FILES[$i]}")" "$(cat "${FILES[$i]}.sig")")
done
node -e '
const [out, version, notesFile, date, ...rest] = process.argv.slice(1);
const platforms = {};
for (let i = 0; i < rest.length; i += 3) platforms[rest[i]] = { url: rest[i + 1], signature: rest[i + 2] };
const notes = notesFile ? require("fs").readFileSync(notesFile, "utf8").trim() : "";
require("fs").writeFileSync(out, JSON.stringify({ version, notes, pub_date: date, platforms }, null, 2) + "\n");
' "$MANIFEST" "$VERSION" "$NOTES" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "${PAIRS[@]}"

echo "== 素帛 $VERSION → R2 bucket「$BUCKET」（$BASE）"
echo "应用内更新：${KEYS[*]}"
for i in ${DIRECT[@]+"${!DIRECT[@]}"}; do echo "安装包：$(basename "${DIRECT[$i]}")${ALIAS[$i]:+ → ${ALIAS[$i]}}"; done
# 线上清单的版本不比这次旧才往下走，免得手滑把旧版本发成最新
ONLINE="$(curl -fsS --max-time 15 "$BASE/latest.json" 2>/dev/null | node -e 'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>{try{console.log(JSON.parse(s).version)}catch{}})' || true)"
[ -n "$ONLINE" ] && echo "线上当前版本：$ONLINE"
if [ -n "$ONLINE" ] && [ "$ONLINE" != "$VERSION" ] && [ "$(printf '%s\n%s\n' "$ONLINE" "$VERSION" | sort -V | tail -1)" = "$ONLINE" ]; then
  echo "线上已是 $ONLINE，比要发的 $VERSION 新。先改 tauri.conf.json 的版本号重新打包" >&2
  exit 1
fi
if [ $DRY = 1 ]; then
  echo "== latest.json（--dry-run，没有上传）"
  cat "$MANIFEST"
  exit 0
fi

put() { # 本地文件 远端键 cache-control content-type
  echo "上传 $2"
  npx --yes wrangler r2 object put "$BUCKET/$2" --remote --file "$1" --cache-control "$3" --content-type "$4" >/dev/null
}
type_of() {
  case "$1" in
    *.dmg) echo application/x-apple-diskimage ;;
    *.tar.gz) echo application/gzip ;;
    *.exe) echo application/vnd.microsoft.portable-executable ;;
    *) echo application/octet-stream ;;
  esac
}
IMMUTABLE="public, max-age=31536000, immutable"
for f in "${FILES[@]}" ${DIRECT[@]+"${DIRECT[@]}"}; do
  put "$f" "$VERSION/$(basename "$f")" "$IMMUTABLE" "$(type_of "$f")"
done
for f in "${FILES[@]}"; do
  put "$f.sig" "$VERSION/$(basename "$f").sig" "$IMMUTABLE" text/plain
done
# 固定地址、清单会被覆盖，缓存给短一点
for i in ${DIRECT[@]+"${!DIRECT[@]}"}; do
  [ -n "${ALIAS[$i]}" ] && put "${DIRECT[$i]}" "${ALIAS[$i]}" "public, max-age=300" "$(type_of "${DIRECT[$i]}")"
done
put "$MANIFEST" latest.json "public, max-age=300" application/json
rm -rf "$(dirname "$MANIFEST")"
echo "== 已发布 $VERSION：$BASE/latest.json"
