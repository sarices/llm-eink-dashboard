use serde::{Deserialize, Serialize};

pub const SET_PINS: u8 = 0x00;
pub const INIT: u8 = 0x01;
pub const CLEAR: u8 = 0x02;
pub const REFRESH: u8 = 0x05;
pub const WRITE_IMAGE: u8 = 0x30;
pub const WRITE_BLOCK: u8 = 0x31;
pub const QUERY_STATUS: u8 = 0x32;
pub const RESET_TRANSFER: u8 = 0x33;
pub const DRIVER_4_2_THREE_COLOR_SSD1619: u8 = 0x02;
const INCORRECT_LEGACY_DRIVER_4_2_THREE_COLOR_UC8176: u8 = 0x03;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceConfig {
    pub id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub color_layers: u8,
    pub driver_id: u8,
    pub mtu: usize,
    pub block_size: usize,
}

impl DeviceConfig {
    pub fn monochrome_400x300() -> Self {
        Self {
            id: "default".into(),
            name: "NRF_EPD".into(),
            width: 400,
            height: 300,
            // The nRF52811 firmware's pinned 4.2-inch tri-color configuration uses SSD1619 (model 0x02).
            color_layers: 2,
            driver_id: DRIVER_4_2_THREE_COLOR_SSD1619,
            mtu: 185,
            block_size: 160,
        }
    }
    pub fn expected_bytes(&self) -> usize {
        self.expected_layer_bytes() * self.color_layers as usize
    }
    pub fn expected_layer_bytes(&self) -> usize {
        (self.width as usize * self.height as usize).div_ceil(8)
    }

    /// Early builds either omitted the driver ID or selected UC8176 (`0x03`) for the nRF52811
    /// 4.2-inch hardware. Its firmware configuration selects SSD1619 (`0x02`).
    pub fn migrate_default_driver(mut self) -> Self {
        if self.id == "default"
            && self.width == 400
            && self.height == 300
            && self.color_layers == 2
            && matches!(
                self.driver_id,
                0 | INCORRECT_LEGACY_DRIVER_4_2_THREE_COLOR_UC8176
            )
        {
            self.driver_id = DRIVER_4_2_THREE_COLOR_SSD1619;
        }
        self
    }
}

pub const BW_LAYER: u8 = 0x0f;
pub const CONTINUATION_FLAG: u8 = 0xf0;

/// The EPD firmware uses the reflected CCITT implementation with an all-ones seed.
/// Its checksum covers only the image payload, not the transfer header.
pub fn crc16_ccitt(payload: &[u8]) -> u16 {
    let mut crc = 0xffff_u16;
    for byte in payload {
        crc ^= u16::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0x8408
            } else {
                crc >> 1
            };
        }
    }
    crc
}

pub fn image_cfg(block_id: u16, layer: u8) -> u8 {
    (if block_id == 0 { 0 } else { CONTINUATION_FLAG }) | (layer & 0x0f)
}

pub fn write_block(block_id: u16, total_blocks: u16, layer: u8, payload: &[u8]) -> Vec<u8> {
    let mut packet = vec![WRITE_BLOCK];
    packet.extend(block_id.to_le_bytes());
    packet.extend(total_blocks.to_le_bytes());
    packet.push(image_cfg(block_id, layer));
    packet.extend(payload);
    packet.extend(crc16_ccitt(payload).to_le_bytes());
    packet
}

pub fn write_legacy_image(block_id: u16, layer: u8, payload: &[u8]) -> Vec<u8> {
    let mut packet = vec![WRITE_IMAGE, image_cfg(block_id, layer)];
    packet.extend(payload);
    packet
}

pub fn validate_transfer_packets(packets: &[Vec<u8>]) -> Result<u16, String> {
    if packets.is_empty() {
        return Err("EPD 传输没有可写入的图像块".into());
    }
    if packets.len() > u16::MAX as usize {
        return Err("图像块数超过协议限制".into());
    }
    let total = packets.len() as u16;
    for (index, packet) in packets.iter().enumerate() {
        if packet.len() < 8 || packet[0] != WRITE_BLOCK {
            return Err(format!("第 {index} 个 EPD 包不是有效 WRITE_BLOCK"));
        }
        let block_id = u16::from_le_bytes([packet[1], packet[2]]);
        let packet_total = u16::from_le_bytes([packet[3], packet[4]]);
        if block_id != index as u16 || packet_total != total {
            return Err(format!("第 {index} 个 EPD 包序号或总块数不一致"));
        }
        let payload = &packet[6..packet.len() - 2];
        let checksum = &packet[packet.len() - 2..];
        if crc16_ccitt(payload) != u16::from_le_bytes(checksum.try_into().unwrap()) {
            return Err(format!("第 {index} 个 EPD 包 CRC 无效"));
        }
    }
    Ok(total)
}

pub fn chunk_image(image: &[u8], config: &DeviceConfig, layer: u8) -> Result<Vec<Vec<u8>>, String> {
    if image.len() != config.expected_layer_bytes() {
        return Err(format!(
            "位图长度错误：需要 {}，收到 {}",
            config.expected_layer_bytes(),
            image.len()
        ));
    }
    if config.block_size == 0 || config.block_size + 8 > config.mtu {
        return Err("EPD 块大小必须小于 MTU（含协议头）".into());
    }
    let total = image.len().div_ceil(config.block_size);
    if total > u16::MAX as usize {
        return Err("图像块数超过协议限制".into());
    }
    Ok(image
        .chunks(config.block_size)
        .enumerate()
        .map(|(index, payload)| write_block(index as u16, total as u16, layer, payload))
        .collect())
}

pub fn chunk_legacy_image(
    image: &[u8],
    config: &DeviceConfig,
    layer: u8,
) -> Result<Vec<Vec<u8>>, String> {
    if image.len() != config.expected_layer_bytes() {
        return Err(format!(
            "位图长度错误：需要 {}，收到 {}",
            config.expected_layer_bytes(),
            image.len()
        ));
    }
    if config.block_size == 0 || config.block_size + 2 > config.mtu {
        return Err("传统 EPD 块大小必须小于 MTU（含协议头）".into());
    }
    Ok(image
        .chunks(config.block_size)
        .enumerate()
        .map(|(index, payload)| write_legacy_image(index as u16, layer, payload))
        .collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceResponse {
    BlockAck {
        block_id: u16,
        status: u8,
    },
    Status {
        total_blocks: u16,
        received_blocks: u16,
        session: u8,
        active: bool,
        bitmap: Vec<u8>,
    },
}

pub fn parse_response(data: &[u8]) -> Result<DeviceResponse, String> {
    match data.first().copied() {
        Some(0xA0) if data.len() == 4 => Ok(DeviceResponse::BlockAck {
            block_id: u16::from_le_bytes([data[1], data[2]]),
            status: data[3],
        }),
        Some(0xA1) if data.len() >= 5 => {
            let total_blocks = u16::from_le_bytes([data[1], data[2]]);
            let received_blocks = u16::from_le_bytes([data[3], data[4]]);
            let bitmap_len = (total_blocks as usize).div_ceil(8);
            let payload = &data[5..];

            // Firmware revisions differ here: some include session + active before the
            // bitmap, while others omit one or both fields. Use the advertised block count
            // to identify the bitmap tail and preserve compatibility with each layout.
            let (session, active, bitmap) = if payload.len() >= bitmap_len + 2 {
                (
                    payload[0],
                    payload[1] == 1,
                    payload[payload.len() - bitmap_len..].to_vec(),
                )
            } else if payload.len() >= bitmap_len + 1 {
                (
                    payload[0],
                    true,
                    payload[payload.len() - bitmap_len..].to_vec(),
                )
            } else if payload.len() >= bitmap_len {
                (0, true, payload[payload.len() - bitmap_len..].to_vec())
            } else {
                // A complete status may omit the bitmap entirely; the transfer layer
                // accepts this only when received_blocks == total_blocks.
                (0, true, Vec::new())
            };
            Ok(DeviceResponse::Status {
                total_blocks,
                received_blocks,
                session,
                active,
                bitmap,
            })
        }
        _ => Err("未知或截断的 EPD 响应".into()),
    }
}

pub fn missing_blocks_from_bitmap(total_blocks: u16, bitmap: &[u8]) -> Vec<u16> {
    (0..total_blocks)
        .filter(|block| {
            bitmap
                .get((*block / 8) as usize)
                .map(|byte| byte & (1 << (*block % 8)) == 0)
                .unwrap_or(true)
        })
        .collect()
}

pub fn validate_status_bitmap(
    total_blocks: u16,
    received_blocks: u16,
    bitmap: &[u8],
) -> Result<(), String> {
    if received_blocks > total_blocks {
        return Err("EPD 状态的已接收块数超过总块数".into());
    }
    if bitmap.is_empty() && received_blocks == total_blocks {
        return Ok(());
    }
    let marked = (0..total_blocks)
        .filter(|block| {
            bitmap
                .get((*block / 8) as usize)
                .map(|byte| byte & (1 << (*block % 8)) != 0)
                .unwrap_or(false)
        })
        .count() as u16;
    if marked != received_blocks {
        return Err(format!(
            "EPD 状态位图与已接收块数不一致：位图 {marked}，设备 {received_blocks}"
        ));
    }
    Ok(())
}

pub const MAX_RETRY_ROUNDS: u8 = 3;

/// The reference client refreshes after a successful transfer and keeps the device awake.
pub fn finalization_commands() -> [u8; 1] {
    [REFRESH]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryDecision {
    pub pending: Vec<u16>,
    pub retry_rounds: u8,
    pub complete: bool,
}

pub fn decide_retry(
    total_blocks: u16,
    bitmap: &[u8],
    retry_rounds: u8,
) -> Result<RetryDecision, String> {
    let pending = missing_blocks_from_bitmap(total_blocks, bitmap);
    if pending.is_empty() {
        return Ok(RetryDecision {
            pending,
            retry_rounds,
            complete: true,
        });
    }
    if retry_rounds >= MAX_RETRY_ROUNDS {
        return Err(format!(
            "EPD 在 {MAX_RETRY_ROUNDS} 轮重传后仍缺失 {} 个块",
            pending.len()
        ));
    }
    Ok(RetryDecision {
        pending,
        retry_rounds: retry_rounds + 1,
        complete: false,
    })
}

pub fn missing_blocks(received: &[bool]) -> Vec<u16> {
    received
        .iter()
        .enumerate()
        .filter_map(|(index, ok)| (!ok).then_some(index as u16))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packet_crc_covers_only_payload() {
        let packet = write_block(1, 3, BW_LAYER, &[1, 2]);
        let checksum = &packet[packet.len() - 2..];
        assert_eq!(
            crc16_ccitt(&[1, 2]),
            u16::from_le_bytes(checksum.try_into().unwrap())
        );
        assert_ne!(
            crc16_ccitt(&packet[..packet.len() - 2]),
            u16::from_le_bytes(checksum.try_into().unwrap())
        );
    }
    #[test]
    fn image_cfg_marks_first_and_continuation_blocks() {
        assert_eq!(image_cfg(0, BW_LAYER), 0x0f);
        assert_eq!(image_cfg(1, BW_LAYER), 0xff);
    }
    #[test]
    fn parses_ack_and_missing_bitmap_blocks() {
        assert_eq!(
            parse_response(&[0xA0, 2, 0, 0]).unwrap(),
            DeviceResponse::BlockAck {
                block_id: 2,
                status: 0
            }
        );
        assert_eq!(missing_blocks_from_bitmap(4, &[0b0000_0101]), vec![1, 3]);
    }
    #[test]
    fn validates_status_bitmap_cardinality() {
        assert!(validate_status_bitmap(4, 2, &[0b0000_0101]).is_ok());
        assert!(validate_status_bitmap(4, 3, &[0b0000_0101]).is_err());
        assert!(validate_status_bitmap(4, 5, &[0b0000_1111]).is_err());
    }
    #[test]
    fn finalization_only_contains_refresh() {
        assert_eq!(finalization_commands(), [REFRESH]);
    }
    #[test]
    fn retry_plan_retransmits_only_missing_blocks_then_completes() {
        let first = decide_retry(4, &[0b0000_0101], 0).unwrap();
        assert_eq!(first.pending, vec![1, 3]);
        assert_eq!(first.retry_rounds, 1);
        assert!(!first.complete);
        let complete = decide_retry(4, &[0b0000_1111], first.retry_rounds).unwrap();
        assert!(complete.complete);
        assert!(complete.pending.is_empty());
    }
    #[test]
    fn retry_plan_fails_after_maximum_rounds() {
        assert!(decide_retry(2, &[0], MAX_RETRY_ROUNDS).is_err());
    }
    #[test]
    fn rejects_empty_or_corrupt_transfer_packets() {
        assert!(validate_transfer_packets(&[]).is_err());
        let good = vec![write_block(0, 1, BW_LAYER, &[7])];
        assert_eq!(validate_transfer_packets(&good).unwrap(), 1);
        let mut corrupt = good.clone();
        corrupt[0][6] ^= 0xFF;
        assert!(validate_transfer_packets(&corrupt).is_err());
    }
    #[test]
    fn chunks_have_contiguous_ids() {
        let mut config = DeviceConfig::monochrome_400x300();
        config.width = 16;
        config.height = 8;
        config.mtu = 32;
        config.block_size = 8;
        let packets = chunk_image(&[0; 16], &config, BW_LAYER).unwrap();
        assert_eq!(packets.len(), 2);
        assert_eq!(u16::from_le_bytes([packets[1][1], packets[1][2]]), 1);
        assert_eq!(packets[0][5], 0x0f);
        assert_eq!(packets[1][5], 0xff);
    }

    #[test]
    fn migrates_the_old_default_to_the_tri_color_driver() {
        let mut legacy = DeviceConfig::monochrome_400x300();
        legacy.driver_id = 0;
        assert_eq!(
            legacy.migrate_default_driver().driver_id,
            DRIVER_4_2_THREE_COLOR_SSD1619
        );
    }
    #[test]
    fn parses_status_with_active_byte_before_bitmap() {
        assert_eq!(
            parse_response(&[0xA1, 4, 0, 2, 0, 9, 1, 0b0000_0101]).unwrap(),
            DeviceResponse::Status {
                total_blocks: 4,
                received_blocks: 2,
                session: 9,
                active: true,
                bitmap: vec![0b0000_0101],
            }
        );
    }

    #[test]
    fn parses_status_without_active_byte() {
        assert_eq!(
            parse_response(&[0xA1, 4, 0, 4, 0, 9, 0b0000_1111]).unwrap(),
            DeviceResponse::Status {
                total_blocks: 4,
                received_blocks: 4,
                session: 9,
                active: true,
                bitmap: vec![0b0000_1111],
            }
        );
    }

    #[test]
    fn accepts_complete_status_without_bitmap() {
        assert!(validate_status_bitmap(4, 4, &[]).is_ok());
        assert!(validate_status_bitmap(4, 3, &[]).is_err());
        assert_eq!(
            parse_response(&[0xA1, 4, 0, 4, 0]).unwrap(),
            DeviceResponse::Status {
                total_blocks: 4,
                received_blocks: 4,
                session: 0,
                active: true,
                bitmap: vec![],
            }
        );
    }
    #[test]
    fn chunks_legacy_packets_with_the_same_layer_flags() {
        let mut config = DeviceConfig::monochrome_400x300();
        config.width = 16;
        config.height = 8;
        config.mtu = 32;
        config.block_size = 8;
        let packets = chunk_legacy_image(&[0; 16], &config, BW_LAYER).unwrap();
        assert_eq!(packets[0][..2], [WRITE_IMAGE, 0x0f]);
        assert_eq!(packets[1][..2], [WRITE_IMAGE, 0xff]);
    }
}
