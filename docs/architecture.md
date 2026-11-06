# 架构

> 本文讲设计与取舍。文中的数值（超时、上限、体积）是写作时的取值，以代码为准；对不上时改本文。

## 1. 定位

**本地优先的 LaTeX 工作台。** 界面与工作流参考 Overleaf，运行与数据归属本地。

### 目标

1. 离线完整可用：没有网络时，已经取过的字盘能编译，功能不减配。
2. 按需取包：首次安装体积小，缺什么取什么，只拉依赖闭包，不整包下载。
3. 多语言：Unicode 原生引擎（XeTeX），CJK、RTL 等文种不靠过时宏包硬撑。
4. 不臃肿：常驻依赖可控，冷启动快，安装包与缓存都要有明确上限。

### 非目标

- 多人实时协作、账号体系、云端项目
- 模板市场
- 移动端
- 兼容 `.cls` 之外的 Overleaf 私有功能

## 2. 形态与进程模型

三件东西，一个进程：

```
┌────────────────────────────────────────────┐
│ 桌面壳（Tauri v2）                          │
│  ┌──────────────────────────────────────┐  │
│  │ Web 前端：文件树 · 编辑器 · PDF 预览   │  │
│  └──────────────────┬───────────────────┘  │
│                     │ HTTP + SSE           │
│  ┌──────────────────▼───────────────────┐  │
│  │ 本地服务 subo-server                  │  │
│  │  quire（项目） · case（字盘） · press │  │
│  └──────────────────┬───────────────────┘  │
│                     │ 子进程                │
│  ┌──────────────────▼───────────────────┐  │
│  │ 排版引擎（TeX Live；tectonic 可选）   │  │
│  └──────────────────────────────────────┘  │
└────────────────────────────────────────────┘
```

关键决定：**服务与界面分离**。本地起一个 HTTP 服务，前端是普通 Web 应用。
同一套代码既能装进桌面壳，也能在浏览器里打开调试；编译这类长任务走 SSE 推送天然顺。

## 3. 组件

| 组件 | 职责 | 不负责 |
|---|---|---|
| **quire** | 项目 = 一个目录。读取入口文件、解析项目内相对引用、记录 UI 状态 | 不解析宏包依赖 |
| **case** | 字盘。宏包与字体的本地存储、索引、按需获取、快照导入导出 | 不决定排版 |
| **press** | 编译调度。入队、并发控制、超时、日志采集、产物回收 | 不做依赖解析 |
| **proof** | 预览。PDF 渲染、源码与 PDF 双向跳转（SyncTeX） | — |

依赖方向：`press → case`，`press → quire`，`case` 与 `quire` 互不依赖。

## 4. 核心：双模字盘

这是整个项目最硬的部分。要求是「离线能编、在线按需」，冲突点在于：
**要离线可用，就得先知道依赖是什么；要知道依赖是什么，往往得先编译一次。**

### 4.1 三层字盘

| 层 | 内容 | 体积量级 | 何时存在 |
|---|---|---|---|
| **L0 内置核心** | 精简 TeX Live：xetex + ctex 链 + biber | 约 400 MB，macOS 安装包约 195 MB | 随应用分发，包内只读 |
| **L1 在线增量** | 缺的宏包，`tlmgr --usermode` 装进 `~/.subo/texlive/<年份>` | 按项目增长，设上限 | 联网时，用户确认后 |
| **L2 兼容层** | 用户自己的完整 TeX Live | 数百 MB 起 | 仅用户显式开启 |

L1 的取包粒度是**文件**而非宏包：一个 `.sty` 可能依赖其它 `.sty`、`.def`、字体度量文件，
真正的闭包由引擎在编译过程中边跑边报告。因此机制是「编译驱动取包」，
而不是「先分析 `\usepackage` 再决定」。扫描 `\usepackage` 只用于**预取提示**和 UI 展示。

### 4.2 编译状态机

```
请求编译
  │
  ├─ 依赖全部命中本地 ──────────────→ 编译 → 成功
  │
  ├─ 缺文件 & 在线 ─→ 取字（可取消，显示进度）→ 重试 → 成功 / 仍缺
  │                        │
  │                        └─ 软件源里没有 → 提示 L2 或手动放入项目
  │
  └─ 缺文件 & 离线 ─→ 中止，产出「缺字清单」
                        │
                        └─ 加入待取队列，联网后一键补齐
```

三条硬要求：

1. **离线失败必须可解释。** 不是一句 "compilation failed"，而是列出缺哪些文件、
   属于哪个宏包、联网后预计多大。
2. **取字可取消、可重试。** 一次编译的取字失败不应污染字盘（半写入的缓存比没缓存更糟）。
3. **永不隐式下载全量。** 任何超过阈值的下载必须显式确认，并显示体积。

### 4.3 字盘快照（离线搬运）

L1 缓存一旦成型，就可以导出为快照：一份依赖闭包 + 哈希清单，拿到离线的机器上导入即可编。
这是对「离线」需求的真正闭环——不是「碰巧缓存过」，而是「可以确定地搬运」。实现见 §7「字盘快照」。

## 5. 数据布局

```
~/.subo/                  # 设了 SUBO_HOME 时为 $SUBO_HOME
├── settings.json         # 用户设置：项目库位置、下载源、自动安装、编译超时、内存上限
├── folders.json          # 「打开文件夹」登记的本地目录
├── ai.json               # AI 服务与模型配置（不含 Key；Key 在系统钥匙串）
├── secrets.json          # 仅在系统钥匙串不可用时出现，权限 0600
├── onboarding.json       # 看过哪个版本的引导
├── texlive/<年份>/       # L1 用户层：补装的宏包与引擎
├── case/                 # 文件名 → 包名索引缓存、待取队列
├── press/                # 界面里每次编译一个 job 目录，产物与日志
├── cli/                  # 命令行 `subo build` 的中间产物与日志
├── preview/              # 模板预览的临时副本
├── projects/             # 项目级偏好（如选定的引擎）
├── history/              # 本地历史
├── lastgood/             # 上次编译成功时的源码与排版信号，AI 修复和校验的基准
├── ai-verify/            # AI 改动的试编副本
├── fontspec/             # 系统字体 → fontspec 名称映射的缓存
└── appearance/           # 导入的配色、外观与语言包
~/Documents/Subo/         # 默认项目库（可在设置里改；设了 SUBO_HOME 时为 $SUBO_HOME/quires）
```

原则：编译产物和应用状态不写进项目目录。`~/.subo` 不只是缓存，还包含设置、文件夹登记、
本地历史以及补装的宏包；删除前应备份需要保留的数据。

### 项目模板

模板放在项目库下的隐藏目录 `.templates/<id>/`（`template.json` + `files/`），不放 `~/.subo`：
模板是用户的东西。来源两种：把项目「存为模板」，或上传 zip。

入库**只查形状，不编译**（`server/src/templates.rs`）：顶层要有含 `\documentclass` 与
`\begin{document}` 的 UTF-8 主文件；文件类型走白名单，内容要和扩展名对得上；编译产物、
隐藏目录、系统杂项直接略过；体积与文件数比导入卡得更紧。缺的宏包、字体不在入库时管——
从源码扫出的 `\usepackage`、`\setmainfont` 等只记成线索给人看；真正缺什么，等用模板新建
项目、首次编译时走缺字清单补。

## 6. 编译流水线

1. `press` 收到编译请求，取 `quire` 的入口文件
2. 在 `~/.subo/press/<项目路径哈希>/<job>` 下编译，隔离每次编译
3. 启动引擎：优先本地字盘，返回缺失文件清单
4. 缺失文件走 `case` 获取 → 写缓存（先写临时文件再原子改名）→ 重跑
5. 采集日志、产物 PDF、SyncTeX 映射；日志只在服务端解析一次（`logparse.rs`），问题面板、AI 修复与编译校验、
   命令行、HTTP 接口用的是同一份结果，前端不再自己读日志
6. 推送事件：`job.started` / `case.resolving` / `job.log` / `job.issues` / `job.done` / `job.failed` / `case.missing`

并发：不同项目并行，**同项目串行**（避免中间文件互相踩）。
超时与资源上限是必需的——用户写死循环宏不是罕见事。

## 7. 实现要点

### 技术选型

| 层 | 选择 | 理由 |
|---|---|---|
| 桌面壳 | Tauri v2 | 体积小，Rust 侧与服务同进程，无多余运行时 |
| 前端 | React + CodeMirror 6 + PDF.js | CM6 比 Monaco 轻，LaTeX 高亮可自写；PDF.js 是唯一现实选择 |
| 本地服务 | Rust（axum） | 与壳同语言，服务直接内嵌进壳的进程 |
| 引擎 | TeX Live（xelatex、pdflatex 内置） | 有大量镜像、`tlmgr` 可按包装；tectonic 只有一台取包服务器、没有镜像，离线要靠预先装满缓存，biber / shell-escape 支持有限，降为可选引擎（`--engine`） |
| 存储 | 文件系统 + JSON | 不引入外部数据库 |
| 实时通道 | SSE | 单向推送够用，编译中断用另一个 POST |

**为什么 `case` 难**：xelatex 的报错只给文件名（`File 'realscripts.sty' not found`），
甚至给的是 TFM 度量名（`phvr7t`）。「文件名 → TeX Live 包名」必须靠索引或
`tlmgr search --global --file`，不能靠猜；一个模板可能连字体度量都要按需补。

**PDF.js 与 CID 字体**：渲染 XeTeX 的 CID 字体必须配置 `cMapUrl` / `cMapPacked` / `standardFontDataUrl`，
否则字体加载失败、整页错位（渲染出字但位置全乱，很难一眼看出是渲染问题而非排版问题）。

**SyncTeX**：xelatex 以 `-synctex=-1` 产出不压缩的 `.synctex`，服务端 `synctex.rs` 直接解析
（精简 TeX Live 没有 `synctex` 命令行）。

### press

- **同项目串行，请求合并**：编译中再收到请求只记一个「待编」标记，跑完补编一次。
  连按 ⌘↵ 不会并发踩中间文件，也不会排出一长串队列。
- **超时**：默认 120 s（设置里可改），到点杀整个进程组（xelatex 会再拉起 xdvipdfmx）。
- **资源限额**（`engine.rs`）：子进程用 `setrlimit(RLIMIT_FSIZE)` 限单个输出文件 2 GB；常驻内存每 200 ms 查一次
  （macOS `proc_pidinfo`，Linux `/proc/<pid>/statm`），超过设置里的上限（默认 4096 MB）连进程组一起杀。
  不用 RLIMIT_AS：macOS 不执行它，LuaTeX 预留的虚拟地址又远大于实际用量。
  `job.failed` 带 `reason: timeout | memory | fileSize | failed`。
- **日志上限**：stdout / stderr 各收 1 MB，超出部分读出丢弃，防刷屏宏撑爆内存。
- **产物回收**：只保留最近 5 次编译，更早的连目录删除；启动时清空上次运行的残留。
- **路径安全**：`resolve` 规范化最近的已存在祖先并要求仍在项目根内，挡住符号链接
  （含悬空链接）逃逸；文件树只列指向项目内的链接，且不跟进遍历。
- **前端**：编译与切换文件前先落盘防抖期内的编辑，⌘↵ 不会编到旧稿。

### case：缺字清单

编译失败时说清楚缺哪个文件、属于哪个 TeX Live 包（§4.2「离线失败必须可解释」）。

- **日志识别**（`server/src/case.rs`）：三种模式——`File `X' not found`（LaTeX / 宏包报错、
  `\input`）、`I can't find file `X'`、`Font …=NAME … not loadable: Metric (TFM) file`（记为
  `NAME.tfm`）。先把 TeX 按 79 列硬折断的行接回去，长文件名和 TFM 报错都会被折断。
  mktextfm 里 mf 回显的 `phvr8t` 与 `phvr8t.tfm` 合并为一条。
- **宽进严出**：TeX 很多缺文件不报「找不到」（NFSS 缺 `.fd` 只记一句 `No file MDAcmr.fd.`，随后用替代字体
  或报一个看不出原因的错）。编译失败时把日志里所有 `No file X` / `file 'X' not found` 都当候选，
  再用随包带的完整版 TeX Live 文件索引（`<TeX Live>/tlpkg/subo-file-index.txt`，裁剪时由
  `scripts/texlive-index.sh` 生成，约 4 MB 文本）筛：完整版里有、本机没装的才报，其余是无害回退，丢掉。
  索引也让已认定的缺失文件离线就能查到包名，查不到索引（用户自己的 TeX Live）才退回 tlmgr。
  xdvipdfmx 缺字体文件（`... can find this font: x.pfb`、CMap、TFM 等）也算候选，编译成功时也看；
  为此 xelatex 用 `-output-driver=xdvipdfmx -E` 调驱动，默认的 `-q` 会把这些警告全吞掉。
- **包名映射**：`tlmgr search --global --file '/NAME$'`，30 s 超时、杀进程组。
  必须按末段**精确**锚定：不锚定时 `xltxtra.sty` 还会命中 `lwarp-xltxtra.sty`。
  `texmf-dist/doc/`、`texmf-dist/source/` 下的同名文件**不算**：项目自己的 `chap1.tex`
  会被映射到某些宏包示例里的同名文件，引擎根本不会去那里找。
- **缓存**：`~/.subo/case/file-index-v1.json`，文件名 → 包名或 `null`（远端明确没有）。
  查询失败（离线）不写缓存；一次失败后本次剩余文件不再挨个等超时。判定规则变了就升文件名里的版本号。
- **事件**：`job.failed` 照常先推，随后 `case.missing` 推两次——先 `resolved:false`
  （文件名，`lookup:"pending"`），查完 `resolved:true`。
  `files: [{ file, package?, lookup: "found" | "absent" | "unknown" | "pending" | "project", projectPath? }]`。
  项目子目录里就有同名文件的（TeX 不进子目录找）标 `project`、附 `projectPath`，不查 tlmgr。
- **预扫**（`server/src/prescan.rs`）：`-halt-on-error` 下一次编译只暴露第一个缺失文件。编译因缺文件失败时，
  从入口顺着 `\input` / `\include` / `\subfile` 和项目自带的 `.sty` / `.cls` 读下去，收齐 `\usepackage`、
  `\RequirePackage`、`\documentclass`、beamer 主题，并进宽进候选交给索引严出，一次列全。只扫项目文件，
  不钻已装的宏包（依赖本来就齐，又满是 `\IfFileExists` 包着的可选依赖）。
- **装完补依赖**（`case::install`）：读刚装上的文件（连同它们拉进来的同包模块），
  `\RequirePackage` 的东西本机没有就按索引换成包名再装，最多 3 层；结果的 `also` 列出补装的包。
  走管理员授权时不补，免得一层弹一次密码框。

### case：取字、队列与快照

- **取字可取消**：tlmgr 由我们直接起的（用户层、目录可写）在连软件源、下载阶段可以停；开始解包后
  请求一律拒绝（`case.cancelTooLate`），因为杀在解包中途会留下半个包。走系统授权框的安装不提供取消。
- **自动安装**：开了以后缺字清单查出包名就直接装、装完重编；同一组包只自动装一次，防止来回打转。
  需要管理员授权（系统 TeX Live 归 root）时返回 `case.autoNeedsAdmin`，退回手动点安装。
- **待取队列**（`queue.rs`，`~/.subo/case/queue.json`）：查不到包名的文件（离线）、安装时连不上软件源的包
  进队列；装上了（安装、导入快照）就出队列。联网后先查文件属于哪个包，再一次装齐。
- **字盘快照**（`snapshot.rs`）：`.subocase` 是 gzip 过的 tar，`subo-case.json` 清单 + `texmf/` 下的用户层文件，
  每个文件记大小和 SHA-256。项目快照靠编译时加的 `-recorder`：`.fls` 里用户层的文件按包归属换成包名，只打这些包。
  导入先整包解到临时目录、逐个核对，任何一个不对就整包拒收；`tlpkg/`、`web2c/`、`ls-R` 不许出现在快照里；
  已装的包跳过，内容不同的同名文件保留本机的；导入的包按文件登记，设置页照样能卸载。

### 项目与本地文件夹

Subo 是「打开你论文的编辑器」，项目库只是起步的便利，不是唯一归宿。

- **项目库**：默认 `~/Documents/Subo/<id>/`，id 即目录名（`SUBO_HOME` / `--home` 可改）。
  用户起的名字经 `slug` 变目录名：保留中文等 Unicode 字母数字，空白变 `-`，重名加 `-2`。改名即改目录名。
- **本地文件夹**：`~/.subo/folders.json`（`[{id, path}]`）。id 为 `@` + 目录名 slug，与项目库不会撞名。
  登记时目录下 3 层内要有 .tex，防止误选整个主目录；原地编辑，不改名，编译产物不写进去。
  找不到的文件夹在列表里沉底，不能打开，只能移出。
- **接口**：`GET/POST /api/projects`，项目内接口统一在 `/api/p/:id/…`；事件流是一条 `/api/events`，
  每条事件带 `projectId`，前端按当前项目过滤。完整列表见 [cli.md](cli.md)。
- **zip 导入**：自己解（`zip` crate，只开解压），不调系统 `unzip`——Windows 上没有它。先看清单里的总量
  （≤ 500 MB、≤ 10000 个文件），解的时候再按实际字节数封顶，带 `..` 的项和符号链接跳过。解到项目库下
  点开头的临时目录，删符号链接、`__MACOSX`、`.DS_Store`，只有一层顶层目录时剥掉，必须含 .tex，成功才改名上架。
- **arXiv 源码包导入**（`tarball.rs`）：gzip 过的 tar、不压缩的 tar，或 gzip 过的单个 .tex（存成 `main.tex`；
  解出来是 PDF 的报 `import.pdfOnly`）。不看扩展名——arXiv 的包常没有扩展名。只落普通文件和目录，
  链接、设备跳过，带 `..` 的项丢掉，边解边按同样的 500 MB / 10000 个卡。模板上传走同一个 `library::unpack`。
- **自带 .bbl**：arXiv 包通常只给编好的 `<主文件>.bbl`、不给 .bib。这种项目不跑 bibtex / biber，
  否则输出目录里新写的空 .bbl 会盖掉项目里那份（xelatex 先找输出目录）。
- **入口探测**：`main.tex` → arXiv `00README.json` 里标 `toplevel` 的 .tex → 顶层含 `\documentclass` 的 .tex（按名排序）→ 任意顶层 .tex。
- **外部修改与本地历史**（`history.rs`）：读文件时返回修改时间，保存时带回去，对不上返回 409（`file.changedOnDisk`）；
  前端定时（和窗口回到前台时）检查当前文件和文件树。历史放 `~/.subo/history/<项目哈希>/<文件哈希>/`，
  保存时磁盘上的旧内容离上一份超过 5 分钟才记，恢复、冲突时覆盖、自动修复之前强制记。
- **符号**（`symbols.rs`）：大纲从入口顺着 `\input`、`\include`、`\subfile`、`\import` 按文档顺序读；标签收全项目
  `.tex`，文献收全项目 `.bib`（宽松解析，坏条目跳过）。

### 设置

- **存储**：`~/.subo/settings.json`（`server/src/settings.rs`），读不出来用默认值；先写临时文件再改名。
  外观、语言是前端 localStorage 里的本机偏好。
- **项目库位置**：立即生效。搬项目只用 `rename`：同盘瞬间完成，跨盘或重名的留在原处、不删除。
  `--home` 启动时位置锁定，设置页只读。设置里的位置建不出来（外接盘没插）就退回默认位置，不改设置。
- **下载源**：「跟随 TeX Live」（默认）、预置 CTAN 官方与国内镜像、自定义网址或本机 tlnet 目录（离线安装源）。
  实际用法是每次给 tlmgr 加 `--repository`，不改 TeX Live 自己的配置。
  测速取 `tlpkg/texlive.tlpdb.sha512` 的第一个字节，6 s 超时。
- **清单**：宏包来自 `tlmgr info --only-installed`（只读本地库），内置 TeX 时再并上用户层；
  字体扫 TeX Live 各树 + 系统字体目录（含 .ttc），结果缓存，装宏包后失效。

### 界面多语言

- 前端文案全部在 `app/src/i18n/locales/<语言代码>.ts`，zh-CN 为基准；
  语言文件由 `import.meta.glob` 自动收录，加语言不改代码。写法与守门规则见 [i18n.md](i18n.md)。
- 复数、相对时间、数字格式走 `Intl`；CodeMirror 的面板文案用 Compartment 随语言重配，不重建编辑器。
- 服务端错误统一成 `{ code, params, message }`，前端按错误码渲染。

### 桌面壳

- Tauri v2，服务内嵌进壳的进程（`subo-server` 拆成库 + 命令行入口），窗口打开 `http://127.0.0.1:<随机端口>/`，
  不走 IPC。macOS、Linux、Windows（Windows 版开发中）。
- **Windows 的平台差异**：编译进程挂进作业对象（`server/src/winjob.rs`），超时连子进程一起结束，
  内存看整个作业的提交内存峰值，输出大小看作业累计写出的字节数（没有 `RLIMIT_FSIZE`）；
  找程序按 `PATHEXT` 补后缀（`tlmgr.bat`）；用户目录读 `USERPROFILE`，项目库放系统「文档」目录的真实位置
  （可能被重定向到 OneDrive）；路径规范化用 `dunce`，不带 `\\?\` 前缀；`aux`、`con` 这类设备名不能作文件名
  （`path.reserved`）；机密存凭据管理器。桌面壳补找 `C:\texlive\<年份>` 和 MiKTeX 的常见位置。
- 从 Finder 启动时取登录 shell 的 PATH；系统没装 TeX 时首页显示「未找到 TeX」并退回 mock 引擎。
- **内置 TeX Live**：`scripts/texlive-trim.sh` 从完整 TeX Live 裁出精简版（约 400 MB），
  macOS 安装包 dmg 约 195 MB。缺的宏包装进用户层。
- **macOS 菜单栏**：菜单文案跟界面语言走，语言只有前端知道（含导入的语言包），所以由前端（`app/src/menu.ts`）
  拼好整份菜单，`PUT /api/shell/menu` 交给服务端，服务端校验后转给壳去画（`desktop/src/menu.rs`）。
  点了菜单项，壳往窗口 `eval` 一个 `subo:menu` 事件，前端用 `useMenu` 接。
  菜单栏上的应用名取自 `CFBundleName`，跟系统语言：`desktop/macos/*.lproj/InfoPlist.strings` 里中文系统是「素帛」、其他是 Subo。
  Linux 不做菜单栏。
- **关于**：前端对话框（`app/src/About.tsx`）显示版本与构建号（`server/build.rs` 编进 git 提交号和提交日期）、
  排版引擎、TeX、项目库、系统，「复制诊断信息」把这些拼成一段文字。第三方许可清单在 `npm run build` 时由
  `app/scripts/gen-licenses.mjs` 生成 `public/licenses.json`，不入库。

## 8. 安全

用户的 `.tex` 是任意代码（`\input`、巨量输出、内存炸弹）：编译在独立目录里跑，有超时、内存与输出文件上限，
路径解析不允许逃出项目根；导入压缩包限总量与文件数、丢弃链接与路径穿越项。
AI 辅助只在用户主动触发时调用户自己配置的服务，Key 存系统钥匙串，接口从不返回它。
