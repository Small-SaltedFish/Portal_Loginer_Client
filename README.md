# Portal Loginer 桌面客户端

基于**沈阳建筑大学移动校园宽带**开发的校园网自动登录客户端，支持开机认证、断线重连和托盘常驻。

当前提供 Windows x64 安装包，macOS 尚待适配。其他学校或运营商的认证系统尚未验证。

[下载安装包](https://github.com/Small-SaltedFish/Portal_Loginer_Client/releases/latest) · [反馈问题](https://github.com/Small-SaltedFish/Portal_Loginer_Client/issues)

## 安装与使用

1. 从 Releases 下载 `PortalLoginer-版本号-Setup.exe`，运行后选择安装目录。
2. 连接校园网络，打开桌面快捷方式，填写校园网账号和密码，点击“保存并登录”。首次配置若已有联网会话，按页面提示先退出登录，再验证账号。
3. 登录成功后可自行关闭设置窗口，程序继续在托盘和后台运行。

常用操作：

- **登录**：先尝试退出当前会话，再探测并登录；登出失败不会中断后续登录。
- **退出登录**：保留账号密码，本次开机暂停自动认证；再次点击“登录”恢复，下次开机正常运行。
- **退出程序**：关闭本次运行的托盘和后台服务；启用开机自启时，下次开机仍会启动。
- **检查更新**：在托盘或关于页检查。下载完成后关闭设置窗口，等待后台安装。
- **卸载**：在 Windows“已安装的应用”中卸载，无需单独下载 Cleanup。

程序暂未做系统代码签名。

## 功能

- 单账号配置，密码由 Windows DPAPI 保护，退出登录不会清除密码。
- 后台服务随系统启动，网络就绪后自动认证，不依赖托盘出现。登录前认证需要 Windows 已连接物理网络。
- 认证失效后自动恢复，异常时退避重试。
- 可限制仅在指定 Wi-Fi 下认证，支持选择网络出口。
- 设置窗口按需打开，关闭即退出；托盘提供状态和常用操作。
- 登录成功和网络错误使用系统原生通知。
- 认证请求绕过普通 HTTP/SOCKS 代理；TUN 或透明过滤异常时提供状态提示。
- 从本仓库 Releases 检查更新，GitHub 下载失败时对备用镜像测速；依据官方接口提供的 SHA-256 校验后静默安装。

自动更新约每天检查一次，检查失败后 15 分钟重试。关闭自动更新后仍可手动检查。更新保留账号密码和设置。

## 开发

使用 Rust，固定依赖版本并提交锁文件。在 Windows PowerShell 中运行：

```powershell
.\scripts\dev.ps1 test
.\scripts\dev.ps1 clippy
.\scripts\dev.ps1 build
```

脚本优先使用项目 `.tools` 内的工具链；其他环境可安装固定版本 Rust 后直接使用 Cargo。测试使用构造响应、本机模拟网络和平台接口，不执行真实校园认证。

主要代码：

- `crates/portal-core/`：认证协议、加密、探测与状态机。
- `crates/portal-service/`：Windows 服务、凭据、配置和更新调度。
- `crates/portal-desktop/`：托盘、设置窗口和系统通知。
- `crates/portal-update/`：版本与文件校验、镜像测速及下载。

打包使用 NSIS 3.11，发布步骤见[更新说明](docs/UPDATES.md)。

## 文档与来源

- [桌面版使用说明](docs/DESKTOP.md)
- [Windows 服务说明](docs/WINDOWS.md)
- [更新与镜像下载](docs/UPDATES.md)
- [迁移兼容清单](docs/MIGRATION.md)
- [原行为复核](docs/BASELINE-RESTORATION.md)
- [核心说明](docs/CORE.md)
- [验证记录](docs/VALIDATION.md)

认证逻辑迁移自 [Small-SaltedFish/Portal_Loginer](https://github.com/Small-SaltedFish/Portal_Loginer)，基线提交为 `dcaffa81af551e3f259ddb1433a170aff8b840b6`。沿用 Apache-2.0 许可证，来源记录见 `NOTICE` 和 `baseline.json`；依赖许可见 `THIRD-PARTY-NOTICES.txt`。
