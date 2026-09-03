/// ICO 条目：尺寸与 DIB 像素区（BMP BITMAPINFOHEADER 起，含 AND mask）。
pub(crate) struct IcoEntry {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) bit_count: u16,
    /// 图像数据区整体（40B BITMAPINFOHEADER + 像素 + AND mask）。
    pub(crate) data: &'static [u8],
}

/// 解析 app.ico 的全部条目（ICONDIR 骨架，零依赖）。
/// 本资源（P71 用户四档 ICO，首字节 28 00 00 00 = BITMAPINFOHEADER）
/// 全部为 ICO 内嵌 DIB；PNG 条目不在支持范围（不存在于本资源）。
pub(crate) fn parse_app_ico_entries(raw: &'static [u8]) -> Vec<IcoEntry> {
    let mut out = Vec::new();
    if raw.len() < 6 {
        return out;
    }
    let count = u16::from_le_bytes([raw[4], raw[5]]) as usize;
    for i in 0..count {
        let o = 6 + i * 16;
        if o + 16 > raw.len() {
            break;
        }
        let width = if raw[o] == 0 { 256 } else { raw[o] as u32 };
        let height = if raw[o + 1] == 0 { 256 } else { raw[o + 1] as u32 };
        let bytes = u32::from_le_bytes([raw[o + 8], raw[o + 9], raw[o + 10], raw[o + 11]]) as usize;
        let off = u32::from_le_bytes([raw[o + 12], raw[o + 13], raw[o + 14], raw[o + 15]]) as usize;
        if off + bytes > raw.len() || bytes < 40 {
            continue;
        }
        let bit_count = u16::from_le_bytes([raw[off + 14], raw[off + 15]]);
        out.push(IcoEntry { width, height, bit_count, data: &raw[off..off + bytes] });
    }
    out
}

/// ICO 内嵌 32bpp DIB → RGBA（自底向上翻转行，忽略 AND mask）。
/// 仅支持 BI_RGB + 32bpp（本资源全为此格式）；其余返回 None。
pub(crate) fn dib_bgra32_to_rgba(dib: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    if dib.len() < 40 {
        return None;
    }
    let w = i32::from_le_bytes([dib[4], dib[5], dib[6], dib[7]]);
    let h2 = i32::from_le_bytes([dib[8], dib[9], dib[10], dib[11]]);
    if w <= 0 || h2 <= 0 {
        return None;
    }
    // ICO 内嵌 DIB 的 biHeight = 2 × 实际高度（高度 + AND mask 各一份）
    let h = h2 / 2;
    let bit_count = u16::from_le_bytes([dib[14], dib[15]]);
    let comp = u32::from_le_bytes([dib[16], dib[17], dib[18], dib[19]]);
    if bit_count != 32 || comp != 0 {
        return None;
    }
    let (w, h) = (w as u32, h as u32);
    let row_stride = w as usize * 4;
    let px = 40usize; // 32bpp 无调色板：头部后直接是底向上 BGRA 像素
    if dib.len() < px + row_stride * h as usize {
        return None;
    }
    let mut rgba = vec![0u8; row_stride * h as usize];
    for y in 0..h as usize {
        let src = &dib[px + row_stride * y..px + row_stride * (y + 1)];
        let dst = &mut rgba[row_stride * (h as usize - 1 - y)..row_stride * (h as usize - y)];
        for x in 0..w as usize {
            dst[x * 4] = src[x * 4 + 2]; // BGR → RGB
            dst[x * 4 + 1] = src[x * 4 + 1];
            dst[x * 4 + 2] = src[x * 4];
            dst[x * 4 + 3] = src[x * 4 + 3];
        }
    }
    Some((w, h, rgba))
}

/// 窗口标题栏/任务栏图标（第 76 轮用户点单：替换标题栏默认通用图标）：
/// P71 自定义四档 ICO 的 **48px** 条目 → RGBA（winit 期望**预乘 alpha**，
/// 防透明边缘发亮/漏色）。解析失败返回 None（winit 回退 exe 资源图标）。
/// OnceLock 缓存：只解析一次（Icon: Clone，重复取零成本）。
pub(crate) fn window_title_icon() -> Option<iced::window::Icon> {
    static ICON: std::sync::OnceLock<Option<iced::window::Icon>> = std::sync::OnceLock::new();
    ICON.get_or_init(|| {
        for e in parse_app_ico_entries(include_bytes!("../assets/app.ico")) {
            if e.width == 48 && e.height == 48 && e.bit_count == 32 {
                if let Some((w, h, rgba)) = dib_bgra32_to_rgba(e.data) {
                    // sRGB → 预乘 alpha（winit/windows 标题栏渲染要求）
                    let premul: Vec<u8> = rgba
                        .as_chunks::<4>().0.iter()
                        .flat_map(|px| {
                            let a = px[3] as u32;
                            [
                                (px[0] as u32 * a / 255) as u8,
                                (px[1] as u32 * a / 255) as u8,
                                (px[2] as u32 * a / 255) as u8,
                                px[3],
                            ]
                        })
                        .collect();
                    return iced::window::icon::from_rgba(premul, w, h).ok();
                }
            }
        }
        None
    })
    .clone()
}
