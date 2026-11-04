//! 7PLUS 2.2 encode and decode, in Rust.
//!
//! This is a port of the radix-216 coder written by Axel Bauda, DG1BBQ
//! (C sources kept by HB9XAR). It exists so GPNG can split and join files
//! on Windows, Linux, and macOS. The old MS-DOS `7PLUS.EXE` cannot.
//!
//! The port is not covered by this crate's MIT / Apache-2.0 licence.
//! 7PLUS terms still apply to this module: no commercial use, no sale,
//! circulate freely.
//!
//! `-sb` is the same threshold the C encoder uses: a part ends once at
//! least that many source bytes have been written, in steps of 62. It is
//! not the AX.25 packet length.

use std::sync::OnceLock;

const MARK_A: u8 = 0xb0;
const MARK_B: u8 = 0xb1;
const MARK_C: u8 = 0xb2;
const EXT_83: u8 = 0xdb;
const RADIX: u32 = 216;

struct Tables {
    code: [u8; 216],
    decode: [u8; 256],
    crc: [u16; 256],
}

fn tables() -> &'static Tables {
    static TAB: OnceLock<Tables> = OnceLock::new();
    TAB.get_or_init(build_tables)
}

fn build_tables() -> Tables {
    let mut code = [0u8; 216];
    let mut j = 0;
    for i in 0x21u8..0x2a {
        code[j] = i;
        j += 1;
    }
    for i in 0x2b..0x7f {
        code[j] = i;
        j += 1;
    }
    for i in 0x80..0x91 {
        code[j] = i;
        j += 1;
    }
    code[j] = 0x92;
    j += 1;
    for i in 0x94..0xfd {
        code[j] = i;
        j += 1;
    }
    debug_assert_eq!(j, 216);

    let mut decode = [255u8; 256];
    let mut n = 0u8;
    for i in 0x21..0x2a {
        decode[i] = n;
        n += 1;
    }
    for i in 0x2b..0x7f {
        decode[i] = n;
        n += 1;
    }
    for i in 0x80..0x91 {
        decode[i] = n;
        n += 1;
    }
    decode[0x92] = n;
    n += 1;
    for i in 0x94..0xfd {
        decode[i] = n;
        n += 1;
    }

    let bitrmdrs: [u16; 8] = [
        0x9188, 0x48C4, 0x2462, 0x1231, 0x8108, 0x4084, 0x2042, 0x1021,
    ];
    let mut crc = [0u16; 256];
    for n in 0..256u16 {
        let mut r = 0u16;
        let mut mask = 0x0080u16;
        for m in 0..8 {
            if n & mask != 0 {
                r ^= bitrmdrs[m];
            }
            mask >>= 1;
        }
        crc[n as usize] = r;
    }
    Tables { code, decode, crc }
}

fn crc_calc(x: u16, y: u8) -> u16 {
    let tab = &tables().crc;
    tab[(x >> 8) as usize] ^ (((x & 255) << 8) | u16::from(y))
}

/// One encoded part, ready to write under `TEMP` or to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedPart {
    /// `picture.7pl`, or `picture.p01` … `picture.p0a`.
    pub name: String,
    /// The part text, CR LF between lines, high bytes kept.
    pub body: Vec<u8>,
}

/// Original file recovered from a full set of parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedFile {
    /// 8.3 name from the header, lower case (`picture.jpg`).
    pub name: String,
    pub data: Vec<u8>,
}

/// Encode `data` under an 8.3 name.
///
/// `part_bytes` is 7plus `-sb`. `0` means one part for the whole file.
/// A part stops after a 62-byte line once at least `part_bytes` source
/// bytes have been encoded. The hard cap is 512 lines (31744 bytes).
pub fn encode_parts(
    original_name: &str,
    data: &[u8],
    part_bytes: u32,
) -> Result<Vec<EncodedPart>, String> {
    if data.is_empty() {
        return Err("7plus: cannot encode an empty file".into());
    }
    let (header_name, disk_stem) = dos_8_3(original_name)?;
    let size = data.len();
    let mut blocksize = part_bytes as usize;
    if blocksize == 0 || blocksize > size {
        blocksize = size;
    }
    if blocksize > 512 * 62 {
        blocksize = 512 * 62;
    }
    let blocklines = (blocksize + 61) / 62;
    let parts = (size + blocksize - 1) / blocksize;
    if parts > 255 {
        return Err("7plus: more than 255 parts — use a larger -sb".into());
    }
    let block_full = blocksize;
    let mut pos = 0usize;
    let mut out = Vec::with_capacity(parts);
    for part in 1..=parts {
        let mut this_block = block_full;
        if part == parts && parts > 1 {
            let rem = size % block_full;
            if rem != 0 {
                this_block = rem;
            }
            this_block = ((this_block + 61) / 62) * 62;
        }
        let mut lines: Vec<Vec<u8>> = Vec::new();
        let mut binbytes = 0usize;
        let mut curline = 0u16;
        while pos < size && (binbytes < this_block || parts == 1) {
            let n = (size - pos).min(62);
            if n == 0 {
                break;
            }
            lines.push(encode_data_line(&data[pos..pos + n], curline));
            pos += n;
            binbytes += 62;
            curline += 1;
            if parts == 1 && pos >= size {
                break;
            }
        }
        let name = if parts == 1 {
            format!("{disk_stem}.7pl")
        } else {
            format!("{disk_stem}.p{part:02x}")
        };
        out.push(EncodedPart {
            name,
            body: render_part(part, parts, &header_name, size, this_block, blocklines, &lines),
        });
    }
    Ok(out)
}

/// Join part files. Every part of the set must be present.
pub fn decode_parts(files: &[Vec<u8>]) -> Result<DecodedFile, String> {
    if files.is_empty() {
        return Err("7plus: no parts".into());
    }
    let mut parsed = Vec::new();
    for file in files {
        parsed.push(parse_part(file)?);
    }
    let name = parsed[0].name.clone();
    let size = parsed[0].size;
    let parts = parsed[0].parts;
    if parsed.iter().any(|p| p.name != name || p.size != size || p.parts != parts) {
        return Err("7plus: parts are not one file".into());
    }
    let mut have = vec![false; parts];
    let mut chunks: Vec<Option<Vec<u8>>> = (0..parts).map(|_| None).collect();
    for p in parsed {
        let idx = p.part - 1;
        if have[idx] {
            return Err(format!("7plus: part {} is repeated", p.part));
        }
        have[idx] = true;
        chunks[idx] = Some(p.data);
    }
    if let Some(missing) = have.iter().position(|h| !h) {
        return Err(format!("7plus: part {} of {parts} is missing", missing + 1));
    }
    let mut data = Vec::with_capacity(size);
    for chunk in chunks.into_iter().flatten() {
        data.extend(chunk);
    }
    if data.len() < size {
        return Err("7plus: decoded file is short".into());
    }
    data.truncate(size);
    Ok(DecodedFile { name, data })
}

struct ParsedPart {
    name: String,
    part: usize,
    parts: usize,
    size: usize,
    data: Vec<u8>,
}

fn parse_part(raw: &[u8]) -> Result<ParsedPart, String> {
    let lines = split_lines(raw);
    let mut header_at = None;
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with(b" go_7+.") {
            header_at = Some(i);
            break;
        }
    }
    let Some(at) = header_at else {
        return Err("7plus: no go_7+. line".into());
    };
    let header = lines[at];
    if !mcrc_ok(header) {
        return Err("7plus: header check failed".into());
    }
    if !crc2_ok(header) {
        return Err("7plus: header crc failed".into());
    }
    let (part, parts, name, size, _blocklines) = parse_header(header)?;
    let mut data = Vec::new();
    for line in lines.iter().skip(at + 1) {
        if line.starts_with(b" stop_7+.") {
            break;
        }
        if line.first().is_some_and(|b| *b == b'/') {
            continue;
        }
        if line.len() < 69 || tables().decode[line[0] as usize] == 255 {
            continue;
        }
        let row = &line[..69];
        if !data_crc_ok(row) || !crc2_ok(row) {
            return Err(format!("7plus: bad code line in part {part}"));
        }
        data.extend_from_slice(&unpack_line(row));
    }
    Ok(ParsedPart {
        name,
        part,
        parts,
        size,
        data,
    })
}

fn parse_header(line: &[u8]) -> Result<(usize, usize, String, usize, usize), String> {
    let text = latin1_head(line);
    let mut words = text.split_whitespace();
    let marker = words.next().unwrap_or("");
    if marker != "go_7+." {
        return Err("7plus: bad header".into());
    }
    let part: usize = words
        .next()
        .unwrap_or("")
        .parse()
        .map_err(|_| "7plus: bad part number".to_string())?;
    let of = words.next().unwrap_or("");
    if !of.eq_ignore_ascii_case("of") {
        return Err("7plus: header has no 'of'".into());
    }
    let parts: usize = words
        .next()
        .unwrap_or("")
        .parse()
        .map_err(|_| "7plus: bad part count".to_string())?;
    let name = words.next().unwrap_or("").to_ascii_lowercase();
    let size: usize = words
        .next()
        .unwrap_or("")
        .parse()
        .map_err(|_| "7plus: bad file size".to_string())?;
    let _span = words.next().unwrap_or("");
    let blocklines = usize::from_str_radix(words.next().unwrap_or(""), 16)
        .map_err(|_| "7plus: bad line count".to_string())?;
    if part == 0 || parts == 0 || part > parts || name.is_empty() || size == 0 {
        return Err("7plus: header is not usable".into());
    }
    Ok((part, parts, name, size, blocklines))
}

fn latin1_head(line: &[u8]) -> String {
    let end = line.iter().position(|b| *b == MARK_A).unwrap_or(line.len());
    line[..end].iter().map(|b| char::from(*b)).collect()
}

fn render_part(
    part: usize,
    parts: usize,
    header_name: &str,
    size: usize,
    this_block: usize,
    blocklines: usize,
    lines: &[Vec<u8>],
) -> Vec<u8> {
    let span = ((this_block + 61) / 62) * 64;
    let head = format!(
        " go_7+. {part:03} of {parts:03} {header_name:<12} {size:07} {span:04X} {blocklines:03X} (7PLUS v2.2) "
    );
    let mut body = Vec::new();
    push_line(&mut body, &seal_marked(head.into_bytes()));
    for line in lines {
        push_line(&mut body, line);
    }
    let stop = if parts > 1 {
        format!(" stop_7+. ({header_name}.P{part:02X}/{parts:02X}) [0]")
    } else {
        format!(" stop_7+. ({header_name}.7PL) [0]")
    };
    push_line(&mut body, &seal_marked(stop_text(&stop)));
    body
}

fn stop_text(text: &str) -> Vec<u8> {
    let mut line = vec![b' '; 62];
    let raw = text.as_bytes();
    let n = raw.len().min(62);
    line[..n].copy_from_slice(&raw[..n]);
    line
}

fn seal_marked(mut prefix: Vec<u8>) -> Vec<u8> {
    prefix.extend_from_slice(&[MARK_A, MARK_B, MARK_C, EXT_83]);
    let j = prefix.iter().position(|b| *b == MARK_A).unwrap() + 4;
    debug_assert_eq!(
        prefix.len(),
        j,
        "7plus marker must sit at column 62 (prefix {})",
        prefix.len() - 4
    );
    let mut crc = 0u16;
    for b in &prefix {
        crc = crc_calc(crc, *b);
    }
    let crc = (u32::from(crc) % RADIX) as u16;
    prefix.push(tables().code[crc as usize]);
    append_crc2(&mut prefix);
    prefix
}

fn append_crc2(line: &mut Vec<u8>) {
    let mut crc = 0u16;
    for i in (0..line.len()).rev() {
        crc = crc_calc(crc, line[i]);
    }
    crc &= 0x7fff;
    let tab = &tables().code;
    line.push(tab[(crc % 0xd8) as usize]);
    line.push(tab[(crc / 0xd8) as usize]);
}

fn encode_data_line(chunk: &[u8], line_no: u16) -> Vec<u8> {
    let mut raw = [0u8; 62];
    raw[..chunk.len()].copy_from_slice(chunk);
    let mut after = [0u32; 16];
    after[..8].copy_from_slice(&pack_31(&raw[..31]));
    after[8..].copy_from_slice(&pack_31(&raw[31..]));
    let tab = &tables().code;
    let mut line = Vec::with_capacity(69);
    for value in after {
        let mut v = u64::from(value);
        line.push(tab[(v % u64::from(RADIX)) as usize]);
        v /= u64::from(RADIX);
        line.push(tab[(v % u64::from(RADIX)) as usize]);
        v /= u64::from(RADIX);
        line.push(tab[(v % u64::from(RADIX)) as usize]);
        line.push(tab[(v / u64::from(RADIX)) as usize]);
    }
    let mut crc = 0u16;
    for b in line.iter().take(64) {
        crc = crc_calc(crc, *b);
    }
    let packed = (u32::from(line_no & 0x1ff) << 14) | u32::from(crc & 0x3fff);
    let mut v = u64::from(packed);
    line.push(tab[(v % u64::from(RADIX)) as usize]);
    v /= u64::from(RADIX);
    line.push(tab[(v % u64::from(RADIX)) as usize]);
    line.push(tab[(v / u64::from(RADIX)) as usize]);
    append_crc2(&mut line);
    line
}

fn pack_31(bytes: &[u8]) -> [u32; 8] {
    let mut af = [0u32; 8];
    let mut idx = 0;
    for j in 0..8 {
        let count = if j == 7 { 3 } else { 4 };
        for _ in 0..count {
            let h = bytes.get(idx).copied().unwrap_or(0);
            idx += 1;
            af[j] = (af[j] << 8) | u32::from(h);
        }
    }
    af[7] = af[7] | ((af[6] & 127) << 24);
    af[6] = (af[6] >> 7) | ((af[5] & 63) << 25);
    af[5] = (af[5] >> 6) | ((af[4] & 31) << 26);
    af[4] = (af[4] >> 5) | ((af[3] & 15) << 27);
    af[3] = (af[3] >> 4) | ((af[2] & 7) << 28);
    af[2] = (af[2] >> 3) | ((af[1] & 3) << 29);
    af[1] = (af[1] >> 2) | ((af[0] & 1) << 30);
    af[0] >>= 1;
    af
}

fn unpack_line(chars: &[u8]) -> [u8; 62] {
    let dec = &tables().decode;
    let mut after = [0u32; 16];
    for k in 0..16 {
        let end = k * 4 + 3;
        let mut v = 0u32;
        for j in (end - 3..=end).rev() {
            v = v * RADIX + u32::from(dec[chars[j] as usize]);
        }
        after[k] = v;
    }
    let mut out = [0u8; 62];
    let mut n = 0;
    for g in 0..2 {
        let mut af = [0u32; 8];
        af.copy_from_slice(&after[g * 8..g * 8 + 8]);
        af[0] = (af[0] << 1) | (af[1] >> 30);
        af[1] = (af[1] << 2) | (af[2] >> 29);
        af[2] = (af[2] << 3) | (af[3] >> 28);
        af[3] = (af[3] << 4) | (af[4] >> 27);
        af[4] = (af[4] << 5) | (af[5] >> 26);
        af[5] = (af[5] << 6) | (af[6] >> 25);
        af[6] = (af[6] << 7) | (af[7] >> 24);
        af[7] <<= 8;
        for j in 0..8 {
            for shift in [24u32, 16, 8] {
                out[n] = (af[j] >> shift) as u8;
                n += 1;
            }
            if j == 7 {
                break;
            }
            out[n] = af[j] as u8;
            n += 1;
        }
    }
    out
}

fn mcrc_ok(line: &[u8]) -> bool {
    let Some(pos) = line.windows(2).position(|w| w == [MARK_A, MARK_B]) else {
        return false;
    };
    let j = pos + 4;
    if line.len() <= j {
        return false;
    }
    let mut crc = 0u16;
    for b in &line[..j] {
        crc = crc_calc(crc, *b);
    }
    let crc = (u32::from(crc) % RADIX) as u8;
    tables().decode[line[j] as usize] == crc
}

fn crc2_ok(line: &[u8]) -> bool {
    if line.len() < 69 {
        return false;
    }
    let mut crc = 0u16;
    for i in (0..=66).rev() {
        crc = crc_calc(crc, line[i]);
    }
    crc &= 0x7fff;
    let tab = &tables().code;
    line[67] == tab[(crc % 0xd8) as usize] && line[68] == tab[(crc / 0xd8) as usize]
}

fn data_crc_ok(line: &[u8]) -> bool {
    let dec = &tables().decode;
    if line.len() < 67 || dec[line[64] as usize] == 255 {
        return false;
    }
    let mut crc = 0u16;
    for b in &line[..64] {
        crc = crc_calc(crc, *b);
    }
    let packed = u32::from(dec[line[64] as usize])
        + u32::from(dec[line[65] as usize]) * RADIX
        + u32::from(dec[line[66] as usize]) * RADIX * RADIX;
    (crc & 0x3fff) == (packed as u16 & 0x3fff)
}

fn push_line(body: &mut Vec<u8>, line: &[u8]) {
    body.extend_from_slice(line);
    body.extend_from_slice(b"\r\n");
}

fn split_lines(raw: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'\n' || raw[i] == b'\r' {
            let mut end = i;
            if end > start && raw[end - 1] == b'\r' {
                end -= 1;
            }
            if end > start {
                out.push(&raw[start..end]);
            }
            if raw[i] == b'\r' && raw.get(i + 1) == Some(&b'\n') {
                i += 1;
            }
            start = i + 1;
        }
        i += 1;
    }
    if start < raw.len() {
        out.push(&raw[start..]);
    }
    out
}

/// Upper-case 8.3 for the header, lower-case stem for `name.7pl` / `name.p01`.
fn dos_8_3(original: &str) -> Result<(String, String), String> {
    let base = original
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(original)
        .trim();
    if base.is_empty() || base == "." || base == ".." {
        return Err("7plus: need a file name".into());
    }
    let (stem, ext) = match base.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && !e.is_empty() => (s, e),
        _ => (base, ""),
    };
    let stem = filter_dos(stem, 8);
    let ext = filter_dos(ext, 3);
    if stem.is_empty() {
        return Err("7plus: file name has no usable characters".into());
    }
    let header = if ext.is_empty() {
        stem.clone()
    } else {
        format!("{stem}.{ext}")
    };
    Ok((header.to_ascii_uppercase(), stem.to_ascii_lowercase()))
}

fn filter_dos(text: &str, max: usize) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(max)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alphabet_is_216_and_skips_star() {
        let t = tables();
        assert_eq!(t.code.iter().filter(|b| **b != 0).count(), 216);
        assert_eq!(t.code[0], 0x21);
        assert!(t.decode[b'*' as usize] == 255);
        assert_eq!(t.decode[0xb0], t.code.iter().position(|c| *c == 0xb0).unwrap() as u8);
    }

    #[test]
    fn one_part_roundtrip_keeps_every_byte() {
        let data: Vec<u8> = (0u8..=255).cycle().take(400).collect();
        let parts = encode_parts("Photo.JPG", &data, 0).unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].name, "photo.7pl");
        assert!(parts[0].body.windows(8).any(|w| w == b" go_7+. "));
        assert!(parts[0].body.windows(2).any(|w| w == [MARK_A, MARK_B]));
        let back = decode_parts(&[parts[0].body.clone()]).unwrap();
        assert_eq!(back.name, "photo.jpg");
        assert_eq!(back.data, data);
    }

    #[test]
    fn short_file_and_split_parts_join() {
        let data = b"Hello packet\r\nline two \xff\x00\xb0".to_vec();
        let one = encode_parts("note.txt", &data, 0).unwrap();
        assert_eq!(decode_parts(&[one[0].body.clone()]).unwrap().data, data);

        let many = encode_parts("map.bin", &vec![0x5a; 200], 80).unwrap();
        assert!(many.len() > 1);
        assert_eq!(many[0].name, "map.p01");
        let bodies: Vec<Vec<u8>> = many.iter().map(|p| p.body.clone()).collect();
        let back = decode_parts(&bodies).unwrap();
        assert_eq!(back.name, "map.bin");
        assert_eq!(back.data, vec![0x5a; 200]);
    }

    #[test]
    fn part_ten_uses_hex_in_the_file_name() {
        let parts = encode_parts("pic.bin", &vec![1u8; 20], 1).unwrap();
        assert!(parts.len() >= 10, "parts {}", parts.len());
        assert_eq!(parts[9].name, "pic.p0a");
        let bodies: Vec<Vec<u8>> = parts.iter().map(|p| p.body.clone()).collect();
        assert_eq!(decode_parts(&bodies).unwrap().data, vec![1u8; 20]);
    }
}
