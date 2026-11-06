/** 能导入成项目、模板的压缩包：Overleaf 导出的 zip，arXiv 下载的源码包（tar.gz）。
 *  服务端按内容认格式，这里只按文件名挡掉明显不对的文件。 */

/** 文件选择框的过滤；arXiv 早年的源码包没有扩展名，拖进窗口照样能导 */
export const ARCHIVE_ACCEPT = ".zip,.gz,.tgz,.tar,application/zip,application/gzip,application/x-gzip,application/x-tar";

const EXT = /\.(zip|tar\.gz|tgz|tar|gz)$/i;
/** arXiv 编号：2401.12345、2401.12345v2、arXiv-2401.12345v2 */
const ARXIV_ID = /^(arxiv-)?\d{4}\.\d{4,5}(v\d+)?$/i;

export const isArchive = (name: string) => EXT.test(name) || ARXIV_ID.test(name);

/** 去掉扩展名，当默认项目名 */
export const archiveBase = (name: string) => name.replace(EXT, "");
