# LLM E-Ink Dashboard 开发文档

> 状态：开发设计稿（MVP 可执行）  
> 目标平台：macOS  
> 技术栈：Rust、Tauri 2、React、SQLite、BLE  
> 目标设备协议来源：[YCD12/EPD-nRF5_DYC](https://github.com/YCD12/EPD-nRF5_DYC)

## 1. 项目目标、范围与非目标

### 1.1 目标

构建一个 macOS 常驻桌面应用，用于统一采集、统计和展示多个 LLM 服务的 token、余额和费用，并将摘要渲染为图片，通过 BLE 更新 nRF5 电子墨水屏。

应用必须同时支持：

- 内置服务适配器：DeepSeek 和 OpenAI-compatible 服务（例如 New API、Sub2API 等网关）；
- 自定义脚本适配器：通过稳定 JSON 契约接入 Codex、Claude Code、企业内部服务或暂未内置的供应商；
- 按服务、账户、模型、自然日/自然月聚合 token；
- 余额和 token 用量分开记录；
- 手动刷新与可配置定时刷新；
- 系统托盘常驻、窗口关闭后继续后台运行、macOS 开机启动；
- 可配置 EPD 型号、分辨率、引脚配置和 BLE 设备；
- Rust 端生成黑白/三色/四色位图，React 不参与设备协议和图像传输。

### 1.2 MVP 范围

首个可用版本包含：

1. Tauri 2 + React 桌面壳；
2. DeepSeek 原生适配器；
3. OpenAI-compatible 通用适配器；
4. 自定义脚本适配器及 JSON 校验；
5. SQLite 快照与按日聚合；
6. 托盘、开机启动、定时器、手动同步；
7. EPD-nRF5 BLE 连接、CRC 图像传输和屏幕刷新；
8. 一个紧凑统计卡片模板；
9. macOS 打包、签名和本地安装说明。

### 1.3 非目标与约束

- 不抓取 ChatGPT、Codex 或 Claude Code 的私有网页、浏览器 Cookie 或未公开接口；这些服务通过官方 API、程序代理或用户脚本接入。
- 不在 nRF5 固件中实现 HTTPS、数据库或 LLM 业务逻辑；nRF5 只接收 EPD 指令和图像数据。
- 不保存 prompt、completion、完整对话或 API 响应正文；默认只保存规范化用量和必要的错误信息。
- 不承诺所有供应商都能提供历史用量或余额；无法获得的字段使用 `null`，并显示数据来源和可信度。
- 首版不做云同步、账号共享、脚本市场和可视化布局编辑器。

## 2. 技术选型与总体架构

### 2.1 选择 Tauri 2 + Rust + React

Tauri 2 提供 Rust 命令与 Web 前端之间的边界。React 负责配置、表格、状态和日志；Rust 负责网络、密钥、SQLite、脚本进程、定时任务、位图渲染和 BLE。前端使用 `invoke` 调用 `#[tauri::command]` 命令，使用事件订阅进度和错误。

这样可以避免：

- 将 DeepSeek/API Key 暴露给 WebView；
- 在浏览器中实现 BLE 协议和 CRC 重传；
- 把供应商差异、定时器和数据库逻辑散落在 React 组件中。

### 2.2 分层架构

```text
React UI
  ├─ 配置页 / 数据源页 / 统计页 / 设备页 / 日志页
  └─ invoke(command) + listen(event)
             ↓
Tauri 应用层
  ├─ commands：配置、同步、预览、设备控制
  ├─ scheduler：手动/定时任务、取消与去重
  └─ state：运行状态、连接状态、最近快照
             ↓
领域层（纯 Rust，可单元测试）
  ├─ provider：DeepSeek、OpenAI-compatible、Script
  ├─ normalize：统一 UsageSnapshot
  ├─ aggregate：日/月/累计统计、费用计算
  ├─ render：统计卡片 → EPD 位图
  └─ epd：BLE、协议包、CRC、重试
             ↓
基础设施层
  ├─ SQLite（快照、聚合、同步日志）
  ├─ macOS Keychain（密钥）
  ├─ reqwest/Tokio（HTTP）
  ├─ 子进程（用户脚本）
  └─ CoreBluetooth 封装（BLE）
```

### 2.3 设计原则

- `ProviderAdapter` 只负责获取和规范化数据，不直接写数据库或更新 UI。
- 统计函数接收不可变快照，保证重复同步不会改变历史结果。
- 所有外部数据先通过 schema 校验，再进入聚合和渲染。
- 每个同步任务拥有 `sync_id`，日志和错误可追踪。
- 设备同步使用队列串行化；同一设备同时只允许一个传输任务。
- 供应商不可用时保留上次成功数据，并在屏幕上标记采集时间，而不是显示为零。

## 3. 模块、目录结构与数据模型

### 3.1 推荐目录

```text
llm-eink-dashboard/
├── src/                         # React 前端
│   ├── app/                     # 路由、全局状态、Tauri 事件
│   ├── components/              # 表格、卡片、表单、日志
│   ├── pages/                   # Overview、Sources、Devices、Settings
│   ├── lib/                     # invoke 封装、格式化、校验
│   └── styles/
├── src-tauri/
│   ├── src/
│   │   ├── main.rs
│   │   ├── commands.rs
│   │   ├── state.rs
│   │   ├── scheduler.rs
│   │   ├── domain/
│   │   │   ├── snapshot.rs
│   │   │   ├── aggregate.rs
│   │   │   └── pricing.rs
│   │   ├── providers/
│   │   │   ├── mod.rs
│   │   │   ├── deepseek.rs
│   │   │   ├── openai_compatible.rs
│   │   │   └── script.rs
│   │   ├── storage/              # migrations、repositories
│   │   ├── render/               # 字体、布局、位图、抖动
│   │   ├── epd/                  # BLE 与 EPD-nRF5 协议
│   │   └── secrets/              # macOS Keychain
│   ├── migrations/
│   └── tauri.conf.json
├── scripts/examples/             # 示例数据源脚本
├── fixtures/                     # API、脚本、BLE 测试样例
└── DEVELOPMENT.md
```

### 3.2 核心类型

```rust
struct UsageSnapshot {
    source_id: String,
    provider: String,
    account_id: String,
    model: String,
    observed_at: DateTime<Utc>,
    period: Period,                 // instant/day/month/total
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cached_tokens: Option<u64>,
    total_tokens: Option<u64>,
    balance_amount: Option<Decimal>,
    balance_currency: Option<String>,
    cost_amount: Option<Decimal>,
    cost_currency: Option<String>,
    quota_used: Option<Decimal>,
    quota_limit: Option<Decimal>,
    confidence: DataConfidence,     // exact/estimated/manual/stale
}
```

`total_tokens` 不应盲目用 `input + output` 覆盖供应商值；若供应商提供官方 total，优先使用官方值。缓存命中/未命中等扩展字段放入结构化 JSON，不改变基础聚合字段。

### 3.3 SQLite 表

- `sources`：数据源类型、名称、启用状态、配置 JSON、密钥引用；
- `accounts`：数据源下的账户和显示名称；
- `snapshots`：规范化快照、时间、周期、唯一去重键；
- `daily_usage`：按本机时区自然日、源、账户、模型聚合；
- `monthly_usage`：按自然月聚合；
- `pricing_rules`：模型价格、币种、生效时间；
- `devices`：BLE 标识、屏幕型号、宽高、颜色层、引脚配置；
- `sync_runs`：开始/结束时间、状态、错误摘要、传输字节数；
- `app_settings`：刷新频率、默认模板、是否开机启动等。

快照去重键建议为 `(source_id, account_id, model, period, observed_at, provider_record_id)`。只支持累计值的服务必须额外保存 `source_record_id` 或采集时间，避免重复累加。

## 4. 数据源适配器与脚本契约

### 4.1 适配器接口

```rust
#[async_trait]
trait ProviderAdapter: Send + Sync {
    fn kind(&self) -> &'static str;
    async fn validate(&self, config: &SourceConfig) -> Result<ValidationReport>;
    async fn fetch(&self, config: &SourceConfig, range: QueryRange)
        -> Result<Vec<UsageSnapshot>>;
}
```

适配器必须声明能力：`usage_api`、`balance_api`、`request_proxy`、`model_breakdown`、`historical_range`。UI 根据能力显示可用字段，不伪造供应商未提供的数据。

### 4.2 内置数据源

#### DeepSeek

- 请求统计：应用自身调用 `/chat/completions` 时读取响应 `usage.prompt_tokens`、`completion_tokens`、`total_tokens`，并可记录缓存 token 细分；
- 余额：调用 `GET /user/balance`，记录币种、总余额、赠送余额和充值余额；
- API Key 只从 Keychain 取出并在 Rust 内存中使用；
- 支持模型维度统计和价格规则配置；
- 不将余额当作 token 配额。

#### OpenAI-compatible

用于 New API、Sub2API 或其他兼容服务：

- `base_url`、API 路径、认证头、模型名、请求格式均可配置；
- 优先解析标准 `usage` 对象；
- 余额和历史用量接口因网关实现不同，作为可选 endpoint 配置；
- 若只支持代理请求，则使用代理模式记录精确用量。

### 4.3 自定义脚本

脚本在受控环境中执行，参数通过环境变量或标准输入传递，结果只读标准输出。禁止脚本直接修改应用数据库。

输入环境变量：`LLM_DASHBOARD_SOURCE_ID`、`LLM_DASHBOARD_RANGE_START`、`LLM_DASHBOARD_RANGE_END`、`LLM_DASHBOARD_CONFIG_JSON`。API Key 不通过命令行参数传递；如脚本需要密钥，由脚本自行从 macOS Keychain 或其安全配置读取。

输出必须是 UTF-8 JSON，推荐结构：

```json
{
  "schemaVersion": 1,
  "source": "claude-code",
  "updatedAt": "2026-08-20T14:00:00Z",
  "accounts": [
    {
      "id": "personal",
      "label": "个人账户",
      "balance": {"amount": 12.5, "currency": "USD"},
      "models": [
        {
          "id": "claude-code",
          "period": "day",
          "inputTokens": 1200,
          "outputTokens": 3400,
          "cachedTokens": 0,
          "totalTokens": 4600,
          "cost": {"amount": 0.12, "currency": "USD"},
          "confidence": "exact"
        }
      ]
    }
  ]
}
```

必填字段为 `schemaVersion`、`source`、`updatedAt`、账户 ID、模型 ID 和周期；缺失指标使用 `null`。应用需要限制执行时长、输出大小、子进程数量，并将 stderr 截断后写入同步日志。

### 4.4 代理模式

当供应商没有历史用量接口时，可将 LLM 请求配置为由本程序转发。代理只保存响应中的规范化 `usage` 和请求元数据（时间、模型、请求 ID、状态码），不保存请求消息或回复内容。代理必须提供关闭开关、超时、重试和隐私提示。

## 5. 用量采集、统计与余额

### 5.1 时间口径

- 统计周期为自然日和自然月；
- 日期边界使用 macOS 当前时区，数据库内部使用 UTC 时间戳；
- 夏令时切换日按本地日历边界处理；
- UI 明确显示“数据更新时间”和“统计时区”。

### 5.2 聚合规则

- 即时响应记录按请求 ID 去重；
- 日/月聚合按 `(provider, account, model, local_date/local_month)` 分组；
- 若上游返回累计值，使用差分或记录为累计快照，不重复相加；
- 只有 `exact` 数据参与精确费用汇总；`estimated` 数据单独标记；
- 数据源失败时保留最近成功值，标记 `stale` 并显示失败时间。

### 5.3 费用与余额

MVP 主显示指标为 token 和余额。费用为可选派生字段：

```text
cost = input_tokens × input_price
     + output_tokens × output_price
     + cached_tokens × cached_price
```

价格规则必须带币种、生效时间和来源；不同币种不在数据库内隐式换算。余额使用供应商返回的原币种，屏幕上最多显示一个主币种，其他币种在 UI 展开查看。

### 5.4 同步流程

```text
触发（手动/定时）
  → 创建 sync_run
  → 并发获取启用数据源（同一源串行）
  → 校验与规范化
  → 写入 snapshots
  → 更新日/月聚合
  → 生成屏幕 ViewModel
  → Rust 渲染位图
  → BLE 传输并刷新
  → 写入结果与耗时
```

任何单个源失败不应阻断其他源；设备传输失败则保留最新快照，下一次任务可重试。

## 6. EPD-nRF5 BLE 适配与 Rust 图像渲染

### 6.1 仓库协议摘要

目标仓库的 EPD 服务 UUID 基于 vendor UUID，服务短 UUID 为 `0x0001`，写入/通知特征短 UUID 为 `0x0002`，版本特征为 `0x0003`。设备名默认前缀为 `NRF_EPD`。

主要命令：

| 命令 | 值 | 用途 |
|---|---:|---|
| `SET_PINS` | `0x00` | 设置引脚映射 |
| `INIT` | `0x01` | 初始化 EPD 驱动 |
| `CLEAR` | `0x02` | 清屏 |
| `WRITE_IMAGE` | `0x30` | 传统 RAM 写入 |
| `WRITE_BLOCK` | `0x31` | 带 CRC 的分块写入 |
| `QUERY_STATUS` | `0x32` | 查询已收块位图 |
| `RESET_TRANSFER` | `0x33` | 重置传输状态 |
| `REFRESH` | `0x05` | 刷新屏幕 |
| `SLEEP` | `0x06` | EPD 休眠 |

优先使用 `WRITE_BLOCK`。数据包格式为：

```text
[cmd:1][block_id:2 LE][total_blocks:2 LE][cfg:1][payload:N][crc16:2 LE]
```

CRC 为 CRC16-CCITT（实现与仓库 `ble_transfer.js` / 固件一致）。每个块确认响应为 `[0xA0, block_id LE, status]`；状态查询响应以 `0xA1` 开头，包含总块数、已接收块数、session 和位图。断线重连后根据位图只重传缺失块，最多执行 3 轮重试。

### 6.2 设备型号配置

配置项包括：设备名称过滤、BLE 地址/标识、屏幕宽高、黑白/三色/四色层、EPD 驱动 ID、MTU、块大小、引脚映射。README 当前列出的常见型号包括 nRF52810/nRF52811 + UC8176、UC8276、SSD1619、SSD1683、JD79668 等；实际可用型号以设备固件配置为准，不在应用内硬编码为唯一选项。

### 6.3 渲染器

渲染输入是与设备无关的 `DashboardViewModel`，输出是一个或多个颜色层的 `Vec<u8>`。首版布局：标题、更新时间、今日总 token、本月总 token、按模型前 N 名、余额和数据源状态。

渲染器职责：

- 选择内置字体并处理 UTF-8 文本回退；
- 根据设备分辨率自动缩放字号和边距；
- 将黑白/红/黄层分别编码为 EPD RAM 字节；
- 在刷新前做尺寸、字节数和颜色层校验；
- 输出 PNG 预览供 React 展示，实际传输使用原始位图。

### 6.4 刷新策略

默认只在数据发生变化、手动点击或定时任务到期时刷新。传输前比较渲染内容 hash；内容未变化时跳过 BLE 写入。刷新结束后发送 `REFRESH`，等待设备完成，再发送 `SLEEP`（如设备驱动要求）。

## 7. React UI、系统托盘、开机启动与刷新调度

### 7.1 页面

- **概览**：今日/月度/累计 token、余额、费用、最近同步状态；
- **数据源**：新增 DeepSeek、兼容接口或脚本，测试连接，启停和排序；
- **模型统计**：按服务、账户、模型筛选，查看日/月表格；
- **设备**：扫描、连接、选择型号/分辨率、预览和立即刷新；
- **计划任务**：启用、间隔、静默时段、失败重试；
- **设置**：开机启动、托盘行为、时区、默认布局和日志级别。

### 7.2 Tauri 命令

建议命令：

```text
list_sources / save_source / delete_source / test_source
sync_all / sync_source / cancel_sync / get_sync_status
get_overview / query_usage / preview_dashboard
scan_devices / connect_device / disconnect_device / push_dashboard
get_settings / save_settings / set_autostart
```

事件：`sync-progress`、`source-updated`、`device-state`、`render-preview`、`sync-error`、`tray-action`。

### 7.3 常驻与调度

- 关闭主窗口默认隐藏到托盘，不退出进程；
- 托盘菜单提供“打开面板、立即同步、连接设备、退出”；
- 调度器使用单一任务队列，避免重复同步；
- UI 可配置间隔、首次启动是否立即执行、失败重试次数和静默时段；
- 睡眠唤醒后重新检查 BLE 和数据源，再决定是否补偿同步；
- macOS 开机启动使用 Tauri 官方 autostart 能力，设置变更可回滚。

## 8. 配置、安全与隐私

### 8.1 密钥

API Key 不进入 React 状态持久化、SQLite、日志、命令行参数或屏幕。使用 macOS Keychain 保存，SQLite 只保存 `secret_ref`。日志对 Authorization、Cookie 和脚本环境变量做脱敏。

### 8.2 脚本安全

脚本是本机代码，安装/启用时必须显示完整路径并要求确认。执行时设置超时、最大输出、工作目录和环境变量白名单；默认不继承全部 shell 环境。脚本返回非零码或 schema 错误时只产生失败日志，不写入部分结果。

### 8.3 网络与隐私

只连接用户配置的 endpoint。默认不上传数据、不启用遥测。请求日志只保存 URL 主机、状态码、耗时和 request ID，不保存请求正文。用户删除数据时仅删除本地快照和日志，不影响 Keychain 中未引用的密钥；删除密钥前要求二次确认。

### 8.4 权限与错误

首次使用 BLE、开机启动或脚本时给出 macOS 权限说明。错误分为可重试网络错误、认证错误、schema 错误、设备连接错误和渲染错误，并在 UI 显示用户可执行的下一步。

## 9. MVP、后续迭代、测试、构建与发布

### 9.1 实施阶段

**阶段 A：骨架与数据层**

- 初始化 Tauri 2 + React；
- 建立 SQLite migration、Keychain 封装和统一类型；
- 完成 mock provider、概览页面和同步日志。

**阶段 B：数据源**

- 实现 DeepSeek；
- 实现 OpenAI-compatible；
- 实现脚本执行器、JSON schema 和示例脚本；
- 增加按模型日/月聚合。

**阶段 C：设备链路**

- macOS BLE 扫描/连接；
- 实现 EPD 服务发现、命令封装、CRC 分块、状态查询和断线重试；
- 实现 Rust 位图渲染器和 PNG 预览；
- 完成一款设备的端到端刷新。

**阶段 D：常驻与发布**

- 托盘、开机启动、调度器、静默时段；
- 签名、DMG/应用包、升级说明；
- 完成 macOS 权限和恢复流程。

### 9.2 后续迭代

- 多个墨水屏轮播和不同布局；
- 历史图表、预算告警、余额阈值通知；
- 更多官方适配器；
- 可签名脚本包/脚本市场；
- 布局 DSL 或可视化编辑器；
- Windows/Linux BLE 后端；
- 加密导出与跨设备同步。

### 9.3 测试策略

- **领域单测**：聚合、时区边界、累计值差分、费用计算、schema 校验；
- **适配器测试**：固定 JSON fixtures、HTTP mock、认证失败和限流；
- **脚本测试**：正常输出、空字段、超时、超大输出、非法 JSON、恶意 stderr；
- **BLE 测试**：CRC、块序号、位图恢复、断线重连、重复块和超 MTU；
- **渲染测试**：不同分辨率/颜色层的 golden image 与字节长度；
- **端到端测试**：mock provider → SQLite → render → fake EPD；
- **手工验收**：macOS 睡眠唤醒、托盘退出、开机启动、Keychain 权限和真实屏幕刷新。

### 9.4 构建与发布

开发环境：稳定版 Rust、Node.js LTS、Tauri CLI、Xcode Command Line Tools。常用命令：

```bash
npm install
npm run tauri dev
npm run test
npm run tauri build
```

发布前检查：

1. 生产构建不包含测试 API Key、fixture 密钥或调试日志；
2. macOS 应用完成签名和 notarization（如公开分发）；
3. 数据库 migration 可从空库和上一版本升级；
4. 首次运行没有设备或数据源时仍能打开 UI；
5. 同步失败不会清空上一次可用统计；
6. README/变更日志明确支持的 EPD 型号和 BLE 权限要求。

## 10. 验收标准

MVP 视为完成的条件：

- 能在 macOS 托盘常驻，关闭窗口不终止后台任务；
- 能配置至少一个 DeepSeek 和一个 OpenAI-compatible 数据源；
- 能运行一个自定义脚本并拒绝非法 JSON；
- 能按自然日、自然月、账户和模型显示 token；
- 能显示余额，并区分余额、token 用量和估算费用；
- 不保存 prompt、completion 或 API Key 明文；
- 能连接 `NRF_EPD`，使用 CRC 分块协议完成一次黑白屏刷新；
- 设备断线后能提示错误并在下一次任务重试；
- 能预览与实际屏幕一致的统计卡片；
- 可打包为 macOS 应用并在无开发环境机器上启动。
