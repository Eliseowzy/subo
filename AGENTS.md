# Subo 开发约定

- 交付形态是离线桌面软件（本地服务 + 前端），不要默认联网或按 Web 应用部署。
- **界面多语言是硬约束**：前端不写任何给人看的自然语言，一律 `t("分组.键")`，文案放
  `app/src/i18n/locales/`（zh-CN 为基准，其他语言必须同步补齐）。新增页面就在语言文件里新开一组。
  完整规范见 [docs/i18n.md](docs/i18n.md)；`npm run build` 会跑 `check:i18n` 拦硬编码文案。
- 改完前端至少跑：`cd app && npx tsc --noEmit && npm run check:i18n`。
- 要在终端里编译文档、补装宏包、新建项目，用命令行 `subo-server build <目录> --install`
  （装了素帛的机器上是 `subo`），说明见 [docs/cli.md](docs/cli.md)；本地 HTTP 接口也在那里。
- 改命令行（`server/src/cli.rs`）守住：输入输出简单（`--json` 只有平铺字段，没有嵌套对象和对象数组），
  出错时说清哪里坏了、为什么、怎么修（`message` + `hint`/`next`），退出码 0/1/2/3 含义不变。
