# 命令行与本地接口

在终端里驱动素帛，有两层：

1. **命令行**（首选）：`subo <命令>`，一次性跑完，不起服务、不占端口。编译、补装宏包、检查环境、
   新建项目这些常用的事都在这里。给人用，也按 agent 的需要设计：输入输出简单，出错说清楚。
2. **本地 HTTP 接口**：`subo-server` 起服务后的 `/api/*`，界面用的就是它。要和打开着的界面
   同时操作、要实时事件时才用（见第 3 节）。

> 以代码为准：命令行在 `server/src/cli.rs`，接口在 `server/src/lib.rs` 的路由表。对不上时改本文。

---

## 1. 命令行

### 程序在哪

| 场景 | 命令 | 用的 TeX |
| --- | --- | --- |
| 装了素帛（macOS） | `/Applications/Subo.app/Contents/MacOS/subo <命令>` | 本机有完整 TeX Live（xelatex 和 pdflatex 都在）就用它，否则用内置的；同桌面端 |
| 装了素帛（Linux） | `subo <命令>` | 同上 |
| 开发（本仓库） | `server/target/debug/subo-server <命令>` | PATH 里的 TeX；加 `--bundled-tex` 按内置 TeX 的方式补装（PATH、`TEXMFHOME` 由调用方配好） |

两个程序的命令完全相同（同一份 `cli.rs`），下文写作 `subo`。`subo-server` 不带命令时照旧启动服务（第 2 节）。

### 命令

| 命令 | 做什么 |
| --- | --- |
| `subo build [目录或入口.tex]` | 编译。成功时 PDF 写在项目里、与入口同名（`main.tex` → `main.pdf`） |
| `subo build … --install` | 缺的宏包能查到就自动装上再重编，最多 3 轮；项目要的引擎没装（如日文文档的 `lualatex`、`uplatex`）时先装引擎，装好的引擎写在 `installedEngine` |
| `subo install <宏包或文件名>…` | 装宏包；可以直接给报错里缺的文件名（`bytefield.sty`），会先查在哪个包里 |
| `subo install --engine <引擎或程序名>` | 装按需引擎（`luahbtex`、`ptex`，也可以写 `lualatex`、`platex`）；只用于内置 TeX |
| `subo doctor [--net]` | 检查环境：用哪个 TeX、缺哪些程序、装宏包要不要管理员权限、有没有 Ghostscript、用哪个下载源；`--net` 再测下载源连不连得上 |
| `subo inspect [目录]` | 不编译，只看入口是哪个、会用哪个引擎及原因、有哪些 `.tex` 文件 |
| `subo mirror` | 看当前下载源和所有可选的（预置镜像、`tlmgr` 自己的、自定义的） |
| `subo mirror probe [--custom <地址>]` | 并发测速（要联网），按下载速度排，快的在前 |
| `subo mirror use <id\|地址\|tlmgr>` | 换下载源，存进设置（和界面共用）；`--fastest` 测速后换成最快的 |
| `subo templates` | 列出模板，每行 `id：名称（来源，入口）` |
| `subo new <目录> [--template <id>]` | 新建项目（目录须不存在或为空）；不给模板就是空白中文文档 |
| `subo help [命令]` | 看说明；`subo <命令> --help` 同 |

`build` 的选项：`--entry <文件>`、`--engine <auto|pdflatex|xelatex|lualatex|platex|uplatex>`、
`--out <文件>`、`--no-pdf`（只看能不能编过）、`--timeout <秒>`、`--mirror <源>`（这次装宏包用哪个下载源，不改设置）、
`--fresh`（丢掉上次的中间产物从头编）、`--tex <auto|own|bundled>`（这次用哪套 TeX，不改设置：`auto` 是本机 TeX 的
LaTeX 内核不早于 2023-06-01 就用它，否则用素帛内置的；只有装了素帛桌面版的 `subo` 才有内置 TeX）。`install`、`doctor` 也认 `--mirror`。

中间产物按项目、按引擎留在素帛目录里（见「读写的位置」），下次接着编：`.aux` 和文献工具的输入都没变时只跑一遍；
项目里的 `.bib`/`.bst` 改了会重跑 biber/bibtex。没编过的那次的中间产物会丢掉，不带到下次。
同一个项目同时只编一个，另一个 `build` 会等它结束。

引擎 `auto` 的选法与界面一致：`--engine` > 在素帛里给这个项目选过的 > 源码线索
（`% !TEX program = …`、`latexmkrc`、arXiv 的 `00README`、导言区的 `fontspec`/`ctex` 等）> pdflatex。
`subo new` 会在入口开头写上 `% !TEX program = xelatex`（素帛的模板多按 xelatex 写；日文模板 `paper-ja` 自带 `lualatex`）。

### 输出

- **默认是给人读的文本**，格式固定：第一行 `成功：…` 或 `失败：<结论>`，后面是 `键：值`，列表缩进两格，空的项不打。
- **`--json`** 输出**一个平铺的 JSON 对象**：值只有字符串、数字、布尔、字符串数组，没有嵌套对象、没有对象数组。
  `ok` 总在第一个；列表字段总是存在（空就是 `[]`），不用判断有没有。
- stdout 只有结果；进度（`== 编译 …`、`第 2 遍`）走 stderr，`--json` 时 stderr 也不出声
  （各模块的诊断输出改写进 `$SUBO_HOME/cli/stderr.log`）。
- `hint` 是一句话的修法，只在有话说时出现；`next` 只放可以直接跑的命令，没有就是 `[]`。
- 不交互、不弹窗。要管理员授权的安装不会弹框，直接失败，`hint` 里说明要用户跑的 `sudo` 命令（不放进 `next`：agent 跑不了）。

**退出码**

| 码 | 含义 | 该做什么 |
| --- | --- | --- |
| 0 | 成功 | |
| 1 | 文档没编过，改源码或装包能修 | 看 `error`、`errorFile`/`errorLine`、`errors`、`missing`，照 `next` 做 |
| 2 | 用法或输入不对（选项拼错、路径不存在、模板不存在、下载源写错……） | 看 `message` 和 `hint` 改命令 |
| 3 | 环境问题（没有 TeX、装不了包、离线、要管理员权限、下载源都不通） | 看 `hint`；`next` 里有命令就先跑（多是 `subo mirror probe`），否则多半要用户处理 |

`build --install` 自动安装因离线、要管理员权限等没成时，退出码是 3 而不是 1：改源码修不好。
本机 TeX 的 LaTeX 内核比项目用到的宏包旧（日志里的 `expl3 too old`、`You have requested release …`）也是 3，
`hint` 说怎么修：有内置 TeX 时 `next` 是加 `--tex bundled` 重编；MiKTeX 是 `miktex packages update`。

**命令本身出错时**（退出码 2、3）：

```text
错误：build 不认识选项 --foo
怎么办：build 能用的选项：--entry <值>  --engine <值>  --out <值>  --timeout <值>  --install  --no-pdf
```

```json
{"ok":false,"error":"project.notFound","message":"找不到项目目录 /work/nope","hint":"检查路径；要新建就用 subo new nope","next":[]}
{"ok":false,"error":"case.offline","message":"装 bytefield 失败：连不上下载源 tuna（https://…）","hint":"联网后重试；连得上但慢或连不上，就测速换一个下载源","next":["subo mirror probe"]}
```

`error` 是稳定的错误码，可以据此分支；`message` 说清哪里坏了、为什么（带上完整路径、收到的值）；
`hint` 说怎么修；`next` 是可以直接跑的命令（总是存在，可能为空）。

### `build` 的结果

成功：

```text
成功：main.tex → /work/paper/main.pdf（xelatex，3 页，3539 ms）
```

编过了但内容有问题时，成功那行下面接着列出来（PDF 里会是 `??`、`[?]`、豆腐块或出了版心）：

```text
成功：main.tex → /work/paper/main.pdf（pdflatex，1 页，830 ms）
未定义的引用：
  fig:arch
未定义的文献：
  knuth84
缺字：
  字 (U+5B57)，字体 cmr10
排版溢出：
  第 12 行附近有一行超出版心 45pt
```

失败（缺宏包）：

```text
失败：缺宏包：bytefield（bytefield.sty）
怎么办：装上缺的宏包再重编
入口：main.tex
引擎：pdflatex
错误：
  LaTeX Error: File `bytefield.sty' not found.
缺：
  bytefield.sty：在宏包 bytefield 里，可以装
日志：/Users/you/.subo/cli/28b04d1cce101db3/main.log
下一步：
  subo build /work/paper --install
  subo install bytefield
  subo build /work/paper
```

失败（源码错误）——`errors` 带上 TeX 停下处的源码（`⟨here⟩` 是它停下的位置，停在行尾时不标），一眼看出是哪个命令；
`hint` 按错误类别说该去哪找（报错的位置常常不是原因所在），并给出诊断材料的路径：

```text
失败：源码有错：main.tex:3: Unicode character 汉 (U+6C49) not set up for use with LaTeX.（出错处 Plain text with 汉⟨here⟩字 in it.）
```

诊断材料（`diagnosis` 指向的 `<入口名>.diagnosis.md`，和日志放在一起）就是素帛里 AI 修复拿到的那一份：
错误、类别和它通常意味着什么、TeX 停下处、上次编过以来改了哪里、按类别挑出的带行号的源码片段。
agent 读这一个文件就够改了，不必自己翻日志。

`--json` 的字段（顺序固定；标「仅失败」的只在 `ok:false` 时出现，其余总在）：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `ok` | 布尔 | 编过没有 |
| `error` | 字符串 | 仅失败：一句话结论，按「超时/超内存 > 缺宏包 > 缺文件 > 源码错误」挑最能说明原因的 |
| `hint` | 字符串 | 仅失败：一句话说怎么修 |
| `errorFile`, `errorLine` | 字符串或 null、数字或 null | 仅失败：第一处错误在项目里的哪个文件（项目内相对路径）、哪一行；出在 TeX Live 自己的文件里时为 null |
| `errorKind` | 字符串或 null | 仅失败：第一处错误的类别，决定该去哪找原因：`here`（就在停下处）、`scan`（参数没闭合，错在参数开头）、`env`（环境不配对）、`output`（页眉页脚、浮动体，报错行只是分页处）、`end`（停在 `\end{document}`，原因在前面）、`package`（宏包报的）、`aux`（辅助文件里的）、`driver`（转 PDF 失败） |
| `diagnosis` | 字符串或 null | 仅失败：诊断材料的路径，见上 |
| `entry`, `engine`, `ms` | | 入口、实际用的程序、耗时 |
| `pages` | 数字或 null | PDF 页数；没编过为 null |
| `pdf` | 字符串或 null | 写出的 PDF 的绝对路径 |
| `log` | 字符串 | 完整日志的路径（在素帛目录里，不写进项目） |
| `errors` | 字符串数组 | 日志里的错误，`文件:行: 消息（出错处 …）`，去掉了「Emergency stop」之类的套话，最多 10 条（编译遇错即停，通常只有一条） |
| `packages` | 字符串数组 | 查到了、可以直接装的宏包名 |
| `missing` | 字符串数组 | 缺的文件、字体、程序，一项一句话：缺什么、为什么、怎么办 |
| `installed` | 字符串数组 | 这次 `--install` 装上的包 |
| `installedEngine` | 字符串或 null | 这次 `--install` 先装上的引擎（`luahbtex`、`ptex`），没装就是 null |
| `undefinedRefs` | 字符串数组 | 编过了但没定义的 `\ref` 标签名（最后一遍里的） |
| `undefinedCites` | 字符串数组 | 编过了但没定义的 `\cite` 键 |
| `missingChars` | 字符串数组 | 字体里没有的字，`字 (U+5B57)，字体 cmr10`：中文用了 pdflatex 或没设中文字体时常见 |
| `overfull` | 字符串数组 | 明显的溢出：超出版心 30pt 以上的行、比版心高的页面内容、比页面大的浮动体 |
| `warnings` | 字符串数组 | 其他提醒：项目要的引擎没装先用别的试编、已知的旧样式不兼容、biber/bibtex 报的错 |
| `next` | 字符串数组 | 下一步，只放可以直接跑的命令；成功时为空 |

`undefinedRefs` 等四项最多各 20 条，只在编过时有内容；它们不影响 `ok` 和退出码，但 agent 交付前应当清空。

### 典型流程

agent 写好文档后：

```bash
subo build ./paper --install --json
```

- 退出码 0：PDF 路径在 `pdf`。再看 `undefinedRefs`、`undefinedCites`、`missingChars`、`overfull`，
  有内容就改源码再编，都空了才算交付。
- 退出码 1：
  - `missing` 里是「TeX Live 里没有这个文件」→ 多半是拼错了包名，或项目该自带的文件没放进来；
  - 否则读 `diagnosis` 指向的诊断材料（或按 `errorKind`、`errorFile`/`errorLine` 和 `errors`）改源码，再跑一次同样的命令。
- 退出码 3：环境问题。`next` 里有 `subo mirror probe` 就先换个快的、连得上的源再重编；否则先跑 `subo doctor`，
  `hint` 里要用户做的（`sudo`、装 TeX）转告用户。

下载源慢或连不上时：

```bash
subo mirror use --fastest --json         # 测速并换成最快的（存进设置，界面也跟着用）
subo build ./paper --install --mirror tuna   # 或者只这一次用某个源
```

其他：

```bash
subo doctor                              # 先确认环境
subo templates                           # 看有哪些模板
subo new ./paper --template article-zh   # 从模板新建
subo inspect ./paper                     # 确认入口和引擎
subo install bytefield.sty               # 单独装一个（文件名、包名都行）
```

### 下载源 `mirror`

补装宏包、按需引擎从下载源取。可选的有：预置镜像（`ctan` 是 CTAN 官方按地理位置调度，另有
`tuna`、`ustc`、`aliyun`、`huawei`、`bfsu`、`sjtu`、`nju` 几家国内镜像）、`tlmgr`（跟随 TeX Live 自己的设置）、
自定义地址（http(s) 网址，或本机含 `tlpkg/texlive.tlpdb` 的 tlnet 目录，离线安装源）。

`subo mirror probe --json`：

```json
{"ok":true,"current":"tlmgr","fastest":"tuna","fastestUrl":"https://mirrors.tuna.tsinghua.edu.cn/CTAN/systems/texlive/tlnet","fastestKBps":5120,"fastestMs":42,
 "results":["tuna：5120 KB/s，首字节 42 ms，https://…","ustc：3800 KB/s，首字节 51 ms，https://…","ctan：不通，https://mirror.ctan.org/…"],
 "next":["subo mirror use tuna"]}
```

- 每个源下载 `texlive.tlpdb` 开头的 512 KB，最多等 8 秒，按下载速度排（不只看延迟：连得快、下得慢的源装大包时更慢）；
  本机目录排最前，不通的沉底。当前用的那个在 `results` 里标「（当前）」。
- 一个都不通：`mirror.allDown`，退出码 3。
- `mirror use` 只改下载源（`settings.json` 的 `mirror`、`customMirror`），别的设置原样留着；素帛开着也没关系，
  服务下次取设置时发现文件变了会重读。`--fastest` 返回字段多了 `kbps`、`ms`。
- 写错的源：`settings.badMirror`（不认识的 id）、`settings.badCustomMirror`（地址格式不对）、
  `settings.localMirrorInvalid`（本机目录里没有 `tlpkg/texlive.tlpdb`），退出码 2。

`subo mirror --json` 的字段：`ok`、`mirror`（当前 id）、`url`（实际地址）、`mirrors`（每行 `id：地址`）。
`subo doctor --json` 里有 `mirror`、`mirrorUrl`、`mirrorReachable`、`mirrorMs`；后两个只有加 `--net` 才测，否则为 null
（素帛是离线软件，不默认联网）。

### 读写的位置

- PDF：项目里、与入口同名；`--out` 改到别处。项目里不会多出其他文件（中间产物都在素帛目录）。
- 中间产物与日志：`$SUBO_HOME/cli/<项目路径的哈希>/`（默认 `~/.subo/cli/…`）：日志 `<入口>.log`，
  中间产物在按引擎分的子目录（`pdflatex/`、`xelatex/`……），`.lock` 管同一项目的并发编译。
- 补装的宏包：本机 TeX Live 的目录树；内置 TeX 时是用户层 `~/.subo/texlive/<年份>/`。
- 读用户在素帛里的设置（下载源、超时、内存上限）；只有 `subo mirror use` 会改，且只改下载源。

---

## 2. 本地服务 `subo-server`

```bash
cd server && cargo build          # 产物 server/target/debug/subo-server
server/target/debug/subo-server --help
```

改了服务端代码后要重新 `cargo build` 并重启服务，否则跑的还是旧二进制。

| 参数 | 默认 | 作用 |
| --- | --- | --- |
| `--port <n>` | `8787` | 监听 `127.0.0.1:<n>`。`0` 表示随机空闲端口，实际地址看启动输出的 `地址: http://127.0.0.1:<端口>` 一行 |
| `--home <dir>` | 见下 | 项目库目录。给了就**锁定**：设置里不能再改库位置（`settings.libraryLocked`） |
| `--dist <dir>` | 无 | 托管前端构建产物（`app/dist`），单端口同时提供界面和 `/api` |
| `--engine <e>` | `auto` | `auto` / `xelatex` / `tectonic` / `mock`。`auto`：PATH 有 xelatex 用它，否则 tectonic，再否则 mock |
| `--timeout <秒>` | `120` | 单次编译超时（设置里的 `timeoutSecs` 优先） |
| `--bundled-tex` | 关 | 按桌面端内置 TeX Live 的方式行事：补装宏包走 `tlmgr --usermode` 装进用户层；PATH、`TEXMFHOME` 等由调用方配好 |
| `--bundled-root <dir>` | 无 | 同桌面端选 TeX：`<dir>` 是内置 TeX Live 的根目录，PATH 里的 TeX 缺引擎或太旧时用它，设置里能换；`none` 表示没有内置版 |
| `-h`, `--help` | | 打印用法后退出 |

参数解析是严格的：未知参数、漏写值、端口/超时不是数字、`--engine` 取值不对、`--dist` 下没有 `index.html`、
端口绑不上，都会在 stderr 打一行原因加用法，**退出码 2**，不会带着默认值悄悄起来。

启动成功时 stdout 依次打印 `项目库:`、`引擎:`、`地址:` 几行；脚本可以等到出现 `地址:` 再发请求，
或者轮询 `GET /api/health`。

**`mock` 引擎**不需要 TeX：生成一份占位 PDF，用来验证整条链路（建项目 → 编译 → 事件 → 取 PDF）。

### 环境变量

| 变量 | 作用 |
| --- | --- |
| `SUBO_HOME` | Subo 自己的配置与缓存目录（默认 `~/.subo`）。设了它，默认项目库也变成 `$SUBO_HOME/quires`。**测试务必设成临时目录**，否则会读写用户真实的 `~/.subo` |
| `SUBO_TL_ARCHIVE` | 下载 TeX Live 历史版本的地址模板，`{year}` 换成年份（内网镜像、测试用） |
| `SUBO_DIST` | 仅桌面壳：前端产物目录 |
| `SUBO_TEX=bundled` | 仅桌面壳：本机装了 TeX 也强制用内置版 |
| `SUBO_WS` | 仅脚本：自包含工具链工作区（内含 `.texlive/`、`.toolchain/`） |

### 目录

- 项目库：优先级 `--home` > 设置里选的位置 > 默认 `~/Documents/Subo`（设了 `SUBO_HOME` 时为 `$SUBO_HOME/quires`）。
- `$SUBO_HOME`（`~/.subo`）下各文件与目录的用途见 [architecture.md §5 数据布局](architecture.md#5-数据布局)。

### 推荐的隔离启动方式（agent 自测）

```bash
S=$(mktemp -d)
SUBO_HOME=$S/subo server/target/debug/subo-server \
  --home $S/home --port 0 --engine mock > $S/server.log 2>&1 &
sleep 1; B=http://$(grep -o '127.0.0.1:[0-9]*' $S/server.log)
```

要连界面一起看，再加 `--dist app/dist`（先 `npm run build`）。

要用真实 TeX 就去掉 `--engine mock`，并把 TeX 的 `bin` 放进 PATH。自测实例别用 8787：那是
`scripts/dev.sh` 的固定端口，开发时可能正被占用。

---

## 3. 本地 HTTP 接口

界面用的接口。命令行能做的事优先用命令行；这里留作参考，以及要和打开着的界面同时操作时用。

基址 `http://127.0.0.1:<port>`。请求/响应体都是 JSON（另注明的除外），写操作带
`-H 'content-type: application/json'`。

### 约定

- **错误**统一为 `{ "code", "params", "message" }`，HTTP 状态码表示类别。`code` 是稳定错误码
  （对应前端 `server.<code>` 文案），`message` 是中文兜底。判断错误请看 `code`，不要匹配 `message`。
- 未知的 `/api/*` 返回 404 `api.missing`（不会落到前端的 index.html）。
- **项目 id**：项目库里的项目，id 就是目录名（如 `demo`）；登记进来的本地文件夹 id 以 `@` 开头
  （如 `@paper`）；模板预览以 `~` 开头，只读（写操作返回 403 `preview.readOnly`）。
  id 里有中文等字符时记得 URL 编码。
- 文件路径 `path` 一律是**项目内相对路径**；`..`、绝对路径、符号链接逃逸会被拒（`path.parent` 等）。
- 错误响应永远是这个 JSON，包括请求体本身不对的情况（缺字段、不是 JSON、没带
  `content-type`、路径参数类型不对）：`code` 为 `request.badBody`，原因在 `params.detail`。
- 编译默认是**异步**的：`POST build` 立刻返回，结果从 SSE `/api/events` 推送。
  脚本和 agent 用 **`POST build?wait=1`**，一直等到这次编译收尾、直接返回结果，不用碰事件流。
- 凡是要 `:job` 的地方都可以写 `latest`，指最近一次编译。

### 最小闭环（已在 mock 引擎和内置 TeX Live 上实测）

```bash
B=http://127.0.0.1:8811
curl -s $B/api/health
# {"ok":true,"engine":"mock","bundledTex":false,"home":"…/home"}

curl -s -XPOST $B/api/projects -H 'content-type: application/json' -d '{"name":"demo"}'
# 201 {"id":"demo"}

curl -s -XPUT $B/api/p/demo/file -H 'content-type: application/json' \
  -d '{"path":"main.tex","content":"\\documentclass{article}\\begin{document}Hi\\end{document}"}'
# {"ok":true,"path":"main.tex","mtime":1792566355118}

curl -s -XPOST "$B/api/p/demo/build?wait=1"
# {"jobId":"j1792…","entry":"main.tex","ok":true,"engine":"xelatex","ms":239,"pdf":true}

curl -s -o out.pdf $B/api/p/demo/pdf/latest      # application/pdf
curl -s $B/api/p/demo/log/latest                 # 纯文本，末尾 [ok=true|false]
curl -s -XPOST $B/api/p/demo/export/latest       # 复制到项目根：{"path":"main.pdf"}
```

编译失败时（HTTP 仍是 200，看 `ok`）：

```json
{"jobId":"j…","entry":"main.tex","ok":false,"engine":"xelatex","ms":59,
 "reason":"failed","error":"编译失败，详见日志","pdf":false,
 "errors":["main.tex:3: File `bytefield.sty' not found."],
 "issues":[{"id":"…","level":"error","kind":"here","message":"File `bytefield.sty' not found.","file":"main.tex","line":3,"here":"\\usepackage{bytefield}⟨here⟩","logRange":[41,52]}],
 "missing":[{"kind":"file","file":"bytefield.sty","lookup":"found","package":"bytefield"}]}
```

`build?wait=1` 返回的字段：

| 字段 | 说明 |
| --- | --- |
| `jobId`, `entry` | 这次编译的 id 和入口 |
| `ok`, `engine`, `ms` | 成败、实际用的程序、耗时 |
| `reason`, `error` | 仅失败：`failed`/`timeout`/`memory`/`fileSize`，及中文说明 |
| `errors` | 仅失败：日志里的错误，一条一句 `文件:行: 消息`（出在发行版文件里时没有位置）；全文看 `log/:job` |
| `issues` | 解析好的问题（错误、警告、排版提示），同 `job.issues` 事件里的 `report.issues`，字段见下文事件表 |
| `pdf` | 有没有可取的 PDF |
| `missing` | 缺字清单，同 `case.missing` 事件的 `files`（已查过包名的那一版） |
| `compat` | 认出的旧样式不兼容，交给 `POST compat` |
| `engineMissing` | 项目要的引擎没装，同 `engine.missing` 事件 |
| `texStale` | `{kernel, want}`：LaTeX 内核比宏包旧，同 `tex.stale` 事件 |

注意：
- 已有编译在跑时再 `build`，会并入「跑完再编一次」；`wait` 等的是请求之后开跑的那一次，结果对应最新的文件内容。
- 不带 `wait` 时返回 202 `{queued}`，要自己订阅 `/api/events`（**先订阅再 build**，SSE 不回放历史），见下文事件表。
- 每个项目只在**内存**里保留最近 5 次编译（`KEEP_JOBS`），重启服务后全部清空；`GET /api/p/:id/jobs` 看还剩哪些。

### 健康与引导

| 方法 路径 | 说明 |
| --- | --- |
| `GET /api/health` | `{ok, engine, bundledTex, home}` |
| `GET /api/about` | `{version, build, buildDate, os, arch, engine, bundledTex, texRoot, texVersion, home}`；`build` / `buildDate` 是构建时的 git 提交号与提交日期，没有 git 时为 null |
| `GET /api/onboarding` | `{kind: "fresh"\|"upgrade"\|null, version, previous}` |
| `POST /api/onboarding/done` | 记下已看过引导 |

### 项目与文件夹

| 方法 路径 | 请求 | 返回 / 说明 |
| --- | --- | --- |
| `GET /api/projects` | | 数组 `{id, name, path, entry, modified, folder, missing, archived}`，丢失的沉底，其余按修改时间 |
| `POST /api/projects` | `{name}` | 201 `{id}`；新建在项目库里 |
| `POST /api/projects/import?name=<名>` | 请求体为 zip / tar.gz 原始字节（≤200 MB） | 201 `{id}`。`curl --data-binary @a.zip` |
| `POST /api/folders` | `{path}`（绝对路径，可用 `~/`） | 登记本地文件夹，原地编辑。201 `{id:"@…"}`；在项目库内则直接返回库里的 id |
| `POST /api/folders/pick` | | 弹系统选择框（GUI，agent 别用）；`{path\|null}` |
| `PUT /api/shell/menu` | 平铺数组 `{kind, id, label, accel?, enabled?}` | 前端把翻译好的菜单交给桌面壳（GUI，agent 别用）；204。没有壳时直接丢掉；形状不对 400 `shell.menuInvalid` |
| `DELETE /api/folders/:id` | | 只从列表移除，磁盘不动；编译中 409 `folder.busy` |
| `GET /api/p/:id/project` | | `{id, name, folder, preview, archived, root, entry, files:[{path,dir}]}`；`entry` 是识别出的主 `.tex` |
| `POST /api/p/:id/rename` | `{name}` | `{id: 新id}`；改名即改目录名，id 会变。本地文件夹不能改（`rename.folder`） |
| `POST /api/p/:id/archive` | `{archived: bool}` | 只影响首页列表，文件不动 |
| `POST /api/p/:id/reveal?path=` | | 在访达里显示（GUI）；给 `path` 就选中项目里那个文件 |

### 文件读写与历史

| 方法 路径 | 请求 | 返回 / 说明 |
| --- | --- | --- |
| `GET /api/p/:id/file?path=` | | `{path, content, mtime}`（文本） |
| `PUT /api/p/:id/file` | `{path, content, base?, keep?}` | `{ok, path, mtime}`。父目录自动建。`base` 填读到时的 `mtime`，磁盘被改过则 409 `file.changedOnDisk`（带当前 `mtime`）；覆盖前旧内容自动进本地历史，`keep:true` 强制记一份 |
| `GET /api/p/:id/raw?path=` | | 原样字节（图片、字体、PDF） |
| `POST /api/p/:id/fs/new` | `{path, dir?}` | `{path}`；新建空文件（`dir:true` 为文件夹），已存在 409 `file.exists` |
| `POST /api/p/:id/fs/rename` | `{from, to}` | `{path}`；改名或移动，目标已存在 409 `file.exists`，文件夹不能移进自己（`file.intoItself`） |
| `POST /api/p/:id/fs/duplicate` | `{path, suffix?}` | `{path: 新路径}`；同目录复制成 `名字 副本.ext`、`名字 副本 2.ext`……`suffix` 默认 `copy` |
| `POST /api/p/:id/fs/delete` | `{path}` | `{ok}`；移到系统废纸篓 / 回收站，走不通时挪到项目里的 `.subo/trash/` 下，不真删 |
| `GET /api/p/:id/stat?path=` | | `{mtime, tree}`；`tree` 是文件树指纹，外部增删文件时会变 |
| `GET /api/p/:id/symbols` | | 大纲、`\label`、文献条目 |
| `GET /api/p/:id/history?path=` | | `{versions:[…]}`，由新到旧 |
| `GET /api/p/:id/history/:vid?path=` | | `{id, content}` |
| `POST /api/p/:id/history` | `{path, content}` | 把一份内容记进历史，`{id}` |

agent 直接改磁盘文件也可以（服务会通过 `stat` 被前端发现），但走 `PUT file` 能留历史、能做冲突检测。

### 编译、引擎与定位

| 方法 路径 | 请求 | 返回 / 说明 |
| --- | --- | --- |
| `POST /api/p/:id/build?wait=1` | | 等编完返回结果（见上表）；等待上限是编译超时 + 10 分钟，超了 504 `build.waitTimeout` |
| `POST /api/p/:id/build` | | 202 `{queued}`，结果走事件流；没有 `.tex` 时 400 `build.noTex` |
| `GET /api/p/:id/jobs` | | `{running, jobs:[{jobId, ok, entry, pdf}]}`，由新到旧 |
| `GET /api/p/:id/pdf/:job` | | PDF；没有则 404 `pdf.none` |
| `GET /api/p/:id/log/:job` | | 编译日志纯文本 + `[ok=…]` |
| `POST /api/p/:id/export/:job` | | 把 PDF 复制到项目根，名字随入口（`thesis.tex` → `thesis.pdf`），覆盖同名 |
| `GET /api/p/:id/engine` | | `{mode, program, source, reason, wanted, switchable}`；`source` ∈ `user`/`detected`/`default`/`missing`/`fixed` |
| `PUT /api/p/:id/engine` | `{mode}` | `auto` 或 `pdflatex`/`xelatex`/`lualatex`/`platex`/`uplatex` |
| `POST /api/p/:id/compat` | `{id, file}` | 应用 `compat.found` 里的某条修复（改前留历史），`{ok, changed}`；不自动重编 |
| `GET /api/p/:id/synctex/:job/view?file=&line=` | | 源码 → PDF 位置 |
| `GET /api/p/:id/synctex/:job/edit?page=&x=&y=` | | PDF 位置（bp，页左上为原点）→ 源码 |

### 事件流 `GET /api/events`（SSE）

每条 `data:` 是一个 JSON，都带 `type` 和 `projectId`。编译相关：

| `type` | 字段 |
| --- | --- |
| `job.started` | `jobId, entry` |
| `job.progress` | `jobId, step`：`pass`（`pass`）/ `pages`（`pages`）/ `bib`·`index`·`glossary`（`tool`）/ `pdf` |
| `job.output` | `jobId, text`：编译中的终端输出（TeX 和辅助工具的，各段以 `== 程序名` 开头），实时推送；一块可能有好几行，不带结尾换行 |
| `job.log` | `jobId, text`：完整日志，编译结束时一次推送，以它为准 |
| `job.issues` | `jobId, report`：解析好的问题，紧跟 `job.log`。`report` 是 `{issues, errors, warnings, boxes}`，后三项是总数（警告最多列 300 条、排版提示只列超出最多的 200 条）。每条 `issue`：`id`（重编后不变）、`level`（`error`/`warning`/`box`）、`kind`（错误同 `errorKind`；警告 `undefinedRef`/`undefinedCite`/`multiplyDefined`/`missingChar`/`floatTooLarge`/`font`/`bib`/`summary`/`other`；排版提示 `overfullH`/`underfullH`/`overfullV`/`underfullV`）、`message`、`logRange`（在完整日志里的行号，从 1 起、含两端），以及有才出现的 `file`、`line`、`lineEnd`、`here`（停下处的源码，断点标 `⟨here⟩`）、`chain`（宏调用链）、`runaway`、`source`（宏包名、biber……）、`key`（标签、文献键、缺的字）、`pt`。解析见 `server/src/logparse.rs` |
| `job.done` | `jobId, engine, ms` |
| `job.failed` | `jobId, engine, ms, error`（中文）, `reason`：`failed`/`timeout`/`memory`/`fileSize` |
| `engine.missing` | `jobId, program, engine, name, bytes, managed`：项目要的引擎没装 |
| `compat.found` | `jobId, fixes`：已知的旧样式不兼容，交给 `POST compat` |
| `tex.stale` | `jobId, kernel, want, tex`：LaTeX 内核比宏包旧（`tex` 同 `GET /api/tex`）；改用内置 TeX 走 `PUT /api/settings {tex:"bundled"}` |
| `case.missing` | `jobId, files, resolved`：缺的宏包/字体/外部程序。先推一次 `resolved:false`（只有文件名），查完包名再推 `resolved:true` |
| `job.end` | `jobId`：这次编译的事件到此为止 |

`case.missing` 的 `files[]`：`{kind: "file"|"font"|"tool", file, package?, lookup: "found"|"absent"|"unknown"|"project", neededBy?, fontFile?, projectPath?}`。
`lookup:"found"` 的 `package` 就是可以交给 `POST /api/case/install` 的包名。

注意：`job.done`/`job.failed` 之后还可能跟着 `engine.missing`、`compat.found`、`tex.stale`、`case.missing`，
要等到 `job.end` 才算收完（`build?wait=1` 就是这么做的）。

### 补装宏包（「缺字清单」，模块名 case）

| 方法 路径 | 请求 | 返回 / 说明 |
| --- | --- | --- |
| `POST /api/case/install` | `{packages:[…], auto?, project?}` | 同步装完才返回。包名必须出现在缺字清单或待取队列里，否则 400 `case.unknownPackage`；同时只能装一次（409 `case.busy`）。装完不自动重编 |
| `GET /api/case/install/progress` | | `{progress}` |
| `POST /api/case/install/cancel` | | 取消进行中的安装 |
| `GET /api/case/queue` | | 离线时查不到/装不了的待取队列 |
| `POST /api/case/queue/resolve` | | 联网后重查队列里的文件名，`{queue, unresolved}` |
| `POST /api/case/queue/remove` | `{packages?, files?}` | |
| `GET /api/case/packages` | | 用户层已补装的包 |
| `POST /api/case/packages/remove` · `/reinstall` | `{packages}` | |
| `GET /api/case/fonts?…` | | 字体查询 |
| `GET /api/case/snapshot` | | 导出全部补装宏包的快照（gzip 附件） |
| `POST /api/case/snapshot/import` | 快照原始字节（≤1 GB） | 离线机器导入 |
| `POST /api/p/:id/snapshot` | | 按最近一次编译的 `-recorder` 记录，只打这个项目用到的包，放进项目根 |
| `POST /api/p/:id/snapshot/import` | | 导入项目里带的快照 |

补装要联网（走设置里的镜像）。典型自动修复循环（已实测）：

```bash
curl -s -XPOST "$B/api/p/demo/build?wait=1"
# … "missing":[{"kind":"file","file":"bytefield.sty","lookup":"found","package":"bytefield"}]
curl -s -XPOST $B/api/case/install -H 'content-type: application/json' \
  -d '{"packages":["bytefield"],"project":"demo"}'
# {"ok":true,"offline":false,"admin":false,"log":"…","also":[]}
curl -s -XPOST "$B/api/p/demo/build?wait=1"       # ok:true
```

`lookup` 为 `absent` 的是 TeX Live 里没有的（多半是项目自己的文件或拼错），`unknown` 是离线查不了，
`project` 是项目子目录里就有、只是不在搜索路径上（看 `projectPath`）。`kind:"font"`/`"tool"` 不能用 `case/install` 装。

### 引擎（仅 `--bundled-tex` 时可用，否则 409 `engines.unmanaged`）

| 方法 路径 | 说明 |
| --- | --- |
| `GET /api/engines` | 可按需安装的引擎（LuaTeX、pTeX 等）及状态、大小 |
| `POST /api/engines/:id/install` | `{source?: "import"}`；默认从软件源装 |
| `POST /api/engines/:id/uninstall` | |
| `POST /api/engines/import/:name` | 上传离线引擎包（≤64 MB） |

### 模板

| 方法 路径 | 请求 | 说明 |
| --- | --- | --- |
| `GET /api/templates` | | 预置 + 用户模板 |
| `POST /api/templates/:id/use` | `{name}` | 用模板新建项目，201 `{id}` |
| `POST /api/p/:id/template` | `{name}` | 把项目存为模板 |
| `POST /api/templates/import?name=` | zip 原始字节 | |
| `PATCH /api/templates/:id` | `{category}` | 预置模板只读（403 `template.builtin`） |
| `DELETE /api/templates/:id` | | |

### 设置、外观、AI

| 方法 路径 | 说明 |
| --- | --- |
| `GET /api/settings` | `{settings, presets, env}`；`env` 里有项目库、`suboDir`、TeX 根目录与版本、超时/内存范围等，**排查环境先看这里** |
| `PUT /api/settings` | 只带要改的：`autoInstall`、`mirror`、`customMirror`、`timeoutSecs`、`memoryMb`、`parallel`（并行编译数，1 到本机核数）（`null` 恢复默认）、`tex`（`auto`/`own`/`bundled`，立即生效）、`updateCheck`、`updateDownload`、`updateSource`（空串用默认，否则要 https） |
| `GET /api/tex` | 用哪套 TeX：`choice`、`bundled`（正在用内置的）、`own`（本机 TeX 的种类 `miktex`/`texlive`/`distro`/`other`、内核日期、是否太旧） |
| `POST /api/tex/miktex/update` | 更新 MiKTeX（内核和宏包一起），要几分钟；成功后自动档按新内核重选 |
| `POST /api/settings/library` | `{path\|null, moveProjects?}` 换项目库位置；`--home` 启动时 409 |
| `POST /api/settings/mirrors/probe` | 测镜像速度 |
| `POST /api/settings/reveal` | 在访达里打开某个目录（GUI） |
| `GET /api/update` | 软件更新：`{supported, current, defaultSource, status}`。`supported` 只有打包出来的桌面版为 true；`status.phase` 是 `idle`/`checking`/`latest`/`available`/`downloading`/`ready`/`failed` |
| `POST /api/update/check` | 立即查新版本，查完才返回（形状同上）；不支持时 501，连不上 502 |
| `POST /api/update/download` | 后台下载查到的版本，立即返回，进度看 `status.received/total` |
| `POST /api/update/apply` | `ready` 时重启应用让新版本生效（Windows 上运行安装程序）。**会关掉应用**，agent 别调 |
| `GET /api/appearance` · `PUT /api/appearance/prefs` · `PUT\|DELETE /api/appearance/:kind/:id` | 配色/外观包 |
| `GET\|PUT /api/ai` | AI 服务与模型配置（不含 Key）。`models` 有先后：第一个是首选，出错时依次往后换；`vision`（能看图）只能由检测设上，保存时只能取消 |
| `PUT /api/ai/services/:id/key` | `{key}`，空串清除；Key 只进不出 |
| `POST /api/ai/services/:id/test` | `{model?}` |
| `POST /api/ai/models/:id/vision` | 测试识图：发一张写着数字的小图，答对就把这个模型标成能看图 |
| `POST /api/ai/complete` | `{model?, messages:[{role,content,images?}], temperature?, maxTokens?}`，流式返回；`images` 是 data URL，只发给能看图的模型；换模型时多一个 `fallback` 事件 |
| `POST /api/p/:id/ai/assist` | 修编译错误 / ⌘K 改写：`{kind:"fix"\|"edit", file, before, target, after, instruction?, preset?, error?}` |
| `POST /api/p/:id/ai/verify` | `{file, text}`：在影子副本里试编改完的内容，不动原文件 |

AI 相关接口会联网调用户配置的第三方服务，agent 别在用户不知情时调用，也别往里写 Key。

---

## 4. 脚本

| 脚本 | 用途 |
| --- | --- |
| `scripts/dev.sh` | 构建前端（`npm run build`）+ 服务（debug），在 **8787** 启动并托管 `app/dist`。可选 `SUBO_WS=<目录>`：工具链（`.texlive/`、`.toolchain/`）装在自包含工作区时用 |
| `scripts/restart.sh` | 先杀掉占 8787 的旧服务，再跑 `dev.sh`。改了服务端接口后用它 |
| `scripts/desktop.sh` | 调试版桌面壳；`bundle` 打当前平台安装包；`SUBO_RELEASE=1` 用发版档（慢） |
| `scripts/bundled-smoke.sh [项目目录…]` | 用内置精简 TeX（`desktop/texlive`，`SUBO_TL` 可改）在临时目录里跑「编译 → 缺字清单 → 安装 → 重编」，不碰用户目录。`SUBO_SMOKE_ONLY=1` 只跑参数里的项目；`SUBO_SMOKE_LOGS=<dir>` 存日志；`SUBO_SMOKE_PORT`（默认 8799）；`SUBO_SMOKE_TIMEOUT` |
| `scripts/texlive-trim.sh <源TL根> [目标]` | 从完整 TeX Live 裁出随应用分发的精简版（默认 `desktop/texlive`） |
| `scripts/texlive-index.sh <tlmgr> <输出>` | 生成「文件名 → 包名」索引 |
| `scripts/rungs.sh`、`scripts/epstopdf.sh` | 装进内置 TeX Live 的 `rungs`/`epstopdf` 替身，转调系统 Ghostscript；不是给人直接用的 |

前端检查（改了 `app/` 必跑）：

```bash
cd app && npx tsc --noEmit && npm run check:i18n
```

服务端测试：

```bash
cd server && cargo test
```

---

## 5. 给 agent 的注意事项

- 编译、装宏包用命令行 `subo build … --install`；只有要和打开着的界面配合时才起服务、走 HTTP
  （那时用 `build?wait=1`，`:job` 写 `latest`，不必解析 SSE）。
- 自测一律用**隔离实例**：`SUBO_HOME` + `--home` 指临时目录、换一个端口；不要动用户的 `~/.subo`、
  `~/Documents/Subo`，也不要杀用户 8787 上的进程，除非用户要求。没设 `SUBO_HOME` 就启动，
  服务会读写真实的 `~/.subo` 和默认项目库。
- 改了服务端代码要重新 `cargo build` 并重启实例才生效；前端改动要重新 `npm run build`（`--dist` 托管的是构建产物）。
- 交付形态是离线桌面软件：只有补装宏包、引擎安装、镜像测速、AI 这几类接口会联网。
- 接口返回给人看的文案走前端 i18n；新增服务端错误码时，先在 `app/src/i18n/locales/zh-CN.ts`
  的 `server` 分组加同名键，其他语言同步补齐（见 [i18n.md](i18n.md)）。
