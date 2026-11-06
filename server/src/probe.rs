//! 「测试识图」用的小图：白底黑字写一个三位数，现场画、每次不同，模型只能真看图才答得对。
//!
//! 字形是 5×7 点阵放大，PNG 自己编码（灰度、zlib），不为这一张图引入图像库。

use flate2::{write::ZlibEncoder, Compression, Crc};
use std::io::Write;

/// 0–9 的 5×7 点阵，每行低 5 位，最高位在左
const DIGITS: [[u8; 7]; 10] = [
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
    [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110],
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
];

/// 点阵放大倍数、字间距、四周留白（像素）
const SCALE: usize = 10;
const GAP: usize = 20;
const PAD: usize = 30;

/// 一个三位数，取自当前时间
pub fn number() -> String {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    format!("{}", 100 + n % 900)
}

/// 把一串数字画成 PNG，返回 `data:image/png;base64,…`
pub fn data_url(digits: &str) -> String {
    format!("data:image/png;base64,{}", base64(&png(digits)))
}

fn png(digits: &str) -> Vec<u8> {
    let ds: Vec<usize> = digits.bytes().filter(u8::is_ascii_digit).map(|b| (b - b'0') as usize).collect();
    let n = ds.len().max(1);
    let w = PAD * 2 + n * 5 * SCALE + (n - 1) * GAP;
    let h = PAD * 2 + 7 * SCALE;
    // 每行前面一个字节是 PNG 的行过滤类型（0：不过滤）
    let mut raw = vec![255u8; (w + 1) * h];
    for y in 0..h {
        raw[y * (w + 1)] = 0;
    }
    for (i, d) in ds.iter().enumerate() {
        let x0 = PAD + i * (5 * SCALE + GAP);
        for (row, bits) in DIGITS[*d].iter().enumerate() {
            for col in 0..5 {
                if bits & (0b10000 >> col) == 0 {
                    continue;
                }
                for dy in 0..SCALE {
                    let y = PAD + row * SCALE + dy;
                    let start = y * (w + 1) + 1 + x0 + col * SCALE;
                    raw[start..start + SCALE].fill(0);
                }
            }
        }
    }
    let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
    z.write_all(&raw).expect("写内存不会失败");
    let idat = z.finish().expect("写内存不会失败");

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    // 位深 8、灰度、默认压缩、默认过滤、不隔行
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &idat);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = Crc::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.sum().to_be_bytes());
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= c.len() {
                s.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn png_is_well_formed() {
        let p = png("583");
        assert!(p.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(p.ends_with(&[0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82]));
        assert!(data_url("583").starts_with("data:image/png;base64,iVBORw0KGgo"));
        let n = number();
        assert_eq!(n.len(), 3);
    }
}
