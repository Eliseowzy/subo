//! arXiv 源码包：gzip 过的 tar，个别论文只有一个 gzip 过的 .tex；也收不压缩的 tar。
//!
//! 自己解 tar，不调系统 `tar`：各平台的 tar 参数、清单格式都不一样，炸弹也不好挡。
//! 只认普通文件和目录，链接、设备文件一律跳过；边解边记体积和个数，超了就停。

use crate::fail::Fail;
use flate2::{read::MultiGzDecoder, write::GzEncoder, Compression};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

const BLOCK: usize = 512;
/// GNU 长文件名、pax 扩展头的数据块最多这么大，正常只有几百字节
const META_MAX: u64 = 1 << 20;

pub fn is_gzip(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x1f, 0x8b])
}

/// 看头块校验和认 tar：老式 v7 tar 没有 `ustar` 魔数，校验和人人都有。
pub fn is_tar(block: &[u8]) -> bool {
    if block.len() < BLOCK || block.iter().all(|&b| b == 0) {
        return false;
    }
    let Some(want) = octal(&block[148..156]) else { return false };
    let sum: u64 = block
        .iter()
        .enumerate()
        .map(|(i, &b)| if (148..156).contains(&i) { b' ' as u64 } else { b as u64 })
        .sum();
    sum == want
}

/// 解到 `dest`（已存在的空目录）。gzip 里不是 tar 时按单个 .tex 存成 `main.tex`。
pub fn extract(bytes: &[u8], dest: &Path, max_bytes: u64, max_files: u64) -> Result<(), Fail> {
    let mut r: Box<dyn Read + '_> = if is_gzip(bytes) { Box::new(MultiGzDecoder::new(bytes)) } else { Box::new(bytes) };
    let mut first = [0u8; BLOCK];
    let n = read_full(&mut r, &mut first).map_err(unpack_failed)?;
    if n == BLOCK && is_tar(&first) {
        return untar(first, r, dest, max_bytes, max_files);
    }
    if !is_gzip(bytes) {
        return Err(not_archive());
    }
    // arXiv 上只有一个文件的投稿，源码就是 gzip 过的 .tex；没有源码的给的是 PDF
    let mut body = first[..n].to_vec();
    r.take(max_bytes + 1 - n as u64).read_to_end(&mut body).map_err(unpack_failed)?;
    if body.len() as u64 > max_bytes {
        return Err(too_big(body.len() as u64, max_bytes));
    }
    if body.starts_with(b"%PDF") {
        return Err(Fail::new("import.pdfOnly", "压缩包里只有 PDF，没有 LaTeX 源码"));
    }
    fs::write(dest.join("main.tex"), body).map_err(unpack_failed)
}

/// 写 tar.gz（字盘快照用）：只写普通文件，名字超过 100 字节时先写一项 GNU 长文件名，
/// 与 [`extract`] 认的一致
pub struct Writer<W: Write> {
    gz: GzEncoder<W>,
}

impl<W: Write> Writer<W> {
    pub fn new(w: W) -> Self {
        Writer { gz: GzEncoder::new(w, Compression::default()) }
    }

    pub fn add(&mut self, name: &str, data: &[u8]) -> io::Result<()> {
        let bytes = name.as_bytes();
        if bytes.len() > 100 {
            let mut long = bytes.to_vec();
            long.push(0);
            self.gz.write_all(&write_header("././@LongLink", long.len() as u64, b'L'))?;
            self.write_padded(&long)?;
        }
        self.gz.write_all(&write_header(name, data.len() as u64, b'0'))?;
        self.write_padded(data)
    }

    fn write_padded(&mut self, data: &[u8]) -> io::Result<()> {
        self.gz.write_all(data)?;
        self.gz.write_all(&vec![0u8; pad(data.len() as u64) as usize])
    }

    /// 写结尾的两个零块，收尾压缩流
    pub fn finish(mut self) -> io::Result<W> {
        self.gz.write_all(&[0u8; 2 * BLOCK])?;
        self.gz.finish()
    }
}

/// ustar 头。名字截到 100 字节（长名字前面已经有 GNU 长文件名项）；时间写 0，同样的内容打出同样的包
fn write_header(name: &str, size: u64, kind: u8) -> [u8; BLOCK] {
    let mut h = [0u8; BLOCK];
    let n = name.as_bytes();
    let n = &n[..n.len().min(100)];
    h[..n.len()].copy_from_slice(n);
    h[100..107].copy_from_slice(b"0000644");
    h[108..115].copy_from_slice(b"0000000");
    h[116..123].copy_from_slice(b"0000000");
    h[124..135].copy_from_slice(format!("{size:011o}").as_bytes());
    h[136..147].copy_from_slice(b"00000000000");
    h[156] = kind;
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    h[148..156].fill(b' ');
    let sum: u32 = h.iter().map(|&b| b as u32).sum();
    h[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
    h
}

fn untar(
    mut head: [u8; BLOCK],
    mut r: impl Read,
    dest: &Path,
    max_bytes: u64,
    max_files: u64,
) -> Result<(), Fail> {
    let (mut bytes, mut files) = (0u64, 0u64);
    // GNU 长文件名（`L`）和 pax（`x`）给的是下一项的真名
    let mut long_name: Option<String> = None;
    let mut names = crate::library::EntryNames::default();
    loop {
        if head.iter().all(|&b| b == 0) {
            return Ok(());
        }
        if !is_tar(&head) {
            return Err(Fail::new("import.unpackFailed", "解压失败：tar 头损坏").with("detail", "bad header"));
        }
        files += 1;
        if files > max_files {
            return Err(Fail::new("import.tooMany", format!("文件超过上限 {max_files} 个"))
                .with("count", files)
                .with("max", max_files));
        }
        let size = size(&head[124..136]).ok_or_else(|| Fail::new("import.unpackFailed", "解压失败：tar 头损坏").with("detail", "bad size"))?;
        bytes += size;
        if bytes > max_bytes {
            return Err(too_big(bytes, max_bytes));
        }
        let kind = head[156];
        match kind {
            b'L' | b'x' => {
                if size > META_MAX {
                    return Err(Fail::new("import.unpackFailed", "解压失败：tar 头损坏").with("detail", "oversized header"));
                }
                let mut data = Vec::new();
                (&mut r).take(size).read_to_end(&mut data).map_err(unpack_failed)?;
                skip(&mut r, pad(size))?;
                let name = if kind == b'L' { Some(cstr(&data)) } else { pax_path(&data) };
                if name.is_some() {
                    long_name = name;
                }
            }
            b'0' | b'\0' | b'7' | b'5' => {
                let name = long_name.take().unwrap_or_else(|| header_name(&head));
                let target = safe_join(dest, &name);
                if kind == b'5' {
                    if let Some(t) = &target {
                        fs::create_dir_all(t).map_err(unpack_failed)?;
                    }
                } else if let Some(t) = target {
                    names.check(t.strip_prefix(dest).unwrap_or(&t))?;
                    if let Some(parent) = t.parent() {
                        fs::create_dir_all(parent).map_err(unpack_failed)?;
                    }
                    let mut f = fs::File::create(&t).map_err(unpack_failed)?;
                    let copied = io::copy(&mut (&mut r).take(size), &mut f).map_err(unpack_failed)?;
                    if copied < size {
                        return Err(truncated());
                    }
                    skip(&mut r, pad(size))?;
                } else {
                    // 带 `..` 或绝对路径的项不落盘，跳过数据接着解
                    skip(&mut r, size + pad(size))?;
                }
            }
            // 链接、设备、全局 pax 头等：只跳过数据
            _ => {
                long_name = None;
                skip(&mut r, size + pad(size))?;
            }
        }
        match read_full(&mut r, &mut head).map_err(unpack_failed)? {
            BLOCK => {}
            // 没写结尾的两个零块也算完
            0 => return Ok(()),
            _ => return Err(truncated()),
        }
    }
}

/// 名字落到 `dest` 里才给路径：去掉开头的 `/` 和 `./`，带 `..` 的不要。
fn safe_join(dest: &Path, name: &str) -> Option<PathBuf> {
    let mut out = dest.to_path_buf();
    let mut any = false;
    for c in Path::new(name.trim_start_matches('/')).components() {
        match c {
            Component::Normal(p) => {
                out.push(p);
                any = true;
            }
            Component::CurDir => {}
            _ => return None,
        }
    }
    any.then_some(out)
}

fn header_name(h: &[u8; BLOCK]) -> String {
    let name = cstr(&h[0..100]);
    let prefix = if &h[257..262] == b"ustar" { cstr(&h[345..500]) } else { String::new() };
    if prefix.is_empty() {
        name
    } else {
        format!("{prefix}/{name}")
    }
}

/// pax 记录形如 `30 path=dir/very/long/name.tex\n`
fn pax_path(data: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(data);
    text.lines().find_map(|l| l.split_once(' ')?.1.strip_prefix("path=").map(str::to_string))
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

fn octal(field: &[u8]) -> Option<u64> {
    let s = cstr(field);
    let s = s.trim_matches([' ', '\0']);
    if s.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(s, 8).ok()
}

/// 超过 8 GB 的文件 GNU tar 用 base-256 写大小（首字节高位置 1）
fn size(field: &[u8]) -> Option<u64> {
    if field[0] & 0x80 != 0 {
        return field[1..].iter().try_fold(0u64, |acc, &b| acc.checked_mul(256).map(|v| v + b as u64));
    }
    octal(field)
}

fn pad(size: u64) -> u64 {
    (BLOCK as u64 - size % BLOCK as u64) % BLOCK as u64
}

fn skip(r: &mut impl Read, n: u64) -> Result<(), Fail> {
    let got = io::copy(&mut r.take(n), &mut io::sink()).map_err(unpack_failed)?;
    if got < n {
        return Err(truncated());
    }
    Ok(())
}

fn read_full(r: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

fn unpack_failed(e: io::Error) -> Fail {
    Fail::new("import.unpackFailed", format!("解压失败: {e}")).detail(e)
}

fn truncated() -> Fail {
    Fail::new("import.unpackFailed", "解压失败：压缩包不完整").with("detail", "truncated")
}

fn not_archive() -> Fail {
    Fail::new("import.notArchive", "不认识的压缩包：支持 zip、tar.gz")
}

fn too_big(bytes: u64, max: u64) -> Fail {
    let (mb, max) = (bytes >> 20, max >> 20);
    Fail::new("import.tooBig", format!("解压后 {mb} MB，超过上限 {max} MB"))
        .with("mb", mb)
        .with("max", max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "subo-tar-{tag}-{}",
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn header(name: &str, size: usize, kind: u8) -> [u8; BLOCK] {
        let mut h = [0u8; BLOCK];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..107].copy_from_slice(b"0000644");
        h[124..135].copy_from_slice(format!("{size:011o}").as_bytes());
        h[156] = kind;
        h[257..263].copy_from_slice(b"ustar\0");
        h[148..156].fill(b' ');
        let sum: u32 = h.iter().map(|&b| b as u32).sum();
        h[148..155].copy_from_slice(format!("{sum:06o}\0").as_bytes());
        h
    }

    fn tar(entries: &[(&str, &[u8], u8)]) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, data, kind) in entries {
            out.extend_from_slice(&header(name, data.len(), *kind));
            out.extend_from_slice(data);
            out.resize(out.len() + pad(data.len() as u64) as usize, 0);
        }
        out.resize(out.len() + 2 * BLOCK, 0);
        out
    }

    fn gz(data: &[u8]) -> Vec<u8> {
        let mut e = GzEncoder::new(Vec::new(), Compression::fast());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn untars_gzip_and_skips_escapes() {
        let d = scratch("ok");
        let long = format!("{}/x.tex", "d".repeat(120));
        let t = tar(&[
            ("./figs/", b"", b'5'),
            ("./main.tex", b"\\documentclass{article}", b'0'),
            ("figs/a.png", b"png", b'0'),
            ("../evil.tex", b"no", b'0'),
            ("/abs.tex", b"abs", b'0'),
            ("link", b"", b'2'),
            ("././@LongLink", long.as_bytes(), b'L'),
            ("ignored", b"long", b'0'),
        ]);
        extract(&gz(&t), &d, 1 << 20, 100).unwrap();
        assert!(fs::read_to_string(d.join("main.tex")).unwrap().starts_with("\\documentclass"));
        assert_eq!(fs::read(d.join("figs/a.png")).unwrap(), b"png");
        assert_eq!(fs::read(d.join(&long)).unwrap(), b"long");
        assert_eq!(fs::read(d.join("abs.tex")).unwrap(), b"abs");
        assert!(!d.parent().unwrap().join("evil.tex").exists());
        assert!(!d.join("link").exists());
        // 不压缩的 tar 也收
        let d2 = scratch("plain");
        extract(&t, &d2, 1 << 20, 100).unwrap();
        assert!(d2.join("main.tex").is_file());
        let _ = fs::remove_dir_all(&d);
        let _ = fs::remove_dir_all(&d2);
    }

    #[test]
    fn case_clash_rejected_but_repeats_allowed() {
        let d = scratch("case");
        // 同一个名字出现两次（tar 追加更新常见）以后一个为准；`./` 开头的写法算同一个名字
        let t = tar(&[("main.tex", b"1", b'0'), ("./main.tex", b"2", b'0')]);
        extract(&t, &d, 1 << 20, 100).unwrap();
        assert_eq!(fs::read(d.join("main.tex")).unwrap(), b"2");
        let d2 = scratch("case2");
        let t = tar(&[("Fig.png", b"1", b'0'), ("fig.png", b"2", b'0')]);
        assert_eq!(extract(&t, &d2, 1 << 20, 100).unwrap_err().code, "import.caseClash");
        let _ = fs::remove_dir_all(&d);
        let _ = fs::remove_dir_all(&d2);
    }

    #[test]
    fn writer_round_trips_long_names() {
        let long = format!("tex/latex/{}/x.sty", "p".repeat(150));
        let mut w = Writer::new(Vec::new());
        w.add("subo-case.json", b"{}").unwrap();
        w.add(&long, b"\\ProvidesPackage{x}").unwrap();
        w.add("empty", b"").unwrap();
        let bytes = w.finish().unwrap();
        let d = scratch("writer");
        extract(&bytes, &d, 1 << 20, 100).unwrap();
        assert_eq!(fs::read(d.join("subo-case.json")).unwrap(), b"{}");
        assert_eq!(fs::read(d.join(&long)).unwrap(), b"\\ProvidesPackage{x}");
        assert_eq!(fs::read(d.join("empty")).unwrap(), b"");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn single_gzipped_tex_becomes_main() {
        let d = scratch("single");
        extract(&gz(b"\\documentclass{article}\n"), &d, 1 << 20, 100).unwrap();
        assert!(d.join("main.tex").is_file());
        assert_eq!(extract(&gz(b"%PDF-1.5 ..."), &scratch("pdf"), 1 << 20, 100).unwrap_err().code, "import.pdfOnly");
        assert_eq!(extract(b"hello", &scratch("junk"), 1 << 20, 100).unwrap_err().code, "import.notArchive");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn limits_stop_bombs() {
        let big = vec![0u8; 4096];
        let t = tar(&[("a.tex", &big, b'0')]);
        assert_eq!(extract(&gz(&t), &scratch("big"), 1024, 100).unwrap_err().code, "import.tooBig");
        let many = tar(&[("a", b"", b'0'), ("b", b"", b'0'), ("c", b"", b'0')]);
        assert_eq!(extract(&gz(&many), &scratch("many"), 1 << 20, 2).unwrap_err().code, "import.tooMany");
        // 单个 gzip 文件解出来超限
        assert_eq!(extract(&gz(&vec![b'x'; 4096]), &scratch("gzbig"), 1024, 100).unwrap_err().code, "import.tooBig");
        let mut cut = gz(&t);
        cut.truncate(cut.len() / 2);
        assert!(extract(&cut, &scratch("cut"), 1 << 20, 100).is_err());
    }
}
