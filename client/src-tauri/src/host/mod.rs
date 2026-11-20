//! 被控端服务：保持与服务器的连接（自动重连、取回原控制码），并为每个控制端请求启动会话。

pub(crate) mod audio;
pub mod capture;
pub(crate) mod clock;
mod cursor;
#[cfg(windows)]
pub(crate) mod desktop;
pub mod encoder;
pub mod gate;
mod input;
#[cfg(target_os = "macos")]
pub(crate) mod macos;
pub(crate) mod mic;
mod pace;
mod pipeline;
#[cfg(target_os = "macos")]
pub(crate) mod sck;
mod session;
#[cfg(target_os = "linux")]
pub(crate) mod x11;

use std::sync::Arc;

use airhand_link::ServerState;
use airhand_link::files::LocalFs;
use airhand_link::host::HostEvents;
use airhand_proto::signal::Reclaim;

use gate::DesktopGate;
use tauri::async_runtime::JoinHandle;
use tokio::sync::mpsc;
use tracing::warn;

pub use airhand_link::host::HostCmd;

use crate::config;
use crate::status::StatusHub;

/// 在"每显示器 DPI 感知"上下文中执行（线程级）。
///
/// DXGI 返回的是物理像素坐标，而 DPI 未感知的线程调用 `GetSystemMetrics` 等 API 会得到
/// 缩放后的逻辑坐标，两者混用会导致鼠标位置错位。所有涉及坐标的调用都包在这里面。
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn dpi_aware<T>(f: impl FnOnce() -> T) -> T {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext,
        };
        let old = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let r = f();
        if !old.is_invalid() {
            SetThreadDpiAwarenessContext(old);
        }
        r
    }
    #[cfg(not(windows))]
    f()
}

/// 被控端缺少的系统权限：`(屏幕与系统录音, 辅助功能)`，true = 缺少。目前只有 macOS 需要用户授权。
pub(crate) fn missing_permissions() -> (bool, bool) {
    #[cfg(target_os = "macos")]
    {
        let p = macos::permissions();
        (!p.screen, !p.input)
    }
    #[cfg(not(target_os = "macos"))]
    {
        (false, false)
    }
}

/// 本机复制的文件交给谁读（公告给控制端，它粘贴时来取）：一般是本进程直接读（[`LocalFs`]）；
/// Windows 服务里的被控端是 SYSTEM，交给托盘以登录的用户的身份读（见 `winsvc::clip`），没有托盘就不公告。
/// 一个都不在了（剪贴板里的路径都没了）时也不公告
pub(crate) async fn clip_provider(
    paths: Vec<std::path::PathBuf>,
) -> Option<Arc<dyn airhand_link::files::Provider>> {
    #[cfg(windows)]
    if crate::winsvc::is_agent() {
        return crate::winsvc::clip::provider(paths).await;
    }
    let local = tokio::task::spawn_blocking(move || LocalFs::new(&paths))
        .await
        .ok()?;
    (!local.is_empty()).then(|| Arc::new(local) as Arc<dyn airhand_link::files::Provider>)
}

/// 采集失败是不是因为缺少系统权限（这种情况由会话用 `Notice::MissingPermissions` 单独说明）。
pub(crate) fn missing_permission(e: &anyhow::Error) -> bool {
    #[cfg(target_os = "macos")]
    {
        e.chain().any(|c| c.is::<macos::PermissionDenied>())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = e;
        false
    }
}

pub struct HostService {
    /// 监督任务：接收本机命令和"切换中转"，管理当前那一轮连接任务
    task: JoinHandle<()>,
    /// 托盘菜单发来的命令入口（长期不变，切换中转后仍然有效）
    cmd: mpsc::UnboundedSender<HostCmd>,
    /// 切换中转的入口（托盘进程直接调用 / Windows 服务收到 IPC 后调用）
    switch: mpsc::UnboundedSender<Arc<airhand_link::LinkConfig>>,
    gate: Arc<DesktopGate>,
}

impl HostService {
    /// 启动被控端，状态写进 `hub` 的 [`AppStatus::host`](crate::status::AppStatus::host)。
    pub fn start(hub: StatusHub) -> Self {
        clock::init();
        let gate = DesktopGate::load(hub.clone());
        let (cmd, rx) = mpsc::unbounded_channel();
        let (switch, switch_rx) = mpsc::unbounded_channel();
        let task = tauri::async_runtime::spawn(supervise(
            hub,
            gate.clone(),
            rx,
            switch_rx,
            config::link(),
        ));
        Self {
            task,
            cmd,
            switch,
            gate,
        }
    }

    pub fn send(&self, c: HostCmd) {
        let _ = self.cmd.send(c);
    }

    /// 热切换中转：旧连接任务被中止（QUIC 会话随之关闭），用新 link 重连，进程不重启。
    pub fn switch_relay(&self, link: Arc<airhand_link::LinkConfig>) {
        let _ = self.switch.send(link);
    }

    /// 交给系统服务：IPC 收到托盘的切换命令后调用（见 [`crate::winsvc`]）
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn switcher(&self) -> mpsc::UnboundedSender<Arc<airhand_link::LinkConfig>> {
        self.switch.clone()
    }

    /// 本机的身份、配对码、配对过的控制端（托盘菜单里换配对码、忘掉配对过的控制端）
    pub fn gate(&self) -> &Arc<DesktopGate> {
        &self.gate
    }

    /// 向被控端发命令的另一个入口（系统服务里的被控端经命名管道收到托盘的命令）
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn commands(&self) -> mpsc::UnboundedSender<HostCmd> {
        self.cmd.clone()
    }
}

impl Drop for HostService {
    fn drop(&mut self) {
        // 中止任务会依次释放会话、QUIC 连接（自动向服务器发送关闭帧）
        self.task.abort();
    }
}

/// 常驻监督任务：外部命令入口和切换入口都是长期的，内层连接任务每次切换重建。
///
/// `airhand_link::host::run` 自己会重连掉线，但 link 是启动时传进去的、运行中不变；
/// 换中转只能中止它再用新 link 跑一遍。中止时 `JoinHandle` 的 Drop 会关掉 QUIC 会话。
async fn supervise(
    hub: StatusHub,
    gate: Arc<DesktopGate>,
    mut cmds: mpsc::UnboundedReceiver<HostCmd>,
    mut switch: mpsc::UnboundedReceiver<Arc<airhand_link::LinkConfig>>,
    link: Arc<airhand_link::LinkConfig>,
) {
    // 内层任务用的命令通道：切换时换一对，外层的 cmds 接收端始终不变
    let (inner_tx, inner_rx) = mpsc::unbounded_channel();
    let mut current = tauri::async_runtime::spawn(run_with(
        hub.clone(),
        gate.clone(),
        inner_rx,
        link,
    ));
    let mut inner_tx = inner_tx;
    loop {
        tokio::select! {
            // 外层收命令，转发给当前内层任务
            Some(cmd) = cmds.recv() => {
                let _ = inner_tx.send(cmd);
            }
            // 切换中转：中止旧任务、换新的 cmd 通道和 link
            Some(link) = switch.recv() => {
                current.abort();
                let (tx, rx) = mpsc::unbounded_channel();
                inner_tx = tx;
                current = tauri::async_runtime::spawn(run_with(
                    hub.clone(), gate.clone(), rx, link,
                ));
            }
            // 内层任务结束（正常运行中它自己会重连，只有被中止才会到这里）
            _ = &mut current => {
                return;
            }
        }
    }
}

async fn run_with(
    hub: StatusHub,
    gate: Arc<DesktopGate>,
    cmds: mpsc::UnboundedReceiver<HostCmd>,
    link: Arc<airhand_link::LinkConfig>,
) {
    // 保存过的控制码（config.toml）：第一次连接就取回它
    let reclaim = config::settings().code();
    let events = Arc::new(TrayStatus(hub, gate.clone()));
    airhand_link::host::run(
        link,
        config::persistent(),
        reclaim,
        events,
        move |inc| {
            let gate = gate.clone();
            async move {
                let _ = session::run(inc, gate).await;
            }
        },
        cmds,
    )
    .await;
}

/// 被控端的状态写进托盘的 [`StatusHub`]，控制码保存在 config.toml 里
struct TrayStatus(StatusHub, Arc<DesktopGate>);

/// 保存控制码（找不到配置文件的位置时什么也不做）。写不进去只是下次启动时换一个控制码，不影响这次运行
async fn save_code(code: Option<Reclaim>) {
    let r = tokio::task::spawn_blocking(move || config::save_code(code)).await;
    if let Ok(Err(e)) = r {
        warn!("can't save the control code: {e:#}");
    }
}

impl HostEvents for TrayStatus {
    fn server(&self, state: ServerState) {
        self.0.send_if_modified(|st| {
            let changed = st.host.server != state;
            st.host.server = state;
            changed
        });
    }

    async fn registered(&self, code: Reclaim, changed: bool) {
        if changed {
            save_code(Some(code)).await;
        }
        self.0.send_modify(|s| s.host.code = Some(code.code));
    }

    async fn released(&self) {
        save_code(None).await;
    }

    fn ended(&self, session_id: u64) {
        self.0
            .send_modify(|s| s.host.controllers.retain(|c| c.session_id != session_id));
    }

    fn offline(&self) {
        self.0.send_modify(|s| {
            s.host.code = None;
            s.host.controllers.clear();
        });
    }

    fn kicked(&self, cmd: HostCmd) {
        // 更换控制码：配对码也换一个，配对过的控制端全部忘掉（被断开的人不能拿着旧的配对连回来）
        if cmd == HostCmd::NewCode {
            self.1.reset();
        }
    }
}
