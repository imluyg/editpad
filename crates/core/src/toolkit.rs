//! 选区文本工具（P128）：Base64/URL 编解码与 MD5/SHA-256 摘要。
//!
//! 全部纯函数、零第三方依赖（离线环境不引新 crate）——哈希按 RFC 1321 /
//! FIPS 180-4 手写，测试用公开标准向量钉死正确性。哈希输出小写十六进制。

// ---------- Base64（RFC 4648 standard alphabet，带填充） ----------

const B64_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_index(c: char) -> Option<u32> {
    match c {
        'A'..='Z' => Some(c as u32 - 'A' as u32),
        'a'..='z' => Some(c as u32 - 'a' as u32 + 26),
        '0'..='9' => Some(c as u32 - '0' as u32 + 52),
        '+' => Some(62),
        '/' => Some(63),
        _ => None,
    }
}

/// Base64 编码（标准字母表 + `=` 填充）。
pub fn base64_encode(src: &[u8]) -> String {
    let mut out = String::with_capacity(src.len().div_ceil(3) * 4);
    for chunk in src.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64_ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(B64_ALPHABET[(n >> 12) as usize & 63] as char);
        if chunk.len() > 1 {
            out.push(B64_ALPHABET[(n >> 6) as usize & 63] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(B64_ALPHABET[n as usize & 63] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Base64 解码：容忍任意位置空白（换行分块的常见格式）；`=` 视为终止；
/// 其余非法字符、长度 ≡1 (mod 4) 返回 None。
pub fn base64_decode(src: &str) -> Option<Vec<u8>> {
    let mut vals: Vec<u32> = Vec::with_capacity(src.len());
    for c in src.chars() {
        match c {
            ' ' | '\t' | '\r' | '\n' => continue,
            '=' => break,
            other => vals.push(b64_index(other)?),
        }
    }
    if vals.len() % 4 == 1 {
        return None; // 1 个悬挂字符：信息不完整
    }
    let valid = match vals.len() % 4 {
        2 => 1,
        3 => 2,
        _ => 3,
    };
    while !vals.len().is_multiple_of(4) {
        vals.push(0);
    }
    let mut out = Vec::with_capacity(vals.len() / 4 * 3);
    for quad in vals.chunks(4) {
        let n = (quad[0] << 18) | (quad[1] << 12) | (quad[2] << 6) | quad[3];
        out.push((n >> 16) as u8);
        out.push((n >> 8) as u8);
        out.push(n as u8);
    }
    out.truncate(out.len() - (3 - valid));
    Some(out)
}

// ---------- URL 百分号编解码（RFC 3986） ----------

fn is_url_unreserved(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~')
}

/// URL 编码（RFC 3986 strict）：仅字母数字与 `-._~` 保留原样，其余字节
/// （含空格）按 UTF-8 逐字节转 `%XX` 大写十六进制。
pub fn url_encode(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for &b in src.as_bytes() {
        if is_url_unreserved(b) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(char::from_digit((b >> 4) as u32, 16).unwrap().to_ascii_uppercase());
            out.push(char::from_digit((b & 0xF) as u32, 16).unwrap().to_ascii_uppercase());
        }
    }
    out
}

fn hex_val(c: char) -> Option<u8> {
    c.to_digit(16).map(|d| d as u8)
}

/// URL 解码：`%XX` 逐字节还原（结果须为合法 UTF-8，否则 None）；
/// `+` 还原为空格（application/x-www-form-urlencoded 惯例）。
pub fn url_decode(src: &str) -> Option<String> {
    let bytes = src.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' => {
                let hi = hex_val(*bytes.get(i + 1)? as char)?;
                let lo = hex_val(*bytes.get(i + 2)? as char)?;
                out.push((hi << 4) | lo);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

// ---------- MD5（RFC 1321） ----------

const MD5_S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

#[rustfmt::skip]
const MD5_K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

/// MD5 摘要，小写十六进制输出。
pub fn md5_hex(src: &[u8]) -> String {
    // 填充：0x80 + 零 + 64 位小端比特长度
    let bit_len = (src.len() as u64).wrapping_mul(8);
    let mut msg = src.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    let (mut a0, mut b0, mut c0, mut d0) =
        (0x6745_2301u32, 0xefcd_ab89u32, 0x98ba_dcfeu32, 0x1032_5476u32);
    for block in msg.chunks(64) {
        let m: Vec<u32> = block
            .chunks(4)
            .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
            .collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let tmp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                f.wrapping_add(a)
                    .wrapping_add(MD5_K[i])
                    .wrapping_add(m[g])
                    .rotate_left(MD5_S[i]),
            );
            a = tmp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut digest = Vec::with_capacity(16);
    for v in [a0, b0, c0, d0] {
        digest.extend_from_slice(&v.to_le_bytes());
    }
    hex_lower(&digest)
}

// ---------- SHA-256（FIPS 180-4） ----------

#[rustfmt::skip]
const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 摘要，小写十六进制输出。
pub fn sha256_hex(src: &[u8]) -> String {
    let bit_len = (src.len() as u64).wrapping_mul(8);
    let mut msg = src.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    let (mut h0, mut h1, mut h2, mut h3, mut h4, mut h5, mut h6, mut h7) = (
        0x6a09_e667u32, 0xbb67_ae85u32, 0x3c6e_f372u32, 0xa54f_f53au32, 0x510e_527fu32,
        0x9b05_688cu32, 0x1f83_d9abu32, 0x5be0_cd19u32,
    );
    for block in msg.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h) =
            (h0, h1, h2, h3, h4, h5, h6, h7);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
        h5 = h5.wrapping_add(f);
        h6 = h6.wrapping_add(g);
        h7 = h7.wrapping_add(h);
    }
    let mut digest = Vec::with_capacity(32);
    for v in [h0, h1, h2, h3, h4, h5, h6, h7] {
        digest.extend_from_slice(&v.to_be_bytes());
    }
    hex_lower(&digest)
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        out.push(char::from_digit((b & 0xF) as u32, 16).unwrap());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648_vectors() {
        // RFC 4648 §10 测试向量
        for (src, want) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(base64_encode(src.as_bytes()), want, "encode {src:?}");
            assert_eq!(base64_decode(want).as_deref(), Some(src.as_bytes()), "decode {want}");
        }
        // 二进制字节全值域往返
        let all: Vec<u8> = (0..=255u8).collect();
        assert_eq!(base64_decode(&base64_encode(&all)).as_deref(), Some(all.as_slice()));
        // 容忍空白分块；非法字符拒绝；悬挂单字符拒绝
        assert_eq!(base64_decode("Zm9v\r\nYmFy").as_deref(), Some(&b"foobar"[..]));
        assert!(base64_decode("Zm9*v").is_none());
        assert!(base64_decode("Z").is_none());
    }

    #[test]
    fn url_codec_is_strict_rfc3986_with_plus_space() {
        assert_eq!(url_encode("a b/c?d=1&x"), "a%20b%2Fc%3Fd%3D1%26x");
        assert_eq!(url_encode("保留~-. _"), "%E4%BF%9D%E7%95%99~-.%20_");
        // `+` 还原空格、%XX 还原字节、多字节 UTF-8 往返
        assert_eq!(url_decode("a+b%20c").as_deref(), Some("a b c"));
        assert_eq!(
            url_decode(&url_encode("中文+ emoji 🚀")).as_deref(),
            Some("中文+ emoji 🚀"),
            "编解码恒等：编码只产 %XX，不产 +"
        );
        assert_eq!(url_decode("中文+emoji").as_deref(), Some("中文 emoji"), "+ 单独还原为空格");
        // 残缺转义与非法 UTF-8 拒绝
        assert!(url_decode("%2").is_none());
        assert!(url_decode("%ZZ").is_none());
        assert!(url_decode("%FF").is_none());
    }

    #[test]
    fn md5_matches_rfc1321_vectors() {
        assert_eq!(
            md5_hex(b""),
            "d41d8cd98f00b204e9800998ecf8427e"
        );
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            md5_hex(b"The quick brown fox jumps over the lazy dog"),
            "9e107d9d372bb6826bd81d3542a419d6"
        );
        // 80 字节 = 跨两个分组
        assert_eq!(
            md5_hex(b"1234567890".repeat(8).as_slice()),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
    }

    #[test]
    fn sha256_matches_fips180_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // 56 字节：填充跨两个分组
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }
}
