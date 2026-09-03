//! Property-based tests for the shaping API.
//!
//! A failing case is shrunk, printed, and saved under `.hegel/` to be replayed
//! on the next run. Composite inputs are drawn with `draw_silent` and recorded
//! with `tc.note`, so the report names the font, the text, the settings, the
//! features and the variations rather than every draw that built them. A test
//! that adjusts a drawn input before using it draws the `_silent` variant and
//! notes the adjusted value. A `return` before the assertion counts the case
//! as passed. A failed `tc.assume` discards it and draws another.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use harfrust::font::{Font, FontInstance};
use harfrust::{
    script, BufferClusterLevel, BufferFlags, Direction, Feature, FontRef, GlyphBuffer, GlyphFlags,
    Language, SerializeFlags, ShapeOptions, ShapePlan, ShaperData, ShaperInstance, Tag,
    UnicodeBuffer, Variation,
};
use hegel::generators;
use read_fonts::TableProvider;

/// Fonts from benches/fonts and tests/fonts. Between them they carry GSUB and
/// GPOS for Latin, Arabic (Amiri and the Nastaliq Urdu), Devanagari, Kannada,
/// Balinese, Tai Tham, Javanese and Ethiopic, plus Malayalam, Sinhala,
/// Myanmar, Gujarati and CJK subsets that map a handful of characters each.
/// They also cover AAT `morx` and `trak`, a `kern` table, `fvar` and `CFF2`
/// variable fonts, CFF outlines, a format 14 cmap, and a cmap that maps only
/// private-use codepoints (`TradArabicTest.ttf`). No font here has `kerx`,
/// and none maps Hebrew, Hangul or Khmer, so those shapers only ever see
/// `.notdef`.
const PROPERTY_FONTS: &[&str] = &[
    "benches/fonts/Roboto-Regular.ttf",
    "benches/fonts/Amiri-Regular.ttf",
    "benches/fonts/NotoSansDevanagari-Regular.ttf",
    "benches/fonts/NotoNastaliqUrdu-Regular.ttf",
    "benches/fonts/SourceSerifVariable-Roman.ttf",
    "tests/fonts/rb_custom/OpenSans.subset1.ttf",
    "tests/fonts/rb_custom/NotoSansCJK.subset1.otf",
    "tests/fonts/rb_custom/NotoSansMalayalam.subset1.ttf",
    "tests/fonts/rb_custom/NotoSansMyanmarUI-Regular.subset1.otf",
    "tests/fonts/rb_custom/NotoSansSinhala.subset1.otf",
    "tests/fonts/rb_custom/Rasa.subset1.otf",
    "tests/fonts/rb_custom/BungeeTint-Regular.ttf",
    "tests/fonts/in-house/HBTest-VF.ttf",
    "tests/fonts/in-house/TRAK.ttf",
    "tests/fonts/in-house/MORXTwentyeight.ttf",
    "tests/fonts/in-house/NotoSerifHK-subset.ttf",
    "tests/fonts/in-house/FallbackPlus-Javanese-no-GDEF.otf",
    "tests/fonts/in-house/TradArabicTest.ttf",
    "tests/fonts/text-rendering-tests/NotoSansBalinese-Regular.ttf",
    "tests/fonts/text-rendering-tests/NotoSansKannada-Regular.ttf",
    "tests/fonts/text-rendering-tests/AdobeVFPrototype-Subset.otf",
    "tests/fonts/text-rendering-tests/TestShapeEthi.ttf",
    "tests/fonts/text-rendering-tests/TestShapeLana.ttf",
    "tests/fonts/text-rendering-tests/TestCMAP14.otf",
    "tests/fonts/text-rendering-tests/TestAATMorx.ttf",
    "tests/fonts/text-rendering-tests/TestKERNOne.otf",
    "tests/fonts/text-rendering-tests/TestGPOSFour.ttf",
    "tests/fonts/text-rendering-tests/Zycon.ttf",
    "tests/fonts/aots/gsub4_1_multiple_ligatures_f1.otf",
    "tests/fonts/aots/gpos2_1_lookupflag_f1.otf",
];

fn font_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn font_bytes() -> &'static [Vec<u8>] {
    static BYTES: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    BYTES.get_or_init(|| {
        PROPERTY_FONTS
            .iter()
            .map(|p| fs::read(font_path(p)).unwrap_or_else(|e| panic!("read {p}: {e}")))
            .collect()
    })
}

/// Parsed fonts with their shaping caches, in `PROPERTY_FONTS` order.
///
/// Built once rather than once per case: `ShaperData` construction resolves
/// every layout table, which would otherwise dominate the runtime.
fn test_fonts() -> &'static [(FontRef<'static>, ShaperData)] {
    static FONTS: OnceLock<Vec<(FontRef<'static>, ShaperData)>> = OnceLock::new();
    FONTS.get_or_init(|| {
        font_bytes()
            .iter()
            .zip(PROPERTY_FONTS)
            .map(|(bytes, name)| {
                let font = FontRef::new(bytes).unwrap_or_else(|e| panic!("parse {name}: {e}"));
                let data = ShaperData::new(&font);
                (font, data)
            })
            .collect()
    })
}

fn draw_font_index(tc: &hegel::TestCase) -> usize {
    let index = tc.draw_silent(generators::integers::<usize>().max_value(PROPERTY_FONTS.len() - 1));
    tc.note(&format!("font = {}", PROPERTY_FONTS[index]));
    index
}

/// Unicode blocks at least one of `PROPERTY_FONTS` maps, plus a few generic
/// ones. The complex shapers only engage when several characters of the same
/// script appear in a run, so most characters of a generated string come from
/// one block.
const SCRIPT_BLOCKS: &[(u32, u32)] = &[
    (0x0020, 0x007E),   // ASCII
    (0x00A0, 0x00FF),   // Latin-1
    (0x0300, 0x036F),   // combining marks
    (0x0600, 0x06FF),   // Arabic
    (0x0900, 0x097F),   // Devanagari
    (0x0A80, 0x0AFF),   // Gujarati
    (0x0C80, 0x0CFF),   // Kannada
    (0x0D00, 0x0D7F),   // Malayalam
    (0x0D80, 0x0DFF),   // Sinhala
    (0x0E00, 0x0E7F),   // Thai
    (0x1000, 0x109F),   // Myanmar
    (0x1200, 0x137F),   // Ethiopic
    (0x1A20, 0x1AAF),   // Tai Tham
    (0x1B00, 0x1B7F),   // Balinese
    (0x4E00, 0x9FFF),   // CJK unified ideographs
    (0xA980, 0xA9DF),   // Javanese
    (0x1F300, 0x1F6FF), // emoji
];

/// The entries of `SCRIPT_BLOCKS` each font maps at least one character of, in
/// `PROPERTY_FONTS` order. A font here typically maps ASCII and one script, so
/// a block drawn without regard to the font left two runs in three entirely
/// `.notdef`. Drawing from these leaves under half, since the subsets map only
/// a few characters of their block and a fifth of the text stays arbitrary.
fn mapped_blocks() -> &'static [Vec<(u32, u32)>] {
    static BLOCKS: OnceLock<Vec<Vec<(u32, u32)>>> = OnceLock::new();
    BLOCKS.get_or_init(|| {
        test_fonts()
            .iter()
            .map(|(font, _)| {
                let cmap = font.cmap().ok();
                SCRIPT_BLOCKS
                    .iter()
                    .copied()
                    .filter(|&(lo, hi)| {
                        (lo..=hi).any(|c| {
                            cmap.as_ref()
                                .and_then(|cmap| cmap.map_codepoint(c))
                                .is_some_and(|glyph| glyph.to_u32() != 0)
                        })
                    })
                    .collect()
            })
            .collect()
    })
}

/// Characters the shapers give special meaning to: joiners, default
/// ignorables, variation selectors, dotted circle, tatweel, matras, and
/// whitespace.
const SPECIAL_CHARS: &[char] = &[
    '\u{200C}', '\u{200D}', '\u{00AD}', '\u{25CC}', '\u{FE00}', '\u{FE0F}', '\u{FFFD}', '\u{034F}',
    '\u{180B}', '\n', '\r', '\t', '\u{0640}', '\u{093F}', '\u{0E33}', '\u{1B35}', '\u{0}', ' ',
];

fn draw_codepoint_in(tc: &hegel::TestCase, lo: u32, hi: u32) -> char {
    let n = tc.draw_silent(generators::integers::<u32>().min_value(lo).max_value(hi));
    char::from_u32(n).unwrap_or('\u{FFFD}')
}

/// Draws text biased towards a single script run that `PROPERTY_FONTS[font]`
/// maps, with special characters and fully arbitrary codepoints mixed in.
fn draw_text(tc: &hegel::TestCase, font: usize, name: &str, max_len: usize) -> String {
    let text = draw_text_silent(tc, font, max_len);
    tc.note(&format!("{name} = {text:?}"));
    text
}

fn draw_text_silent(tc: &hegel::TestCase, font: usize, max_len: usize) -> String {
    if tc.draw_silent(generators::integers::<u8>().max_value(4)) == 0 {
        return tc.draw_silent(generators::text().max_size(max_len));
    }
    let mapped = &mapped_blocks()[font];
    // One block in five is drawn without regard to the font, so the shapers
    // still see runs of a script the font lacks.
    let block =
        if mapped.is_empty() || tc.draw_silent(generators::integers::<u8>().max_value(4)) == 0 {
            tc.draw_silent(generators::sampled_from(SCRIPT_BLOCKS.to_vec()))
        } else {
            tc.draw_silent(generators::sampled_from(mapped.clone()))
        };
    let len = tc.draw_silent(generators::integers::<usize>().max_value(max_len));
    let mut text = String::new();
    for _ in 0..len {
        text.push(
            // Weighted rather than a `one_of!`: eight of the ten indices
            // fall through to the block, so most of the text is in the
            // script under test.
            match tc.draw_silent(generators::integers::<u8>().max_value(9)) {
                0 => tc.draw_silent(generators::sampled_from(SPECIAL_CHARS.to_vec())),
                1 => draw_codepoint_in(tc, 0, 0x10_FFFF),
                _ => draw_codepoint_in(tc, block.0, block.1),
            },
        );
    }
    text
}

const COMMON_FEATURE_TAGS: &[&[u8; 4]] = &[
    b"kern", b"liga", b"dlig", b"smcp", b"calt", b"ccmp", b"mark", b"mkmk", b"rlig", b"init",
    b"medi", b"fina", b"isol", b"ss01", b"aalt", b"vert", b"frac", b"locl", b"test",
];

const VARIATION_TAGS: &[&[u8; 4]] = &[b"wght", b"wdth", b"slnt", b"ital", b"opsz", b"CNTR"];

fn draw_tag(tc: &hegel::TestCase, common: &[&'static [u8; 4]]) -> Tag {
    if tc.draw_silent(generators::booleans()) {
        Tag::new(tc.draw_silent(generators::sampled_from(common.to_vec())))
    } else {
        Tag::new(&tc.draw_silent(generators::arrays(generators::integers::<u8>())))
    }
}

fn draw_features(tc: &hegel::TestCase) -> Vec<Feature> {
    let features = draw_features_silent(tc);
    tc.note(&format!("features = {features:?}"));
    features
}

fn draw_features_silent(tc: &hegel::TestCase) -> Vec<Feature> {
    let n = tc.draw_silent(generators::integers::<usize>().max_value(3));
    (0..n)
        .map(|_| {
            let tag = draw_tag(tc, COMMON_FEATURE_TAGS);
            let value = tc.draw_silent(hegel::one_of!(
                generators::integers::<u32>().max_value(1),
                generators::integers::<u32>(),
            ));
            // A feature is global unless it carries a range, and the two are
            // handled by different code paths, so generate both.
            let (start, end) = if tc.draw_silent(generators::booleans()) {
                (0, u32::MAX)
            } else {
                (
                    tc.draw_silent(generators::integers::<u32>()),
                    tc.draw_silent(generators::integers::<u32>()),
                )
            };
            Feature {
                tag,
                value,
                start,
                end,
            }
        })
        .collect()
}

fn draw_variations(tc: &hegel::TestCase) -> Vec<Variation> {
    let n = tc.draw_silent(generators::integers::<usize>().max_value(3));
    let variations: Vec<Variation> = (0..n)
        .map(|_| Variation {
            tag: draw_tag(tc, VARIATION_TAGS),
            value: tc.draw_silent(generators::floats::<f32>()),
        })
        .collect();
    tc.note(&format!("variations = {variations:?}"));
    variations
}

const DIRECTIONS: &[Direction] = &[
    Direction::LeftToRight,
    Direction::RightToLeft,
    Direction::TopToBottom,
    Direction::BottomToTop,
];

const CLUSTER_LEVELS: &[BufferClusterLevel] = &[
    BufferClusterLevel::MonotoneGraphemes,
    BufferClusterLevel::MonotoneCharacters,
    BufferClusterLevel::Characters,
    BufferClusterLevel::Graphemes,
];

/// Scripts set on the buffer. Hebrew, Hangul and Khmer have no font here that
/// maps them, but the shaper is chosen by the script alone
/// (src/hb/ot_shaper.rs), so setting them still runs those shapers.
const SCRIPTS: &[harfrust::Script] = &[
    script::LATIN,
    script::ARABIC,
    script::HEBREW,
    script::DEVANAGARI,
    script::KANNADA,
    script::MALAYALAM,
    script::SINHALA,
    script::MYANMAR,
    script::KHMER,
    script::BALINESE,
    script::TAI_THAM,
    script::JAVANESE,
    script::ETHIOPIC,
    script::HAN,
    script::HANGUL,
    script::THAI,
    script::COMMON,
    script::UNKNOWN,
];

const LANGUAGES: &[&str] = &["en", "ar", "hi", "tr", "ja", "URD", "zh-cn", "az-ir", "sa"];

/// The buffer settings that shaping reads, all of them generated.
#[derive(Debug, Clone)]
struct Settings {
    direction: Direction,
    script: Option<harfrust::Script>,
    language: Option<Language>,
    cluster_level: BufferClusterLevel,
    flags: BufferFlags,
    pre_context: Option<String>,
    post_context: Option<String>,
    not_found_variation_selector_glyph: Option<u32>,
}

fn draw_settings(tc: &hegel::TestCase, font: usize) -> Settings {
    let settings = draw_settings_silent(tc, font);
    tc.note(&format!("settings = {settings:?}"));
    settings
}

fn draw_settings_silent(tc: &hegel::TestCase, font: usize) -> Settings {
    // A direction is required: shaping without one is refused (see
    // `shaping_without_a_direction_is_refused` in tests/buffer.rs).
    let direction = tc.draw_silent(generators::sampled_from(DIRECTIONS.to_vec()));
    let script = tc.draw_silent(generators::optional(generators::sampled_from(
        SCRIPTS.to_vec(),
    )));
    let language = tc
        .draw_silent(generators::optional(generators::sampled_from(
            LANGUAGES.to_vec(),
        )))
        .and_then(Language::new);
    let cluster_level = tc.draw_silent(generators::sampled_from(CLUSTER_LEVELS.to_vec()));
    let flags = BufferFlags::from_bits_truncate(tc.draw_silent(generators::integers::<u32>()));
    let pre_context = if tc.draw_silent(generators::booleans()) {
        Some(draw_text_silent(tc, font, 8))
    } else {
        None
    };
    let post_context = if tc.draw_silent(generators::booleans()) {
        Some(draw_text_silent(tc, font, 8))
    } else {
        None
    };
    // Values above `u16::MAX` are excluded here because they end up in
    // `GlyphInfo::glyph_id`, which is documented to stay within `u16::MAX`;
    // see `known_failure_not_found_variation_selector_glyph_exceeds_u16` for
    // the pinned case.
    let not_found_variation_selector_glyph = tc.draw_silent(generators::optional(
        generators::integers::<u32>().max_value(u32::from(u16::MAX)),
    ));
    Settings {
        direction,
        script,
        language,
        cluster_level,
        flags,
        pre_context,
        post_context,
        not_found_variation_selector_glyph,
    }
}

/// `Direction::is_forward` is crate-private, so mirror it here.
fn is_forward(direction: Direction) -> bool {
    matches!(direction, Direction::LeftToRight | Direction::TopToBottom)
}

fn fill_buffer(text: &str, s: &Settings) -> UnicodeBuffer {
    let mut buffer = UnicodeBuffer::new();
    buffer.push_str(text);
    apply_settings(buffer, s)
}

/// Everything shaping produces, in a form two runs can be compared by.
fn output(glyphs: &GlyphBuffer) -> Vec<(u32, u32, u32, i32, i32, i32, i32)> {
    glyphs
        .glyph_infos()
        .iter()
        .zip(glyphs.glyph_positions())
        .map(|(i, p)| {
            (
                i.glyph_id,
                i.cluster,
                i.flags().to_bits(),
                p.x_advance,
                p.y_advance,
                p.x_offset,
                p.y_offset,
            )
        })
        .collect()
}

/// Everything shaping produces except the glyph flags, which describe the run
/// as a whole rather than the glyph.
fn placement(glyphs: &GlyphBuffer) -> Vec<(u32, u32, i32, i32, i32, i32)> {
    glyphs
        .glyph_infos()
        .iter()
        .zip(glyphs.glyph_positions())
        .map(|(i, p)| {
            (
                i.glyph_id,
                i.cluster,
                p.x_advance,
                p.y_advance,
                p.x_offset,
                p.y_offset,
            )
        })
        .collect()
}

/// Property: shaping and serializing never panic, whatever the font, text,
/// features, variations, scale or buffer settings.
///
/// This is the contract the crate's own fuzz target asserts
/// (`fuzz/fuzz_targets/fuzz_shape.rs`), widened from that target's fixed text
/// and auto-detected properties to generated text and generated buffer
/// settings.
#[hegel::test]
fn shaping_never_panics(tc: hegel::TestCase) {
    let fonts = test_fonts();
    let index = draw_font_index(&tc);
    let (font, data) = &fonts[index];
    let variations = draw_variations(&tc);
    let instance = ShaperInstance::from_variations(font, &variations);
    let shaper = data.shaper(font).instance(Some(&instance)).build();

    let text = draw_text(&tc, index, "text", 32);
    let settings = draw_settings(&tc, index);
    let features = draw_features(&tc);
    let scale = tc.draw(generators::optional(generators::integers::<i32>()));
    let point_size = tc.draw(generators::optional(generators::floats::<f32>()));
    let options = ShapeOptions::new()
        .features(&features)
        .scale(scale)
        .point_size(point_size);

    let glyphs = shaper.shape(fill_buffer(&text, &settings), options);
    drop(glyphs.serialize(
        &shaper,
        SerializeFlags::GLYPH_EXTENTS | SerializeFlags::GLYPH_FLAGS,
    ));
}

/// Property: shaping the same input twice gives the same output.
///
/// The shaper caches lookups and cmap lookups across calls (`ShaperData`), so
/// a second run over the same input goes through warmed caches; the result
/// must not depend on that.
#[hegel::test]
fn shaping_is_deterministic(tc: hegel::TestCase) {
    let fonts = test_fonts();
    let index = draw_font_index(&tc);
    let (font, data) = &fonts[index];
    let variations = draw_variations(&tc);
    let instance = ShaperInstance::from_variations(font, &variations);
    let shaper = data.shaper(font).instance(Some(&instance)).build();

    let text = draw_text(&tc, index, "text", 32);
    let settings = draw_settings(&tc, index);
    let features = draw_features(&tc);

    let once = shaper.shape(
        fill_buffer(&text, &settings),
        ShapeOptions::new().features(&features),
    );
    let twice = shaper.shape(
        fill_buffer(&text, &settings),
        ShapeOptions::new().features(&features),
    );
    assert_eq!(output(&once), output(&twice));
}

/// Property: shaping with a plan built for the buffer's properties gives the
/// same result as letting `shape` build the plan itself.
///
/// `Shaper::shape_buffer` compiles exactly this plan when none is supplied, so
/// the two paths must agree. `a_matching_plan_shapes` in tests/buffer.rs
/// asserts the same thing for one fixed string.
#[hegel::test]
fn an_explicit_plan_matches_the_implicit_one(tc: hegel::TestCase) {
    let fonts = test_fonts();
    let index = draw_font_index(&tc);
    let (font, data) = &fonts[index];
    let shaper = data.shaper(font).build();

    let text = draw_text(&tc, index, "text", 32);
    let settings = draw_settings(&tc, index);
    let features = draw_features(&tc);

    let implicit = shaper.shape(
        fill_buffer(&text, &settings),
        ShapeOptions::new().features(&features),
    );

    let plan = ShapePlan::new(
        &shaper,
        settings.direction,
        settings.script,
        settings.language.as_ref(),
        &features,
    );
    let explicit = shaper.shape(
        fill_buffer(&text, &settings),
        ShapeOptions::new().features(&features).plan(Some(&plan)),
    );

    assert_eq!(output(&implicit), output(&explicit));
}

/// `PROPERTY_FONTS` through the `font::FontInstance` API, in the same order.
fn font_instances() -> &'static [FontInstance] {
    static INSTANCES: OnceLock<Vec<FontInstance>> = OnceLock::new();
    INSTANCES.get_or_init(|| {
        font_bytes()
            .iter()
            .map(|bytes| {
                let font = Font::new(bytes.clone(), 0).expect("model font should parse");
                FontInstance::builder(&font).build()
            })
            .collect()
    })
}

/// Property: shaping through `font::FontInstance` matches shaping through
/// `FontRef`.
///
/// The two entry points reach different implementations of the font-data
/// accessors (`FontKind::FontInstance` vs `FontKind::FontRef` in
/// src/hb/face.rs) and must produce the same glyphs.
/// `buffer_matches_typed_buffer_shaping` in tests/buffer.rs asserts the same
/// thing for one fixed string.
#[hegel::test]
fn the_font_instance_path_matches_the_font_ref_path(tc: hegel::TestCase) {
    let fonts = test_fonts();
    let instances = font_instances();
    let index = draw_font_index(&tc);
    let (font, data) = &fonts[index];
    let shaper = data.shaper(font).build();

    let text = draw_text(&tc, index, "text", 32);
    let settings = draw_settings(&tc, index);
    let features = draw_features(&tc);

    let via_font_ref = shaper.shape(
        fill_buffer(&text, &settings),
        ShapeOptions::new().features(&features),
    );
    let via_instance = harfrust::shape(
        &instances[index],
        fill_buffer(&text, &settings),
        ShapeOptions::new().features(&features),
    );
    assert_eq!(output(&via_font_ref), output(&via_instance));
}

/// Property: at the monotone cluster levels, output cluster values are
/// non-decreasing in a forward direction and non-increasing in a backward one.
///
/// This is what "monotone" in the level names means, and what
/// <https://harfbuzz.github.io/clusters.html> promises of levels 0 and 1.
#[hegel::test]
fn clusters_are_monotone_at_the_monotone_cluster_levels(tc: hegel::TestCase) {
    let fonts = test_fonts();
    let index = draw_font_index(&tc);
    let (font, data) = &fonts[index];
    let shaper = data.shaper(font).build();

    let text = draw_text(&tc, index, "text", 32);
    let mut settings = draw_settings_silent(&tc, index);
    settings.cluster_level = tc.draw_silent(generators::sampled_from(vec![
        BufferClusterLevel::MonotoneGraphemes,
        BufferClusterLevel::MonotoneCharacters,
    ]));
    tc.note(&format!("settings = {settings:?}"));
    let features = draw_features(&tc);

    let glyphs = shaper.shape(
        fill_buffer(&text, &settings),
        ShapeOptions::new().features(&features),
    );
    let clusters: Vec<u32> = glyphs.glyph_infos().iter().map(|i| i.cluster).collect();
    let forward = is_forward(settings.direction);
    for pair in clusters.windows(2) {
        if forward {
            assert!(pair[0] <= pair[1], "clusters not ascending: {clusters:?}");
        } else {
            assert!(pair[0] >= pair[1], "clusters not descending: {clusters:?}");
        }
    }
}

/// Property: every output cluster value is the UTF-8 byte offset of one of the
/// input characters.
///
/// `Buffer::push_str`, which `UnicodeBuffer::push_str` forwards to, documents
/// that it assigns each character's byte offset as its cluster value, and
/// shaping only ever merges clusters (taking the minimum) or copies them, so
/// no other value can appear.
#[hegel::test]
fn clusters_are_input_character_offsets(tc: hegel::TestCase) {
    let fonts = test_fonts();
    let index = draw_font_index(&tc);
    let (font, data) = &fonts[index];
    let shaper = data.shaper(font).build();

    let text = draw_text(&tc, index, "text", 32);
    let settings = draw_settings(&tc, index);
    let features = draw_features(&tc);

    let glyphs = shaper.shape(
        fill_buffer(&text, &settings),
        ShapeOptions::new().features(&features),
    );
    let offsets: HashSet<u32> = text.char_indices().map(|(i, _)| i as u32).collect();
    for info in glyphs.glyph_infos() {
        assert!(
            offsets.contains(&info.cluster),
            "cluster {} is not a character offset of {text:?}",
            info.cluster
        );
    }
}

/// Property: at a cluster start that is not flagged unsafe-to-break, shaping
/// the two halves of the text separately and concatenating gives the same
/// glyphs and positions as shaping the whole.
///
/// This is what `GlyphFlags::UNSAFE_TO_BREAK` documents: without the flag
/// "it's safe to break the glyph-run at the beginning of this cluster, and the
/// two sides represent the exact same result one would get if breaking input
/// text at the beginning of this cluster and shaping the two sides
/// separately".
///
/// The comparison leaves out the glyph flags: `UNSAFE_TO_CONCAT` describes
/// whether a run can be joined onto its neighbours, which is a different
/// question for a half than for the whole.
///
/// Ignored because it does not hold, and HarfBuzz does not uphold it either:
/// see `known_failure_an_unflagged_break_changes_an_arabic_joining_form`.
/// Fifteen fresh 20000-case runs shrank to five kinds of case. A cluster's
/// Arabic joining form changes. `À` under the Indic and Khmer shapers composes
/// only when a variation selector is elsewhere in the run. A half made only
/// of default ignorables merges into one cluster, or a default ignorable swaps
/// places with the mark after it. A variation selector with no base takes the
/// buffer's not-found glyph in the whole run and not on its own. A mark's
/// fallback offset changes with what follows it.
/// Failing cases are rare, so run it as
/// `HEGEL_TEST_CASES=20000 cargo test --test shaping -- --ignored safe_break`.
/// It is done when that run passes, or when the flag's documentation is
/// narrowed to what the analysis covers.
#[hegel::test]
#[ignore = "the unsafe-to-break flag does not cover every context that changes the result"]
fn safe_break_positions_shape_independently(tc: hegel::TestCase) {
    let fonts = test_fonts();
    let index = draw_font_index(&tc);
    let (font, data) = &fonts[index];
    let shaper = data.shaper(font).build();

    let text = draw_text(&tc, index, "text", 32);
    let mut settings = draw_settings_silent(&tc, index);
    // The beginning/end-of-text flags describe the whole run, so they
    // would legitimately differ between the halves and the whole.
    settings.flags &= !(BufferFlags::BEGINNING_OF_TEXT | BufferFlags::END_OF_TEXT);
    // Splitting the text moves what the context is; keep it unset so the
    // halves see the same context the whole run did.
    settings.pre_context = None;
    settings.post_context = None;
    // A monotone cluster level is what makes "the beginning of a cluster"
    // a single position: at the other levels a cluster value can turn up
    // in several places in the run, so there is no one break to test.
    settings.cluster_level = tc.draw_silent(generators::sampled_from(vec![
        BufferClusterLevel::MonotoneGraphemes,
        BufferClusterLevel::MonotoneCharacters,
    ]));
    tc.note(&format!("settings = {settings:?}"));
    let features = draw_features(&tc);
    let options = || ShapeOptions::new().features(&features);

    // Output is in visual order, so a backward run has to be turned back
    // into logical order before its clusters can be lined up with the
    // text's byte offsets or with a second run's.
    let forward = is_forward(settings.direction);
    let logical = |glyphs: &GlyphBuffer| {
        let mut items = placement(glyphs);
        if !forward {
            items.reverse();
        }
        items
    };

    let whole = shaper.shape(fill_buffer(&text, &settings), options());
    let items = logical(&whole);
    // The flag is set on every glyph of an unsafe cluster, so a cluster is
    // safe to break at when none of its glyphs carries it.
    let unsafe_clusters: HashSet<u32> = whole
        .glyph_infos()
        .iter()
        .filter(|info| info.unsafe_to_break())
        .map(|info| info.cluster)
        .collect();
    let breaks: Vec<usize> = items
        .windows(2)
        .filter(|pair| pair[0].1 != pair[1].1 && !unsafe_clusters.contains(&pair[1].1))
        .map(|pair| pair[1].1 as usize)
        .filter(|split| *split > 0 && *split < text.len())
        .collect();
    if breaks.is_empty() {
        return;
    }
    let split = breaks[tc.draw_silent(generators::integers::<usize>().max_value(breaks.len() - 1))];
    tc.note(&format!("split at byte {split}"));

    let head = shaper.shape(fill_buffer(&text[..split], &settings), options());
    let tail = shaper.shape(fill_buffer(&text[split..], &settings), options());

    // Clusters in the tail restart from zero, so shift them back onto the
    // whole run's offsets before comparing.
    let mut joined = logical(&head);
    joined.extend(logical(&tail).into_iter().map(|mut item| {
        item.1 += split as u32;
        item
    }));
    assert_eq!(items, joined);
}

/// Property: a font whose bytes have been truncated or corrupted either fails
/// to parse or shapes without panicking.
///
/// Font data is untrusted input; this is the contract
/// `fuzz/fuzz_targets/fuzz_shape.rs` asserts, with mutations of the repo's own
/// fonts standing in for the fuzzer's corpus.
#[hegel::test]
fn shaping_mutated_fonts_never_panics(tc: hegel::TestCase) {
    let index = draw_font_index(&tc);
    let mut bytes = font_bytes()[index].clone();
    // The sfnt header and table directory (12 bytes, then 16 per table) are
    // left alone. `FontRef::new` rejects a damaged directory before shaping
    // sees it, which tests read-fonts rather than the shaper. With them
    // intact every case parses, where 41% did not before.
    let directory_len = 12 + 16 * usize::from(u16::from_be_bytes([bytes[4], bytes[5]]));
    let truncate = tc.draw(generators::booleans());
    if truncate {
        let len = tc.draw(
            generators::integers::<usize>()
                .min_value(directory_len)
                .max_value(bytes.len()),
        );
        bytes.truncate(len);
    }
    let flips = tc.draw(generators::integers::<usize>().max_value(16));
    for _ in 0..flips {
        if bytes.len() <= directory_len {
            break;
        }
        let at = tc.draw(
            generators::integers::<usize>()
                .min_value(directory_len)
                .max_value(bytes.len() - 1),
        );
        let byte = tc.draw(generators::integers::<u8>());
        bytes[at] = byte;
    }

    let Ok(font) = FontRef::new(&bytes) else {
        return;
    };
    let data = ShaperData::new(&font);
    let variations = draw_variations(&tc);
    let instance = ShaperInstance::from_variations(&font, &variations);
    let shaper = data.shaper(&font).instance(Some(&instance)).build();

    let text = draw_text(&tc, index, "text", 16);
    let settings = draw_settings(&tc, index);
    let features = draw_features(&tc);
    let glyphs = shaper.shape(
        fill_buffer(&text, &settings),
        ShapeOptions::new().features(&features),
    );
    drop(glyphs.serialize(&shaper, SerializeFlags::GLYPH_EXTENTS));
}

/// Property: shaping into a buffer recycled with `GlyphBuffer::clear` gives
/// the same result as shaping into a fresh one.
///
/// `clear` is documented as the way to get the `UnicodeBuffer` back "without
/// allocating a new one", so nothing of the previous run may survive it. It
/// resets direction, script, language and cluster level, so the settings are
/// re-applied on both sides.
#[hegel::test]
fn a_recycled_buffer_matches_a_fresh_one(tc: hegel::TestCase) {
    let fonts = test_fonts();
    let index = draw_font_index(&tc);
    let (font, data) = &fonts[index];
    let shaper = data.shaper(font).build();

    let first = draw_text(&tc, index, "first", 32);
    let second = draw_text(&tc, index, "second", 32);
    let settings = draw_settings(&tc, index);
    let features = draw_features(&tc);
    let options = || ShapeOptions::new().features(&features);

    let used = shaper.shape(fill_buffer(&first, &settings), options());
    let mut recycled = used.clear();
    recycled.push_str(&second);
    let recycled = apply_settings(recycled, &settings);
    let recycled = shaper.shape(recycled, options());

    let fresh = shaper.shape(fill_buffer(&second, &settings), options());
    assert_eq!(output(&recycled), output(&fresh));
}

fn apply_settings(mut buffer: UnicodeBuffer, s: &Settings) -> UnicodeBuffer {
    buffer.set_direction(s.direction);
    if let Some(script) = s.script {
        buffer.set_script(script);
    }
    if let Some(language) = &s.language {
        buffer.set_language(language.clone());
    }
    buffer.set_cluster_level(s.cluster_level);
    buffer.set_flags(s.flags);
    if let Some(text) = &s.pre_context {
        buffer.set_pre_context(text);
    }
    if let Some(text) = &s.post_context {
        buffer.set_post_context(text);
    }
    if let Some(glyph) = s.not_found_variation_selector_glyph {
        buffer.set_not_found_variation_selector_glyph(glyph);
    }
    buffer
}

/// KNOWN FAILURE: breaking the run at a cluster the shaper flagged as safe
/// changes the Arabic joining form of the cluster before it.
///
/// `GlyphFlags::UNSAFE_TO_BREAK` promises that a break at an unflagged cluster
/// gives "the exact same result" as shaping the two sides separately. Here
/// U+0620 shapes to three glyphs in the whole run and to one on its own, while
/// the cluster the break lands on carries no flag. The first assertions check
/// that HarfBuzz 11.3.3, through `harfbuzz_rs`, gives the same glyphs,
/// clusters and flags for all three strings, so this is the flag's analysis
/// falling short rather than a porting mistake. It is done when the last
/// assertion passes, or when the flag's documentation is narrowed.
#[test]
#[ignore = "the unsafe-to-break flag does not cover joining context"]
fn known_failure_an_unflagged_break_changes_an_arabic_joining_form() {
    let bytes = fs::read(font_path("benches/fonts/NotoNastaliqUrdu-Regular.ttf")).unwrap();
    let font = FontRef::new(&bytes).unwrap();
    let data = ShaperData::new(&font);
    let shaper = data.shaper(&font).build();
    let face = harfbuzz_rs::Face::from_bytes(&bytes, 0);
    let hb_font = harfbuzz_rs::Font::new(face);

    let text = "\u{0620}\u{200C}\u{200C}";
    let split = 5;
    let shape = |text: &str| -> Vec<(u32, u32, u32)> {
        let mut buffer = UnicodeBuffer::new();
        buffer.push_str(text);
        buffer.set_direction(Direction::LeftToRight);
        buffer.set_script(script::SINHALA);
        let glyphs = shaper.shape(buffer, ShapeOptions::new());
        glyphs
            .glyph_infos()
            .iter()
            .map(|i| (i.glyph_id, i.cluster, i.flags().to_bits()))
            .collect()
    };
    let hb_shape = |text: &str| -> Vec<(u32, u32, u32)> {
        let buffer = harfbuzz_rs::UnicodeBuffer::new()
            .add_str(text)
            .set_direction(harfbuzz_rs::Direction::Ltr)
            .set_script(hb_tag(script::SINHALA.tag()));
        harfbuzz_rs::shape(&hb_font, buffer, &[])
            .get_glyph_infos()
            .iter()
            .map(|i| (i.codepoint, i.cluster, i.glyph_flags().0))
            .collect()
    };

    let whole = shape(text);
    let head = shape(&text[..split]);
    let tail = shape(&text[split..]);
    for (ours, text) in [
        (&whole, text),
        (&head, &text[..split]),
        (&tail, &text[split..]),
    ] {
        assert_eq!(
            *ours,
            hb_shape(text),
            "HarfBuzz shapes {text:?} differently"
        );
    }
    assert!(
        !whole.iter().any(|&(_, cluster, flags)| {
            cluster == split as u32 && flags & GlyphFlags::UNSAFE_TO_BREAK.to_bits() != 0
        }),
        "the break has to be flagged safe for the rest to mean anything"
    );

    let ids_and_clusters = |glyphs: &[(u32, u32, u32)]| -> Vec<(u32, u32)> {
        glyphs.iter().map(|g| (g.0, g.1)).collect()
    };
    let mut joined = ids_and_clusters(&head);
    joined.extend(
        ids_and_clusters(&tail)
            .into_iter()
            .map(|(id, cluster)| (id, cluster + split as u32)),
    );
    assert_eq!(ids_and_clusters(&whole), joined);
}

/// KNOWN FAILURE (harfbuzz/harfrust#409): a not-found variation-selector glyph
/// above `u16::MAX` reaches `GlyphInfo::glyph_id`, which is documented as
/// "Guarantee to be <= `u16::MAX`".
///
/// `deal_with_variation_selectors` (src/hb/ot_shape.rs) writes the value
/// straight into `glyph_id`. behdad's reply on harfbuzz/harfrust#409 to the
/// documented bound was "We should kill any such suggestions", so this pins
/// the disagreement between code and docs rather than a contract the project
/// has accepted, and it retires by deletion once that doc line goes.
#[test]
#[ignore = "harfbuzz/harfrust#409"]
fn known_failure_not_found_variation_selector_glyph_exceeds_u16() {
    let bytes = fs::read(font_path("tests/fonts/rb_custom/OpenSans.subset1.ttf")).unwrap();
    let font = FontRef::new(&bytes).unwrap();
    let data = ShaperData::new(&font);
    let shaper = data.shaper(&font).build();

    let mut buffer = UnicodeBuffer::new();
    buffer.push_str("A\u{FE00}");
    buffer.guess_segment_properties();
    buffer.set_not_found_variation_selector_glyph(0x1_0000);

    let glyphs: GlyphBuffer = shaper.shape(buffer, ShapeOptions::new());
    assert!(
        glyphs
            .glyph_infos()
            .iter()
            .all(|i| u16::try_from(i.glyph_id).is_ok()),
        "{:?}",
        glyphs.glyph_infos()
    );
}

/// Property: HarfRust picks the same glyphs, for the same clusters and with
/// the same glyph flags, as HarfBuzz.
///
/// This is the differential harness asked for in harfbuzz/harfrust#288, using
/// the `harfbuzz_rs` bindings already in the dev-dependencies. It is scoped to
/// what can be compared meaningfully through those bindings and against the
/// HarfBuzz they vendor:
///
/// - Glyph ids, clusters and flags only. Offsets diverge systematically
///   wherever fallback mark positioning runs, because `GlyphMetrics`
///   (src/hb/glyph_metrics.rs) reads extents from `glyf` only, so CFF glyphs
///   have no extents to position marks against.
/// - Global features only. `harfbuzz_rs::Feature::new` takes a cluster range
///   rather than raw `start`/`end`, so a generated range cannot be handed over
///   unambiguously.
/// - No buffer flags and cluster levels 0 to 2: `harfbuzz_rs` exposes neither
///   `hb_buffer_set_flags` nor cluster level 3.
///
/// Ignored because the oracle is behind: `harfbuzz_rs` vendors HarfBuzz
/// 11.3.3 while HarfRust matches 14.3.1 (README), so a divergence has to be
/// triaged against current HarfBuzz before it means anything. Divergences are
/// rare, so run it as
/// `HEGEL_TEST_CASES=3000 cargo test --test shaping -- --ignored harfbuzz`.
/// It stops being ignored when `harfbuzz_rs` vendors the HarfBuzz that
/// HarfRust matches.
#[hegel::test]
#[ignore = "oracle is the HarfBuzz 11.3.3 that harfbuzz_rs vendors; divergences need triage"]
fn matches_harfbuzz(tc: hegel::TestCase) {
    let fonts = test_fonts();
    let index = draw_font_index(&tc);
    let (font, data) = &fonts[index];
    let shaper = data.shaper(font).build();

    let text = draw_text(&tc, index, "text", 32);
    let mut settings = draw_settings_silent(&tc, index);
    settings.flags = BufferFlags::empty();
    settings.pre_context = None;
    settings.post_context = None;
    settings.not_found_variation_selector_glyph = None;
    tc.assume(settings.cluster_level != BufferClusterLevel::Graphemes);
    // HarfBuzz fills unset properties in from the locale, so pin both.
    let script = settings.script.unwrap_or(script::LATIN);
    settings.script = Some(script);
    let language = settings.language.clone().or_else(|| Language::new("en"));
    settings.language = language.clone();
    let language = language.unwrap();
    tc.note(&format!("settings = {settings:?}"));

    let features: Vec<Feature> = draw_features_silent(&tc)
        .into_iter()
        .map(|f| Feature {
            start: 0,
            end: u32::MAX,
            ..f
        })
        .collect();
    tc.note(&format!("features = {features:?}"));

    let ours: Vec<(u32, u32, u32)> = shaper
        .shape(
            fill_buffer(&text, &settings),
            ShapeOptions::new().features(&features),
        )
        .glyph_infos()
        .iter()
        .map(|i| (i.glyph_id, i.cluster, i.flags().to_bits()))
        .collect();

    let face = harfbuzz_rs::Face::from_bytes(&font_bytes()[index], 0);
    let hb_font = harfbuzz_rs::Font::new(face);
    let buffer = harfbuzz_rs::UnicodeBuffer::new()
        .add_str(&text)
        .set_direction(hb_direction(settings.direction))
        .set_script(hb_tag(script.tag()))
        .set_language(language.as_str().parse().expect("non-empty language"))
        .set_cluster_level(match settings.cluster_level {
            BufferClusterLevel::MonotoneGraphemes => harfbuzz_rs::ClusterLevel::MonotoneGraphemes,
            BufferClusterLevel::MonotoneCharacters => harfbuzz_rs::ClusterLevel::MonotoneCharacters,
            _ => harfbuzz_rs::ClusterLevel::Characters,
        });
    let hb_features: Vec<harfbuzz_rs::Feature> = features
        .iter()
        .map(|f| harfbuzz_rs::Feature::new(hb_tag(f.tag), f.value, ..))
        .collect();
    let shaped = harfbuzz_rs::shape(&hb_font, buffer, &hb_features);
    // Reading the glyph arrays of an empty HarfBuzz buffer through
    // `harfbuzz_rs` dereferences a null pointer, so stop short of it.
    let theirs: Vec<(u32, u32, u32)> = if shaped.is_empty() {
        Vec::new()
    } else {
        shaped
            .get_glyph_infos()
            .iter()
            .map(|i| (i.codepoint, i.cluster, i.glyph_flags().0))
            .collect()
    };

    assert_eq!(ours, theirs, "{}", PROPERTY_FONTS[index]);
}

fn hb_tag(tag: Tag) -> harfbuzz_rs::Tag {
    harfbuzz_rs::Tag(u32::from_be_bytes(tag.to_be_bytes()))
}

fn hb_direction(direction: Direction) -> harfbuzz_rs::Direction {
    match direction {
        Direction::Invalid => harfbuzz_rs::Direction::Invalid,
        Direction::LeftToRight => harfbuzz_rs::Direction::Ltr,
        Direction::RightToLeft => harfbuzz_rs::Direction::Rtl,
        Direction::TopToBottom => harfbuzz_rs::Direction::Ttb,
        Direction::BottomToTop => harfbuzz_rs::Direction::Btt,
    }
}
