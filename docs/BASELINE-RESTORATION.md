# 0.1.3 原行为复核

基线：Small-SaltedFish/Portal_Loginer，`dcaffa81af551e3f259ddb1433a170aff8b840b6`。2026-11-08 通过远端 HEAD 再次核对一致，本地 upstream 未修改。

| 原代码位置 | 原行为 | 桌面实现 |
|---|---|---|
| RPC method_set_config | 验证字段后直接保存，空密码保留 | Save 原子保存，移除凭据已验证标记的运行门槛 |
| main login | 直接探测，在线结束，不等 daemon 稳定期 | Trigger::Login，不伪称密码已验证 |
| main relogin | 未知不注销；需要认证直接登录；在线先注销 | Trigger::Relogin |
| main relogin 在线回退 | 注销后仍在线时使用当前地址匹配的缓存 | 保留回退，不允许 Unknown 时盲目用缓存 |
| run_daemon | WAN/路由稳定至少 10 秒，再进入自动探测 | 仅 Automatic 等待，手动不额外等 10 秒 |
| run_daemon detect_key | 自动分支识别密钥，手动 CLI 不调用 | 恢复入口差异 |
| _direct_curl | 先独立解析 DNS，再用 --resolve 请求；HTTP 超时不覆盖前置 DNS | PreparedDns，保留域名 Host/TLS 校验 |
| _network_run system | curl 不绑定源 IP，路由由系统决定 | 移除源 IP 与 DNS socket 源地址绑定；禁用普通代理 |
| _network_run openclash-owner | 经 GID 65534 绕过 OpenClash 防火墙和 DNS 接管 | Windows 没有等价机制；不声称通用 TUN 绕过 |
| _do_login | Cookie GET → login → ACK → 实际探测 | 顺序、字段、返回码与加密保留 |
| _do_login 结果 | ACK 失败但实际在线仍成功；ACK 成功但网络未确认为 VERIFYING | 移除首次配置额外的强制双成功条件 |
| 自动失败 | 失败退避 10/20/40…最多 300；未知按检测间隔复查 | 保留，未知不计密码失败 |
| 缓存/停止 | 参数可跨配置重载保留；IP 失配清除，不因停止主动注销 | 暂停/重载保留匹配缓存，Wi-Fi 不匹配仍按桌面要求清理 |

已删除 0.1.2 的首次 Verify 事务、示例地址注销入口和保存前强制联网验证。旧配置中的遗留字段仍可读取以避免升级丢失配置，但不作为认证条件，也不参与注销。

桌面专属能力保留：Wi-Fi 条件、Windows 服务、IPC 身份检查、DPAPI、托盘和安装卸载。它们不是 OpenWrt Shell/UCI 的逐行替换；系统代理绕过与 TUN 绕过分开验收。

验证：Rust 覆盖协议、网络请求、状态机与平台组件。直接执行原 tests/core.sh、probe.sh、daemon.sh、network.sh，分别通过 31、9、4、26 项断言。原 rpc.sh 缺少 OpenWrt jshn.sh、locking.sh 缺少 flock，当前 Windows 环境不能运行，未记为通过。

补充核对：tests/baseline-audit.sh 直接调用原源码的 AES、密钥识别、URL 解析和登录判断，62 项断言通过。当前 Rust 共 53 项测试通过：核心 34、桌面 4、服务 15。严格静态检查和正式构建通过。桌面服务使用串行队列及取消令牌替代 flock；保存后即时应用配置，区别于原 LuCI 保存后重启服务。这些平台行为单独保留，不作为 Shell 逐行等价的证据。

真实校园认证、TUN 下直连和管理员安装卸载仍未完成现场验收。当前无凭据诊断中，Rust 和系统 curl 的三个探针均返回 204，之前的超时尚未复现。
