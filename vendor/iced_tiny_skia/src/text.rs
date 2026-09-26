use crate::core::alignment;
use crate::core::text::{Alignment, Shaping};
use crate::core::{Color, Font, Pixels, Point, Rectangle, Transformation};
use crate::graphics::text::cache::{self, Cache};
use crate::graphics::text::editor;
use crate::graphics::text::font_system;
use crate::graphics::text::paragraph;

use rustc_hash::{FxHashMap, FxHashSet};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::hash_map;

#[derive(Debug)]
pub struct Pipeline {
    glyph_cache: GlyphCache,
    cache: RefCell<Cache>,
}

impl Pipeline {
    pub fn new() -> Self {
        Pipeline {
            glyph_cache: GlyphCache::new(),
            cache: RefCell::new(Cache::new()),
        }
    }

    // TODO: Shared engine
    #[allow(dead_code)]
    pub fn load_font(&mut self, bytes: Cow<'static, [u8]>) {
        font_system()
            .write()
            .expect("Write font system")
            .load_font(bytes);

        self.cache = RefCell::new(Cache::new());
    }

    pub fn draw_paragraph(
        &mut self,
        paragraph: &paragraph::Weak,
        position: Point,
        color: Color,
        pixels: &mut tiny_skia::PixmapMut<'_>,
        clip_mask: Option<&tiny_skia::Mask>,
        transformation: Transformation,
    ) {
        let Some(paragraph) = paragraph.upgrade() else {
            return;
        };

        let mut font_system = font_system().write().expect("Write font system");

        draw(
            font_system.raw(),
            &mut self.glyph_cache,
            paragraph.buffer(),
            position,
            color,
            pixels,
            clip_mask,
            transformation,
        );
    }

    pub fn draw_editor(
        &mut self,
        editor: &editor::Weak,
        position: Point,
        color: Color,
        pixels: &mut tiny_skia::PixmapMut<'_>,
        clip_mask: Option<&tiny_skia::Mask>,
        transformation: Transformation,
    ) {
        let Some(editor) = editor.upgrade() else {
            return;
        };

        let mut font_system = font_system().write().expect("Write font system");

        draw(
            font_system.raw(),
            &mut self.glyph_cache,
            editor.buffer(),
            position,
            color,
            pixels,
            clip_mask,
            transformation,
        );
    }

    pub fn draw_cached(
        &mut self,
        content: &str,
        bounds: Rectangle,
        color: Color,
        size: Pixels,
        line_height: Pixels,
        font: Font,
        align_x: Alignment,
        align_y: alignment::Vertical,
        shaping: Shaping,
        pixels: &mut tiny_skia::PixmapMut<'_>,
        clip_mask: Option<&tiny_skia::Mask>,
        transformation: Transformation,
    ) {
        let line_height = f32::from(line_height);

        let mut font_system = font_system().write().expect("Write font system");
        let font_system = font_system.raw();

        let key = cache::Key {
            bounds: bounds.size(),
            content,
            font,
            size: size.into(),
            line_height,
            shaping,
            align_x,
        };

        let (_, entry) = self.cache.get_mut().allocate(font_system, key);

        let width = entry.min_bounds.width;
        let height = entry.min_bounds.height;

        let x = match align_x {
            Alignment::Default | Alignment::Left | Alignment::Justified => {
                bounds.x
            }
            Alignment::Center => bounds.x - width / 2.0,
            Alignment::Right => bounds.x - width,
        };

        let y = match align_y {
            alignment::Vertical::Top => bounds.y,
            alignment::Vertical::Center => bounds.y - height / 2.0,
            alignment::Vertical::Bottom => bounds.y - height,
        };

        draw(
            font_system,
            &mut self.glyph_cache,
            &entry.buffer,
            Point::new(x, y),
            color,
            pixels,
            clip_mask,
            transformation,
        );
    }

    pub fn draw_raw(
        &mut self,
        buffer: &cosmic_text::Buffer,
        position: Point,
        color: Color,
        pixels: &mut tiny_skia::PixmapMut<'_>,
        clip_mask: Option<&tiny_skia::Mask>,
        transformation: Transformation,
    ) {
        let mut font_system = font_system().write().expect("Write font system");

        draw(
            font_system.raw(),
            &mut self.glyph_cache,
            buffer,
            position,
            color,
            pixels,
            clip_mask,
            transformation,
        );
    }

    pub fn trim_cache(&mut self) {
        self.cache.get_mut().trim();
        self.glyph_cache.trim();
    }
}

thread_local! {
    // P120：SwashCache 提升为线程级复用。原实现每次 draw() 调用新建
    // 一个 SwashCache（内含哈希表等分配），一帧内每个文本图元各建一次、
    // 用完即弃——纯分配 churn，加剧打字期堆水位爬升（P120 层栈同因）。
    // 渲染恒单线程（tiny_skia 在主线程栅格化），thread_local 无竞争；
    // 内部图像缓存按字形 CacheKey 键控，规模 ≤ 同键的 GlyphCache，有界。
    static SWASH_CACHE: RefCell<cosmic_text::SwashCache> =
        RefCell::new(cosmic_text::SwashCache::new());
}

// P287：窗口外落笔的跳过开关与被跳过的次数（守卫用例拿它当"这帧真的跳了"的自证）。
thread_local! {
    static BLIT_CULL: Cell<bool> = Cell::new(true);
    static GLYPH_CULLED: Cell<usize> = Cell::new(0);
}

fn blit_cull_on() -> bool {
    BLIT_CULL.with(|c| c.get())
}

/// 取走并清零"本线程被跳过的窗口外落笔数"。
pub fn take_blit_culls() -> usize {
    GLYPH_CULLED.with(|c| {
        let n = c.get();
        c.set(0);
        n
    })
}

/// 测试用同帧老口径开关：`false` ⇒ 不跳窗口外落笔（与上游行为逐字相同）。
/// 线程级 ⇒ 并行用例互不影响。
pub fn set_blit_cull_for_test(on: bool) {
    BLIT_CULL.with(|c| c.set(on));
}

thread_local! {
    // P285 计量：本线程"真正的字形栅格未命中"次数，一次 = 走了一趟
    // `get_image_uncached` 或零面积早退。测试用它把「同一内容重画一帧不该
    // 再栅格任何字形」钉成断言，而不是注释里的一次手工计数。
    // 恒开（不挂 env）：读一次 thread_local + `Cell` 自增，落在本来就要
    // 分配像素缓冲的分支里，代价可忽略；挂 env 反而测不稳——并行测试下
    // 谁先初始化门控，决定别人有没有计数。
    static GLYPH_MISSES: Cell<usize> = Cell::new(0);
}

/// 取走并清零本线程的字形栅格未命中计数。
pub fn take_glyph_probe_misses() -> usize {
    GLYPH_MISSES.with(|c| {
        let n = c.get();
        c.set(0);
        n
    })
}

fn draw(
    font_system: &mut cosmic_text::FontSystem,
    glyph_cache: &mut GlyphCache,
    buffer: &cosmic_text::Buffer,
    position: Point,
    color: Color,
    pixels: &mut tiny_skia::PixmapMut<'_>,
    clip_mask: Option<&tiny_skia::Mask>,
    transformation: Transformation,
) {
    let position = position * transformation;

    // 目标像素图尺寸：P287 的窗口外落笔判据只用它（掩码矩形在这一层拿不到）。
    let (pw, ph) = (pixels.width() as i32, pixels.height() as i32);

    SWASH_CACHE.with(|swash_cell| {
        let mut swash = swash_cell.borrow_mut();

        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                let physical_glyph = glyph.physical(
                    (position.x, position.y),
                    transformation.scale_factor(),
                );

                if let Some((buffer, placement)) = glyph_cache.allocate(
                    physical_glyph.cache_key,
                    glyph.color_opt.map(from_color).unwrap_or(color),
                    font_system,
                    &mut swash,
                ) {
                    let pixmap = tiny_skia::PixmapRef::from_bytes(
                        buffer,
                        placement.width,
                        placement.height,
                    )
                    .expect("Create glyph pixel map");

                    let bx = physical_glyph.x + placement.left;
                    let by = physical_glyph.y - placement.top
                        + (run.line_y * transformation.scale_factor()).round() as i32;
                    // P287：位图整体落在目标像素图之外的落笔**注定什么都不写**，
                    // 直接跳过。判据只读落笔矩形与目标尺寸，不改任何落笔参数 ⇒
                    // 像素与不跳完全相同（守卫用例用 `set_blit_cull_for_test(false)`
                    // 当同帧老口径 oracle 逐像素对拍）。关态长行才吃得到：实测
                    // 一帧 105/205 次落笔在窗口外（普通行与折行为 0）。
                    let outside = bx + placement.width as i32 <= 0
                        || by + placement.height as i32 <= 0
                        || bx >= pw
                        || by >= ph;
                    if outside && blit_cull_on() {
                        GLYPH_CULLED.with(|c| c.set(c.get() + 1));
                        continue;
                    }
                    let opacity = color.a
                        * glyph
                            .color_opt
                            .map(|c| c.a() as f32 / 255.0)
                            .unwrap_or(1.0);

                    pixels.draw_pixmap(
                        bx,
                        by,
                        pixmap,
                        &tiny_skia::PixmapPaint {
                            opacity,
                            ..tiny_skia::PixmapPaint::default()
                        },
                        tiny_skia::Transform::identity(),
                        clip_mask,
                    );
                }
            }
        }
    });
}

fn from_color(color: cosmic_text::Color) -> Color {
    let [r, g, b, a] = color.as_rgba();

    Color::from_rgba8(r, g, b, a as f32 / 255.0)
}

#[derive(Debug, Clone, Default)]
struct GlyphCache {
    entries: FxHashMap<
        (cosmic_text::CacheKey, [u8; 3]),
        (Vec<u32>, cosmic_text::Placement),
    >,
    recently_used: FxHashSet<(cosmic_text::CacheKey, [u8; 3])>,
    trim_count: usize,
}

impl GlyphCache {
    const TRIM_INTERVAL: usize = 300;
    const CAPACITY_LIMIT: usize = 16 * 1024;

    fn new() -> Self {
        GlyphCache::default()
    }

    fn allocate(
        &mut self,
        cache_key: cosmic_text::CacheKey,
        color: Color,
        font_system: &mut cosmic_text::FontSystem,
        swash: &mut cosmic_text::SwashCache,
    ) -> Option<(&[u8], cosmic_text::Placement)> {
        let [r, g, b, _a] = color.into_rgba8();
        let key = (cache_key, [r, g, b]);

        if let hash_map::Entry::Vacant(entry) = self.entries.entry(key) {
            GLYPH_MISSES.with(|c| c.set(c.get() + 1));
            // EDITPAD_GLYPH_PROBE=1：打出未命中的字形键。关心它是因为 `cache_key`
            // 含亚像素分箱、`key` 又含颜色三元组 ⇒ 滚动或高亮配色都可能让同一个
            // 视觉字形变成新键、每帧重栅格。
            if std::env::var_os("EDITPAD_GLYPH_PROBE").is_some() {
                let sub = (cache_key.glyph_id, cache_key.x_bin, cache_key.y_bin);
                eprintln!("GLYPHMISS {sub:?} rgb {r} {g} {b}");
            }
            // TODO: Outline support
            let Some(image) = swash.get_image_uncached(font_system, cache_key) else {
                // P285：连"取不到图"也要记进缓存（空 buffer = 没什么可画）。
                // 早退的话每帧都会重来一次 `get_image_uncached`（含分配）。
                let _ = entry.insert((Vec::new(), cosmic_text::Placement::default()));
                return None;
            };

            let glyph_size = image.placement.width as usize
                * image.placement.height as usize;

            if glyph_size == 0 {
                // P285：空格这类"有放置、零面积"的字形以前直接 `return None`，
                // 于是**永远不进缓存**——同一份内容重画一帧要为它们重跑一遍
                // `get_image_uncached`（含分配）。本夹具实测：热帧未命中 42 次，
                // 而像素与冷帧分毫不动 ⇒ 这 42 次栅格产不出一个像素。
                // 这里插一条空 buffer 当负缓存，读取侧见下面的 `and_then`。
                let _ = entry.insert((Vec::new(), image.placement));
                return None;
            }

            let mut buffer = vec![0u32; glyph_size];

            match image.content {
                cosmic_text::SwashContent::Mask => {
                    let mut i = 0;

                    // TODO: Blend alpha

                    for _y in 0..image.placement.height {
                        for _x in 0..image.placement.width {
                            buffer[i] = bytemuck::cast(
                                tiny_skia::ColorU8::from_rgba(
                                    b,
                                    g,
                                    r,
                                    image.data[i],
                                )
                                .premultiply(),
                            );

                            i += 1;
                        }
                    }
                }
                cosmic_text::SwashContent::Color => {
                    let mut i = 0;

                    for _y in 0..image.placement.height {
                        for _x in 0..image.placement.width {
                            // TODO: Blend alpha
                            buffer[i >> 2] = bytemuck::cast(
                                tiny_skia::ColorU8::from_rgba(
                                    image.data[i + 2],
                                    image.data[i + 1],
                                    image.data[i],
                                    image.data[i + 3],
                                )
                                .premultiply(),
                            );

                            i += 4;
                        }
                    }
                }
                cosmic_text::SwashContent::SubpixelMask => {
                    // TODO
                }
            }

            let _ = entry.insert((buffer, image.placement));
        }

        let _ = self.recently_used.insert(key);

        self.entries.get(&key).and_then(|(buffer, placement)| {
            // 空 buffer = 负缓存命中（空格/取不到图）：不进栅格，也不交给
            // `draw_pixmap`。像素与改前完全一致（这类字形本来就不落一个像素）。
            (!buffer.is_empty()).then(|| (bytemuck::cast_slice(buffer.as_slice()), *placement))
        })
    }

    pub fn trim(&mut self) {
        if self.trim_count > Self::TRIM_INTERVAL
            || self.recently_used.len() >= Self::CAPACITY_LIMIT
        {
            self.entries
                .retain(|key, _| self.recently_used.contains(key));

            self.recently_used.clear();

            self.entries.shrink_to(Self::CAPACITY_LIMIT);
            self.recently_used.shrink_to(Self::CAPACITY_LIMIT);

            self.trim_count = 0;
        } else {
            self.trim_count += 1;
        }
    }
}
