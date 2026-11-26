//! The app icon (icon.svg at the project root: a blue square, three bars on a dock), drawn here for the tray
//! and for the exe files' .ico (`cargo run -p proxydock --example make_ico` writes app/icons/proxydock.ico).

/// size x size RGBA (straight alpha, every pixel opaque), drawn with anti-aliasing
pub fn rgba(size: u32) -> Vec<u8> {
    use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};
    // icon.svg is 163 units wide
    let k = size as f32 / 163.0;
    let rounded = |x: f32, y: f32, w: f32, h: f32, r: f32| {
        // a rounded rectangle (cubic corners)
        let c = r * 0.552_284_8;
        let mut pb = PathBuilder::new();
        pb.move_to(x + r, y);
        pb.line_to(x + w - r, y);
        pb.cubic_to(x + w - r + c, y, x + w, y + r - c, x + w, y + r);
        pb.line_to(x + w, y + h - r);
        pb.cubic_to(x + w, y + h - r + c, x + w - r + c, y + h, x + w - r, y + h);
        pb.line_to(x + r, y + h);
        pb.cubic_to(x + r - c, y + h, x, y + h - r + c, x, y + h - r);
        pb.line_to(x, y + r);
        pb.cubic_to(x, y + r - c, x + r - c, y, x + r, y);
        pb.close();
        pb.finish()
    };
    let mut pm = Pixmap::new(size, size).unwrap();
    let mut paint = Paint { anti_alias: true, ..Default::default() };
    paint.set_color_rgba8(0x00, 0x78, 0xD4, 255);
    pm.fill_rect(Rect::from_xywh(0.0, 0.0, size as f32, size as f32).unwrap(), &paint, Transform::identity(), None);
    paint.set_color_rgba8(0xD9, 0xD9, 0xD9, 255);
    for (x, y, w, h) in
        [(24.0, 106.0, 116.0, 20.0), (104.0, 65.0, 20.0, 35.0), (72.0, 49.0, 20.0, 51.0), (40.0, 37.0, 20.0, 63.0)]
    {
        if let Some(path) = rounded(x * k, y * k, w * k, h * k, 6.0 * k) {
            pm.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }
    }
    // (every pixel is opaque: premultiplied and straight alpha are the same)
    pm.take()
}

/// A Windows .ico holding the icon at each of `sizes` (32-bit BGRA bitmaps)
pub fn ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes
        .iter()
        .map(|&s| {
            let px = rgba(s);
            let mut dib = Vec::new();
            // BITMAPINFOHEADER: the height counts the colour rows and the mask rows
            for v in [40u32, s, s * 2] {
                dib.extend_from_slice(&v.to_le_bytes());
            }
            dib.extend_from_slice(&1u16.to_le_bytes());
            dib.extend_from_slice(&32u16.to_le_bytes());
            dib.extend_from_slice(&[0u8; 24]);
            // colour rows bottom-up, BGRA
            for y in (0..s).rev() {
                for x in 0..s {
                    let i = ((y * s + x) * 4) as usize;
                    dib.extend_from_slice(&[px[i + 2], px[i + 1], px[i], px[i + 3]]);
                }
            }
            // the AND mask (all opaque), rows padded to 4 bytes
            let row = s.div_ceil(32) * 4;
            dib.extend(std::iter::repeat_n(0u8, (row * s) as usize));
            dib
        })
        .collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (&s, img) in sizes.iter().zip(&images) {
        let d = if s >= 256 { 0 } else { s as u8 };
        out.extend_from_slice(&[d, d, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(img.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        out.extend_from_slice(&img);
    }
    out
}

/// The icon as a PNG of `size` x `size` (stored, not compressed: for the Linux and macOS packages' icons)
pub fn png(size: u32) -> Vec<u8> {
    fn crc(data: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc(&body).to_be_bytes());
    }
    // the rows, each after its filter byte (0: none)
    let px = rgba(size);
    let mut raw = Vec::with_capacity((size * (size * 4 + 1)) as usize);
    for row in px.chunks((size * 4) as usize) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    // zlib with stored deflate blocks, then Adler-32
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, b) in blocks.iter().enumerate() {
        z.push((i + 1 == blocks.len()) as u8);
        z.extend_from_slice(&(b.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
        z.extend_from_slice(b);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + x as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = size.to_be_bytes().to_vec();
    ihdr.extend_from_slice(&size.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn png_is_well_formed() {
        let p = super::png(16);
        assert_eq!(&p[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&p[12..16], b"IHDR");
        assert_eq!(u32::from_be_bytes(p[16..20].try_into().unwrap()), 16);
        assert!(p.ends_with(&[0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82]));
    }

    #[test]
    fn ico_has_every_size() {
        let ico = super::ico(&[16, 32, 256]);
        assert_eq!(&ico[..6], &[0, 0, 1, 0, 3, 0]);
        // the first image starts after the header and three entries
        assert_eq!(u32::from_le_bytes(ico[18..22].try_into().unwrap()), 6 + 48);
        let px = super::rgba(32);
        assert_eq!(px.len(), 32 * 32 * 4);
        // the corner is the blue background
        assert_eq!(&px[..4], &[0x00, 0x78, 0xD4, 0xFF]);
    }
}
