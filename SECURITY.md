# 安全策略 / Security Policy

## 报告漏洞

**请不要在公开 issue 里报告安全问题。** 请使用 GitHub 的私密漏洞报告：
在本仓库的 **Security** 页点 **Report a vulnerability**。

请尽量附上：受影响的版本与系统、复现步骤或最小样例（如一个 `.tex` 或压缩包）、你判断的影响范围。
我们会尽快确认收到，修复后在更新说明里致谢（如你愿意）。

*Please do not report security issues in public issues. Use GitHub's private vulnerability reporting
(**Security** tab → **Report a vulnerability**) with the affected version, platform, reproduction steps and impact.*

## 支持的版本

素帛处于早期预览阶段，只为**最新发布的版本**修复安全问题。

## 关注的范围

素帛在本机编译用户的文档，并可能打开来源不明的项目。下面这些属于安全问题：

- **逃出项目目录**：通过路径、符号链接、压缩包条目（zip、tar、arXiv 源码包、模板、字盘快照）读写项目之外的文件
- **本地接口被滥用**：其他网页或本机其他用户通过 `127.0.0.1` 上的本地服务读写文件、触发编译或安装
- **机密泄露**：AI 服务的 API Key 出现在接口响应、日志、配置文件或进程参数里
- **更新与安装**：绕过更新包签名校验，或让补装宏包、导入快照写入预期之外的位置
- **资源上限失效**：编译超时、内存与输出文件上限、导入体积上限被绕过，导致拖垮整台机器

下面这些通常**不算**：

- 文档本身能执行 TeX 允许的操作（在编译目录内读写、受限的 shell-escape 等），这是 LaTeX 的正常行为
- 需要攻击者已经能在本机以你的身份执行任意程序的情形
- 你自己配置的第三方 AI 服务、下载镜像本身的问题
