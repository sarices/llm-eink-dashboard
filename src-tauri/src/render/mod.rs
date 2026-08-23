use chrono::{DateTime, Local, Utc};
use serde::Serialize;
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardViewModel {
    pub today_tokens: u64,
    pub month_tokens: u64,
    pub updated_at: DateTime<Utc>,
    pub balance: Option<String>,
    pub source_status: String,
}
/// Produces a 1-bit packed EPD layer (1=white, 0=black) from the dashboard model.
pub fn render_mono_bitmap(
    model: &DashboardViewModel,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 || width % 8 != 0 {
        return Err("单色 EPD 宽度必须为 8 的倍数且尺寸大于零".into());
    }
    if width >= 280 && height >= 220 {
        return Ok(render_tricolor_bitmaps(model, width, height)?.0);
    }
    let row_bytes = (width / 8) as usize;
    let mut bitmap = vec![0xFF; row_bytes * height as usize];
    if width < 240 || height < 300 {
        draw_rule(&mut bitmap, row_bytes, width, height, 0, 0, width);
        return Ok(bitmap);
    }
    draw_text(
        &mut bitmap,
        row_bytes,
        width,
        height,
        24,
        18,
        4,
        "LLM DASHBOARD",
    );
    draw_rule(
        &mut bitmap,
        row_bytes,
        width,
        height,
        24,
        64,
        width.saturating_sub(24),
    );

    draw_text(&mut bitmap, row_bytes, width, height, 40, 84, 3, "TODAY");
    draw_text(
        &mut bitmap,
        row_bytes,
        width,
        height,
        40,
        112,
        6,
        &format_compact(model.today_tokens),
    );

    draw_text(&mut bitmap, row_bytes, width, height, 40, 178, 3, "MONTH");
    draw_text(
        &mut bitmap,
        row_bytes,
        width,
        height,
        40,
        206,
        6,
        &format_compact(model.month_tokens),
    );

    draw_rule(
        &mut bitmap,
        row_bytes,
        width,
        height,
        24,
        266,
        width.saturating_sub(24),
    );
    draw_text(&mut bitmap, row_bytes, width, height, 40, 278, 3, "BALANCE");
    let balance = model.balance.as_deref().unwrap_or("N/A").to_uppercase();
    draw_text(&mut bitmap, row_bytes, width, height, 196, 278, 3, &balance);
    Ok(bitmap)
}

/// Renders the dashboard as black and red EPD layers for a two-layer tri-color panel.
pub fn render_tricolor_bitmaps(
    model: &DashboardViewModel,
    width: u32,
    height: u32,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    if width == 0 || height == 0 || width % 8 != 0 {
        return Err("三色 EPD 宽度必须为 8 的倍数且尺寸大于零".into());
    }
    let row_bytes = (width / 8) as usize;
    let mut black = vec![0xFF; row_bytes * height as usize];
    let mut red = vec![0xFF; row_bytes * height as usize];
    if width < 280 || height < 220 {
        return Ok((render_mono_bitmap(model, width, height)?, red));
    }

    let margin = 14;
    let gap = 12;
    let card_width = (width.saturating_sub(margin * 2 + gap)) / 2;
    let card_height = height.saturating_sub(106).max(120);
    let left = margin;
    let right = margin + card_width + gap;
    let card_bottom = 44 + card_height;

    draw_cjk_text(&mut black, row_bytes, width, height, margin, 10, "用量概览");
    let sync_time = model
        .updated_at
        .with_timezone(&Local)
        .format("%H:%M")
        .to_string();
    draw_mixed_text(
        &mut red,
        row_bytes,
        width,
        height,
        width.saturating_sub(78),
        10,
        &sync_time,
        true,
    );
    draw_rule(
        &mut black,
        row_bytes,
        width,
        height,
        margin,
        34,
        width - margin,
    );

    for x in [left, right] {
        draw_rect_outline(
            &mut black,
            row_bytes,
            width,
            height,
            x,
            44,
            card_width,
            card_height,
        );
        fill_rect(
            &mut red,
            row_bytes,
            width,
            height,
            x + 1,
            45,
            card_width.saturating_sub(2),
            6,
        );
    }

    draw_cjk_text(&mut black, row_bytes, width, height, left + 12, 60, "今日");
    draw_cjk_text(&mut black, row_bytes, width, height, left + 12, 60, "今日");
    draw_text(
        &mut black,
        row_bytes,
        width,
        height,
        left + 12,
        84,
        4,
        &format_compact(model.today_tokens),
    );
    draw_text_color(
        &mut red,
        row_bytes,
        width,
        height,
        left + 12,
        122,
        2,
        "TOKEN",
    );
    draw_rule(
        &mut black,
        row_bytes,
        width,
        height,
        left + 12,
        144,
        left + card_width - 12,
    );
    draw_cjk_text(
        &mut black,
        row_bytes,
        width,
        height,
        left + 12,
        154,
        "今日用量",
    );

    draw_cjk_text(&mut black, row_bytes, width, height, right + 12, 60, "本月");
    draw_text(
        &mut black,
        row_bytes,
        width,
        height,
        right + 12,
        84,
        4,
        &format_compact(model.month_tokens),
    );
    draw_text_color(
        &mut red,
        row_bytes,
        width,
        height,
        right + 12,
        122,
        2,
        "TOKEN",
    );
    draw_rule(
        &mut black,
        row_bytes,
        width,
        height,
        right + 12,
        144,
        right + card_width - 12,
    );
    draw_cjk_text(
        &mut black,
        row_bytes,
        width,
        height,
        right + 12,
        154,
        "余额",
    );
    let balance = model.balance.as_deref().unwrap_or("N/A").to_uppercase();
    draw_text_color(
        &mut red,
        row_bytes,
        width,
        height,
        right + 12,
        174,
        2,
        &balance,
    );

    draw_rule(
        &mut black,
        row_bytes,
        width,
        height,
        margin,
        card_bottom + 16,
        width - margin,
    );
    draw_cjk_text(
        &mut black,
        row_bytes,
        width,
        height,
        margin,
        card_bottom + 26,
        "数据源",
    );
    let source_name = model
        .source_status
        .split_once('：')
        .map(|(name, _)| name)
        .unwrap_or(model.source_status.as_str());
    draw_mixed_text(
        &mut black,
        row_bytes,
        width,
        height,
        margin + 72,
        card_bottom + 26,
        source_name,
        false,
    );
    draw_cjk_text_color(
        &mut red,
        row_bytes,
        width,
        height,
        margin,
        card_bottom + 44,
        "已同步",
    );
    Ok((black, red))
}

fn format_compact(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{}M", value / 1_000_000)
    } else if value >= 1_000 {
        format!("{}K", value / 1_000)
    } else {
        value.to_string()
    }
}

fn draw_rule(
    bitmap: &mut [u8],
    row_bytes: usize,
    width: u32,
    height: u32,
    x0: u32,
    y: u32,
    x1: u32,
) {
    for x in x0..x1.min(width) {
        set_black(bitmap, row_bytes, width, height, x, y);
    }
}

fn draw_rect_outline(
    bitmap: &mut [u8],
    row_bytes: usize,
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    rect_width: u32,
    rect_height: u32,
) {
    draw_rule_color(
        bitmap,
        row_bytes,
        width,
        height,
        x,
        y,
        x + rect_width,
        false,
    );
    draw_rule_color(
        bitmap,
        row_bytes,
        width,
        height,
        x,
        y + rect_height.saturating_sub(1),
        x + rect_width,
        false,
    );
    for offset in 0..rect_height {
        set_black(bitmap, row_bytes, width, height, x, y + offset);
        set_black(
            bitmap,
            row_bytes,
            width,
            height,
            x + rect_width.saturating_sub(1),
            y + offset,
        );
    }
}

fn fill_rect(
    bitmap: &mut [u8],
    row_bytes: usize,
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    rect_width: u32,
    rect_height: u32,
) {
    for dy in 0..rect_height {
        for dx in 0..rect_width {
            set_red(bitmap, row_bytes, width, height, x + dx, y + dy);
        }
    }
}

fn draw_rule_color(
    bitmap: &mut [u8],
    row_bytes: usize,
    width: u32,
    height: u32,
    x0: u32,
    y: u32,
    x1: u32,
    red: bool,
) {
    for x in x0..x1.min(width) {
        if red {
            set_red(bitmap, row_bytes, width, height, x, y);
        } else {
            set_black(bitmap, row_bytes, width, height, x, y);
        }
    }
}

fn draw_text(
    bitmap: &mut [u8],
    row_bytes: usize,
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    scale: u32,
    text: &str,
) {
    draw_text_with_ink(bitmap, row_bytes, width, height, x, y, scale, text, false);
}

fn draw_text_color(
    bitmap: &mut [u8],
    row_bytes: usize,
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    scale: u32,
    text: &str,
) {
    draw_text_with_ink(bitmap, row_bytes, width, height, x, y, scale, text, true);
}

fn draw_cjk_text(
    bitmap: &mut [u8],
    row_bytes: usize,
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    text: &str,
) {
    draw_mixed_text(bitmap, row_bytes, width, height, x, y, text, false);
}

fn draw_cjk_text_color(
    bitmap: &mut [u8],
    row_bytes: usize,
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    text: &str,
) {
    draw_mixed_text(bitmap, row_bytes, width, height, x, y, text, true);
}

fn draw_mixed_text(
    bitmap: &mut [u8],
    row_bytes: usize,
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    text: &str,
    red: bool,
) {
    let mut cursor = x;
    for character in text.chars() {
        if let Some(glyph) = glyph_cjk_16(character) {
            for (row, bits) in glyph.into_iter().enumerate() {
                for column in 0..16 {
                    if bits & (1 << (15 - column)) != 0 {
                        if red {
                            set_red(
                                bitmap,
                                row_bytes,
                                width,
                                height,
                                cursor + column,
                                y + row as u32,
                            );
                        } else {
                            set_black(
                                bitmap,
                                row_bytes,
                                width,
                                height,
                                cursor + column,
                                y + row as u32,
                            );
                        }
                    }
                }
            }
            cursor += 18;
        } else {
            let ascii = character.to_string();
            draw_text_with_ink(
                bitmap,
                row_bytes,
                width,
                height,
                cursor,
                y + 4,
                2,
                &ascii,
                red,
            );
            cursor += 14;
        }
    }
}

fn draw_text_with_ink(
    bitmap: &mut [u8],
    row_bytes: usize,
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    scale: u32,
    text: &str,
    red: bool,
) {
    let mut cursor = x;
    for character in text.chars() {
        if character == ' ' {
            cursor += 3 * scale;
            continue;
        }
        let glyph = glyph_5x7(character);
        for (row, bits) in glyph.into_iter().enumerate() {
            for column in 0..5 {
                if bits & (1 << (4 - column)) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        if red {
                            set_red(
                                bitmap,
                                row_bytes,
                                width,
                                height,
                                cursor + column * scale + dx,
                                y + row as u32 * scale + dy,
                            );
                        } else {
                            set_black(
                                bitmap,
                                row_bytes,
                                width,
                                height,
                                cursor + column * scale + dx,
                                y + row as u32 * scale + dy,
                            );
                        }
                    }
                }
            }
        }
        cursor += 6 * scale;
    }
}

fn set_black(bitmap: &mut [u8], row_bytes: usize, width: u32, height: u32, x: u32, y: u32) {
    if x < width && y < height {
        let index = y as usize * row_bytes + (x / 8) as usize;
        bitmap[index] &= !(0x80 >> (x % 8));
    }
}

fn set_red(bitmap: &mut [u8], row_bytes: usize, width: u32, height: u32, x: u32, y: u32) {
    set_black(bitmap, row_bytes, width, height, x, y);
}

fn glyph_cjk_16(character: char) -> Option<[u16; 16]> {
    let glyph = match character {
        '用' => [
            0x1FFE, 0x18C6, 0x18C2, 0x18C2, 0x1FFE, 0x1FFE, 0x1882, 0x18C2, 0x1FFE, 0x18C6, 0x10C2,
            0x3082, 0x30C2, 0x6086, 0x600E, 0x0000,
        ],
        '量' => [
            0x0000, 0x3FFC, 0x3FFC, 0x300C, 0x1008, 0xFFFF, 0x3FFC, 0x1188, 0x3FFC, 0x3FFC, 0x318C,
            0x1FF8, 0x3FFC, 0x0180, 0xFFFF, 0x0000,
        ],
        '概' => [
            0x1000, 0x13BE, 0x1288, 0x3AA8, 0x3BA8, 0x12A8, 0x32BE, 0x3BBE, 0x3B18, 0x7218, 0x5298,
            0x5298, 0x1328, 0x126A, 0x10CE, 0x1000,
        ],
        '览' => [
            0x0060, 0x3660, 0x367E, 0x26C0, 0x2790, 0x360C, 0x0000, 0x1FF8, 0x1FF8, 0x1808, 0x1988,
            0x19C8, 0x02C0, 0x0E63, 0x787E, 0x0000,
        ],
        '今' => [
            0x0000, 0x0180, 0x0380, 0x0240, 0x0460, 0x0818, 0x318E, 0x6180, 0x0000, 0x1FF8, 0x0030,
            0x0030, 0x0060, 0x00C0, 0x0180, 0x0000,
        ],
        '日' => [
            0x0000, 0xFFFF, 0xE007, 0xE007, 0xE007, 0xE007, 0xF00F, 0xFFFF, 0xE007, 0xE007, 0xE007,
            0xE007, 0xFFFF, 0xFFFF, 0xC003, 0x0000,
        ],
        '本' => [
            0x0000, 0x0180, 0x0180, 0x0180, 0x7FFE, 0x03C0, 0x03C0, 0x07A0, 0x05A0, 0x0D90, 0x1998,
            0x318E, 0x67F6, 0x0180, 0x0180, 0x0180,
        ],
        '月' => [
            0x0000, 0x0FFF, 0x0C07, 0x0803, 0x0C03, 0x0FFF, 0x0C07, 0x0803, 0x0E07, 0x0FFF, 0x1803,
            0x1803, 0x3803, 0x3007, 0x600F, 0x0000,
        ],
        '余' => [
            0x0000, 0x0180, 0x0380, 0x0640, 0x0C30, 0x1818, 0x3FFE, 0x6180, 0x0080, 0x3FFC, 0x0180,
            0x0CB0, 0x1898, 0x318C, 0x0380, 0x0000,
        ],
        '额' => [
            0x0000, 0x087E, 0x7F30, 0x4110, 0x107C, 0x3E7C, 0x7654, 0x4C54, 0x1E54, 0x1354, 0x7254,
            0x3E10, 0x2228, 0x3E24, 0x2282, 0x0000,
        ],
        '令' => [
            0x0000, 0x0180, 0x0380, 0x0240, 0x0660, 0x0D18, 0x318E, 0x6082, 0x0000, 0x1FFC, 0x0018,
            0x0020, 0x0640, 0x03C0, 0x00C0, 0x0040,
        ],
        '牌' => [
            0x0030, 0x2420, 0x25FE, 0x2512, 0x2532, 0x3FFE, 0x3122, 0x2126, 0x39FE, 0x3C58, 0x24D8,
            0x24D8, 0x67FE, 0x4418, 0x0418, 0x0018,
        ],
        '数' => [
            0x0430, 0x3530, 0x0430, 0x3FBE, 0x1C64, 0x1D44, 0x25E4, 0x0C2C, 0x0C28, 0x3FB8, 0x1918,
            0x1F18, 0x0F3C, 0x1DEE, 0x3080, 0x0000,
        ],
        '据' => [
            0x0000, 0x11FE, 0x1302, 0x1302, 0x7BFE, 0x1B30, 0x1310, 0x1BFF, 0x1B38, 0x7B38, 0x13FE,
            0x12C2, 0x12C2, 0x14C2, 0x34FE, 0x0000,
        ],
        '源' => [
            0x0000, 0x37FE, 0x1620, 0x0620, 0x06FC, 0x668C, 0x36FC, 0x048C, 0x048C, 0x04FC, 0x3420,
            0x2D24, 0x2924, 0x6B26, 0x0060, 0x0000,
        ],
        '当' => [
            0x0080, 0x0180, 0x3186, 0x198C, 0x0998, 0x0180, 0x3FFE, 0x3FFE, 0x0002, 0x0006, 0x3FFE,
            0x0006, 0x0002, 0x0006, 0x7FFE, 0x0003,
        ],
        '前' => [
            0x0000, 0x0C30, 0x0460, 0xFFFF, 0x0000, 0x3E04, 0x7F24, 0x6364, 0x7F64, 0x7764, 0x6364,
            0x7F64, 0x6324, 0x630C, 0x671C, 0x0000,
        ],
        '已' => [
            0x0000, 0x7FF8, 0x7FF8, 0x0018, 0x0018, 0x2018, 0x3018, 0x3FF8, 0x3018, 0x2000, 0x2000,
            0x2000, 0x2004, 0x3006, 0x3C3C, 0x3FFC,
        ],
        '同' => [
            0x0000, 0x7FFF, 0x7007, 0x6003, 0x6FF3, 0x6013, 0x6003, 0x67E3, 0x67E3, 0x6423, 0x6623,
            0x67E3, 0x6003, 0x6003, 0x600F, 0x0000,
        ],
        '步' => [
            0x0000, 0x0100, 0x1100, 0x11FC, 0x1180, 0x1100, 0xFFFE, 0x0100, 0x0180, 0x1988, 0x3198,
            0x63B0, 0x03E0, 0x0380, 0x7E00, 0x0000,
        ],
        '逻' => [
            0x0000, 0x27FE, 0x36D2, 0x16D2, 0x06F6, 0x77FE, 0x30C0, 0x21E4, 0x338C, 0x7F8C, 0x1078,
            0x1070, 0x33C0, 0x3F00, 0x43FE, 0x0000,
        ],
        '辑' => [
            0x0000, 0x10FC, 0x1084, 0x7C84, 0x2000, 0x2BFE, 0x298C, 0x7C8C, 0x7CFC, 0x0884, 0x0EFC,
            0x7884, 0x0BFE, 0x0804, 0x0804, 0x0000,
        ],
        '云' => [
            0x1830, 0x3FF8, 0x0000, 0x0000, 0x0000, 0x700E, 0xFFFE, 0x0300, 0x0300, 0x0600, 0x0620,
            0x0C30, 0x1818, 0x187C, 0x7FFC, 0x3804,
        ],
        _ => return None,
    };
    Some(glyph)
}

fn glyph_5x7(character: char) -> [u8; 7] {
    match character {
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110,
        ],
        '6' => [
            0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b11100,
        ],
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'G' => [
            0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        '/' => [
            0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b00000, 0b00000,
        ],
        '-' => [
            0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000,
        ],
        '.' => [
            0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100,
        ],
        _ => [0; 7],
    }
}

pub fn render_svg(model: &DashboardViewModel, width: u32, height: u32) -> String {
    let updated = model
        .updated_at
        .with_timezone(&Local)
        .format("%Y-%m-%d %H:%M");
    format!(
        r#"<svg xmlns='http://www.w3.org/2000/svg' width='{width}' height='{height}' viewBox='0 0 {width} {height}'><rect width='100%' height='100%' fill='white'/><g fill='#111' font-family='sans-serif'><text x='24' y='42' font-size='24' font-weight='bold'>LLM 用量仪表盘</text><text x='24' y='68' font-size='13'>更新于 {updated}</text><line x1='24' y1='88' x2='{line}' y2='88' stroke='#111'/><text x='24' y='132' font-size='16'>今日 Token</text><text x='24' y='178' font-size='40' font-weight='bold'>{today}</text><text x='24' y='224' font-size='16'>本月 Token</text><text x='24' y='270' font-size='40' font-weight='bold'>{month}</text><text x='24' y='310' font-size='16'>余额：{balance}</text><text x='24' y='340' font-size='13'>数据源：{status}</text></g></svg>"#,
        line = width.saturating_sub(24),
        today = model.today_tokens,
        month = model.month_tokens,
        balance = model.balance.clone().unwrap_or_else(|| "—".into()),
        status = model.source_status
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn creates_packed_mono_bitmap_for_epd() {
        let model = DashboardViewModel {
            today_tokens: 9,
            month_tokens: 15,
            updated_at: Utc::now(),
            balance: None,
            source_status: "ready".into(),
        };
        let bitmap = render_mono_bitmap(&model, 16, 8).unwrap();
        assert_eq!(bitmap.len(), 16);
        assert!(bitmap.iter().any(|byte| *byte != 0xFF));
    }

    #[test]
    fn renders_textual_metrics_in_distinct_screen_regions() {
        let model = DashboardViewModel {
            today_tokens: 1_200,
            month_tokens: 3_400,
            updated_at: Utc::now(),
            balance: Some("12.50 CNY".into()),
            source_status: "ready".into(),
        };
        let bitmap = render_mono_bitmap(&model, 400, 300).unwrap();
        let row_bytes = 50;
        assert!(bitmap[18 * row_bytes..64 * row_bytes]
            .iter()
            .any(|byte| *byte != 0xFF));
        assert!(bitmap[112 * row_bytes..154 * row_bytes]
            .iter()
            .any(|byte| *byte != 0xFF));
        assert!(bitmap[206 * row_bytes..248 * row_bytes]
            .iter()
            .any(|byte| *byte != 0xFF));
    }

    #[test]
    fn tricolor_render_uses_a_nonempty_red_layer() {
        let model = DashboardViewModel {
            today_tokens: 384_730,
            month_tokens: 384_730,
            updated_at: Utc::now(),
            balance: Some("273.31 USD".into()),
            source_status: "ready".into(),
        };
        let (black, red) = render_tricolor_bitmaps(&model, 400, 300).unwrap();
        assert_eq!(black.len(), red.len());
        assert!(black.iter().any(|byte| *byte != 0xFF));
        assert!(red.iter().any(|byte| *byte != 0xFF));
    }
    #[test]
    fn renders_tiny_dimensions_without_panicking() {
        let svg = render_svg(
            &DashboardViewModel {
                today_tokens: 1,
                month_tokens: 2,
                updated_at: Utc::now(),
                balance: None,
                source_status: "ready".into(),
            },
            16,
            8,
        );
        assert!(svg.contains("width='16'"));
    }
}
