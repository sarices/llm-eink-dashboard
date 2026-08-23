use std::{
    collections::BTreeMap,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Result};
use btleplug::{
    api::{
        Central, CharPropFlags, Manager as _, Peripheral as _, ScanFilter, ValueNotification,
        WriteType,
    },
    platform::{Adapter, Manager},
};
use futures::StreamExt;
use serde::Serialize;
use tokio::time::{sleep, timeout};

use crate::epd::{
    chunk_image, chunk_legacy_image, decide_retry, finalization_commands, parse_response,
    validate_status_bitmap, validate_transfer_packets, DeviceConfig, DeviceResponse, BW_LAYER,
    INIT, QUERY_STATUS, RESET_TRANSFER,
};

pub const NRF_EPD_PREFIX: &str = "NRF_EPD";

pub async fn create_adapter() -> Result<Adapter> {
    Manager::new()
        .await?
        .adapters()
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("未找到可用蓝牙适配器"))
}
const STATUS_TIMEOUT: Duration = Duration::from_secs(3);
const INIT_RESPONSE_TIMEOUT: Duration = Duration::from_millis(800);
const CRC_FIRMWARE_VERSION: u8 = 0x19;
const TRANSFER_RESET_DELAY: Duration = Duration::from_millis(100);
const DISPLAY_REFRESH_SETTLE_DELAY: Duration = Duration::from_secs(35);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BleDevice {
    pub id: String,
    pub name: String,
    pub rssi: Option<i16>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BleGattCharacteristic {
    pub service_uuid: String,
    pub uuid: String,
    pub properties: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BleConnectionInfo {
    pub id: String,
    pub name: String,
    pub connected: bool,
    pub characteristic_count: usize,
    pub characteristics: Vec<BleGattCharacteristic>,
    pub epd_control_characteristic: Option<String>,
    pub firmware_version: Option<u8>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TransferMode {
    Crc,
    Legacy,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferResult {
    pub device_id: String,
    pub blocks_sent: usize,
    pub retry_rounds: u8,
    pub refreshed: bool,
    pub connected: bool,
    pub firmware_version: Option<u8>,
    pub transfer_mode: TransferMode,
    pub mtu: usize,
    pub block_size: usize,
    pub driver_id: u8,
}

pub fn is_nrf_epd_name(name: &str) -> bool {
    name.starts_with(NRF_EPD_PREFIX)
}

pub fn matches_scanned_device(
    device_id: &str,
    expected_name: Option<&str>,
    candidate_id: &str,
    candidate_name: &str,
) -> bool {
    device_id == candidate_id
        || (expected_name == Some(candidate_name) && is_nrf_epd_name(candidate_name))
}

pub fn is_epd_notification_for(
    notification: &ValueNotification,
    characteristic_uuid: uuid::Uuid,
) -> bool {
    notification.uuid == characteristic_uuid
}

pub const EPD_SERVICE_UUID: &str = "62750001-d828-918d-fb46-b6c11c675aec";
pub const EPD_CONTROL_UUID: &str = "62750002-d828-918d-fb46-b6c11c675aec";
pub const EPD_VERSION_UUID: &str = "62750003-d828-918d-fb46-b6c11c675aec";

/// NRF_EPD firmware exposes this vendor service/control UUID pair for EPD transfer.
pub fn is_epd_control_characteristic(characteristic: &btleplug::api::Characteristic) -> bool {
    characteristic.service_uuid.to_string() == EPD_SERVICE_UUID
        && characteristic.uuid.to_string() == EPD_CONTROL_UUID
        && characteristic
            .properties
            .intersects(CharPropFlags::WRITE | CharPropFlags::WRITE_WITHOUT_RESPONSE)
        && characteristic
            .properties
            .intersects(CharPropFlags::NOTIFY | CharPropFlags::INDICATE)
}

fn epd_control_characteristic(
    peripheral: &btleplug::platform::Peripheral,
) -> Result<btleplug::api::Characteristic> {
    peripheral
        .characteristics()
        .iter()
        .find(|characteristic| is_epd_control_characteristic(characteristic))
        .cloned()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "未发现 NRF_EPD 控制特征（62750001 服务 / 62750002 特征，且需支持写入与通知）"
            )
        })
}

async fn firmware_version(peripheral: &btleplug::platform::Peripheral) -> Option<u8> {
    let characteristic = peripheral
        .characteristics()
        .iter()
        .find(|characteristic| {
            characteristic.service_uuid.to_string() == EPD_SERVICE_UUID
                && characteristic.uuid.to_string() == EPD_VERSION_UUID
        })
        .cloned()?;
    peripheral
        .read(&characteristic)
        .await
        .ok()
        .and_then(|value| value.first().copied())
}

fn select_transfer_mode(firmware_version: Option<u8>) -> TransferMode {
    if firmware_version.is_some_and(|version| version >= CRC_FIRMWARE_VERSION) {
        TransferMode::Crc
    } else {
        TransferMode::Legacy
    }
}

fn parse_advertised_mtu(value: &[u8]) -> Option<usize> {
    let value = std::str::from_utf8(value).ok()?;
    let mtu = value.strip_prefix("mtu=")?.trim().parse().ok()?;
    (mtu > 8).then_some(mtu)
}

fn with_advertised_mtu(config: &DeviceConfig, advertised_mtu: Option<usize>) -> DeviceConfig {
    let mut effective = config.clone();
    if let Some(mtu) = advertised_mtu {
        effective.mtu = mtu;
    }
    effective.block_size = effective.block_size.min(effective.mtu.saturating_sub(8));
    effective
}

fn init_command(driver_id: u8) -> Vec<u8> {
    if driver_id == 0 {
        vec![INIT]
    } else {
        vec![INIT, driver_id]
    }
}

async fn initialize_epd(
    peripheral: &btleplug::platform::Peripheral,
    characteristic: &btleplug::api::Characteristic,
    driver_id: u8,
) -> Result<()> {
    peripheral
        .write(
            characteristic,
            &init_command(driver_id),
            WriteType::WithResponse,
        )
        .await?;
    Ok(())
}

pub async fn scan_nrf_epd(adapter: &Adapter, timeout: Duration) -> Result<Vec<BleDevice>> {
    let mut devices = BTreeMap::new();
    adapter.start_scan(ScanFilter::default()).await?;
    sleep(timeout).await;
    for peripheral in adapter.peripherals().await? {
        if let Some(properties) = peripheral.properties().await? {
            if let Some(name) = properties.local_name.filter(|name| is_nrf_epd_name(name)) {
                devices.insert(
                    peripheral.id().to_string(),
                    BleDevice {
                        id: peripheral.id().to_string(),
                        name,
                        rssi: properties.rssi,
                    },
                );
            }
        }
    }
    adapter.stop_scan().await?;
    Ok(devices.into_values().collect())
}

async fn matching_peripheral(
    adapter: &Adapter,
    device_id: &str,
    expected_name: Option<&str>,
) -> Result<Option<(btleplug::platform::Peripheral, String)>> {
    for peripheral in adapter.peripherals().await? {
        let Some(name) = peripheral
            .properties()
            .await?
            .and_then(|properties| properties.local_name)
        else {
            continue;
        };
        if matches_scanned_device(
            device_id,
            expected_name,
            &peripheral.id().to_string(),
            &name,
        ) {
            return Ok(Some((peripheral, name)));
        }
    }
    Ok(None)
}

async fn find_connected_peripheral(
    adapter: &Adapter,
    device_id: &str,
    expected_name: Option<&str>,
) -> Result<(btleplug::platform::Peripheral, String)> {
    // A connected peripheral may stop advertising; reuse the Adapter cache before scanning again.
    let candidate =
        if let Some(found) = matching_peripheral(adapter, device_id, expected_name).await? {
            Some(found)
        } else {
            adapter.start_scan(ScanFilter::default()).await?;
            sleep(Duration::from_secs(4)).await;
            let found = matching_peripheral(adapter, device_id, expected_name).await?;
            adapter.stop_scan().await?;
            found
        };
    if let Some((peripheral, name)) = candidate {
        if !is_nrf_epd_name(&name) {
            bail!("拒绝连接非 NRF_EPD 设备：{name}");
        }
        if !peripheral.is_connected().await? {
            peripheral.connect().await?;
            // CoreBluetooth may report a connection before the peripheral is ready for discovery.
            sleep(Duration::from_millis(220)).await;
        }
        peripheral.discover_services().await?;
        return Ok((peripheral, name));
    }
    bail!("未找到设备；请先扫描，并保持 NRF_EPD 设备在蓝牙范围内")
}

pub async fn connect_nrf_epd(
    adapter: &Adapter,
    device_id: &str,
    expected_name: Option<&str>,
    driver_id: u8,
) -> Result<(BleConnectionInfo, btleplug::platform::Peripheral)> {
    let (peripheral, name) = find_connected_peripheral(adapter, device_id, expected_name).await?;
    let discovered = peripheral.characteristics();
    let control_characteristic = epd_control_characteristic(&peripheral).ok();
    let firmware_version = firmware_version(&peripheral).await;
    if let Some(characteristic) = control_characteristic.as_ref() {
        initialize_epd(&peripheral, characteristic, driver_id).await?;
    }
    let characteristics = discovered
        .iter()
        .map(|characteristic| BleGattCharacteristic {
            service_uuid: characteristic.service_uuid.to_string(),
            uuid: characteristic.uuid.to_string(),
            properties: format!("{:?}", characteristic.properties),
        })
        .collect::<Vec<_>>();
    let info = BleConnectionInfo {
        id: device_id.into(),
        name,
        connected: peripheral.is_connected().await?,
        characteristic_count: characteristics.len(),
        characteristics,
        epd_control_characteristic: control_characteristic
            .map(|characteristic| characteristic.uuid.to_string()),
        firmware_version,
    };
    Ok((info, peripheral))
}

async fn next_epd_status<S>(
    notifications: &mut S,
    characteristic_uuid: uuid::Uuid,
) -> Result<DeviceResponse>
where
    S: futures::Stream<Item = ValueNotification> + Unpin,
{
    timeout(STATUS_TIMEOUT, async {
        while let Some(notification) = notifications.next().await {
            if !is_epd_notification_for(&notification, characteristic_uuid)
                || notification.value.first() != Some(&0xA1)
            {
                continue;
            }
            if let Ok(response @ DeviceResponse::Status { .. }) =
                parse_response(&notification.value)
            {
                return Some(response);
            }
            // A few firmware versions emit a short stale status while switching transfer
            // modes. Ignore it and wait for the complete response to this query.
        }
        None
    })
    .await
    .map_err(|_| anyhow::anyhow!("等待 EPD 状态响应超时"))?
    .ok_or_else(|| anyhow::anyhow!("EPD 通知流已关闭或未收到有效状态响应"))
}

async fn next_advertised_mtu<S>(
    notifications: &mut S,
    characteristic_uuid: uuid::Uuid,
) -> Option<usize>
where
    S: futures::Stream<Item = ValueNotification> + Unpin,
{
    timeout(INIT_RESPONSE_TIMEOUT, async {
        while let Some(notification) = notifications.next().await {
            if is_epd_notification_for(&notification, characteristic_uuid) {
                if let Some(mtu) = parse_advertised_mtu(&notification.value) {
                    return Some(mtu);
                }
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

pub async fn transfer_epd_image_on_peripheral(
    device_id: &str,
    image_layers: &[Vec<u8>],
    config: &DeviceConfig,
    peripheral: &btleplug::platform::Peripheral,
) -> Result<TransferResult> {
    if !peripheral.is_connected().await? {
        peripheral.connect().await?;
        sleep(Duration::from_millis(220)).await;
    }
    peripheral.discover_services().await?;
    transfer_epd_image_connected(device_id, image_layers, config, peripheral).await
}

async fn transfer_epd_image_connected(
    device_id: &str,
    image_layers: &[Vec<u8>],
    config: &DeviceConfig,
    peripheral: &btleplug::platform::Peripheral,
) -> Result<TransferResult> {
    let epd_characteristic = epd_control_characteristic(peripheral)?;
    let firmware_version = firmware_version(peripheral).await;
    peripheral.subscribe(&epd_characteristic).await?;
    let mut notifications = peripheral.notifications().await?;
    initialize_epd(peripheral, &epd_characteristic, config.driver_id).await?;
    let advertised_mtu = next_advertised_mtu(&mut notifications, epd_characteristic.uuid).await;
    let effective_config = with_advertised_mtu(config, advertised_mtu);
    if effective_config.block_size == 0 || effective_config.block_size + 8 > effective_config.mtu {
        bail!("EPD 设备通告的 MTU 无法容纳图像传输包")
    }
    let result = transfer_epd_image_subscribed(
        device_id,
        image_layers,
        &effective_config,
        firmware_version,
        peripheral,
        &epd_characteristic,
        &mut notifications,
    )
    .await;
    let _ = peripheral.unsubscribe(&epd_characteristic).await;
    result
}

async fn transfer_epd_image_subscribed<S>(
    device_id: &str,
    image_layers: &[Vec<u8>],
    config: &DeviceConfig,
    firmware_version: Option<u8>,
    peripheral: &btleplug::platform::Peripheral,
    epd_characteristic: &btleplug::api::Characteristic,
    notifications: &mut S,
) -> Result<TransferResult>
where
    S: futures::Stream<Item = ValueNotification> + Unpin,
{
    if image_layers.len() != config.color_layers as usize {
        bail!(
            "EPD 图层数量不匹配：配置 {} 层，收到 {} 层",
            config.color_layers,
            image_layers.len()
        );
    }
    let requested_mode = select_transfer_mode(firmware_version);
    let mut transfer_mode = requested_mode;
    let mut blocks_sent = 0;
    let mut retry_rounds = 0;
    for (index, image) in image_layers.iter().enumerate() {
        let layer = if index == 0 { BW_LAYER } else { 0x00 };
        let (mode, sent, retries) = transfer_epd_layer(
            image,
            config,
            layer,
            requested_mode,
            peripheral,
            epd_characteristic,
            notifications,
        )
        .await?;
        if mode == TransferMode::Legacy {
            transfer_mode = TransferMode::Legacy;
        }
        blocks_sent += sent;
        retry_rounds += retries;
    }
    for command in finalization_commands() {
        peripheral
            .write(epd_characteristic, &[command], WriteType::WithResponse)
            .await?;
    }
    // The firmware acknowledges the BLE write before the panel's refresh waveform completes.
    // Its driver may wait up to 30 seconds for BUSY to release, so do not report a refreshed
    // display while the panel is still processing the command.
    sleep(DISPLAY_REFRESH_SETTLE_DELAY).await;
    Ok(TransferResult {
        device_id: device_id.into(),
        blocks_sent,
        retry_rounds,
        refreshed: true,
        connected: peripheral.is_connected().await.unwrap_or(false),
        firmware_version,
        transfer_mode,
        mtu: config.mtu,
        block_size: config.block_size,
        driver_id: config.driver_id,
    })
}

async fn transfer_epd_layer<S>(
    image: &[u8],
    config: &DeviceConfig,
    layer: u8,
    requested_mode: TransferMode,
    peripheral: &btleplug::platform::Peripheral,
    epd_characteristic: &btleplug::api::Characteristic,
    notifications: &mut S,
) -> Result<(TransferMode, usize, u8)>
where
    S: futures::Stream<Item = ValueNotification> + Unpin,
{
    match requested_mode {
        TransferMode::Crc => {
            let packets = chunk_image(image, config, layer).map_err(anyhow::Error::msg)?;
            match transfer_crc_packets(&packets, peripheral, epd_characteristic, notifications)
                .await
            {
                Ok((blocks_sent, retry_rounds)) => {
                    Ok((TransferMode::Crc, blocks_sent, retry_rounds))
                }
                Err(crc_error) => {
                    let packets =
                        chunk_legacy_image(image, config, layer).map_err(anyhow::Error::msg)?;
                    let blocks_sent =
                        transfer_legacy_packets(&packets, peripheral, epd_characteristic)
                            .await
                            .map_err(|legacy_error| {
                                anyhow::anyhow!(
                                "CRC 传输失败（{crc_error}），传统传输回退也失败：{legacy_error}"
                            )
                            })?;
                    Ok((TransferMode::Legacy, blocks_sent, 0))
                }
            }
        }
        TransferMode::Legacy => {
            let packets = chunk_legacy_image(image, config, layer).map_err(anyhow::Error::msg)?;
            let blocks_sent =
                transfer_legacy_packets(&packets, peripheral, epd_characteristic).await?;
            Ok((TransferMode::Legacy, blocks_sent, 0))
        }
    }
}

async fn transfer_crc_packets<S>(
    packets: &[Vec<u8>],
    peripheral: &btleplug::platform::Peripheral,
    epd_characteristic: &btleplug::api::Characteristic,
    notifications: &mut S,
) -> Result<(usize, u8)>
where
    S: futures::Stream<Item = ValueNotification> + Unpin,
{
    let total_blocks = validate_transfer_packets(packets).map_err(anyhow::Error::msg)?;
    let session = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u8;
    peripheral
        .write(
            epd_characteristic,
            &[RESET_TRANSFER, session],
            WriteType::WithResponse,
        )
        .await?;
    sleep(TRANSFER_RESET_DELAY).await;

    let mut pending: Vec<u16> = (0..total_blocks).collect();
    let mut retry_rounds = 0_u8;
    let mut blocks_sent = 0_usize;
    while !pending.is_empty() {
        for block_id in pending.drain(..) {
            let packet = &packets[block_id as usize];
            peripheral
                .write(epd_characteristic, packet, WriteType::WithResponse)
                .await?;
            blocks_sent += 1;
        }
        peripheral
            .write(epd_characteristic, &[QUERY_STATUS], WriteType::WithResponse)
            .await?;
        let decision = match next_epd_status(notifications, epd_characteristic.uuid).await? {
            DeviceResponse::Status {
                total_blocks: reported_total,
                received_blocks,
                bitmap,
                ..
            } if reported_total == total_blocks => {
                validate_status_bitmap(total_blocks, received_blocks, &bitmap)
                    .map_err(anyhow::Error::msg)?;
                if bitmap.is_empty() && received_blocks == total_blocks {
                    break;
                }
                decide_retry(total_blocks, &bitmap, retry_rounds).map_err(anyhow::Error::msg)?
            }
            DeviceResponse::Status {
                total_blocks: reported_total,
                ..
            } => bail!("EPD 状态块数不匹配：设备 {reported_total}，主机 {total_blocks}"),
            _ => bail!("EPD 返回非状态响应"),
        };
        pending = decision.pending;
        retry_rounds = decision.retry_rounds;
        if decision.complete {
            break;
        }
    }
    Ok((blocks_sent, retry_rounds))
}

async fn transfer_legacy_packets(
    packets: &[Vec<u8>],
    peripheral: &btleplug::platform::Peripheral,
    epd_characteristic: &btleplug::api::Characteristic,
) -> Result<usize> {
    for packet in packets {
        peripheral
            .write(epd_characteristic, packet, WriteType::WithResponse)
            .await?;
    }
    Ok(packets.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use btleplug::api::Characteristic;
    use uuid::Uuid;

    #[test]
    fn selects_only_epd_control_uuid_with_required_capabilities() {
        let control = Characteristic {
            uuid: Uuid::parse_str(EPD_CONTROL_UUID).unwrap(),
            service_uuid: Uuid::parse_str(EPD_SERVICE_UUID).unwrap(),
            properties: CharPropFlags::WRITE | CharPropFlags::NOTIFY,
            descriptors: Default::default(),
        };
        let wrong_uuid = Characteristic {
            uuid: Uuid::parse_str("62750003-d828-918d-fb46-b6c11c675aec").unwrap(),
            ..control.clone()
        };
        let missing_notify = Characteristic {
            properties: CharPropFlags::WRITE,
            ..control.clone()
        };
        let wrong_service = Characteristic {
            service_uuid: Uuid::parse_str("0000fe59-0000-1000-8000-00805f9b34fb").unwrap(),
            ..control.clone()
        };
        assert!(is_epd_control_characteristic(&control));
        assert!(!is_epd_control_characteristic(&wrong_uuid));
        assert!(!is_epd_control_characteristic(&missing_notify));
        assert!(!is_epd_control_characteristic(&wrong_service));
    }
    #[test]
    fn filters_notifications_to_the_selected_epd_characteristic() {
        let expected = Uuid::parse_str("00000002-0000-1000-8000-00805f9b34fb").unwrap();
        assert!(is_epd_notification_for(
            &ValueNotification {
                uuid: expected,
                value: vec![0xA0]
            },
            expected
        ));
        assert!(!is_epd_notification_for(
            &ValueNotification {
                uuid: Uuid::nil(),
                value: vec![0xA0]
            },
            expected
        ));
    }
    #[test]
    fn rediscovery_accepts_stable_nrf_epd_name_when_corebluetooth_id_changes() {
        assert!(matches_scanned_device(
            "old-id",
            Some("NRF_EPD_8BA2"),
            "new-id",
            "NRF_EPD_8BA2"
        ));
        assert!(!matches_scanned_device(
            "old-id",
            Some("NRF_EPD_8BA2"),
            "new-id",
            "Other_EPD"
        ));
    }
    #[test]
    fn filters_only_nrf_epd_prefix() {
        assert!(is_nrf_epd_name("NRF_EPD_400x300"));
        assert!(is_nrf_epd_name("NRF_EPD"));
        assert!(!is_nrf_epd_name("nrf_epd_400x300"));
        assert!(!is_nrf_epd_name("Other_EPD"));
    }
    #[test]
    fn selects_crc_only_for_supported_firmware() {
        assert_eq!(select_transfer_mode(Some(0x19)), TransferMode::Crc);
        assert_eq!(select_transfer_mode(Some(0x18)), TransferMode::Legacy);
        assert_eq!(select_transfer_mode(None), TransferMode::Legacy);
    }

    #[test]
    fn reads_the_mtu_advertised_by_init_notification() {
        assert_eq!(parse_advertised_mtu(b"mtu=185"), Some(185));
        assert_eq!(parse_advertised_mtu(b"mtu=20\n"), Some(20));
        assert_eq!(parse_advertised_mtu(b"mtu=8"), None);
        assert_eq!(parse_advertised_mtu(&[0xA1, 1, 2]), None);
    }

    #[test]
    fn caps_block_payload_to_advertised_mtu() {
        let config = DeviceConfig::monochrome_400x300();
        let effective = with_advertised_mtu(&config, Some(20));
        assert_eq!(effective.mtu, 20);
        assert_eq!(effective.block_size, 12);
        assert_eq!(effective.block_size + 8, effective.mtu);
    }

    #[test]
    fn init_writes_configured_driver_when_present() {
        assert_eq!(init_command(0), vec![INIT]);
        assert_eq!(init_command(0x03), vec![INIT, 0x03]);
    }
}
