# 参与贡献

感谢你愿意改进素帛。Issue 和 PR 用中文或英文都可以。
*Issues and pull requests are welcome in Chinese or English.*

## 先说清楚的几件事

- **素帛是离线桌面软件**（本地服务 + 前端），不是 Web 应用。新功能默认不联网；
  确实要联网的（补装宏包、更新、AI），必须由用户触发或可以关掉。
- **不在计划里的**：多人实时协作、账号与云端项目。这类 PR 不会合入，动手前可以先开 issue 讨论。
- 较大的改动（新模块、改接口、改命令行输出格式）请先开 issue 对齐方向，免得白做。

## 搭开发环境

需要 Node.js 20、Rust stable。要真正编译文档还需要一份 TeX Live（`xelatex` 在 PATH 里）；
只改界面的话可以不装，服务会退回 `mock` 引擎生成占位 PDF。

```bash
(cd app && npm ci)
(cd desktop && npm ci)     # 仅桌面版需要
./scripts/dev.sh           # 构建前端 + 服务，打开 http://127.0.0.1:8787
```

桌面版用 `./scripts/desktop.sh`。要打带内置 TeX 的安装包，先用
`./scripts/texlive-trim.sh <完整 TeX Live 根目录>` 生成 `desktop/texlive`。
Linux 的系统库见 [CI 配置](.github/workflows/desktop.yml)。

自测时用临时目录隔离，别读写自己真实的 `~/.subo` 和项目库：

```bash
S=$(mktemp -d)
SUBO_HOME=$S/subo server/target/debug/subo-server --home $S/home --port 0 --dist app/dist
```

更多参数、环境变量和本地接口见 [docs/cli.md](docs/cli.md)，整体设计见 [docs/architecture.md](docs/architecture.md)。

## 提交前必跑

改了前端（`app/`）：

```bash
cd app && npx tsc --noEmit && npm run check:i18n
```

改了服务端（`server/`）：

```bash
cd server && cargo test
```

动了内置 TeX Live 的裁剪清单或缺包识别：用 `scripts/bundled-smoke.sh` 跑一遍
[冒烟文档](fixtures/smoke/README.md)。

CI 只在手动触发时打包，PR 不会自动跑，所以请在本地跑完上面的检查，并在 PR 里写明跑了哪些。

## 代码约定

- **界面多语言是硬约束**：前端不写任何给人看的自然语言，一律 `t("分组.键")`。
  文案放 `app/src/i18n/locales/`，zh-CN 为基准，**所有内置语言**都要补齐（缺键 `tsc` 会报错）。
  不会某种语言时可以先放英文，在 PR 里注明，维护者再补译。详见 [docs/i18n.md](docs/i18n.md)。
- **服务端错误**用 `Fail::new("分组.键", "中文兜底")`，并在 `locales/*.ts` 的 `server` 分组补同名键。
  错误码发布后不改名。
- **命令行**（`server/src/cli.rs`）：输入输出保持简单，`--json` 只有平铺字段，没有嵌套对象和对象数组；
  出错时用 `message` + `hint`/`next` 说清哪里坏了、为什么、怎么修；退出码 0/1/2/3 的含义不变。
- 编译产物和应用状态不写进用户的项目目录。
- 改了接口、命令行或数据布局，同步改 `docs/` 里对应的文档。

## 提 PR

- 一个 PR 只做一件事，标题说清改了什么。
- 界面改动附截图（浅色、深色各一张更好）。
- 修 bug 时尽量补一个能复现它的测试；编译日志相关的问题可以把日志样本放进 `server/testdata/`
  （去掉其中的本机路径和个人信息）。

## 报告问题

提 issue 时请附上：系统与版本、素帛版本（「关于」里的「复制诊断信息」）、能复现问题的最小 `.tex`、
相关的编译日志。**安全问题不要公开提 issue**，见 [SECURITY.md](SECURITY.md)。

## 许可

提交的贡献按 [Apache License 2.0](LICENSE) 授权，与项目一致。
