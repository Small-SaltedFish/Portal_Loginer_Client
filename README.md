# Portal Loginer 桌面客户端

项目地址：[Small-SaltedFish/Portal_Loginer_Client](https://github.com/Small-SaltedFish/Portal_Loginer_Client)。

先完成 Windows，再接入 macOS。当前开发版包含登录面板、托盘、后台服务，以及带桌面入口和卸载入口的 Windows 安装包；已接入Release 更新和镜像备用下载。

- [迁移兼容清单](docs/MIGRATION.md)：协议、配置、状态、密钥调用及桌面差异。
- [0.1.3 原行为复核](docs/BASELINE-RESTORATION.md)：本轮重新读取原源码后恢复的入口、缓存、密钥与请求规则。
- [更新接入说明](docs/UPDATES.md)：已实现的验签基础、发布协议及后续接入步骤。
- [验证记录](docs/VALIDATION.md)：本机运行结果及未验证范围。
- [Rust 核心说明](docs/CORE.md)：已实现能力、运行方法、平台接入约定与兼容边界。
- [Windows 服务说明](docs/WINDOWS.md)：服务命令、权限、凭据、网络事件及实机验收边界。
- [桌面版使用说明](docs/DESKTOP.md)：设置、托盘和开发版限制。
- `crates/portal-update/`：来源、版本、SHA-256 文件验证、镜像测速与下载。
- `crates/portal-core/`：协议、加密、直连网络、认证事务与状态决策。
- `crates/portal-service/`：Windows 服务宿主、DPAPI、配置、命名管道与事件调度。
- `crates/portal-desktop/`：托盘、设置窗口与原生通知。
- `Cargo.lock` 与 `rust-toolchain.toml`：依赖锁文件和固定工具链。
- `baseline.json`：来源、固定提交及全部 Git blob 标识。
- `fixtures/protocol.json`：人工构造的协议样例，不含真实账号或抓包。
- `tests/baseline-audit.sh`：补充验证原版的加密和密钥识别行为。
- `upstream/`：原仓库独立检出，固定在 `dcaffa81af551e3f259ddb1433a170aff8b840b6`，只作为迁移参照。

原仓库：[Small-SaltedFish/Portal_Loginer](https://github.com/Small-SaltedFish/Portal_Loginer)。原代码采用 Apache-2.0，许可证和来源保留在 `LICENSE`、`NOTICE` 与 `baseline.json`。桌面实现的行为差异单独记录。

在 Windows PowerShell 中运行：

```powershell
.\scripts\dev.ps1 test
.\scripts\dev.ps1 clippy
.\scripts\dev.ps1 build
```

脚本优先使用项目 `.tools` 内的工具链；其他环境安装 Rust 后可直接运行 `cargo test --workspace --locked`。测试使用模拟响应、本机 HTTP/DNS/TLS、真实 DPAPI/命名管道及只读网络查询，不执行真实校园认证。正式构建生成 `target/release/portal-service.exe`；`scripts/package-dev.ps1` 可生成本地开发压缩包。

完整安装包由 `scripts/package-installer.ps1` 构建，需 NSIS 3.11（优先使用项目 `.tools/nsis-download/nsis-3.11`）。使用方法见桌面说明。后续完成安装卸载实机验证、自动更新、登录前联网、Wi-Fi 和崩溃恢复验收，再接入 macOS。

0.2.6 已接入 Releases 更新。具体下载策略、密钥备份和发布步骤见 docs/UPDATES.md；旧版需先手动升级本次。
