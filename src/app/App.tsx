import { FormEvent, ReactNode, useCallback, useEffect, useRef, useState } from "react";
import { HashRouter, NavLink, Navigate, Outlet, Route, Routes, useOutletContext } from "react-router-dom";
import { listen } from "@tauri-apps/api/event";
import { Activity, Cable, CalendarClock, ChevronDown, Database, History, Monitor, Pencil, Plus, RefreshCw, Settings as SettingsIcon, Trash2, Unplug, X } from "lucide-react";
import { api, type BleConnectionInfo, type BleDevice, type LogEntry, type Overview, type Schedule, type Settings, type Source, type SourceConfig, type TransferResult } from "../lib/tauri";

const defaultBaseUrl = (kind: SourceConfig["kind"]) => kind === "deepseek" ? "https://api.deepseek.com" : kind === "newapi" ? "https://your-newapi-host/v1" : "https://api.example.com";
const blankSource = (): SourceConfig => ({ id: crypto.randomUUID(), name: "", kind: "deepseek", enabled: true, config: { baseUrl: "https://api.deepseek.com" } });
type ToastKind = "notice" | "error";
type ToastContext = { toast: (message: string, kind?: ToastKind) => void };
type ShellContext = ToastContext & {
  deviceConnection: BleConnectionInfo | null;
  devices: BleDevice[];
  scanning: boolean;
  connectingDeviceId: string | null;
  autoConnectingDevice: boolean;
  setDeviceConnection: (connection: BleConnectionInfo | null) => void;
  connectDevice: (id: string) => Promise<void>;
  disconnectDevice: () => Promise<void>;
};

function Toast({ message, kind, onClose }: { message: string; kind: ToastKind; onClose: () => void }) {
  return <div className={`toast toast-${kind}`} role={kind === "error" ? "alert" : "status"}><span>{message}</span><button className="toast-close" onClick={onClose} aria-label="关闭提示"><X size={15}/></button></div>;
}

function DeviceMenu({ connection, devices, scanning, scanError, autoConnecting, connectingDeviceId, open, onOpenChange, onConnect, onDisconnect }: { connection: BleConnectionInfo | null; devices: BleDevice[]; scanning: boolean; scanError: string | null; autoConnecting: boolean; connectingDeviceId: string | null; open: boolean; onOpenChange: (open: boolean) => void; onConnect: (id: string) => Promise<void>; onDisconnect: () => Promise<void> }) {
  const menuRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const closeOnOutsidePointer = (event: PointerEvent) => { if (!menuRef.current?.contains(event.target as Node)) onOpenChange(false); };
    const closeOnEscape = (event: KeyboardEvent) => { if (event.key === "Escape") onOpenChange(false); };
    document.addEventListener("pointerdown", closeOnOutsidePointer);
    document.addEventListener("keydown", closeOnEscape);
    return () => { document.removeEventListener("pointerdown", closeOnOutsidePointer); document.removeEventListener("keydown", closeOnEscape); };
  }, [onOpenChange, open]);
  return <div className="device-control" ref={menuRef}>
    <button className="device-trigger" onClick={() => onOpenChange(!open)} aria-expanded={open} aria-haspopup="menu" disabled={autoConnecting}>
      <Cable size={16}/><span className="device-trigger-name">{autoConnecting ? "正在连接设备" : connection?.name ?? "连接 EPD 设备"}</span><ChevronDown size={14}/>
    </button>
    {open && <div className="device-dropdown" role="menu">
      {connection ? <>
        <div className="device-dropdown-heading"><strong>{connection.name}</strong><span>已连接</span></div>
        <dl className="device-details"><div><dt>设备 ID</dt><dd>{connection.id}</dd></div><div><dt>固件</dt><dd>{connection.firmwareVersion === null ? "未读取" : `0x${connection.firmwareVersion.toString(16).padStart(2, "0")}`}</dd></div><div><dt>EPD 控制特征</dt><dd>{connection.epdControlCharacteristic ?? "未发现"}</dd></div></dl>
        <button className="device-disconnect" role="menuitem" onClick={() => void onDisconnect().catch(() => undefined)}><Unplug size={15}/>断开设备</button>
      </> : <>
        <div className="device-dropdown-heading"><strong>可连接设备</strong><span>{scanning ? "持续扫描中" : "等待扫描"}</span></div>
        {scanError ? <p className="device-scan-error">{scanError}</p> : null}
        <div className="device-options">{devices.length ? devices.map(device => <button key={device.id} className="device-option" role="menuitem" disabled={connectingDeviceId !== null} onClick={() => void onConnect(device.id)}><span>{device.name}<small>{device.rssi === null ? "信号未知" : `${device.rssi} dBm`}</small></span><b>{connectingDeviceId === device.id ? "连接中" : "连接"}</b></button>) : <p className="device-scan-status">{scanning ? "正在持续查找 NRF_EPD 设备…" : "等待蓝牙扫描启动…"}</p>}</div>
      </>}
    </div>}
  </div>;
}

function Shell() {
  const [toastState, setToastState] = useState<{ message: string; kind: ToastKind } | null>(null);
  const [syncing, setSyncing] = useState(false);
  const [deviceConnection, setDeviceConnection] = useState<BleConnectionInfo | null>(null);
  const [autoConnectingDevice, setAutoConnectingDevice] = useState(false);
  const [devices, setDevices] = useState<BleDevice[]>([]);
  const [scanning, setScanning] = useState(false);
  const [scanError, setScanError] = useState<string | null>(null);
  const [connectingDeviceId, setConnectingDeviceId] = useState<string | null>(null);
  const [deviceMenuOpen, setDeviceMenuOpen] = useState(false);
  const toast = useCallback((message: string, kind: ToastKind = "notice") => setToastState({ message, kind }), []);
  useEffect(() => {
    if (!toastState) return;
    const timeout = window.setTimeout(() => setToastState(null), toastState.kind === "error" ? 8000 : 3500);
    return () => window.clearTimeout(timeout);
  }, [toastState]);
  const sync = useCallback(async () => {
    setSyncing(true);
    try {
      const message = await api.sync();
      setDeviceConnection(null);
      toast(message || "同步并推送已完成");
    } catch (reason) {
      setDeviceConnection(null);
      toast(String(reason), "error");
    } finally {
      setSyncing(false);
    }
  }, [deviceConnection, toast]);
  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    let stop: undefined | (() => void);
    void listen<string>("tray-action", event => { if (event.payload === "sync") void sync(); })
      .then(unlisten => { stop = unlisten; })
      .catch(reason => toast(`托盘事件监听失败：${String(reason)}`, "error"));
    return () => stop?.();
  }, [sync, toast]);
  const connectDevice = useCallback(async (id: string) => {
    setConnectingDeviceId(id);
    try {
      const info = await api.connectDevice(id);
      setDeviceConnection(info);
      setDeviceMenuOpen(false);
      toast(info.epdControlCharacteristic ? "设备已连接并发现 EPD 控制特征" : "设备已连接，但未发现可用 EPD 控制特征");
    } catch (reason) {
      setDeviceConnection(null);
      toast(String(reason), "error");
    } finally {
      setConnectingDeviceId(null);
    }
  }, [toast]);
  const disconnectDevice = useCallback(async () => {
    if (!deviceConnection) return;
    try {
      await api.disconnectDevice(deviceConnection.id);
      setDeviceConnection(null);
      setDeviceMenuOpen(true);
      toast("设备已断开连接");
    } catch (reason) {
      toast(String(reason), "error");
      throw reason;
    }
  }, [deviceConnection, toast]);
  useEffect(() => {
    if (autoConnectingDevice || syncing || deviceConnection) return;
    let cancelled = false;
    setDeviceMenuOpen(true);
    const scanContinuously = async () => {
      setScanning(true);
      while (!cancelled) {
        try {
          const found = await api.scanDevices();
          if (!cancelled) {
            setDevices(found);
            setScanError(null);
          }
        } catch (reason) {
          if (!cancelled) {
            setScanError(`扫描失败：${String(reason)}`);
            await new Promise(resolve => window.setTimeout(resolve, 1000));
          }
        }
      }
      if (!cancelled) setScanning(false);
    };
    void scanContinuously();
    return () => { cancelled = true; };
  }, [autoConnectingDevice, deviceConnection, syncing]);
  const context: ShellContext = { toast, deviceConnection, devices, scanning, connectingDeviceId, autoConnectingDevice, setDeviceConnection, connectDevice, disconnectDevice };
  return <main><aside><div className="brand"><Monitor/><span>INK / LLM</span></div><DeviceMenu connection={deviceConnection} devices={devices} scanning={scanning} scanError={scanError} autoConnecting={autoConnectingDevice} connectingDeviceId={connectingDeviceId} open={deviceMenuOpen} onOpenChange={setDeviceMenuOpen} onConnect={connectDevice} onDisconnect={disconnectDevice}/><nav>
    <NavLink to="/overview"><Activity size={17}/>概览</NavLink><NavLink to="/sources"><Database size={17}/>数据源</NavLink><NavLink to="/devices"><Cable size={17}/>设备</NavLink><NavLink to="/schedule"><CalendarClock size={17}/>计划任务</NavLink><NavLink to="/logs"><History size={17}/>日志</NavLink><NavLink to="/settings"><SettingsIcon size={17}/>设置</NavLink>
  </nav><div className="side-footer">本地优先<br/>密钥只存于 Keychain</div></aside><section className="workspace"><header><div><p className="eyebrow">DASHBOARD / LOCAL</p><h1>LLM E‑Ink Dashboard</h1><p className="muted">按功能页面管理数据、设备和本机设置</p></div><button className="primary" onClick={() => void sync()} disabled={syncing}><RefreshCw size={16} className={syncing ? "spin" : ""}/>{syncing ? "同步中" : "立即同步"}</button></header>{toastState && <Toast {...toastState} onClose={() => setToastState(null)}/>}<Outlet context={context}/></section></main>;
}

function useToast() { return useOutletContext<ToastContext>(); }
function useShell() { return useOutletContext<ShellContext>(); }
function Metric({ label, value }: { label: string; value: string | number }) { return <article className="metric"><p>{label}</p><strong>{typeof value === "number" ? value.toLocaleString() : value}</strong><span>来自最近成功快照</span></article>; }

function Dialog({ title, onClose, onSubmit, submitting, submitLabel = "保存", submittingLabel = "保存中…", children }: { title: string; onClose: () => void; onSubmit: (event: FormEvent) => void; submitting: boolean; submitLabel?: string; submittingLabel?: string; children: ReactNode }) {
  return <div className="dialog-backdrop" role="presentation"><form className="dialog" onSubmit={onSubmit} role="dialog" aria-modal="true" aria-labelledby="dialog-title"><div className="dialog-header"><h2 id="dialog-title">{title}</h2><button type="button" className="icon-button" onClick={onClose} disabled={submitting} aria-label="关闭"><X size={18}/></button></div><div className="dialog-body">{children}</div><div className="dialog-footer"><button type="button" className="secondary" onClick={onClose} disabled={submitting}>取消</button><button type="submit" className="primary" disabled={submitting}>{submitting ? submittingLabel : submitLabel}</button></div></form></div>;
}

function OverviewPage() { const [overview, setOverview] = useState<Overview | null>(null); const [sources, setSources] = useState<Source[]>([]); const [refreshing, setRefreshing] = useState(false); const { toast } = useToast(); const load = useCallback(async () => { setRefreshing(true); try { const [data, items] = await Promise.all([api.overview(), api.sources()]); setOverview(data); setSources(items); } catch (reason) { toast(String(reason), "error"); } finally { setRefreshing(false); } }, [toast]); useEffect(() => { void load(); if (!("__TAURI_INTERNALS__" in window)) return; let stop: undefined | (() => void); void listen("sync-completed", () => { void load(); }).then(unlisten => { stop = unlisten; }).catch(reason => toast(`同步完成事件监听失败：${String(reason)}`, "error")); return () => stop?.(); }, [load, toast]); return <><div className="page-title log-header"><div><h2>用量概览</h2><p>统计时区：系统本地时区 · {overview ? new Date(overview.updatedAt).toLocaleString() : "读取中…"}</p></div><button className="secondary" onClick={() => void load()} disabled={refreshing}><RefreshCw size={16} className={refreshing ? "spin" : ""}/>{refreshing ? "刷新中…" : "刷新数据"}</button></div><div className="metrics"><Metric label="今日 Token" value={overview?.todayTokens ?? 0}/><Metric label="本月 Token" value={overview?.monthTokens ?? 0}/><Metric label="账户余额" value={overview?.balance ?? "—"}/></div><article className="panel full"><p className="eyebrow">DATA SOURCES</p><h2>数据源状态</h2>{sources.length ? sources.map(source => <div className="source" key={source.id}><span>{source.name}</span><small>{source.kind}</small><b>{source.status}</b></div>) : <div className="empty">还没有数据源。请前往“数据源”页面添加。</div>}</article></>; }

function SourcesPage() {
  const { toast } = useToast();
  const [sources, setSources] = useState<Source[]>([]); const [draft, setDraft] = useState<SourceConfig | null>(null); const [secret, setSecret] = useState(""); const [saving, setSaving] = useState(false); const [testing, setTesting] = useState(false); const [deleting, setDeleting] = useState<Source | null>(null); const [removing, setRemoving] = useState(false); const [selectingId, setSelectingId] = useState<string | null>(null);
  const load = async () => setSources(await api.sources());
  useEffect(() => { void load().catch(reason => toast(String(reason), "error")); }, [toast]);
  const edit = async (id: string) => { try { const source = (await api.sourceConfigs()).find(item => item.id === id); if (!source) throw new Error("未找到数据源配置"); setSecret(""); setDraft(source); } catch (reason) { toast(String(reason), "error"); } };
  const remove = async (event: FormEvent) => { event.preventDefault(); if (!deleting) return; setRemoving(true); try { await api.deleteSource(deleting.id); if (draft?.id === deleting.id) { setDraft(null); setSecret(""); } await load(); toast(`已删除数据源：${deleting.name}`); setDeleting(null); } catch (reason) { toast(String(reason), "error"); } finally { setRemoving(false); } };
  const select = async (id: string) => { setSelectingId(id); try { const selected = await api.selectSource(id); setSources(current => current.map(source => ({ ...source, selected: source.id === selected.id, status: source.id === selected.id ? "当前读取" : source.enabled ? "已配置" : "已停用" }))); toast(`当前读取数据源：${selected.name}`); } catch (reason) { toast(String(reason), "error"); } finally { setSelectingId(null); } };
  const test = async () => { if (!draft) return; try { if (!draft.name.trim()) throw new Error("请填写数据源名称"); if (draft.kind !== "script" && !String(draft.config.baseUrl ?? "").trim()) throw new Error("请填写 API 地址"); setTesting(true); const message = await api.testSourceConfig(draft, secret.trim() || undefined); toast(`连接测试成功：${message}`); } catch (reason) { toast(`连接测试失败：${String(reason)}`, "error"); } finally { setTesting(false); } };
  const save = async (event: FormEvent) => { event.preventDefault(); if (!draft) return; const editingExisting = sources.some(source => source.id === draft.id); try { if (!draft.name.trim()) throw new Error("请填写数据源名称"); if (draft.kind !== "script" && !String(draft.config.baseUrl ?? "").trim()) throw new Error("请填写 API 地址"); if (editingExisting && draft.enabled && !secret.trim()) throw new Error("此数据源的 Keychain 凭据缺失；请输入 API Key 后重新保存，或关闭此数据源"); setSaving(true); await api.saveSourceConfig(draft); if (secret.trim()) await api.saveSourceSecret(draft.id, `source.${draft.id}`, secret); setDraft(null); setSecret(""); await load(); toast("数据源已保存"); } catch (reason) { toast(String(reason), "error"); } finally { setSaving(false); } };
  const changeKind = (kind: SourceConfig["kind"]) => { if (!draft) return; setDraft({ ...draft, kind, config: { ...draft.config, baseUrl: defaultBaseUrl(kind) } }); };
  const editingExisting = draft !== null && sources.some(source => source.id === draft.id);
  return <><div className="page-title"><h2>数据源</h2><p>选择一个已启用的数据源供概览与同步读取。凭据仅写入 macOS Keychain。</p></div><article className="panel full"><div className="source-list">{sources.length ? sources.map(source => <div className="source" key={source.id}><label className="checkbox-label"><input type="radio" name="selected-source" checked={source.selected} disabled={!source.enabled || selectingId !== null} onChange={() => void select(source.id)}/>读取此数据源</label><span>{source.name}</span><small>{source.kind}</small><b>{source.status}</b><button className="secondary compact" onClick={() => void edit(source.id)}><Pencil size={15}/>编辑</button><button className="secondary compact" onClick={() => setDeleting(source)}><Trash2 size={15}/>删除</button></div>) : <div className="empty">尚未配置数据源。</div>}</div><button className="secondary" onClick={() => { setSecret(""); setDraft(blankSource()); }}><Plus size={16}/>添加数据源</button></article>{draft && <Dialog title={editingExisting ? "编辑数据源" : "配置数据源"} onClose={() => { if (!saving && !testing) { setDraft(null); setSecret(""); } }} onSubmit={save} submitting={saving || testing} submittingLabel={testing ? "测试中…" : "保存中…"}><button type="button" className="secondary dialog-test" onClick={() => void test()} disabled={saving || testing}><RefreshCw size={15} className={testing ? "spin" : ""}/>{testing ? "测试中…" : "测试连接"}</button><label>名称<input autoFocus value={draft.name} onChange={event => setDraft({ ...draft, name: event.target.value })}/></label><label>类型<select value={draft.kind} onChange={event => changeKind(event.target.value as SourceConfig["kind"])}><option value="openai_compatible">OpenAI-compatible</option><option value="newapi">New API（OpenAI 兼容）</option><option value="deepseek">DeepSeek</option><option value="script">自定义脚本</option></select></label>{draft.kind !== "script" && <label>API 地址<input value={String(draft.config.baseUrl ?? defaultBaseUrl(draft.kind))} onChange={event => setDraft({ ...draft, config: { ...draft.config, baseUrl: event.target.value } })}/></label>}{draft.kind === "newapi" && <label>用户 ID（个人访问令牌必填）<input value={String(draft.config.userId ?? "")} placeholder="New API 用户 ID" onChange={event => setDraft({ ...draft, config: { ...draft.config, userId: event.target.value } })}/></label>}<label>{draft.kind === "newapi" ? "API Key / 个人访问令牌" : "API Key"}{editingExisting ? "（缺少凭据时需要重新输入）" : ""}<input type="password" value={secret} placeholder={draft.kind === "newapi" ? "输入 API Key 或个人访问令牌" : "输入 API Key"} onChange={event => setSecret(event.target.value)}/></label><label className="checkbox-label"><input type="checkbox" checked={draft.enabled} onChange={event => setDraft({ ...draft, enabled: event.target.checked })}/>启用此数据源</label></Dialog>}{deleting && <Dialog title="删除数据源" onClose={() => { if (!removing) setDeleting(null); }} onSubmit={remove} submitting={removing} submitLabel="删除" submittingLabel="删除中…"><p>删除“{deleting.name}”及其本机凭据引用后不可恢复。</p></Dialog>}</>;
}

function DevicesPage() {
  const { toast, deviceConnection: connection, devices, scanning, connectingDeviceId, autoConnectingDevice, connectDevice, disconnectDevice, setDeviceConnection } = useShell();
  const [validating, setValidating] = useState(false);
  const [pushing, setPushing] = useState(false);
  const [preparedBlocks, setPreparedBlocks] = useState<number | null>(null);
  const [result, setResult] = useState<TransferResult | null>(null);
  const [preview, setPreview] = useState<string | null>(null);
  useEffect(() => {
    void api.preview().then(setPreview).catch(reason => toast(`读取推送预览失败：${String(reason)}`, "error"));
  }, [toast]);
  const validateBitmap = async () => {
    setValidating(true);
    try {
      const config = await api.deviceConfig();
      setPreparedBlocks(await api.prepareTransfer(config));
      toast("仪表盘位图和 CRC 分块校验通过");
    } catch (reason) {
      toast(String(reason), "error");
    } finally {
      setValidating(false);
    }
  };
  const disconnect = async () => {
    try {
      await disconnectDevice();
      setResult(null);
    } catch { /* Disconnect feedback is handled by the shared device control. */ }
  };
  const push = async () => {
    if (!connection) return;
    setPushing(true);
    try {
      const config = await api.deviceConfig();
      const next = await api.pushEpdTestImage(connection.id, config);
      setResult(next);
      if (next.connected) {
        setDeviceConnection({ ...connection, connected: true, firmwareVersion: next.firmwareVersion });
        toast("仪表盘位图已传输并刷新电子墨水屏");
      } else {
        setDeviceConnection(null);
        toast("仪表盘位图已刷新，但设备已断开；请重新连接");
      }
    } catch (reason) {
      setDeviceConnection(null);
      toast(`${String(reason)}。传输失败后请重新连接设备再试。`, "error");
    } finally {
      setPushing(false);
    }
  };
  return <>
    <div className="page-title"><h2>电子墨水屏设备</h2><p>连接状态固定显示在左上角；未连接时会持续查找 `NRF_EPD*` 设备。</p></div>
    <article className="panel full">
      <button className="secondary" onClick={() => void validateBitmap()} disabled={validating}><RefreshCw size={16}/>{validating ? "校验中…" : "校验传输位图"}</button>
      {autoConnectingDevice && <p className="device">正在自动连接上次使用的电子墨水屏设备…</p>}
      {!connection && !autoConnectingDevice && <p className="device">{scanning ? "正在持续扫描可连接设备…" : "等待蓝牙扫描启动…"}</p>}
      {preparedBlocks !== null && <p className="device">位图校验通过，已准备 {preparedBlocks} 个 CRC 分块</p>}
      {!connection && devices.map(device => <button className="device device-button" onClick={() => void connectDevice(device.id)} disabled={autoConnectingDevice || connectingDeviceId !== null} key={device.id}>{device.name}{connectingDeviceId === device.id ? " · 连接中…" : device.rssi === null ? "" : ` · RSSI ${device.rssi} dBm`}</button>)}
      {connection && <>
        <p className="device">已连接 {connection.name} · {connection.epdControlCharacteristic ? `控制特征 ${connection.epdControlCharacteristic}` : "未找到 EPD 控制特征"}</p>
        {!connection.epdControlCharacteristic && <div className="gatt-diagnostic">{connection.characteristics.map(item => <code key={`${item.serviceUuid}-${item.uuid}`}>{item.serviceUuid} / {item.uuid} / {item.properties}</code>)}</div>}
        {preview && <div className="epd-preview"><div className="epd-preview-heading"><strong>推送预览</strong><button className="secondary compact" onClick={() => void api.preview().then(setPreview).catch(reason => toast(`刷新预览失败：${String(reason)}`, "error"))}><RefreshCw size={14}/>刷新预览</button></div><div className="epd-preview-canvas" dangerouslySetInnerHTML={{ __html: preview }}/></div>}
        <div className="actions">
          <button className="primary" onClick={() => void push()} disabled={pushing || !connection.epdControlCharacteristic}>{pushing ? "正在传输仪表盘…" : "推送仪表盘到 EPD"}</button>
          <button className="secondary" onClick={() => void disconnect()} disabled={pushing}><Unplug size={16}/>断开连接</button>
        </div>
      </>}
      {result && <p className="device">已发送 {result.blocksSent} 块 · 驱动 0x{result.driverId.toString(16).padStart(2, "0")} · MTU {result.mtu} / 负载 {result.blockSize} 字节 · 重试 {result.retryRounds} 轮 · {result.transferMode === "crc" ? "CRC 传输" : "传统传输"} · {result.connected ? "连接保持中" : "设备已断开"}</p>}
    </article>
  </>;
}

function SchedulePage() { const { toast } = useToast(); const [schedule, setSchedule] = useState<Schedule | null>(null); useEffect(() => { void api.schedule().then(setSchedule).catch(reason => toast(String(reason), "error")); }, [toast]); const save = async () => { if (!schedule) return; try { setSchedule(await api.saveSchedule(schedule)); toast("计划任务已保存"); } catch (reason) { toast(String(reason), "error"); } }; return <><div className="page-title"><h2>计划任务</h2><p>到期后自动刷新用量、连接上次设备、推送数据，完成后断开设备。</p></div>{schedule && <article className="panel editor"><label className="checkbox-label"><input type="checkbox" checked={schedule.enabled} onChange={event => setSchedule({ ...schedule, enabled: event.target.checked })}/>启用自动同步</label><label>同步间隔（分钟）<input type="number" min="1" value={schedule.intervalMinutes} onChange={event => setSchedule({ ...schedule, intervalMinutes: Number(event.target.value) })}/></label><label>重试次数<input type="number" min="0" value={schedule.retryCount} onChange={event => setSchedule({ ...schedule, retryCount: Number(event.target.value) })}/></label><button className="primary" onClick={() => void save()}>保存计划</button></article>}</>; }
function SettingsPage() { const { toast } = useToast(); const [settings, setSettings] = useState<Settings | null>(null); useEffect(() => { void api.settings().then(setSettings).catch(reason => toast(String(reason), "error")); }, [toast]); const save = async () => { if (!settings) return; try { const launchAtLogin = await api.setAutostart(settings.launchAtLogin); setSettings(await api.saveSettings({ ...settings, launchAtLogin })); toast("设置已保存到本机"); } catch (reason) { toast(String(reason), "error"); } }; return <><div className="page-title"><h2>设置</h2><p>配置本机同步和开机启动行为。</p></div>{settings && <article className="panel editor"><label className="checkbox-label"><input type="checkbox" checked={settings.launchAtLogin} onChange={event => setSettings({ ...settings, launchAtLogin: event.target.checked })}/>登录时启动</label><label className="checkbox-label"><input type="checkbox" checked={settings.quietHoursEnabled} onChange={event => setSettings({ ...settings, quietHoursEnabled: event.target.checked })}/>启用免打扰时段</label><label>默认刷新间隔（分钟）<input type="number" min="1" value={settings.refreshMinutes} onChange={event => setSettings({ ...settings, refreshMinutes: Number(event.target.value) })}/></label><button className="primary" onClick={() => void save()}>保存设置</button></article>}</>; }

function LogsPage() { const { toast } = useToast(); const [logPage, setLogPage] = useState({ items: [] as LogEntry[], page: 1, pageSize: 50, total: 0, totalPages: 0 }); const [loading, setLoading] = useState(true); const load = async (page = logPage.page) => { setLoading(true); try { setLogPage(await api.logs(page, logPage.pageSize)); } catch (reason) { toast(String(reason), "error"); } finally { setLoading(false); } }; useEffect(() => { void load(1); }, []); const changePage = (page: number) => { if (loading || page < 1 || page > logPage.totalPages) return; void load(page); }; return <><div className="page-title log-header"><div><h2>日志</h2><p>保留最近 30 天的本地运行事件 · 共 {logPage.total} 条</p></div><button className="secondary" onClick={() => void load(logPage.page)} disabled={loading}><RefreshCw size={16} className={loading ? "spin" : ""}/>刷新</button></div><article className="panel full logs-panel">{loading ? <div className="empty">正在读取日志…</div> : logPage.items.length ? <><div className="logs-list">{logPage.items.map(log => <div className="log-row" key={log.id}><span className={`log-level ${log.level}`}>{log.level === "error" ? "错误" : "信息"}</span><div><strong>{log.message}</strong><small>{new Date(log.occurredAt).toLocaleString()} · {log.action}{log.details ? ` · ${log.details}` : ""}</small></div></div>)}</div><div className="pagination"><button className="secondary compact" onClick={() => changePage(logPage.page - 1)} disabled={logPage.page <= 1}>上一页</button><span>第 {logPage.page} / {logPage.totalPages} 页</span><button className="secondary compact" onClick={() => changePage(logPage.page + 1)} disabled={logPage.page >= logPage.totalPages}>下一页</button></div></> : <div className="empty">暂无日志记录。</div>}</article></>; }

function StartupScreen() {
  return <div className="startup-screen" role="status" aria-live="polite">
    <div className="startup-mark"><Monitor size={34}/></div>
    <strong>LLM E-Ink Dashboard</strong>
    <span>正在加载本地面板</span>
    <div className="startup-progress" aria-hidden="true"><i/></div>
  </div>;
}

export function App() {
  const [ready, setReady] = useState(false);
  useEffect(() => {
    const timer = window.setTimeout(() => setReady(true), 700);
    return () => window.clearTimeout(timer);
  }, []);
  if (!ready) return <StartupScreen/>;
  return <HashRouter><Routes><Route element={<Shell/>}><Route path="/overview" element={<OverviewPage/>}/><Route path="/sources" element={<SourcesPage/>}/><Route path="/devices" element={<DevicesPage/>}/><Route path="/schedule" element={<SchedulePage/>}/><Route path="/logs" element={<LogsPage/>}/><Route path="/settings" element={<SettingsPage/>}/><Route path="*" element={<Navigate to="/overview" replace/>}/></Route></Routes></HashRouter>;
}
