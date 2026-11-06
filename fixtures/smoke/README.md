# 冒烟文档

`scripts/bundled-smoke.sh` 用随应用内置的精简 TeX Live 逐个编译这些文档：
缺宏包就走缺字清单查包、安装、重编，直到通过或确定装不上。
每份文档覆盖一类真实项目里常见、但精简版没带的宏包组合。
编译通过后还会看最后一遍（按自动探测出的引擎）的日志：引用、术语表没解析出来也算不通过。

| 目录 | 覆盖 |
| --- | --- |
| `resume-ats` | `\input{glyphtounicode}`（pdftex 包，不可重定位）、marvosym、lato、fontawesome5 |
| `pdflatex-style` | 为 pdfLaTeX 写的导言区：`\pdfgentounicode`、fontenc T1、lmodern、microtype、times |
| `science` | tikz、pgfplots、siunitx、booktabs、biblatex + biber |
| `natbib` | natbib + bibtex（plainnat） |
| `bibtex-style` | bibtex 用非内置的 `.bst`（urlbst 的 unsrturl）：编译成功但引用是 [?]，缺字清单也要报 |
| `biblatex-style` | biblatex 用非内置的样式（biblatex-ieee 的 `ieee.bbx` / `ieee.cbx`） |
| `ctex` | ctex 中文排版 |
| `beamer` | beamer + metropolis 主题 |
| `glossary-code` | glossaries、listings、algorithm2e、cleveref |
