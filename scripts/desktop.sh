#!/bin/bash
# 桌面壳开发模式：构建前端，再以调试版启动 Tauri 壳（壳内自起本地服务，随机端口）
#   ./scripts/desktop.sh            # 跑调试版
#   ./scripts/desktop.sh bundle     # 打当前平台的安装包（macOS: .app + .dmg；Linux: .deb + AppImage）
#   ./scripts/desktop.sh bundle x64 # macOS 上打 Intel 版（Apple 芯片机器交叉编；内置 TeX 用 desktop/texlive-x64，
#                                   # 先跑 SUBO_ARCH=x86_64 scripts/texlive-trim.sh <源> desktop/texlive-x64）
#   ./scripts/desktop.sh windows    # 在 macOS / Linux 上交叉打 Windows 安装包（NSIS；有 desktop/texlive-windows 就内置 TeX）
#   ./scripts/desktop.sh windows lite   # 轻量版：不带内置 TeX，用本机的 TeX Live / MiKTeX（产物名带 -lite）
#   ./scripts/desktop.sh windows [lite] offline   # 离线安装包：把 WebView2 离线安装器打进去（多一百多 MB，产物名再带 -offline）
#   SUBO_RELEASE=1 ./scripts/desktop.sh bundle   # 发版档：Cargo profile dist（fat LTO），编译慢
#   SUBO_SIGN_IDENTITY="Developer ID Application: …" SUBO_NOTARY_PROFILE=subo SUBO_RELEASE=1 ./scripts/desktop.sh bundle
#                                                 # macOS 对外分发：Developer ID 签名 + 公证
# 默认 bundle 用 Cargo profile release，给本机自测，几秒出 Rust 产物。两档定义见 desktop/Cargo.toml，
# 产物分别在 desktop/target/release/bundle、desktop/target/dist/bundle
# 安装包内置 desktop/texlive（先跑 scripts/texlive-trim.sh 生成）。Linux 重打 AppImage 要 appimagetool
#（PATH 里，或 APPIMAGETOOL 指过去）
set -euo pipefail

HERE="$(cd "$(dirname "$0")/.." && pwd)"

# 工具链发现与 dev.sh 一致：SUBO_WS 指自包含工作区，否则用 PATH
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

[ -d "$HERE/desktop/node_modules" ] || (cd "$HERE/desktop" && npm install --no-audit --no-fund)

# beforeBuildCommand 会先构建前端
if [ "${SUBO_RELEASE:-}" = 1 ]; then PROFILE=dist; else PROFILE=release; fi

# Windows 安装包（NSIS），在 macOS / Linux 上交叉编：给 Windows 机器上试装用。
# 内置 TeX 用 desktop/texlive-windows（scripts/texlive-windows.sh 从 desktop/texlive 做出来），装到安装目录下的 texlive\
# 用 GNU 工具链，不用下载微软的 MSVC SDK；正式发版应在 Windows 上用 MSVC 打。
# 准备：rustup target add x86_64-pc-windows-gnu；brew install mingw-w64 nsis（Linux 装 mingw-w64、nsis）
if [ "${1:-}" = "windows" ]; then
  cd "$HERE/desktop"
  command -v x86_64-w64-mingw32-gcc >/dev/null || { echo "找不到 mingw-w64：brew install mingw-w64" >&2; exit 1; }
  command -v makensis >/dev/null || { echo "找不到 makensis：brew install nsis" >&2; exit 1; }
  # makensis 3.13 在非 UTF-8 的 locale 下写输出时会崩（bad_alloc）
  export LC_ALL=en_US.UTF-8 LANG=en_US.UTF-8
  LITE=0; OFFLINE=0
  for a in "${@:2}"; do
    case "$a" in
      lite) LITE=1 ;;
      offline) OFFLINE=1 ;;
      *) echo "不认识的参数：$a（可选 lite、offline）" >&2; exit 2 ;;
    esac
  done
  # 只给这一次构建改配置：tauri.conf.json 是各平台共用的，macOS、Linux 的包另有内置 TeX
  RES='"../app/dist/":"dist/"'
  if [ $LITE = 1 ]; then
    echo "== 轻量版：不带内置 TeX"
  elif [ -d texlive-windows ]; then
    RES="$RES,\"texlive-windows/\":\"texlive/\""
  else
    echo "提示：没有 desktop/texlive-windows，安装包不含 TeX。先运行 scripts/texlive-windows.sh" >&2
  fi
  # 标准包缺 WebView2 时联网下载（tauri.conf.json 里的 downloadBootstrapper）；离线包把离线安装器打进去，
  # 给内网、断网机器
  WEBVIEW='{"type":"downloadBootstrapper"}'
  [ $OFFLINE = 1 ] && WEBVIEW='{"type":"offlineInstaller","silent":true}'
  CONF="{\"bundle\":{\"resources\":{$RES},\"windows\":{\"webviewInstallMode\":$WEBVIEW}}}"
  OUT="target/x86_64-pc-windows-gnu/$PROFILE/bundle/nsis"
  rm -f "$OUT"/*_x64-setup.exe
  npx tauri build --target x86_64-pc-windows-gnu --bundles nsis --config "$CONF" -- --profile "$PROFILE"
  # 几种包产物同名，打完立刻按种类改名：-tex / -lite，离线包再带 -offline
  SUFFIX=-tex; [ $LITE = 1 ] && SUFFIX=-lite
  [ $OFFLINE = 1 ] && SUFFIX="$SUFFIX-offline"
  for f in "$OUT"/*_x64-setup.exe; do mv "$f" "${f%.exe}$SUFFIX.exe"; done
  # 应用内更新在 Windows 上直接运行安装程序，给它配上更新签名（.sig）
  UPKEY="${SUBO_UPDATER_KEY:-$HOME/.tauri/subo-updater.key}"
  if [ -f "$UPKEY" ]; then
    VERSION="$(sed -n 's/^  "version": "\(.*\)",$/\1/p' tauri.conf.json)"
    npx tauri signer sign -f "$UPKEY" -p "${SUBO_UPDATER_KEY_PASSWORD:-}" --app-version "$VERSION" "$OUT/Subo_${VERSION}_x64-setup$SUFFIX.exe" >/dev/null
  fi
  ls -la "$OUT/"
  exit 0
fi

if [ "${1:-}" = "bundle" ] && [ "$(uname -s)" = Darwin ]; then
  cd "$HERE/desktop"

  # macOS：内置 TeX Live 不交给 Tauri 复制（它会不会保留符号链接没有保证，xelatex → xetex、
  # tlmgr → tlmgr.pl 都是链接），先出 .app，再用 ditto 拷进去、重签、自己做 dmg
  # 默认本机架构；bundle x64 交叉编 Intel 版，产物在 target/x86_64-apple-darwin/ 下，内置 TeX 换成 x86_64 的那份
  case "${2:-}" in
    "") TARGET=""; TL=texlive; BUILD="target/$PROFILE" ;;
    x64)
      TARGET=x86_64-apple-darwin; TL=texlive-x64; BUILD="target/$TARGET/$PROFILE"
      rustup target list --installed 2>/dev/null | grep -qx "$TARGET" || {
        echo "没装 Intel 的 Rust 目标：rustup target add $TARGET" >&2; exit 1; }
      [ -d "$TL" ] || {
        echo "没有 desktop/$TL。先运行 SUBO_ARCH=x86_64 scripts/texlive-trim.sh <TeX Live 根目录> desktop/$TL" >&2; exit 1; } ;;
    *) echo "不认识的参数：$2（macOS 上可选 x64）" >&2; exit 2 ;;
  esac
  npx tauri build --bundles app ${TARGET:+--target "$TARGET"} -- --profile "$PROFILE"
  APP="$BUILD/bundle/macos/Subo.app"
  if [ -d "$TL" ]; then
    echo "== 内置 TeX Live（desktop/$TL）"
    ditto "$TL" "$APP/Contents/Resources/texlive"
    # 本机用 desktop/texlive 跑过 tlmgr（bundled-smoke.sh 补装宏包）会留下软件源 tlpdb 缓存（约 20 MB），不进包
    rm -f "$APP"/Contents/Resources/texlive/tlpkg/texlive.tlpdb.main.*
  else
    echo "提示：没有 desktop/texlive，安装包不含 TeX。先运行 scripts/texlive-trim.sh <TeX Live 根目录>" >&2
  fi
  # 拷入资源后原签名失效，要重签。没设 SUBO_SIGN_IDENTITY 就 ad-hoc 签，只给本机自测；
  # 设了就用 Developer ID 签，再设 SUBO_NOTARY_PROFILE 就公证并钉票
  ID="${SUBO_SIGN_IDENTITY:-}"
  NOTARY="${SUBO_NOTARY_PROFILE:-}"
  [ -n "$NOTARY" ] && [ -z "$ID" ] && { echo "设了 SUBO_NOTARY_PROFILE 但没设 SUBO_SIGN_IDENTITY：公证要先用 Developer ID 签名" >&2; exit 2; }
  if [ -z "$ID" ]; then
    codesign --force --deep --sign - "$APP"
  else
    # 可以填证书名或 SHA-1 指纹。同名证书有几张（重新生成过）时 codesign 按名字会报 ambiguous，
    # 统一换成指纹，取第一张：同一团队的 Developer ID 证书签出来效果一样
    HASH="$(security find-identity -v -p codesigning | grep -F "$ID" | awk '{print $2}' | head -1)"
    [ -n "$HASH" ] || {
      echo "钥匙串里找不到签名证书：$ID" >&2
      echo "用 security find-identity -v -p codesigning 看有哪些，填引号里的整串名字（含括号里的团队 ID）或前面的指纹" >&2
      exit 1
    }
    echo "== Developer ID 签名：$ID（证书 $HASH）"
    ID="$HASH"
    # 公证要求包里每个 Mach-O 都带 hardened runtime 和时间戳，--deep 不管资源目录里的程序，逐个签，
    # 由里往外，最后签 .app 本身
    SIGN=(codesign --force --timestamp --options runtime --sign "$ID")
    # 只拿可执行文件和动态库给 file 认（texlive 几千个文件全过一遍要半分钟）
    find "$APP/Contents/Resources" -type f \( -perm -u+x -o -name '*.dylib' -o -name '*.so' -o -name '*.bundle' \) -print0 \
      | xargs -0 file --no-pad | grep 'Mach-O' | cut -d: -f1 \
      | while IFS= read -r bin; do
          if [ "$(basename "$bin")" = biber ]; then
            "${SIGN[@]}" --entitlements macos/biber.entitlements "$bin"
          else
            "${SIGN[@]}" "$bin"
          fi
        done
    "${SIGN[@]}" "$APP"
    codesign --verify --strict --deep "$APP"
    # 库校验只在正式签名下生效：用签好的 biber 实跑一次，换个空缓存目录逼它重新解包、加载动态库
    BIBER="$APP/Contents/Resources/texlive/bin/universal-darwin/biber"
    if [ -x "$BIBER" ]; then
      PARTMP="$(mktemp -d)"
      PAR_GLOBAL_TEMP="$PARTMP" "$BIBER" --version >/dev/null || { rm -rf "$PARTMP"; echo "签名后的 biber 跑不起来，检查 macos/biber.entitlements" >&2; exit 1; }
      rm -rf "$PARTMP"
    fi
  fi

  # 交给苹果公证，通过后把票据钉进文件，用户断网也能过 Gatekeeper。参数：要提交的文件、要钉票的文件
  notarize() {
    echo "== 公证 $(basename "$2")（上传加排队，一般几分钟，账号头一次可能要一两个小时）"
    local sid status tries=0
    # 先只上传拿到提交 ID，再单独等：--wait 等的途中网络一断（VPN、代理切换）命令就失败，
    # 苹果那边其实还在审，从头重来又要重新上传
    sid="$(xcrun notarytool submit "$1" --keychain-profile "$NOTARY" --output-format json \
      | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')"
    [ -n "$sid" ] || { echo "上传公证失败，检查网络后重跑" >&2; exit 1; }
    echo "提交 ID：$sid"
    until xcrun notarytool wait "$sid" --keychain-profile "$NOTARY" >/dev/null; do
      tries=$((tries + 1))
      [ $tries -ge 20 ] && { echo "一直等不到公证结果。手动查：xcrun notarytool info $sid --keychain-profile $NOTARY" >&2; exit 1; }
      echo "等公证结果时网络出错，30 秒后接着等（第 $tries 次）" >&2
      sleep 30
    done
    status="$(xcrun notarytool info "$sid" --keychain-profile "$NOTARY" --output-format json \
      | sed -n 's/.*"status":"\([^"]*\)".*/\1/p')"
    if [ "$status" != Accepted ]; then
      echo "公证没通过（$status）。看原因：xcrun notarytool log $sid --keychain-profile $NOTARY" >&2
      exit 1
    fi
    # 刚通过时票据可能还没同步到苹果的票据服务器，钉票报 Record not found（Error 65），等一会儿再钉
    tries=0
    until xcrun stapler staple "$2"; do
      tries=$((tries + 1))
      [ $tries -ge 10 ] && { echo "公证已通过但一直钉不上票。稍后手动：xcrun stapler staple $2" >&2; exit 1; }
      echo "票据还没同步过来，30 秒后重钉（第 $tries 次）" >&2
      sleep 30
    done
  }
  if [ -n "$NOTARY" ]; then
    # .app 自己也钉票：用户把它拖进「应用程序」后就和 dmg 分开了，首次打开时 Gatekeeper 查的是 .app 的票
    ZIP="$(mktemp -d)/Subo.zip"
    ditto -c -k --keepParent "$APP" "$ZIP"
    notarize "$ZIP" "$APP"
    rm -rf "$(dirname "$ZIP")"
  fi

  VERSION="$(sed -n 's/^  "version": "\(.*\)",$/\1/p' tauri.conf.json)"
  if [ -n "$TARGET" ]; then ARCH=x64; else case "$(uname -m)" in arm64) ARCH=aarch64 ;; *) ARCH=x64 ;; esac; fi

  # 应用内更新用的包：公证、钉票后的 .app 打成 tar.gz，再用更新签名私钥签（与苹果证书不同）。
  # 签名绑上版本号，客户端据此拒收拿旧包冒充新版本的清单。
  # 只给公证过的包做；没有私钥就跳过，dmg 照出
  UPKEY="${SUBO_UPDATER_KEY:-$HOME/.tauri/subo-updater.key}"
  if [ -n "$NOTARY" ] && [ -f "$UPKEY" ]; then
    echo "== 应用内更新包"
    UPD="$BUILD/bundle/updater"
    rm -rf "$UPD" && mkdir -p "$UPD"
    TGZ="$UPD/Subo_${VERSION}_${ARCH}.app.tar.gz"
    # COPYFILE_DISABLE：别把扩展属性打成 ._ 文件，解出来会弄坏签名
    COPYFILE_DISABLE=1 tar -czf "$TGZ" -C "$(dirname "$APP")" Subo.app
    npx tauri signer sign -f "$UPKEY" -p "${SUBO_UPDATER_KEY_PASSWORD:-}" --app-version "$VERSION" "$TGZ" >/dev/null
  elif [ -n "$NOTARY" ]; then
    echo "提示：没有更新签名私钥（$UPKEY），这次不出应用内更新包" >&2
  fi

  echo "== 制作 dmg"
  OUT="$BUILD/bundle/dmg"
  STAGE="$(mktemp -d)"
  RW="$(mktemp -d)"
  MNT=""
  # .app 挪进暂存目录而不是再拷一份（600MB 左右），做完挪回；trap 保证中途失败也挪回
  trap '[ -n "$MNT" ] && hdiutil detach -quiet -force "$MNT"; [ -d "$STAGE/Subo.app" ] && mv "$STAGE/Subo.app" "$APP"; rm -rf "$STAGE" "$RW"' EXIT
  mv "$APP" "$STAGE/Subo.app"
  ln -s /Applications "$STAGE/Applications"
  # 卷图标：挂载后桌面和侧边栏显示的图标。要卷根目录带「自定义图标」标记，
  # 而 -srcfolder 不带根目录的 FinderInfo，只能先做可写镜像、挂上去打标记，再转成压缩格式
  cp icons/icon.icns "$STAGE/.VolumeIcon.icns"
  rm -rf "$OUT" && mkdir -p "$OUT"
  hdiutil create -quiet -volname Subo -srcfolder "$STAGE" -ov -format UDRW "$RW/rw.dmg"
  mv "$STAGE/Subo.app" "$APP"
  MNT="$(hdiutil attach -nobrowse -noautoopen "$RW/rw.dmg" | awk -F'\t' '/\/Volumes\//{print $NF}')"
  [ -d "$MNT" ] || { echo "挂载临时 dmg 失败，没法设卷图标" >&2; exit 1; }
  # FinderInfo 第 9 字节 0x04 即 kHasCustomIcon（同 SetFile -a C，但不依赖 Xcode 命令行工具）
  xattr -wx com.apple.FinderInfo 0000000000000000040000000000000000000000000000000000000000000000 "$MNT"
  hdiutil detach -quiet "$MNT"
  MNT=""
  # ULFO（LZFSE）：本机实测 18s / 251MB；ULMO（LZMA）123s / 232MB，只小 4% 却慢 6 倍，挂载也更慢。
  # ULFO 要 macOS 10.11，本应用最低要求 11
  DMG="$OUT/Subo_${VERSION}_${ARCH}.dmg"
  hdiutil convert -quiet "$RW/rw.dmg" -format ULFO -o "$DMG"
  rm -rf "$RW"
  if [ -n "$ID" ]; then
    codesign --force --timestamp --sign "$ID" "$DMG"
  fi
  if [ -n "$NOTARY" ]; then
    notarize "$DMG" "$DMG"
    # 按用户从网上下载后双击的情形检查一遍
    spctl --assess --type open --context context:primary-signature -v "$DMG"
    spctl --assess --type execute -v "$APP"
  fi
  # dmg 文件本身的图标（Finder 里看到的那个），存在资源分叉里，签名、钉票之后再贴。
  # 只在本机、AirDrop、U 盘拷贝时保得住；经网页下载会被丢掉，回到系统默认的磁盘映像图标
  osascript -l JavaScript - "$PWD/icons/icon.icns" "$PWD/$DMG" >/dev/null <<'JS' || echo "提示：没能给 dmg 文件贴图标，不影响安装" >&2
ObjC.import("AppKit")
function run(argv) {
  return $.NSWorkspace.sharedWorkspace.setIconForFileOptions(
    $.NSImage.alloc.initWithContentsOfFile(argv[0]), argv[1], 0)
}
JS
  # .app 留在 bundle/macos 下会被 Spotlight 收录，启动台里就和 /Applications 里装的那个并排出现两个素帛。
  # 挪进 .noindex 结尾的目录（Spotlight 不收录这类目录），本机自测仍可从这里直接打开
  KEEP="$BUILD/bundle/macos.noindex"
  rm -rf "$KEEP" && mkdir -p "$KEEP" && mv "$APP" "$KEEP/" && APP="$KEEP/Subo.app"
  du -sh "$APP" "$OUT"/*.dmg ${UPD:+"$UPD"/*.tar.gz}
  exit 0
fi

if [ "${1:-}" = "bundle" ]; then
  cd "$HERE/desktop"
  # 内置 TeX Live 同样不交给 Tauri 的 resources：它逐个复制文件，符号链接会被展开
  #（tlmgr → tlmgr.pl 展开后找不到自己的 tlpkg/）。先出 deb / AppImage，再拆包拷进
  # usr/lib/subo/texlive 重新打。程序在 usr/bin/subo，platform.rs 按相对位置找。
  npx tauri build --bundles deb,appimage -- --profile "$PROFILE"
  OUT="target/$PROFILE/bundle"
  if [ ! -d texlive ]; then
    echo "提示：没有 desktop/texlive，安装包不含 TeX。先运行 scripts/texlive-trim.sh <TeX Live 根目录>" >&2
    exit 0
  fi
  TL="$PWD/texlive"
  # 包内只读：目录 755、文件 644，可执行的保持可执行
  put_texlive() {
    mkdir -p "$1/usr/lib/subo"
    cp -a "$TL" "$1/usr/lib/subo/texlive"
    rm -f "$1"/usr/lib/subo/texlive/tlpkg/texlive.tlpdb.main.*   # 同 macOS：tlmgr 留下的软件源缓存
    chmod -R u+rwX,go+rX,go-w "$1/usr/lib/subo/texlive"
  }
  STAGE="$(mktemp -d)"
  trap 'rm -rf "$STAGE"' EXIT

  for deb in "$OUT"/deb/*.deb; do
    echo "== 内置 TeX Live → $deb"
    rm -rf "$STAGE/deb"
    dpkg-deb -R "$deb" "$STAGE/deb"
    put_texlive "$STAGE/deb"
    # md5sums 与 Installed-Size 按新内容重算
    (cd "$STAGE/deb" && find . -path ./DEBIAN -prune -o -type f -printf '%P\0' | sort -z | xargs -0 md5sum) \
      > "$STAGE/deb/DEBIAN/md5sums"
    size="$(du -sk --exclude=DEBIAN "$STAGE/deb" | cut -f1)"
    sed -i "s/^Installed-Size: .*/Installed-Size: $size/" "$STAGE/deb/DEBIAN/control"
    dpkg-deb --root-owner-group -Zxz --build "$STAGE/deb" "$deb" >/dev/null
  done

  TOOL="${APPIMAGETOOL:-$(command -v appimagetool || true)}"
  for img in "$OUT"/appimage/*.AppImage; do
    [ -e "$img" ] || continue
    [ -n "$TOOL" ] || { echo "找不到 appimagetool，无法把 TeX Live 放进 $img（设 APPIMAGETOOL）" >&2; exit 1; }
    echo "== 内置 TeX Live → $img"
    rm -rf "$STAGE/squashfs-root"
    # 不挂 FUSE，直接解开；CI 容器里没有 FUSE
    (cd "$STAGE" && APPIMAGE_EXTRACT_AND_RUN=1 "$OLDPWD/$img" --appimage-extract >/dev/null)
    put_texlive "$STAGE/squashfs-root"
    case "$(uname -m)" in aarch64) A=aarch64 ;; *) A=x86_64 ;; esac
    APPIMAGE_EXTRACT_AND_RUN=1 ARCH="$A" "$TOOL" --no-appstream "$STAGE/squashfs-root" "$img" >/dev/null
  done
  du -sh "$OUT"/deb/*.deb "$OUT"/appimage/*.AppImage 2>/dev/null || true
  exit 0
fi

echo "== 构建前端"
(cd "$HERE/app" && npm run build --silent)

echo "== 构建并启动桌面壳"
cd "$HERE/desktop" && exec cargo run
