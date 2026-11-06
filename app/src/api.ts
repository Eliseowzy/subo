import { hasKey, t } from "./i18n";
import type { Vars } from "./i18n/types";

export type Entry = { path: string; dir: boolean };

export type ProjectInfo = {
  id: string;
  /** 显示名：项目库里即 id，本地文件夹是目录名 */
  name: string;
  folder: boolean;
  /** 模板预览：只读副本，不在项目库里 */
  preview: boolean;
  /** 已归档：和模板预览一样只能看、编译，取消归档才能改 */
  archived: boolean;
  root: string;
  entry: string | null;
  files: Entry[];
};

/** 模板预览在服务端也是一个「项目」，id 是 `~` 加模板 id（与服务端 PREVIEW_PREFIX 一致） */
export const previewId = (templateId: string) => `~${templateId}`;

/** 预览上的一块区域：bp（1/72 英寸），原点在页左上 */
export type Spot = { page: number; x: number; y: number; w: number; h: number };

export type ProjectSummary = {
  id: string;
  name: string;
  /** 磁盘上的完整路径 */
  path: string;
  entry: string | null;
  /** Unix 秒 */
  modified: number;
  /** 用户磁盘上原有的文件夹，原地编辑，不归项目库管 */
  folder: boolean;
  /** 文件夹找不到了 */
  missing: boolean;
  /** 已归档：首页收进「已归档」，照样能打开 */
  archived: boolean;
};

/** 模板分类，一个模板只属于一类；顺序即界面上的顺序（与服务端 templates::CATEGORIES 一致） */
export const TEMPLATE_CATEGORIES = ["paper", "thesis", "report", "resume", "slides", "poster", "other"] as const;
export type TemplateCategory = (typeof TEMPLATE_CATEGORIES)[number];

/** 项目模板。packages / fonts 是从源码里扫出的线索，没核对本机装没装 */
export type TemplateSummary = {
  id: string;
  /** 预置模板：随应用带的只读模板，id 以 builtin: 开头，显示名、简介查语言文件 */
  builtin: boolean;
  /** 用户模板就是存时起的名字；预置模板是兜底名 */
  name: string;
  category: TemplateCategory;
  entry: string;
  /** Unix 秒；预置模板为 0 */
  created: number;
  source: "project" | "zip" | "builtin";
  files: number;
  bytes: number;
  packages: string[];
  fonts: string[];
};

/** 存模板的结果：name 是去重后的显示名；dropped 是被略过的编译产物、杂项（最多几项），droppedCount 是总数 */
export type TemplateStored = { id: string; name: string; dropped: string[]; droppedCount: number };

/**
 * found：TeX Live 里有；absent：没有（多半是项目文件）；unknown：查不了；pending：还在查；
 * project：项目子目录里就有同名文件（projectPath），只是 TeX 不进子目录找。
 * kind 为 font 时 file 是字体名，不查 tlmgr：TeX Live 自带同名字体文件时 lookup 为 found、
 * fontFile 是那个文件名（XeTeX 按名字找不到它），否则 lookup 为 absent；
 * kind 为 tool 时 file 是 TeX Live 之外的程序代号（目前只有 ghostscript），neededBy 是用到它的图
 */
/**
 * 旧模板和新版 TeX 的已知不兼容，服务端从失败的编译里认出来，用户点了才改文件（见 server/src/compat.rs）。
 * combinedblfloats：ACL / EMNLP / NAACL 2018–2020 样式给 \@combinedblfloats 打的补丁，新内核下必然失败
 * cjkutf8：pdfLaTeX 的 CJK / CJKutf8 宏包，XeTeX 下碰到 è 这类字符就报错
 */
/** 项目的编译引擎：用户选的、实际用的和来由 */
export type Program = "pdflatex" | "xelatex" | "lualatex" | "platex" | "uplatex";
export type EngineMode = "auto" | Program;
export type EngineInfo = {
  mode: EngineMode;
  /** 实际用的程序；不能切换时是 tectonic、mock */
  program: string;
  /** missing：要用的程序（wanted）没装，先用 xelatex 试编 */
  source: "user" | "detected" | "default" | "missing" | "fixed";
  reason?: { kind: "magic" | "latexmkrc" | "arxiv" | "preamble"; detail?: string } | null;
  wanted?: Program | null;
  /** tectonic、mock、模板预览不给选 */
  switchable: boolean;
};

export type CompatFix = { id: "combinedblfloats" | "cjkutf8"; file: string; line: number };

export type MissingFile = {
  kind: "file" | "font" | "tool";
  file: string;
  package?: string;
  lookup: "pending" | "found" | "absent" | "unknown" | "project";
  neededBy?: string[];
  fontFile?: string;
  projectPath?: string;
};

/** 用户设置，存在服务端 `~/.subo/settings.json` */
/** 宏包安装的实时进度，见 server 的 case::Progress */
export type InstallProgress = {
  /** connect 连软件源；download 下载（新版 tlmgr 先并行预取）；install 逐个解包；finish 刷新索引 */
  phase: "connect" | "download" | "install" | "finish";
  /** 逐个安装时是第几个（从 1 起），预取时为 0 */
  index: number;
  /** 一共几个包（含依赖）；tlmgr 还没报时为 0 */
  total: number;
  package: string | null;
  kb: number | null;
  /** tlmgr 按字节估的已用、预计总秒数；第一个包时为 null */
  spent: number | null;
  estimate: number | null;
  /** 用户要的已装好，正顺着依赖补装 */
  followup: boolean;
  elapsed: number;
  /** 已下载 / 一共要下载的字节数；量不到、查不到为 null */
  got: number | null;
  want: number | null;
  /** 从下载第一个字节算起的秒数 */
  downloading: number;
  /** 这次安装停得下来（不是走系统授权框）；解包开始后照样不能停 */
  cancellable: boolean;
};

export type Settings = {
  autoInstall: boolean;
  /** "tlmgr"（跟随 TeX Live）、预置源 id，或 "custom" */
  mirror: string;
  customMirror: string;
  /** 项目库位置；null 为默认 */
  library: string | null;
  /** 编译超时（秒）；null 用默认 */
  timeoutSecs: number | null;
  /** 编译进程的内存上限（MB）；null 用默认 */
  memoryMb: number | null;
  /** 同时编译的项目数上限；null 为本机逻辑核数 */
  parallel: number | null;
  /** 用哪套 TeX：auto（本机的不太旧就用它）、own（本机的）、bundled（素帛内置的） */
  tex: TexChoice;
  /** 启动后和此后每天查一次新版本 */
  updateCheck: boolean;
  /** 查到新版本就在后台下好，重启生效 */
  updateDownload: boolean;
  /** 版本清单地址；空用默认 */
  updateSource: string;
};

/** 服务端返回的错误体 `{ code, params, message }` */
export type FailBody = { code?: unknown; params?: unknown; message?: unknown };

/** 软件更新的状态（服务端 update.rs） */
export type UpdatePhase = "idle" | "checking" | "latest" | "available" | "downloading" | "ready" | "failed";
export type UpdateInfo = {
  /** 打包出来的桌面版才能自动更新；命令行版、开发版为 false */
  supported: boolean;
  current: string;
  defaultSource: string | null;
  status: {
    phase: UpdatePhase;
    release: { version: string; notes: string | null; date: string | null } | null;
    received: number;
    total: number | null;
    /** 本次启动内上次查成功的时间（Unix 秒） */
    checkedAt: number | null;
    error: FailBody | null;
  };
};

export type TexChoice = "auto" | "own" | "bundled";

/** 用哪套 TeX、本机 TeX 的版本（服务端 tex.rs 的 Status） */
export type TexStatus = {
  choice: TexChoice;
  /** 正在用内置的 */
  bundled: boolean;
  /** 本机和内置的都在，能换 */
  switchable: boolean;
  hasBundled: boolean;
  bundledYear: number | null;
  own: {
    kind: "miktex" | "texlive" | "distro" | "other";
    /** LaTeX 内核日期，如 2021-11-15 */
    kernel: string | null;
    /** 内核早于 floor */
    old: boolean;
    bin: string;
  } | null;
  /** 内核早于这一天算太旧 */
  floor: string;
};

/** 待取队列：离线时没取到的宏包、没查到包名的文件，值是要它的项目 id */
export type CaseQueue = { packages: Record<string, string[]>; files: Record<string, string[]>; updated: number };

/** 宏包安装的结果，见 server 的 case::Installed */
export type InstallResult = {
  ok: boolean;
  /** 用户取消了（下载阶段点了取消，或系统授权框点了取消） */
  cancelled: boolean;
  admin: boolean;
  /** 连不上软件源：已记进待取队列 */
  offline: boolean;
  log: string;
  /** 主软件源已跨到新年度版本，这次改用了这个归档源 */
  archive: string | null;
  /** 软件源比本机 TeX Live 新、又没能回退时，软件源的年份 */
  repoYear: number | null;
  also: string[];
  /** 本机 tlmgr 比软件源旧，装之前先替用户更新了它 */
  tlmgrUpdated: boolean;
};

/** 本机 tlmgr 旧不旧：na 用不着（内置 TeX、不是 TeX Live）；unknown 问不到（多半离线）。admin：更新要电脑密码 */
export type TlmgrStatus =
  | { state: "na" }
  | { state: "current"; admin: boolean }
  | { state: "stale"; admin: boolean; local: number; remote: number }
  | { state: "unknown"; admin: boolean; detail: string };

/** 导入字盘快照的结果 */
export type SnapshotImported = { packages: string[]; skipped: string[]; files: number; kept: string[]; project: string | null };

/** 文件的一个历史版本；id 是毫秒时间戳 */
export type HistoryVersion = { id: number; size: number };

/** 项目里的符号，见 server 的 symbols.rs；大纲 level 0 \part … 5 \paragraph，6 是 beamer 的 frame */
export type Heading = { level: number; title: string; file: string; line: number };
export type LabelSym = { key: string; file: string; line: number };
export type BibSym = { key: string; type: string; title?: string; author?: string; year?: string; file: string; line: number };
export type Symbols = { outline: Heading[]; labels: LabelSym[]; bib: BibSym[] };

/** 读到的文本文件：mtime 是磁盘上的修改时间（毫秒），保存时带回去，用来认外部修改 */
export type FileText = { path: string; content: string; mtime: number | null };

/** 上手引导的两种：全新安装、升级（升级目前与全新安装同一套内容） */
export type OnboardingKind = "fresh" | "upgrade";
/** 「关于」：版本与本机环境（GET /api/about）。build / buildDate 是构建时的 git 提交号和提交日期 */
export type AboutInfo = {
  version: string;
  build: string | null;
  buildDate: string | null;
  os: string;
  arch: string;
  engine: string;
  bundledTex: boolean;
  texRoot: string | null;
  texVersion: string | null;
  home: string;
};

/** 第三方许可清单的一项（构建时由 scripts/gen-licenses.mjs 生成 /licenses.json） */
export type ThirdParty = { name: string; version: string; license: string; source: "npm" | "cargo" };

export type OnboardingInfo = { kind: OnboardingKind | null; version: string; previous: string | null };

export type MirrorPreset ={ id: string; url: string; region: "global" | "cn" };

export type SettingsInfo = {
  settings: Settings;
  presets: MirrorPreset[];
  env: {
    engine: string;
    bundledTex: boolean;
    library: string;
    defaultLibrary: string;
    libraryLocked: boolean;
    suboDir: string;
    defaultTimeout: number;
    timeoutRange: [number, number];
    /** 编译内存上限的默认值与范围（MB） */
    defaultMemory: number;
    memoryRange: [number, number];
    /** 本机逻辑核数、物理内存总量（MB，取不到为 null） */
    cpus: number;
    totalMemory: number | null;
    /** 并行编译数的范围，上限是本机核数 */
    parallelRange: [number, number];
    tex: {
      texRoot: string | null;
      texVersion: string | null;
      /** TeX Live 自己配置的源（「跟随 TeX Live」时用的） */
      tlmgrRepository: string | null;
      /** 主软件源已跨到新年度版本、这次运行中正在用的归档源 */
      archive: string | null;
      /** 内置 TeX 的用户层，补装的宏包在这里 */
    } | null;
  };
};

/** 设置页的一个引擎（服务端 engines.rs）：内置的两个不能卸；bytes 已装时是占用，没装时是下载体积 */
export type EngineStatus = {
  id: string;
  name: string;
  programs: Program[];
  builtin: boolean;
  installed: boolean;
  bytes: number;
  /** 离线导入要的容器文件名 */
  containers: string[];
};
/** managed 为 false：用的是本机自己的 TeX，引擎由 tlmgr 管，这里只看装没装 */
export type EnginesInfo = {
  managed: boolean;
  /** TeX Live 年份（内置或本机的）；读不到时为 null */
  year: number | null;
  /** 主软件源已跨年、这次运行中改用的归档源 */
  archive: string | null;
  platform: string | null;
  engines: EngineStatus[];
  staged: string[];
};
export type EngineInstalled = { id: string; bytes: number; archive: string | null };

/** bytes：占的磁盘空间，量不到为 null */
export type InstalledPackage = { name: string; desc: string; version: string; source: "tex" | "user"; bytes: number | null };
/** packages：用户层字体所属的宏包，卸载字体就是卸载它们；查不到时没有这一项 */
/** bytes：字体文件加起来的大小 */
/** previews：能给浏览器预览的样式（.ttc 集合里的不算） */
export type FontFamily = { name: string; styles: string[]; source: "tex" | "user" | "system"; packages?: string[]; bytes: number; previews?: string[] };

type Ev =
  | { type: "job.started"; jobId: string; entry: string }
  /** 编译中的终端输出，一块可能有好几行、不带结尾换行；实时展示用 */
  | { type: "job.output"; jobId: string; text: string }
  /** 完整日志，编译结束时发一次；以它为准 */
  | { type: "job.log"; jobId: string; text: string }
  /** 解析好的问题，紧跟 job.log 发 */
  | { type: "job.issues"; jobId: string; report: IssueReport }
  | { type: "job.progress"; jobId: string; step: "pass"; pass: number }
  | { type: "job.progress"; jobId: string; step: "pages"; pages: number }
  | { type: "job.progress"; jobId: string; step: "bib" | "index" | "glossary"; tool: string }
  | { type: "job.progress"; jobId: string; step: "pdf" }
  | { type: "job.done"; jobId: string; engine: string; ms: number }
  /** 这次编译的事件到此为止（之后不会再有它的 case.missing 等） */
  | { type: "job.end"; jobId: string }
  | { type: "job.failed"; jobId: string; engine: string; ms: number; error: string; reason: "timeout" | "memory" | "fileSize" | "failed" }
  | { type: "case.missing"; jobId: string; files: MissingFile[]; resolved: boolean }
  | { type: "compat.found"; jobId: string; fixes: CompatFix[] }
  | { type: "engine.missing"; jobId: string; program: Program; engine: string; name: string; bytes: number; managed: boolean }
  /** LaTeX 内核比宏包、模板旧：kernel 是本机内核日期，want 是文件要求的（日志里说了才有） */
  | { type: "tex.stale"; jobId: string; kernel: string | null; want: string | null; tex: TexStatus };

/** 事件流是全局的，每条都带 projectId */
export type JobEvent = Ev & { projectId: string };

/** 服务端错误：code 对应语言文件里的 server.<code>，message 已按当前语言渲染 */
export class ServerError extends Error {
  constructor(
    readonly status: number,
    readonly code: string | null,
    message: string,
    /** 服务端附带的参数（如命令日志 `log`） */
    readonly params: Vars = {}
  ) {
    super(message);
  }
}

/** 服务端返回 `{ code, params, message }`；认得 code 就按当前语言渲染，否则用它的中文兜底 message。
 *  不是这个形状的（代理、axum 自己的 413 之类）原样显示正文 */
export async function fail(r: Response): Promise<never> {
  const text = await r.text();
  let body: FailBody | null = null;
  try {
    body = JSON.parse(text);
  } catch {
    /* 不是 JSON */
  }
  const code = typeof body?.code === "string" ? body.code : null;
  const params = body?.params && typeof body.params === "object" ? (body.params as Vars) : undefined;
  throw new ServerError(r.status, code, failMessage(body) ?? (text || `${r.status} ${r.statusText}`), params);
}

/** 错误体按当前语言读成一句话；认不出的码用服务端的中文兜底，什么都没有返回 null */
export function failMessage(body: FailBody | null): string | null {
  const code = typeof body?.code === "string" ? body.code : null;
  const params = body?.params && typeof body.params === "object" ? (body.params as Vars) : undefined;
  const key = code && `server.${code}`;
  if (key && hasKey(key)) return t(key, params);
  return typeof body?.message === "string" ? body.message : null;
}

async function ok<T>(r: Response): Promise<T> {
  if (!r.ok) return fail(r);
  return r.json() as Promise<T>;
}

/** 项目名最多几个字，和服务端 `library::NAME_MAX_CHARS` 一致 */
export const NAME_MAX = 40;


/** AI 辅助：用户自己配的 OpenAI 兼容服务。Key 存系统钥匙串，接口从不返回它 */
export type AiKeyStore = "none" | "keychain" | "file";
export type AiService = { id: string; name: string; baseUrl: string; key: AiKeyStore };
/** name 是界面上的显示名，model 是请求里的模型 ID（原样透传） */
/** vision：能看图（用户勾选，或测试识图通过后自动勾上）。没勾的模型在 ⌘K 里不能附图 */
export type AiModel = { id: string; service: string; name: string; model: string; vision: boolean };
/** models 有先后：第一个是首选，用不了就依次往后换 */
export type AiConfig = { services: AiService[]; models: AiModel[]; verify: boolean };
export type AiInfo = { config: AiConfig; keychain: boolean; nameMax: number };
/** 测试连接：via 说明是靠 /models 还是靠一次极小的对话确认的；models 是服务列出的模型 ID */
export type AiTest = { ok: true; ms: number; via: "models" | "chat"; models: string[] };
export type AiMessage = { role: "system" | "user" | "assistant"; content: string };
export type AiRequest = { model?: string; messages: AiMessage[]; temperature?: number; maxTokens?: number };
/** 编译日志里的问题，由服务端解析（logparse.rs），界面只管渲染 */
export type IssueLevel = "error" | "warning" | "box";
export type ErrorKind = "here" | "scan" | "env" | "output" | "package" | "aux" | "end" | "driver";
export type WarningKind = "undefinedRef" | "undefinedCite" | "multiplyDefined" | "missingChar" | "floatTooLarge" | "font" | "bib" | "summary" | "other";
export type BoxKind = "overfullH" | "underfullH" | "overfullV" | "underfullV";
export type Issue = {
  /** 重编后不变（不含行号） */
  id: string;
  level: IssueLevel;
  kind: ErrorKind | WarningKind | BoxKind;
  /** TeX 的原话 */
  message: string;
  /** 项目内相对路径；出在发行版文件里或不知道是哪个文件时没有 */
  file?: string;
  line?: number;
  lineEnd?: number;
  /** 错误：TeX 停下处的源码，断开处标 ⟨here⟩ */
  here?: string;
  /** 错误：宏调用链，最里层在前（诊断编译才有） */
  chain?: string[];
  /** 错误：没闭合的参数从哪开始 */
  runaway?: string;
  /** 宏包或文档类名、biber / bibtex、缺字的字体 */
  source?: string;
  /** 引用标签、文献键、缺的字：同类按它聚合 */
  key?: string;
  pt?: number;
  /** 在原始日志里的行号范围（从 1 起，含两端） */
  logRange: [number, number];
};
/** errors 等是总数；issues 里的警告、排版提示可能因上限少于总数 */
export type IssueReport = { issues: Issue[]; errors: number; warnings: number; boxes: number };
/** TeX 停下处的断点标记 */
export const HERE_MARK = "⟨here⟩";

/** 诊断编译读出的第一处错误（diagnose.rs）。kind 决定服务端去源码哪里找证据 */
export type Diagnosis = {
  kind: ErrorKind;
  message: string;
  file: string | null;
  line: number | null;
  here: string | null;
  chain: string[];
  runaway: string | null;
};
export type AiDiagnose = { diagnosis: Diagnosis | null; timedOut: boolean; ms: number };
/** 修复 / ⌘K，提示词由服务端拼（assist.rs）。
 *  ⌘K 给原文片段（before / target / after）；修复给整份内容（text），上下文由服务端按错误类别挑 */
export type AssistRequest = {
  kind: "fix" | "edit";
  file: string;
  before?: string;
  target?: string;
  after?: string;
  text?: string;
  diagnosis?: Diagnosis;
  instruction?: string;
  preset?: string;
  /** context：出错处的源码（Issue.here） */
  error?: { message: string; line?: number; context?: string };
  model?: string;
  /** ⌘K 附的图（data URL） */
  images?: string[];
  /** 光标在数学环境里：只附图时让模型只写公式本身 */
  inMath?: boolean;
};
/** 排版异常（diagnose.rs）：内容比整页还高、浮动体比页面还大、一行超出版心太多 */
export type AiLayout = { kind: "pageOverflow" | "floatTooLarge" | "lineOverflow"; pt: number; line: number | null };
/** 编译校验：用建议改过的内容在影子副本里编一遍的结果；layout 是比上次编译成功时新冒出来的排版异常 */
export type AiVerify = { ok: boolean; timedOut: boolean; issues: Issue[]; ms: number; layout: AiLayout[] };

/** 流式生成：每收到一段文字回调一次，结束时返回所用模型的显示名。url 是 /api/ai/complete 或项目的 ai/assist。
 *  推理模型在正文之前的思考过程走 onThink，只给人看，不算进结果。
 *  一个模型用不了、服务端换下一个时走 onFallback：from、to 是显示名，reason 是前一个的错误（已按当前语言渲染，常带一长串底层原文，界面上不一定显示）。
 *  中途出错抛 ServerError（与普通接口同样按 server.<code> 渲染）；signal 中止时抛 AbortError，服务端随之断开 */
export type AiFallback = { from: string; to: string; reason: string };

export async function aiStream(
  url: string,
  req: unknown,
  onDelta: (text: string) => void,
  signal?: AbortSignal,
  onThink?: (text: string) => void,
  onFallback?: (f: AiFallback) => void
): Promise<{ model: string }> {
  const r = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(req),
    signal,
  });
  if (!r.ok || !r.body) return fail(r);
  const reader = r.body.pipeThrough(new TextDecoderStream()).getReader();
  let buf = "";
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    buf += value;
    let nl: number;
    while ((nl = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, nl).trim();
      buf = buf.slice(nl + 1);
      if (!line) continue;
      const ev = JSON.parse(line) as
        | { t: "delta"; text: string }
        | { t: "think"; text: string }
        | { t: "done"; model: string }
        | { t: "fallback"; from: string; to: string; code: string; params?: Vars; message: string }
        | { t: "error"; code: string; params?: Vars; message: string };
      const render = (e: { code: string; params?: Vars; message: string }) => {
        const key = `server.${e.code}`;
        return hasKey(key) ? t(key, e.params) : e.message;
      };
      if (ev.t === "delta") onDelta(ev.text);
      else if (ev.t === "think") onThink?.(ev.text);
      else if (ev.t === "done") return { model: ev.model };
      else if (ev.t === "fallback") onFallback?.({ from: ev.from, to: ev.to, reason: render(ev) });
      else throw new ServerError(502, ev.code, render(ev));
    }
  }
  // 流断了却没收到 done：服务进程中途退出之类
  throw new ServerError(502, "ai.interrupted", t("server.ai.interrupted"));
}

export const api = {
  projects: () => fetch("/api/projects").then((r) => ok<ProjectSummary[]>(r)),

  create: (name: string) =>
    fetch("/api/projects", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ name }),
    }).then((r) => ok<{ id: string }>(r)),

  /** 改名即改目录名，返回新 id */
  rename: (id: string, name: string) =>
    fetch(`/api/p/${encodeURIComponent(id)}/rename`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ name }),
    }).then((r) => ok<{ id: string }>(r)),

  /** 弹系统的选择文件夹对话框；取消时 path 为 null，系统不支持时抛错 */
  pickFolder: () =>
    fetch("/api/folders/pick", { method: "POST" }).then((r) => ok<{ path: string | null }>(r)),

  /** 登记本地文件夹，返回项目 id；已登记或在项目库里的返回原 id */
  openFolder: (path: string) =>
    fetch("/api/folders", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ path }),
    }).then((r) => ok<{ id: string }>(r)),

  /** 只从列表里移掉，磁盘上的文件夹不动 */
  forgetFolder: (id: string) =>
    fetch(`/api/folders/${encodeURIComponent(id)}`, { method: "DELETE" }).then((r) =>
      ok<{ ok: boolean }>(r)
    ),

  /** 归档 / 取消归档，文件不动 */
  archive: (id: string, archived: boolean) =>
    fetch(`/api/p/${encodeURIComponent(id)}/archive`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ archived }),
    }).then((r) => ok<{ archived: boolean }>(r)),

  /** path：在文件管理器里选中项目里的这个文件；不给就是项目文件夹 */
  reveal: (id: string, path?: string) =>
    fetch(`/api/p/${encodeURIComponent(id)}/reveal${path ? `?path=${encodeURIComponent(path)}` : ""}`, { method: "POST" }).then((r) =>
      ok<{ ok: boolean }>(r)
    ),

  /** 装缺字清单里的宏包；可能弹系统管理员授权框，耗时可达几分钟。project：连不上时记进待取队列用 */
  installPackages: (packages: string[], auto = false, project?: string) =>
    fetch("/api/case/install", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ packages, auto, project }),
    }).then((r) => ok<InstallResult>(r)),

  /** 取消正在进行的安装：只在下载完之前有效，开始解包后报 case.cancelTooLate */
  cancelInstall: () => fetch("/api/case/install/cancel", { method: "POST" }).then((r) => ok<{ ok: boolean }>(r)),

  queue: () => fetch("/api/case/queue").then((r) => ok<CaseQueue>(r)),

  /** 给队列里的文件查包名（要联网）；unresolved 是仍然查不了的文件数 */
  resolveQueue: () =>
    fetch("/api/case/queue/resolve", { method: "POST" }).then((r) => ok<{ queue: CaseQueue; unresolved: number }>(r)),

  removeFromQueue: (packages: string[], files: string[]) =>
    fetch("/api/case/queue/remove", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ packages, files }),
    }).then((r) => ok<CaseQueue>(r)),

  /** 用户层里补装的宏包全打成快照，浏览器直接下载 */
  snapshotUrl: () => "/api/case/snapshot",

  importSnapshot: (file: File) =>
    fetch("/api/case/snapshot/import", {
      method: "POST",
      headers: { "content-type": "application/octet-stream" },
      body: file,
    }).then((r) => ok<SnapshotImported>(r)),

  /** 正在进行的宏包安装进度；没在装时为 null */
  installProgress: () =>
    fetch("/api/case/install/progress").then((r) => ok<{ progress: InstallProgress | null }>(r)),

  settings: () => fetch("/api/settings").then((r) => ok<SettingsInfo>(r)),

  /** 这次启动该弹哪种上手引导；kind 为 null 表示当前版本已经看过 */
  onboarding: () => fetch("/api/onboarding").then((r) => ok<OnboardingInfo>(r)),

  about: () => fetch("/api/about").then((r) => ok<AboutInfo>(r)),

  /** 软件更新：状态、立即检查（查完才返回）、开始下载（立即返回，进度再轮询状态）、重启生效 */
  update: () => fetch("/api/update").then((r) => ok<UpdateInfo>(r)),
  checkUpdate: () => fetch("/api/update/check", { method: "POST" }).then((r) => ok<UpdateInfo>(r)),
  downloadUpdate: () => fetch("/api/update/download", { method: "POST" }).then((r) => ok<UpdateInfo>(r)),
  applyUpdate: () => fetch("/api/update/apply", { method: "POST" }).then((r) => ok<{ ok: boolean }>(r)),

  /** 静态文件，不走 /api：浏览器里直接跑 vite 时可能还没生成，调用方按空清单处理 */
  licenses: () => fetch("/licenses.json").then((r) => (r.ok ? (r.json() as Promise<ThirdParty[]>) : [])),

  /** 看完或跳过引导：服务端记下当前版本，之后不再自动弹 */
  onboardingDone: () => fetch("/api/onboarding/done", { method: "POST" }).then((r) => ok<{ ok: boolean }>(r)),

  /** 只带要改的字段；timeoutSecs 传 null 恢复默认 */
  saveSettings: (patch: Partial<Omit<Settings, "library">>) =>
    fetch("/api/settings", {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(patch),
    }).then((r) => ok<Settings>(r)),

  tex: () => fetch("/api/tex").then((r) => ok<TexStatus>(r)),

  /** 用户在别处更新过 TeX：重读内核日期，自动档跟着重选 */
  refreshTex: () => fetch("/api/tex/refresh", { method: "POST" }).then((r) => ok<TexStatus>(r)),

  /** 问软件源本机 tlmgr 旧不旧（几秒，要联网） */
  tlmgrStatus: () => fetch("/api/tex/tlmgr").then((r) => ok<TlmgrStatus>(r)),

  /** 更新 tlmgr 本身（tlmgr update --self）；目录要管理员权限的会弹系统授权框 */
  updateTlmgr: () => fetch("/api/tex/tlmgr/update", { method: "POST" }).then((r) => ok<InstallResult>(r)),

  /** 打开 MiKTeX Console（要管理员权限、要升级 MiKTeX 本体时交给它） */
  openMiktexConsole: () => fetch("/api/tex/miktex/console", { method: "POST" }).then((r) => ok<{ ok: boolean }>(r)),

  /** 更新 MiKTeX（内核和宏包一起），要几分钟。失败时 ServerError 的 code 是 tex.miktex.<原因>，params.log 是命令输出 */
  updateMiktex: () =>
    fetch("/api/tex/miktex/update", { method: "POST" }).then((r) => ok<{ ok: boolean; log: string; tex: TexStatus }>(r)),

  /** 换项目库位置；path 为 null 恢复默认。moveProjects 把现有项目一起搬过去 */
  setLibrary: (path: string | null, moveProjects: boolean) =>
    fetch("/api/settings/library", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ path, moveProjects }),
    }).then((r) => ok<{ library: string; moved: number; left: string[] }>(r)),

  ai: () => fetch("/api/ai").then((r) => ok<AiInfo>(r)),

  /** 整份替换服务与模型（不含 Key）；返回规范化后的配置 */
  saveAi: (config: { services: Omit<AiService, "key">[]; models: AiModel[]; verify: boolean }) =>
    fetch("/api/ai", {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(config),
    }).then((r) => ok<AiInfo>(r)),

  /** 设置 API Key；空串表示清除 */
  setAiKey: (service: string, key: string) =>
    fetch(`/api/ai/services/${encodeURIComponent(service)}/key`, {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ key }),
    }).then((r) => ok<AiInfo>(r)),

  testAi: (service: string, model?: string) =>
    fetch(`/api/ai/services/${encodeURIComponent(service)}/test`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ model }),
    }).then((r) => ok<AiTest>(r)),

  /** 测试识图：答对了服务端顺手把模型标成能看图，info 是更新后的配置 */
  testVision: (model: string) =>
    fetch(`/api/ai/models/${encodeURIComponent(model)}/vision`, { method: "POST" }).then((r) => ok<{ ok: true; ms: number; info: AiInfo }>(r)),

  /** 下载源测速：id → 毫秒，连不上为 null。最慢约 6 秒 */
  probeMirrors: (custom?: string) =>
    fetch("/api/settings/mirrors/probe", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ custom }),
    }).then((r) => ok<Record<string, number | null>>(r)),

  revealDir: (what: "library" | "subo" | "appearance") =>
    fetch("/api/settings/reveal", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ what }),
    }).then((r) => ok<{ ok: boolean }>(r)),

  /** 外观与语言包：已装的包（未经内容校验）、当前选择、所在目录 */
  appearance: () =>
    fetch("/api/appearance").then((r) =>
      ok<{ prefs: Record<string, string>; themes: unknown[]; locales: unknown[]; dir: string }>(r)
    ),

  saveAppearancePrefs: (patch: Record<string, string | null>) =>
    fetch("/api/appearance/prefs", {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(patch),
    }).then((r) => ok<Record<string, string>>(r)),

  /** 存一个配色方案 / 语言包，同 id 覆盖 */
  putPack: (kind: "theme" | "locale", id: string, pack: unknown) =>
    fetch(`/api/appearance/${kind}/${encodeURIComponent(id)}`, {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(pack),
    }).then((r) => ok<{ id: string }>(r)),

  removePack: (kind: "theme" | "locale", id: string) =>
    fetch(`/api/appearance/${kind}/${encodeURIComponent(id)}`, { method: "DELETE" }).then((r) => ok<{ removed: boolean }>(r)),

  engines: () => fetch("/api/engines").then((r) => ok<EnginesInfo>(r)),

  /** 装引擎：下载容器（或用导入区的文件）、生成格式，耗时几十秒 */
  installEngine: (id: string, source?: "import") =>
    fetch(`/api/engines/${encodeURIComponent(id)}/install`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ source }),
    }).then((r) => ok<EngineInstalled>(r)),

  uninstallEngine: (id: string) =>
    fetch(`/api/engines/${encodeURIComponent(id)}/uninstall`, { method: "POST" }).then((r) => ok<{ ok: boolean }>(r)),

  /** 离线导入：一次放一个容器文件进导入区，返回要它的引擎 */
  stageEngineFile: (file: File) =>
    fetch(`/api/engines/import/${encodeURIComponent(file.name)}`, { method: "POST", body: file }).then((r) =>
      ok<{ engines: string[] }>(r)
    ),

  packages: () => fetch("/api/case/packages").then((r) => ok<{ available: boolean; packages: InstalledPackage[] }>(r)),

  /** 卸载补装进用户层的宏包（只在内置 TeX 时可用） */
  removePackages: (packages: string[]) =>
    fetch("/api/case/packages/remove", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ packages }),
    }).then((r) => ok<{ ok: boolean; log: string }>(r)),

  /** 从安装源重新下载、覆盖补装的宏包；耗时可达几分钟 */
  reinstallPackages: (packages: string[]) =>
    fetch("/api/case/packages/reinstall", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ packages }),
    }).then((r) => ok<InstallResult>(r)),

  fonts: (rescan = false) => fetch(`/api/case/fonts${rescan ? "?rescan=1" : ""}`).then((r) => ok<FontFamily[]>(r)),
  fontFileUrl: (f: FontFamily, style: string) => `/api/case/fonts/file?${new URLSearchParams({ name: f.name, source: f.source, style })}`,

  templates: () => fetch("/api/templates").then((r) => ok<TemplateSummary[]>(r)),

  /** 把项目存为模板：只查形状（主文件、文件类型），不编译 */
  saveTemplate: (id: string, name: string) =>
    fetch(`/api/p/${encodeURIComponent(id)}/template`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ name }),
    }).then((r) => ok<TemplateStored>(r)),

  importTemplate: (file: File, name: string) =>
    fetch(`/api/templates/import?name=${encodeURIComponent(name)}`, {
      method: "POST",
      headers: { "content-type": "application/octet-stream" },
      body: file,
    }).then((r) => ok<TemplateStored>(r)),

  /** 用模板新建项目，返回项目 id */
  useTemplate: (id: string, name: string) =>
    fetch(`/api/templates/${encodeURIComponent(id)}/use`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ name }),
    }).then((r) => ok<{ id: string }>(r)),

  /** 改模板分类；预置模板只读 */
  setTemplateCategory: (id: string, category: TemplateCategory) =>
    fetch(`/api/templates/${encodeURIComponent(id)}`, {
      method: "PATCH",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ category }),
    }).then((r) => ok<{ ok: boolean }>(r)),

  removeTemplate: (id: string) =>
    fetch(`/api/templates/${encodeURIComponent(id)}`, { method: "DELETE" }).then((r) => ok<{ ok: boolean }>(r)),

  /** 直接把压缩包（zip / tar.gz）原始字节作为请求体，免去 multipart；格式由服务端按内容认 */
  importArchive: (file: File, name: string) =>
    fetch(`/api/projects/import?name=${encodeURIComponent(name)}`, {
      method: "POST",
      headers: { "content-type": "application/octet-stream" },
      body: file,
    }).then((r) => ok<{ id: string }>(r)),
};

function fsPost<T>(url: string, body: unknown): Promise<T> {
  return fetch(url, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) }).then((r) => ok<T>(r));
}

/** 单个项目范围内的接口 */
export function projectApi(id: string) {
  const base = `/api/p/${encodeURIComponent(id)}`;
  // save 的参数也叫 base（修改时间），这里留一个不会被遮住的名字
  const base_ = base;
  return {
    project: () => fetch(`${base}/project`).then((r) => ok<ProjectInfo>(r)),

    /** 修复 / ⌘K 的流式接口，交给 aiStream */
    assistUrl: `${base}/ai/assist`,

    /** 报错修复的第一步：诊断编译，读出错误和宏调用链 */
    diagnoseAi: (file: string, text: string, signal?: AbortSignal) =>
      fetch(`${base}/ai/diagnose`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ file, text }),
        signal,
      }).then((r) => ok<AiDiagnose>(r)),

    verifyAi: (file: string, text: string, signal?: AbortSignal) =>
      fetch(`${base}/ai/verify`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ file, text }),
        signal,
      }).then((r) => ok<AiVerify>(r)),

    file: (path: string) => fetch(`${base}/file?path=${encodeURIComponent(path)}`).then((r) => ok<FileText>(r)),

    /** base：读到时的修改时间，磁盘上的对不上（被外部改过）报 409 file.changedOnDisk；
     *  keep：写之前把磁盘上的旧内容无条件记进历史 */
    save: (path: string, content: string, base?: number | null, keep = false) =>
      fetch(`${base_}/file`, {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ path, content, base: base ?? undefined, keep }),
      }).then((r) => ok<{ ok: boolean; mtime: number | null }>(r)),

    /** 文件树的文件操作：已存在报 409 file.exists；删除是移到废纸篓 */
    newFile: (path: string, dir: boolean) => fsPost<{ path: string }>(`${base_}/fs/new`, { path, dir }),
    renameFile: (from: string, to: string) => fsPost<{ path: string }>(`${base_}/fs/rename`, { from, to }),
    duplicateFile: (path: string, suffix: string) => fsPost<{ path: string }>(`${base_}/fs/duplicate`, { path, suffix }),
    deleteFile: (path: string) => fsPost<{ ok: boolean }>(`${base_}/fs/delete`, { path }),

    /** 当前文件的修改时间、文件树指纹（外部增删文件时会变） */
    stat: (path: string | null) =>
      fetch(`${base_}/stat${path ? `?path=${encodeURIComponent(path)}` : ""}`).then((r) =>
        ok<{ mtime: number | null; tree: string }>(r)
      ),

    history: (path: string) =>
      fetch(`${base_}/history?path=${encodeURIComponent(path)}`).then((r) => ok<{ versions: HistoryVersion[] }>(r)),

    historyVersion: (path: string, id: number) =>
      fetch(`${base_}/history/${id}?path=${encodeURIComponent(path)}`).then((r) => ok<{ id: number; content: string }>(r)),

    /** 把一份内容记进历史（整篇换掉编辑器内容之前） */
    keepHistory: (path: string, content: string) =>
      fetch(`${base_}/history`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ path, content }),
      }).then((r) => ok<{ id: number | null }>(r)),

    symbols: () => fetch(`${base_}/symbols`).then((r) => ok<Symbols>(r)),

    /** 导出这个项目用到的补装宏包，存进项目根目录 */
    exportSnapshot: () =>
      fetch(`${base_}/snapshot`, { method: "POST" }).then((r) =>
        ok<{ path: string; packages: string[]; files: number; bytes: number }>(r)
      ),

    /** 导入项目里带着的快照 */
    importSnapshot: (path: string) =>
      fetch(`${base_}/snapshot/import?path=${encodeURIComponent(path)}`, { method: "POST" }).then((r) =>
        ok<SnapshotImported>(r)
      ),

    /** 附件原样读取（图片、字体、PDF） */
    rawUrl: (path: string) => `${base}/raw?path=${encodeURIComponent(path)}&t=${Date.now()}`,

    build: () => fetch(`${base}/build`, { method: "POST" }).then((r) => ok<{ queued: boolean }>(r)),

    engine: () => fetch(`${base}/engine`).then((r) => ok<EngineInfo>(r)),

    /** 选编译引擎，`auto` 为按源码探测；返回新的状态 */
    setEngine: (mode: EngineMode) =>
      fetch(`${base}/engine`, {
        method: "PUT",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ mode }),
      }).then((r) => ok<EngineInfo>(r)),

    /** 修一处已知不兼容；改完不自动重编 */
    applyCompat: (fix: CompatFix) =>
      fetch(`${base}/compat`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ id: fix.id, file: fix.file }),
      }).then((r) => ok<{ ok: boolean; changed: number }>(r)),

    pdfUrl: (jobId: string) => `${base}/pdf/${jobId}?t=${Date.now()}`,

    /** 把这次编译的 PDF 存到项目根目录，与入口同名；返回相对项目的路径 */
    exportPdf: (jobId: string) =>
      fetch(`${base}/export/${jobId}`, { method: "POST" }).then((r) => ok<{ path: string }>(r)),

    /** SyncTeX 源码 → 预览 */
    syncView: (jobId: string, file: string, line: number) =>
      fetch(`${base}/synctex/${jobId}/view?file=${encodeURIComponent(file)}&line=${line}`).then((r) => ok<Spot>(r)),

    /** SyncTeX 预览 → 源码 */
    syncEdit: (jobId: string, page: number, x: number, y: number) =>
      fetch(`${base}/synctex/${jobId}/edit?page=${page}&x=${x.toFixed(2)}&y=${y.toFixed(2)}`).then((r) =>
        ok<{ file: string; line: number }>(r)
      ),
  };
}
