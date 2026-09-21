# Stream 4x5 跨平台控制面板实施计划

## 总结

- 在当前空目录初始化 Rust 2024 Cargo workspace 和 Git 仓库，项目暂名 `lewitt-ctl`，采用 `MIT OR Apache-2.0`。
- 首版仅支持 `Stream 4x5`（USB VID `29C2`、PID `0011`），目标为 Windows 10/11 x64 与 Arch Linux x86_64，均只提供源码构建。
- 应用只负责硬件控制、路由、预设和电平显示；Windows 音频继续使用 ASIO/WASAPI，Linux 使用 ALSA/PipeWire。
- Windows 依赖用户已安装的 Lewitt 2.3.0 原厂驱动；Linux 使用 `rusb/libusb` 实现协议，不捆绑任何原厂文件。
- 首版排除固件升级、其他 DGT 型号、内置音频流、遥测和自动更新。

## 当前实施状态（2026-09-21）

### 已完成并通过本机验证

- 已初始化 Rust 2024 Cargo workspace，包含 `lewitt-core`、Windows/Linux 后端、
  `lewitt-app` 和 `lewittctl` 五个组件。
- `lewitt-core` 已提供统一阻塞式 `DeviceBackend`、领域类型、类型化错误、
  独立设备工作线程、有界命令/事件通道和容量为 1 的最新电平通道。
- 连续控制按约 30 Hz 合并，离散控制立即执行；每次成功写入后强制回读，
  写入或回读失败时尝试刷新完整硬件快照。
- 已实现 `schema_version: 1` 的原生预设模型和完整写前校验；采样率、时钟源、
  ASIO buffer 等排除项不能进入预设。
- 已实现可注入故障的 Mock backend，并覆盖控制合并、回读失败刷新、未知固件
  只读、I/O 中断线、重连、最新电平保留、数值换算和预设校验。
- 已接入 Iced 0.14 daemon：普通启动创建窗口，`--background` 无窗口启动，关闭窗口
  后 daemon 继续驻留，真正退出由生命周期状态机统一处理。
- 已接入 `tray-icon` 的显示、重新连接、状态、退出菜单；Windows 使用原生实现，
  Linux 仅启用 KSNI。菜单可随中英文切换更新，托盘创建失败时后台启动会显示窗口
  报错，避免留下不可操作进程。
- 托盘菜单事件通过独立、可停止的转发线程送入 Iced daemon；窗口已显示时再次点击
  “显示”会先解除最小化，再临时置顶、聚焦并立即恢复普通层级，不会保持长期置顶。
- 托盘图标左键释放会直接执行“显示”；Windows 左键不再弹出菜单，右键菜单保持
  不变，Linux KSNI 使用其原生激活事件。
- Windows 单实例已使用按用户隔离的命名管道接入 daemon；第二次启动发送有界
  `show-window` 命令后退出，主实例显示或一次性置顶现有窗口。监听线程支持停止，
  端点可在主实例退出后重新占用，并有真实命名管道集成测试覆盖。
- 显式自启确认、单实例 IPC 有界消息格式以及中英文资源键一致性检查已完成。
- Windows 后端已按 64 位 CLSID 注册绝对路径动态加载原厂 DLL，仅解析经过文档确认的
  11 个只读导出；已实现 Stream 4x5 严格 VID/PID/型号筛选、句柄 RAII、API 5.2 校验、
  设备属性、40 字节硬件状态、采样率、时钟和 ASIO buffer 读取。写入、raw vendor、
  固件和 DFU 导出均不解析，所有真实写入继续返回 `Unsupported`。
- 已增加显式 ignored 的 Windows 真机只读测试；本机连接的 Stream 4x5 已通过枚举、
  打开、snapshot 读取和关闭验证。Linux 后端仍为安全骨架。
- `lewittctl diagnose [--json]` 已更新为报告已验证的 Windows 只读 ABI 子集；其余要求的
  CLI 命令已建立语法，未验证操作会明确拒绝执行。
- 已建立 `docs/protocol/stream4x5.md` 证据门禁和 Linux udev 示例；已完成控制中心、
  `stream4x5.dll` 与 `dgtstreamapi_x64.dll` 的首轮静态逆向，整理 68 个导出、已用 ABI、
  40/20/64 字节私有控制块、UAC2 时钟只读请求、CRC 与 Windows DSP 属性结构，并加入
  不含序列号的最小只读 fixture。
- 已完成 `dgtstream_mixer_ducker.sys` 的首轮静态逆向，确认其为处理主机 PCM 缓冲区的
  上层过滤 DSP：包含 Q8.24 矩阵/衰减、三路 Ducker、FX 与约 40 ms 窗口的 Peak/RMS
  累积；已记录 WDM/插件回调、DSP 属性校验和 40 字节内部电平记录，并加入不含二进制、
  反汇编正文、PCM 或设备标识的静态 fixture。
- GUI 已接入真实 platform backend 与 controller worker，提供设备连接状态、手动/周期
  刷新、设备/固件/采样率/时钟/ASIO buffer 概览，以及 Input 1/2 前级增益、硬件输出
  增益、Windows 驱动输出 mute、Input 1/2 推子端点状态、48V、80 Hz 高通和相位的只读
  卡片；Input 3/4 明确显示为固定硬件电平，不伪装成可调增益；窗口隐藏时降低
  snapshot 刷新频率。
- Windows x64 本机已通过格式检查、workspace 全特性测试、严格 Clippy 和文档构建。

### 当前未完成和外部门禁

- 已有静态 ABI 分析、脱敏只读 fixture、一台 Stream 4x5 的 Windows vendor-API
  只读验证及 Rust backend 真机 snapshot 验证；尚无 USBPcap 原始总线证据、Linux
  `rusb` 直连、Windows DSP meter 动态验证或任何硬件写入验证，因此真实写控制、
  实时电平和 Lewitt XML 映射仍不得标记为可用。
- 已确认混音矩阵、Ducker、FX 和软件 Peak/RMS 表属于
  `dgtstream_mixer_ducker.sys` 上层过滤驱动的 DSP IOCTL，不是设备 USB 协议；Linux
  版本需另行设计 PipeWire/ALSA 用户态路由与 DSP，不能把这些属性块直接发给 `rusb`。
- 产品范围已收紧：Linux v1 不复刻混音矩阵、虚拟通道、Ducker、Compressor、EQ 或
  Reverb，只在 USB 后端之外设计物理输入/输出的被动 RMS provider；Windows v1 可保留
  驱动混音/路由能力，但界面不提供 Ducker、Compressor、EQ 或 Reverb 控件。逆向所得
  属性结构仅作兼容性证据，不构成必须暴露的产品功能。
- Stream 4x5 配置描述符只有 UAC2 播放 `0x01` 与采集 `0x81` 两个等时 endpoint；没有
  ASIO 专用 USB endpoint。ASIO 是 Windows 主机驱动/API 路径，仍可能因调度、缓冲和
  内核 DSP 与其他主机音频栈表现出不同延迟。
- `x86_64-unknown-linux-gnu` 目标已通过 workspace 全特性交叉检查和严格 Clippy；尚未在
  Arch Linux 环境完成原生构建、KSNI 运行及 ALSA/PipeWire 共存验证。
- Iced/托盘和只读硬件面板已通过 Windows x64 编译、单元测试和静态检查，但本轮
  computer-use 原生应用接口不可用，尚未完成截图/桌面交互验收与 Arch KSNI 运行验证。
- Linux `XDG_RUNTIME_DIR` Unix socket、KSNI、X11/Wayland 条件代码已通过
  `x86_64-unknown-linux-gnu` 交叉编译和严格 Clippy，但尚未在真实 Arch 图形会话运行；
  平台自启写入、完整控制页面和完整 CLI 行为仍待实现。
- 当前仓库没有任何硬件写入测试；以后新增时必须保持显式 opt-in。

### 下一实施批次

1. 在 Arch 环境编译并运行验证 `XDG_RUNTIME_DIR` Unix socket、KSNI 托盘和退出清理。
2. 实现平台自启写入层并保持显式确认门禁。
3. 在 Arch 环境使用系统 `libusb` 实现仅 VID/PID 的只读发现与权限诊断，并验证不解绑
   `snd-usb-audio` 时能否发送 interface-recipient EP0 只读请求。
4. 用 USBPcap 确认文档中的 setup packet，一次只抓取、验证并开放一个真实属性。
5. 单独验证设备侧输出 mute：关闭 Windows DSP mute，只通过原厂推子将一个输出对降至
   `-60 dB`，对照 40 字节块 `12..17`、USBPcap、关闭控制中心后的实际输出及重新连接
   后状态，并恢复测试前增益；在此之前不写入或公开这六个状态位。
6. 为 Linux 物理输入/输出 RMS 另立被动 PipeWire/ALSA meter provider，不混入 USB
   后端，不实现混音、虚拟通道、Ducker 或 FX。

## 架构与公开接口

- 建立共享核心、Windows 后端、Linux USB 后端、Iced GUI、CLI 五个逻辑组件。
- 核心公开统一的阻塞式 `DeviceBackend`，由独立设备线程串行调用：
  - `enumerate/open/close`
  - `capabilities/read_snapshot`
  - `set_control/read_back`
  - `read_meters`
  - `poll_events`
- 公共模型包含：
  - `DeviceInfo`、`DeviceCapabilities`、`DeviceSnapshot`
  - `ChannelId`、`BusId`、`ControlId`
  - `ControlValue`、`ControlCommand`
  - `MeterFrame`、`DeviceEvent`
  - `BackendError::{DriverMissing, PermissionDenied, Busy, Disconnected, ProtocolMismatch, UnsupportedFirmware, InvalidValue}`
- Stream 4x5 目标能力覆盖：
  - Input 1、Input 2 硬件前级增益；Input 3/4 不公开硬件增益或独立静音控制
  - Input 1/2 幻象供电
  - Output 1/2、3/4、5/6 硬件音量；Windows 另读宿主过滤器中的独立输出 mute，
    Linux 不声明该能力且 GUI 按 capability 隐藏，不伪造“关”状态
  - Monitor、Windows 专属混音矩阵权重
  - 采样率、时钟源
  - Windows 专属 ASIO buffer 设置
  - Windows 输入、输出及混音总线 Peak/RMS；Linux 仅物理输入/输出 RMS
- 首版 GUI 不提供 Ducker、Compressor、EQ 或 Reverb 控件；Linux 也不实现 Windows
  驱动提供的虚拟通道和混音矩阵。
- 所有设备调用集中到一个工作线程；连续推子以约 30 Hz 合并写入，离散开关立即写入，每次成功写入后回读。失败时恢复设备真实值并显示错误。
- GUI 可见时按 30 Hz 读取电平；隐藏到托盘后降至 2 Hz，只维持连接和状态检测，使用有界通道丢弃过时帧。

## 实施阶段

### 1. 协议规格与测试基线

- 安装仅供开发使用的 Ghidra、Wireshark/USBPcap。
- 静态分析 `stream4x5.dll` 和 `dgtstreamapi_x64.dll`，还原 API 签名、属性编号、结构布局、范围、字节序和错误码。
- 静态分析 `dgtstream_mixer_ducker.sys`，区分驱动内主机 PCM DSP 与真实 USB 设备协议。
- 在 Windows 中一次只改变一个参数并抓包，记录 USB request、payload、response、CRC、轮询频率和原厂写入顺序。
- 形成 `docs/protocol/stream4x5.md` 和脱敏黄金数据；不得提交原厂 DLL、序列号或完整私有固件数据。

状态：首轮静态逆向、真机只读 vendor-API 验证和最小脱敏 fixture 已完成，见
`docs/protocol/windows-vendor-api.md`、`docs/protocol/windows-filter-driver.md` 与
`docs/protocol/stream4x5.md`。USBPcap 总线抓包、Linux 直连、内核 DSP 属性运行时验证、
任意硬件写入和失败行为验证尚未开始，所有写入门禁保持关闭。

### 2. 平台后端

- Windows 根据 CLSID `{ADACFE1D-A8E1-4606-9093-3A7418223B78}` 从注册表定位 `dgtstreamapi_x64.dll`，动态加载并校验 API/驱动版本；缺少原厂驱动时返回明确诊断，不搜索或加载当前目录中的同名 DLL。
- Linux 使用系统 `libusb` 与 `rusb`，只访问经过确认的控制接口或 EP0 vendor request，不解绑或替换 `snd-usb-audio`。控制操作必须与 ALSA/PipeWire 音频同时工作。
- 实现热插拔、断线重连、权限错误和未知固件保护；未验证固件默认只读。
- 原厂控制中心同时运行时只警告，不终止进程；发生占用或状态覆盖时停止写入、刷新全量状态并提示用户。

状态：Windows 已实现动态加载和真机只读 snapshot，仍禁止全部真实写入和未经验证的
DSP meter；Linux 只读发现及全部 USB 协议操作尚未实现。

### 3. GUI、托盘与自启

- 使用 Iced 0.14 的无初始窗口 `daemon` 模式，配合 `tray-icon`；Arch 使用 KSNI 后端。[Iced daemon](https://docs.rs/iced/0.14.0/iced/fn.daemon.html)、[tray-icon 官方仓库](https://github.com/tauri-apps/tray-icon)。
- 页面包括设备概览、输入、Windows 混音/路由、输出/监听、设备设置、预设和诊断；
  不提供 Ducker、Compressor、EQ 或 Reverb 页面/控件，Linux 不显示混音/路由页。
- 独立现代视觉设计，不复制原厂图标、素材或布局；界面与文档提供中英双语并默认跟随系统。
- 关闭窗口隐藏到托盘；托盘提供显示、重新连接、设备状态和真正退出。
- 首次运行明确询问是否自启，默认勾选但必须由用户确认；Windows 写入当前用户启动项，Arch 写入 XDG autostart 文件。自启使用 `--background` 静默进入托盘。
- 使用 Windows named pipe / Linux Unix socket 保证单实例；再次手动启动时通知已有实例显示窗口。

状态：Iced daemon、托盘交互、生命周期、语言资源、自启同意状态、IPC 消息格式、
Windows 命名管道和首版只读硬件面板已完成并通过 Windows 编译及测试；Linux Unix
socket、KSNI、X11/Wayland 已通过 Linux 目标交叉编译和严格 Clippy，但尚未在 Arch
运行验证；Windows 截图/交互验收、自启写入、可写控件、完整页面和 Arch KSNI 运行
验收尚未实现。

### 4. 预设与 CLI

- 内部预设采用带 `schema_version: 1` 的 JSON，只保存 mixer、routing、ducker 和硬件控制，不保存序列号、电平、固件、ASIO buffer、时钟或自启设置。
- 支持导入、导出原厂 XML 1.6；按嵌入 XSD 验证，保留未知元素和属性但不执行未知控制。
- 应用预设前完成范围和设备型号校验，按抓包确认的依赖顺序串行执行；失败即停止并刷新全量状态，不假装事务成功。
- CLI 提供 `list`、`status`、`watch-meters`、`get`、`set`、`preset import/export/apply`、`diagnose` 和 `--json`；发布构建不提供任意 raw USB 写入。
- 日志保存在用户数据目录，默认脱敏序列号与 USB payload，不上传任何数据。

状态：原生 JSON v1 模型及写前校验已完成；CLI 命令面与 Windows 诊断已建立，
真实控制命令和 XML 1.6 尚未实现。

## 测试与验收

- 单元测试覆盖控制范围、dB 转换、命令编解码、CRC、错误映射、预设版本升级和中英翻译键一致性。
- 使用 mock backend 测试热插拔、写入合并、回读失败、断线重连、未知固件只读、托盘生命周期和单实例 IPC。
- 黄金测试逐字节验证 Windows API 调用映射和 Linux USB request；原厂 XML 1.6 完成语义往返测试。
- Windows 实机逐项与原厂控制中心对照，确认所有控制值和电平一致，并覆盖原厂面板同时运行。
- Arch 实机验证普通用户权限、udev、Wayland/X11、托盘、自启，以及控制过程中 ALSA/PipeWire 音频不中断。
- 电平运行时不得出现无界队列增长；设备在轮询或写入中拔出不得崩溃，重新连接后必须重新读取完整状态。
- CI 在 Windows runner 与 Arch 容器执行 `fmt --check`、测试、严格 Clippy、文档构建和许可证检查；硬件测试保留为手动发布门禁。
- 发布前要求 Windows 与 Arch 的全部安全核心功能、实时电平、预设互通、托盘和自启验收通过。

## 已确定的假设

- Linux 使用系统 `libusb`，不静态捆绑 vendored libusb；`rusb` 作为安全 Rust 封装。[rusb 官方仓库](https://github.com/a1ien/rusb)。
- Windows 必须预装官方驱动，项目不得再分发 Lewitt 二进制。
- 项目说明中明确“非 Lewitt GmbH 官方项目”，商标仅用于兼容性描述。
- 首版源码构建说明同时给出 Windows 与 Arch 依赖、udev 安装及自启撤销步骤。
