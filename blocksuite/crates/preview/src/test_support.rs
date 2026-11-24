use crate::font_pool::FontPool;

/// Builds a deterministic test [`FontPool`] with zero system font scanning.
///
/// Registers bundled Typst text/math fonts, KaTeX math fonts, and a minimal
/// synthesized CJK TrueType font covering every CJK glyph used in tests.
pub(crate) fn build_test_font_pool() -> FontPool {
    let mut pool = FontPool::new();
    for font_bytes in typst_assets::fonts() {
        let _ = pool.register_font_bytes(font_bytes.to_vec());
    }
    for font_bytes in katex_font_blobs() {
        let _ = pool.register_font_bytes(font_bytes);
    }
    let cjk_ttf = synthesize_minimal_cjk_ttf(
        "TestCJK",
        "开始流程数据处理结束渲染用户系统发送请求返回预览发布计划光栅底座公式占比分布图表数学文档速度距离时间最大值当时中文测试",
    );
    pool.register_font_bytes(cjk_ttf)
        .expect("synthesized CJK test font must parse cleanly");
    pool
}

fn katex_font_blobs() -> Vec<Vec<u8>> {
    let names = [
        "KaTeX_AMS-Regular.ttf",
        "KaTeX_Caligraphic-Bold.ttf",
        "KaTeX_Caligraphic-Regular.ttf",
        "KaTeX_Fraktur-Bold.ttf",
        "KaTeX_Fraktur-Regular.ttf",
        "KaTeX_Main-Bold.ttf",
        "KaTeX_Main-BoldItalic.ttf",
        "KaTeX_Main-Italic.ttf",
        "KaTeX_Main-Regular.ttf",
        "KaTeX_Math-BoldItalic.ttf",
        "KaTeX_Math-Italic.ttf",
        "KaTeX_SansSerif-Bold.ttf",
        "KaTeX_SansSerif-Italic.ttf",
        "KaTeX_SansSerif-Regular.ttf",
        "KaTeX_Script-Regular.ttf",
        "KaTeX_Size1-Regular.ttf",
        "KaTeX_Size2-Regular.ttf",
        "KaTeX_Size3-Regular.ttf",
        "KaTeX_Size4-Regular.ttf",
        "KaTeX_Typewriter-Regular.ttf",
    ];
    names
        .iter()
        .filter_map(|name| ratex_katex_fonts::ttf_bytes(name))
        .map(|cow| cow.into_owned())
        .collect()
}

/// Synthesizes a compact, valid TrueType font containing simple filled box glyphs
/// for each unique BMP character in `chars`.
fn synthesize_minimal_cjk_ttf(family_name: &str, chars: &str) -> Vec<u8> {
    let mut codepoints: Vec<u16> = chars
        .chars()
        .filter_map(|ch| u16::try_from(ch as u32).ok())
        .collect();
    codepoints.sort_unstable();
    codepoints.dedup();

    // Glyph 0 = .notdef, Glyph 1 = shared filled CJK square outline.
    let num_glyphs: u16 = 2;
    let units_per_em: u16 = 1000;

    let head = build_head_table(units_per_em);
    let hhea = build_hhea_table(num_glyphs);
    let maxp = build_maxp_table(num_glyphs);
    let os2 = build_os2_table();
    let name = build_name_table(family_name);
    let post = build_post_table();
    let hmtx = build_hmtx_table(num_glyphs);
    let (glyf, loca) = build_glyf_and_loca();
    let cmap = build_cmap_format4(&codepoints);

    let mut tables: Vec<([u8; 4], Vec<u8>)> = vec![
        (*b"OS/2", os2),
        (*b"cmap", cmap),
        (*b"glyf", glyf),
        (*b"head", head),
        (*b"hhea", hhea),
        (*b"hmtx", hmtx),
        (*b"loca", loca),
        (*b"maxp", maxp),
        (*b"name", name),
        (*b"post", post),
    ];
    tables.sort_by_key(|(tag, _)| *tag);

    assemble_sfnt(&tables)
}

fn build_head_table(units_per_em: u16) -> Vec<u8> {
    let mut t = Vec::with_capacity(54);
    t.extend_from_slice(&0x0001_0000_u32.to_be_bytes()); // majorVersion=1, minorVersion=0
    t.extend_from_slice(&0x0001_0000_u32.to_be_bytes()); // fontRevision
    t.extend_from_slice(&0_u32.to_be_bytes()); // checksumAdjustment
    t.extend_from_slice(&0x5F0F_3CF5_u32.to_be_bytes()); // magicNumber
    t.extend_from_slice(&0x000B_u16.to_be_bytes()); // flags
    t.extend_from_slice(&units_per_em.to_be_bytes());
    t.extend_from_slice(&0_i64.to_be_bytes()); // created
    t.extend_from_slice(&0_i64.to_be_bytes()); // modified
    t.extend_from_slice(&100_i16.to_be_bytes()); // xMin
    t.extend_from_slice(&0_i16.to_be_bytes()); // yMin
    t.extend_from_slice(&900_i16.to_be_bytes()); // xMax
    t.extend_from_slice(&800_i16.to_be_bytes()); // yMax
    t.extend_from_slice(&0_u16.to_be_bytes()); // macStyle
    t.extend_from_slice(&8_u16.to_be_bytes()); // lowestRecPPEM
    t.extend_from_slice(&2_i16.to_be_bytes()); // fontDirectionHint
    t.extend_from_slice(&1_i16.to_be_bytes()); // indexToLocFormat (long 32-bit offsets)
    t.extend_from_slice(&0_i16.to_be_bytes()); // glyphDataFormat
    t
}

fn build_hhea_table(num_glyphs: u16) -> Vec<u8> {
    let mut t = Vec::with_capacity(36);
    t.extend_from_slice(&0x0001_0000_u32.to_be_bytes());
    t.extend_from_slice(&880_i16.to_be_bytes()); // ascender
    t.extend_from_slice(&(-120_i16).to_be_bytes()); // descender
    t.extend_from_slice(&0_i16.to_be_bytes()); // lineGap
    t.extend_from_slice(&1000_u16.to_be_bytes()); // advanceWidthMax
    t.extend_from_slice(&100_i16.to_be_bytes()); // minLeftSideBearing
    t.extend_from_slice(&100_i16.to_be_bytes()); // minRightSideBearing
    t.extend_from_slice(&900_i16.to_be_bytes()); // xMaxExtent
    t.extend_from_slice(&1_i16.to_be_bytes()); // caretSlopeRise
    t.extend_from_slice(&0_i16.to_be_bytes()); // caretSlopeRun
    t.extend_from_slice(&0_i16.to_be_bytes()); // caretOffset
    t.extend_from_slice(&[0u8; 8]); // reserved
    t.extend_from_slice(&0_i16.to_be_bytes()); // metricDataFormat
    t.extend_from_slice(&num_glyphs.to_be_bytes()); // numberOfHMetrics
    t
}

fn build_maxp_table(num_glyphs: u16) -> Vec<u8> {
    let mut t = Vec::with_capacity(32);
    t.extend_from_slice(&0x0001_0000_u32.to_be_bytes());
    t.extend_from_slice(&num_glyphs.to_be_bytes());
    t.extend_from_slice(&4_u16.to_be_bytes()); // maxPoints
    t.extend_from_slice(&1_u16.to_be_bytes()); // maxContours
    t.extend_from_slice(&0_u16.to_be_bytes()); // maxCompositePoints
    t.extend_from_slice(&0_u16.to_be_bytes()); // maxCompositeContours
    t.extend_from_slice(&1_u16.to_be_bytes()); // maxZones
    t.extend_from_slice(&0_u16.to_be_bytes()); // maxTwilightPoints
    t.extend_from_slice(&0_u16.to_be_bytes()); // maxStorage
    t.extend_from_slice(&0_u16.to_be_bytes()); // maxFunctionDefs
    t.extend_from_slice(&0_u16.to_be_bytes()); // maxInstructionDefs
    t.extend_from_slice(&0_u16.to_be_bytes()); // maxStackElements
    t.extend_from_slice(&0_u16.to_be_bytes()); // maxSizeOfInstructions
    t.extend_from_slice(&0_u16.to_be_bytes()); // maxComponentElements
    t.extend_from_slice(&0_u16.to_be_bytes()); // maxComponentDepth
    t
}

fn build_os2_table() -> Vec<u8> {
    let mut t = vec![0u8; 78];
    t[0..2].copy_from_slice(&1_u16.to_be_bytes()); // version 1
    t[2..4].copy_from_slice(&1000_i16.to_be_bytes()); // xAvgCharWidth
    t[4..6].copy_from_slice(&400_u16.to_be_bytes()); // usWeightClass (Regular)
    t[6..8].copy_from_slice(&5_u16.to_be_bytes()); // usWidthClass (Normal)
    t[58..62].copy_from_slice(b"TEST"); // achVendID
    t[62..64].copy_from_slice(&0x0040_u16.to_be_bytes()); // fsSelection (REGULAR)
    t[64..66].copy_from_slice(&0x0020_u16.to_be_bytes()); // usFirstCharIndex
    t[66..68].copy_from_slice(&0x9FFF_u16.to_be_bytes()); // usLastCharIndex
    t[68..70].copy_from_slice(&880_i16.to_be_bytes()); // sTypoAscender
    t[70..72].copy_from_slice(&(-120_i16).to_be_bytes()); // sTypoDescender
    t[74..76].copy_from_slice(&880_u16.to_be_bytes()); // usWinAscent
    t[76..78].copy_from_slice(&120_u16.to_be_bytes()); // usWinDescent
    t
}

fn build_name_table(family: &str) -> Vec<u8> {
    let records = [
        (1_u16, family.to_string()),
        (2_u16, "Regular".to_string()),
        (4_u16, format!("{family} Regular")),
        (6_u16, format!("{family}-Regular")),
    ];
    let mut string_storage = Vec::new();
    let mut entries = Vec::new();
    for (name_id, text) in &records {
        let utf16: Vec<u8> = text
            .encode_utf16()
            .flat_map(|unit| unit.to_be_bytes())
            .collect();
        let offset = string_storage.len() as u16;
        let length = utf16.len() as u16;
        string_storage.extend_from_slice(&utf16);
        entries.push((*name_id, length, offset));
    }

    let count = entries.len() as u16;
    let string_offset = 6 + count * 12;
    let mut t = Vec::new();
    t.extend_from_slice(&0_u16.to_be_bytes()); // format
    t.extend_from_slice(&count.to_be_bytes());
    t.extend_from_slice(&string_offset.to_be_bytes());
    for (name_id, length, offset) in entries {
        t.extend_from_slice(&3_u16.to_be_bytes()); // Windows platform
        t.extend_from_slice(&1_u16.to_be_bytes()); // Unicode BMP
        t.extend_from_slice(&0x0409_u16.to_be_bytes()); // en-US
        t.extend_from_slice(&name_id.to_be_bytes());
        t.extend_from_slice(&length.to_be_bytes());
        t.extend_from_slice(&offset.to_be_bytes());
    }
    t.extend_from_slice(&string_storage);
    t
}

fn build_post_table() -> Vec<u8> {
    let mut t = vec![0u8; 32];
    t[0..4].copy_from_slice(&0x0003_0000_u32.to_be_bytes()); // format 3.0 (no glyph names)
    t
}

fn build_hmtx_table(num_glyphs: u16) -> Vec<u8> {
    let mut t = Vec::with_capacity(usize::from(num_glyphs) * 4);
    for _ in 0..num_glyphs {
        t.extend_from_slice(&1000_u16.to_be_bytes()); // advanceWidth
        t.extend_from_slice(&100_i16.to_be_bytes()); // lsb
    }
    t
}

fn build_glyf_and_loca() -> (Vec<u8>, Vec<u8>) {
    // Simple 4-point clockwise rectangle from (100,0) to (900,800).
    let mut rect_glyph = Vec::new();
    rect_glyph.extend_from_slice(&1_i16.to_be_bytes()); // numberOfContours
    rect_glyph.extend_from_slice(&100_i16.to_be_bytes()); // xMin
    rect_glyph.extend_from_slice(&0_i16.to_be_bytes()); // yMin
    rect_glyph.extend_from_slice(&900_i16.to_be_bytes()); // xMax
    rect_glyph.extend_from_slice(&800_i16.to_be_bytes()); // yMax
    rect_glyph.extend_from_slice(&3_u16.to_be_bytes()); // endPtsOfContours[0] = 3
    rect_glyph.extend_from_slice(&0_u16.to_be_bytes()); // instructionLength = 0
    // Flags: 4 on-curve points using 16-bit signed deltas (0x01 = on-curve).
    rect_glyph.extend_from_slice(&[0x01, 0x01, 0x01, 0x01]);
    // X deltas: 100, 0, +800, 0 -> points (100,0), (100,800), (900,800), (900,0)
    for dx in [100_i16, 0, 800, 0] {
        rect_glyph.extend_from_slice(&dx.to_be_bytes());
    }
    // Y deltas: 0, +800, 0, -800
    for dy in [0_i16, 800, 0, -800] {
        rect_glyph.extend_from_slice(&dy.to_be_bytes());
    }
    while rect_glyph.len() % 4 != 0 {
        rect_glyph.push(0);
    }

    let mut glyf = Vec::new();
    let offset0 = 0_u32;
    glyf.extend_from_slice(&rect_glyph); // glyph 0 (.notdef)
    let offset1 = glyf.len() as u32;
    glyf.extend_from_slice(&rect_glyph); // glyph 1 (CJK box)
    let offset2 = glyf.len() as u32;

    let mut loca = Vec::with_capacity(12);
    loca.extend_from_slice(&offset0.to_be_bytes());
    loca.extend_from_slice(&offset1.to_be_bytes());
    loca.extend_from_slice(&offset2.to_be_bytes());
    (glyf, loca)
}

fn build_cmap_format4(codepoints: &[u16]) -> Vec<u8> {
    // Each codepoint forms a 1-char segment mapping to glyph 1, plus terminal 0xFFFF.
    let seg_count = (codepoints.len() + 1) as u16;
    let seg_count_x2 = seg_count * 2;
    let mut search_range = 2_u16;
    let mut entry_selector = 0_u16;
    while search_range * 2 <= seg_count_x2 {
        search_range *= 2;
        entry_selector += 1;
    }
    let range_shift = seg_count_x2 - search_range;

    let mut subtable = Vec::new();
    subtable.extend_from_slice(&4_u16.to_be_bytes()); // format 4
    let subtable_len = 16 + seg_count_x2 * 4;
    subtable.extend_from_slice(&subtable_len.to_be_bytes());
    subtable.extend_from_slice(&0_u16.to_be_bytes()); // language
    subtable.extend_from_slice(&seg_count_x2.to_be_bytes());
    subtable.extend_from_slice(&search_range.to_be_bytes());
    subtable.extend_from_slice(&entry_selector.to_be_bytes());
    subtable.extend_from_slice(&range_shift.to_be_bytes());

    // endCode
    for &cp in codepoints {
        subtable.extend_from_slice(&cp.to_be_bytes());
    }
    subtable.extend_from_slice(&0xFFFF_u16.to_be_bytes());
    subtable.extend_from_slice(&0_u16.to_be_bytes()); // reservedPad

    // startCode
    for &cp in codepoints {
        subtable.extend_from_slice(&cp.to_be_bytes());
    }
    subtable.extend_from_slice(&0xFFFF_u16.to_be_bytes());

    // idDelta: maps cp -> glyph 1 via (cp + delta) mod 65536 == 1
    for &cp in codepoints {
        let delta = 1_u16.wrapping_sub(cp) as i16;
        subtable.extend_from_slice(&delta.to_be_bytes());
    }
    subtable.extend_from_slice(&1_i16.to_be_bytes()); // terminal

    // idRangeOffset (all 0)
    for _ in 0..seg_count {
        subtable.extend_from_slice(&0_u16.to_be_bytes());
    }

    let mut cmap = Vec::new();
    cmap.extend_from_slice(&0_u16.to_be_bytes()); // version
    cmap.extend_from_slice(&1_u16.to_be_bytes()); // numTables
    cmap.extend_from_slice(&3_u16.to_be_bytes()); // platformID = Windows
    cmap.extend_from_slice(&1_u16.to_be_bytes()); // encodingID = Unicode BMP
    cmap.extend_from_slice(&12_u32.to_be_bytes()); // offset
    cmap.extend_from_slice(&subtable);
    cmap
}

fn assemble_sfnt(tables: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let num_tables = tables.len() as u16;
    let mut search_range = 16_u16;
    let mut entry_selector = 0_u16;
    while search_range * 2 <= num_tables * 16 {
        search_range *= 2;
        entry_selector += 1;
    }
    let range_shift = num_tables * 16 - search_range;

    let mut out = Vec::new();
    out.extend_from_slice(&0x0001_0000_u32.to_be_bytes()); // sfntVersion
    out.extend_from_slice(&num_tables.to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&range_shift.to_be_bytes());

    let mut offset = 12 + usize::from(num_tables) * 16;
    let mut head_offset = 0usize;
    for (tag, data) in tables {
        if tag == b"head" {
            head_offset = offset;
        }
        let checksum = sfnt_checksum(data);
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum.to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        offset += (data.len() + 3) & !3;
    }

    for (_, data) in tables {
        out.extend_from_slice(data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }

    let full_checksum = sfnt_checksum(&out);
    let adjustment = 0xB1B0_AFBA_u32.wrapping_sub(full_checksum);
    out[head_offset + 8..head_offset + 12].copy_from_slice(&adjustment.to_be_bytes());
    out
}

fn sfnt_checksum(data: &[u8]) -> u32 {
    let mut sum = 0_u32;
    let (chunks, rem) = data.as_chunks::<4>();
    for chunk in chunks {
        sum = sum.wrapping_add(u32::from_be_bytes(*chunk));
    }
    if !rem.is_empty() {
        let mut last = [0u8; 4];
        last[..rem.len()].copy_from_slice(rem);
        sum = sum.wrapping_add(u32::from_be_bytes(last));
    }
    sum
}
