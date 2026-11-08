> 当前 0.1.3 行为以 [原行为复核](BASELINE-RESTORATION.md) 为准。下文保留阶段记录，其中 0.1.1/0.1.2 的手动密钥识别和首次验证改动已撤回。

# 迁移兼容清单

本清单记录固定基线与阶段 1 的迁移要求。后续 Rust 实现及实际测试覆盖以 [核心说明](CORE.md) 和 [验证记录](VALIDATION.md) 为准；下表中“Rust 需/待实现”是基线核对时提出的验证任务。

## 1. 基线与证据

- 来源：https://github.com/Small-SaltedFish/Portal_Loginer
- 固定提交：`dcaffa81af551e3f259ddb1433a170aff8b840b6`。
- 提交时间：2026-11-04 21:56:45 +08:00；版本常量为 `1.4.1`。
- 本次核对日期：2026-11-08，Asia/Shanghai。
- 已阅读全部运行源码：主程序、网络 Ruby 助手、rpcd、LuCI、init、hotplug、默认配置、ACL、菜单；同时核对构建、升级脚本、全部测试与项目说明。完整文件清单见 `baseline.json`。
- `upstream` 已 detached HEAD 固定提交；不在旧仓库中开发桌面版，不修改路由器或在线校园会话。后续新远程仓库地址尚未指定。

以下行号均对应固定提交的 `src/usr/bin/portal-login`，简称“主程序”；rpcd 指 `src/usr/libexec/rpcd/portal-login`。源码是行为依据，注释和文档与实现不一致时单独说明。

## 2. 密钥调用的核实结论

`detect_key()` 定义在主程序 559 行，生产调用仅在 867 行：

`run_daemon → probe_portal → 返回 0（有效认证跳转）→ detect_key → do_login`。

| 入口 | 是否识别密钥 | 基线行为与迁移决定 |
|---|---|---|
| daemon / start | 是 | 每次有效跳转后、登录前识别，保留 |
| daemon 内每日重登录 | 取决于登出后探测 | 后续返回 0 时进入同一分支识别，保留 |
| CLI login | 否 | 探测返回 0 直接 do_login，原样兼容路径不新增调用 |
| CLI relogin | 否 | 所有 do_login 路径均无识别，原样兼容路径不新增调用 |
| LuCI“立即登录” | 否 | 实际提交 rpcd relogin，最终进入 CLI relogin |
| CLI probe / test-encrypt | 否 | 仅探测或加密 |

**0.1.0 保留原版调用差异；0.1.1 明确改进：手动 Login/Relogin 也先识别密钥。** 已增加手动识别新密钥及脚本不可用时保留旧密钥的测试。这属于桌面行为改进，不归为原样迁移。

识别按 `/assets/js/raas.js`、`/js/raas.js`、`/tp/default/js/index.js` 顺序 GET，连接 5 秒、总时长 8 秒。寻找首个 `Utf8.parse` 后跟单/双引号包围的 16 位 ASCII 字母数字，无法识别则保留配置。找到与现值相同的候选也立即停止；新值先替换内存，再尝试 UCI set/commit；写盘失败不阻止本次使用，也不阻止登录。

实现边界：实际正则不要求 `CryptoJS` 前缀、不要求右括号、不核对开闭引号相同，也不检查 Content-Type；HTML 中包含匹配文本也可能被接受。不能将文档描述的“只接受明确密钥”误写成已实现完整 JS 语义识别。补充离线测试固定这些事实；Rust 如收紧识别须记录差异。

## 3. 协议兼容矩阵

| ID | 必须核对的基线行为 | 源码依据 | 验证样例 |
|---|---|---|---|
| P01 | IPv4 地址存在且出口路由可用才发送探针；无地址返回 2，无路由返回 4 | 410、334、485 | upstream core、probe、daemon |
| P02 | 主探针→华为→vivo；可靠结论即停止，失败继续备用 | 505–527 | upstream probe；新增请求计数用例留给 Rust |
| P03 | 只有传输成功的 HTTP 204 表示在线；200、5xx、超时、普通跳转返回未知 | 515–527 | upstream core、probe |
| P04 | 301/302/303/307/308 且 Location 带协议标记，解析校验成功才表示需认证 | 516–522 | upstream core；fixtures 覆盖其余跳转码 |
| P05 | host 仅 HTTP/S 的 RFC1918 IPv4，可带 1–65535 端口，无用户信息；bras 必填，clientip 必须等于当前出口 IPv4 | 312–331 | upstream core；fixtures |
| P06 | 解析 bras/clientip/wlanuserip/wlanacname/clientmac/paip/vlan；bras 缺失用 wlanacname，clientip 缺失用 wlanuserip | 530–555 | fixtures |
| P07 | URL 参数不百分号解码、不将 + 转空格；取首个非空同名参数，额外等号后内容被 cut 截去 | 534 | Rust 必须显式实现兼容解析，不直接采用表单解码默认值 |
| P08 | bras 与其余六字段合并后仅允许 ASCII 字母数字及 `._:%-`；只严格校验 clientip，不严格校验 mac/vlan/paip 结构 | 326–331 | fixtures；不可把未实现的格式校验写为基线 |
| P09 | 登录前验证参数；先加密，再 GET index 建立 cookie，再 POST login，再 POST ack，最后探测 | 705–771 | upstream core；Rust 需断言请求顺序/字段/cookie |
| P10 | 建立 cookie 的 GET 失败原版仍继续 login；login 传输失败立即失败，不请求 ack | 719–731 | Rust 需单列兼容测试 |
| P11 | login 兼容 ret=0/121/122/3，空 ret、其他 ret、畸形 JSON、HTTP 错误失败 | 733–749 | upstream core 只测 0；其余值列入 fixtures |
| P12 | ack 仅 ret=0 为接口确认；实际探针在线时，即使 ack 失败仍成功 | 752–772 | upstream core |
| P13 | ack=0 且未证实联网：返回 2，保持 VERIFYING，按重试间隔复查，不增加失败计数 | 762–770、871–874 | upstream core；fixtures |
| P14 | ack 失败且未证实联网：失败；login 失败后不会仅凭探针反判成功 | 731、749、769 | upstream core |
| P15 | 实际认证成功才记录登录时间、清零失败；普通在线探测不伪造登录时间 | 774–782、888–911 | upstream core、daemon |
| P16 | API URL 顺序 bras/clientip/wlanuserip/wlanacname/clientmac/paip/vlan，跳过空值，保留原始值 | 684–692 | Rust 请求快照待实现 |
| P17 | 表单 user/pass 编码，固定 authmode=0、pool=0、isp_id=0、pyacct=0；两次 POST 复用同一份密文 | 723–759 | fixtures；Rust 请求快照待实现 |
| P18 | API 共享 cookie、User-Agent 与 Referer；连接 8 秒/总时长 15 秒，不自动跳转 | 695–703 | Rust 需本地 HTTP 样例服务器验证 |
| P19 | logoff POST 无 body，URL 带 bras/clientip、可选 clientmac/vlan，MAC 冒号改连字符；ret=0 才清 cookie | 644–680 | Rust 请求快照待实现 |
| P20 | 未知网络不登出；重启/停止服务不登出；手动/定时重认证才允许主动登出 | 846–858、999 起 CLI、init | upstream daemon；Rust 状态测试待实现 |

默认探针依次为：

1. `http://connect.rom.miui.com/generate_204`
2. `http://connectivitycheck.platform.hicloud.com/generate_204`
3. `http://wifi.vivo.com.cn/generate_204`

门户 GET 查询为 `bras,clientip,wlanuserip,clientmac`（空值也保留）；Referer 只包含 `/index.html?bras=...`。API 请求明确要求表单 Content-Type；login 另加 Accept。原协议允许 HTTP 内网门户，Rust 保留此兼容性；HTTPS 继续校验证书。

## 4. 密码处理

主程序 584–639：

1. 输入恰为 32 位 ASCII 字母数字时原样返回，包括非十六进制的字母；不加前缀、不 AES。
2. 其他输入前加 4 个随机字符，字符池为 `A–Z a–z 0–9 + /`；原版随机源不足回退 `xK9z`。
3. 前缀与密码拼接，补零到 16 字节倍数；已对齐时不加整块填充，不使用 PKCS#7。
4. 密钥是 16 位 ASCII 字母数字的原始字节，不是将十六进制密钥字符串解码。
5. AES-128-ECB，无 salt/IV，输出小写十六进制，不输出 Base64。

补充测试使用固定随机前缀 `xK9z` 与 Node crypto 独立生成的预期密文，对照**原版 encrypt_pass**，覆盖短密码、12/13 字符的块边界和 32 位 token。

未确定的边界：原版 shell `${#...}` 受实现和 locale 影响，中文密码的长度计算不能仅凭源码推断等同于 UTF-8 字节。Rust 拟按 UTF-8 字节填充；在 OpenWrt ash 下补证据前，非 ASCII 密码标为待验证，不能声称已兼容。随机源失败时，Rust 拟报错并退避，替代固定回退前缀，单列为安全性改进。

## 5. 调度、状态与会话

| 条件 | 基线结果 | 等待策略 |
|---|---|---|
| 启动 | IDLE；恢复缓存和同次开机登录记录 | 先检查网络 |
| 新地址/出口不稳定 | WAIT_NETWORK；地址变化清 cookie；clientip 不一致时清旧 host/bras/vlan/paip/mac | 稳定期 10 秒，每次等 3 秒 |
| 无 IPv4 | OFFLINE，不发送认证 | retry_interval |
| 已有地址无路由 | WAIT_NETWORK | retry_interval |
| 有效认证跳转 | LOGGING_IN→VERIFYING→成功或失败 | 一次事务 |
| login/ack+联网综合失败 | FAILED；失败计数 +1 | 初始 retry_interval，连续倍增，封顶 300 秒 |
| 接口已确认但网络未确认 | VERIFYING，计数不增加 | retry_interval |
| 探针未知 | PROBE_UNKNOWN，保留 cookie 与会话，不标在线 | check_interval，无指数退避 |
| 已在线 | LOGGED_IN，清 ERROR 和失败计数 | check_interval |
| 进程结束 | STOPPED | 平台服务管理器恢复 |

指数退避是 `min(retry_interval × 2^(失败次数−1), 300)`，Rust 计算须避免溢出。稳定等待与诊断限频用单调时间；每日重认证用本地日历时间。原版每日记录 `%j`，无年份，桌面版拟改完整日期，记录为差异。

定时重认证仅在当前小时匹配、当天尚未执行且探针在线时尝试；先记录当天，再登出，成功后等 2 秒再探测。失败不会同日同小时反复主动注销。普通成功认证也更新当天标记。若登出后依然在线，daemon 不强制用缓存登录；**手动 relogin** 则可在参数仍有效时用缓存重新认证，这两个入口应分别测试。

原版 daemon 和手动操作共享 flock 事务锁，daemon 另有单实例锁，休眠释放事务锁。Rust 用唯一服务串行队列与取消令牌替换，不让托盘或设置自行发送认证请求。

缓存保存 host/bras/clientip/clientmac/paip/vlan，不保存完整 URL、wlanacname、原始 wlanuserip；恢复时 wlanuserip 被设为 clientip。同组参数未变则不提交 UCI。缓存不构成认证失效证据。原版无效 URL 解析会先修改内存字段，校验失败不写缓存；Rust 拟先解析成临时对象，全部验证后提交，以避免半更新，记录为健壮性改进。

## 6. 配置、状态与日志

| 原选项 | 默认 | 范围/迁移方式 |
|---|---|---|
| user | 空 | rpcd 非空且≤128；单账号 |
| pass | 空 | rpcd≤512；空保存保留；迁移到平台受保护凭据，不提供读取接口 |
| key | 5a3b9f207411a8ed | 16 ASCII 字母数字；空保存保留；高级配置 |
| probe_url | 小米探针 | HTTP URL；rpcd 禁用户信息、空白、fragment；高级配置 |
| check_interval | 30 秒 | 5–600 |
| retry_interval | 10 秒 | 3–300 |
| relogin_hour | 4 | −1 禁用或 0–23 |
| log_max | 500 行 | 50–5000；桌面另加总字节上限 |
| debug | 0 | 0/1；日志仍须脱敏 |
| enabled | 1 | 原版关联服务启用；桌面拆分自动认证暂停、服务自启动、托盘自启动 |
| last_* | 无 | 动态会话缓存，使用前校验当前出口 |

rpcd 接受 string/int、拒控制字符，全部字段验证后暂存提交；主程序 validate_cfg 与 rpcd 校验并不完全一致，Rust 统一为服务端校验，拒绝畸形数字和越界值。是否“配置保存成功”应和“系统启动项修改成功”分开反馈。

原状态含 STATE、LAST_LOGIN、LAST_CHECK、LOGIN_TS、LOGIN_UPTIME、FAIL_COUNT、P_*、ERROR、VERSION 等；rpcd 只读查询不返回密码。账号和地址仍可能出现在原日志，密钥变化也记录前四位。桌面版按计划禁止密码、密文、密钥片段进入日志，脱敏账号、MAC 和敏感地址/门户参数，服务端消息先限长和清理再展示。

原日志每次写入后按行截断，稳定在线每 10 轮仍写汇总。桌面版改容量上限与事件驱动日志，稳定等待不持续写盘。以上均为明确的平台/隐私改进，不属于已有行为。

## 7. 平台替换与新增能力

| 原实现 | Windows / macOS 替代 | 状态 |
|---|---|---|
| netifd/ubus WAN IPv4 与路由 | 系统网络事件、选定物理接口及出口地址，网络代次标记隔离旧任务 | 待实现 |
| procd/init/hotplug | 自动启动 Windows 服务及恢复策略 / 系统 LaunchDaemon | 待实现 |
| UCI、明文 pass、文件权限600 | 原子配置文件 + DPAPI/受保护凭据；服务与客户端身份授权 | 待实现 |
| 状态文件/rpcd/ACL | 受限命名管道 / Unix socket；校验对端身份，限制消息大小与命令集合 | 待实现 |
| LuCI 每2秒轮询与网页 toast | 登录后托盘/菜单栏；独立 egui 设置进程；原生系统通知 | 待实现 |
| curl -q -4 --noproxy、独立上游DNS | reqwest 禁代理/禁跳转、独立 A 解析、保留 Host/SNI/证书校验 | 待实现 |
| nft/iptables GID65534 + Ruby | 不迁移 OpenClash 规则检查/改组；桌面 TUN 不保证绕过，未知即等待 | 明确替换 |
| opkg/conffiles/升级脚本 | 固定新仓库 Releases、Ed25519 清单、SHA-256、受限更新及回滚 | 待实现 |
| 无 SSID 筛选 | 默认关闭；精确匹配，不匹配/未知/有线时不探测；离开取消，不登出 | 新增 |
| 无用户会话通知机制 | 成功事件去重；登录后复核；遵循勿扰；不抢焦点 | 新增 |

原版所有认证相关请求均共用直连封装，包括 JS、门户、login、ack、logoff。DNS 依次使用 WAN 上游与 223.5.5.5/119.29.29.29，去重并跳过本机、Fake-IP 等不可用地址；每个上游有限时。桌面 DNS 总耗时也应受全事务预算限制。原版响应无明确体积限制，桌面增加按响应类型的大小限制，作为新增防护。

两条联网路径分开维护：认证始终直连；更新允许系统正常代理。服务拥有系统权限，更新必须验证固定项目身份、公钥、平台/架构、版本、包摘要、路径与操作集合，不能成为任意执行入口。实际新仓库地址与公钥在发布前确定，不用旧 OpenWrt Releases 作为桌面更新源。

登录前仅在系统已连接物理网络时认证；Wi-Fi 由系统管理。macOS SSID 权限、系统凭据在登录前的可访问性，以及 FileVault 解锁后的边界须实机验证，当前不承诺已解决。

## 8. 下一阶段的验收入口

Rust 核心模块划分为 `protocol`（兼容解析/表单/加密/密钥）、`transport`（直连/DNS/限时限量）、`engine`（状态与调度）、平台接口（网络/凭据/IPC/更新）。桌面平台代码不进入协议模块。

第二阶段先用 `fixtures/protocol.json`、原 tests 及补充 audit 生成对照测试，再实现本地 HTTP 样例服务器核对 cookie、请求顺序、编码、大小限制和取消。每项标注“基线一致”“明确改进”“未验证”，不能仅凭编译通过宣布迁移完成。中文密码、重复参数、畸形 JSON、手动入口密钥差异需独立覆盖。

## 首次配置的明确改进（0.1.2）

桌面新增 Verify / VerifyAndLogout，独立于原 daemon 的自动认证。普通在线结论不能验证账号；首次在线时等待用户明确点击注销。没有有效缓存时允许使用可编辑的源码示例地址与 bras 作为注销预设，IP 动态读取，预设不得用于登录。注销后必须探测到新有效跳转，登录、ACK 与实际联网全部通过后才持久化候选凭据。原自动认证兼容规则继续保留，但不能授予首次验证标记。这是桌面配置流程的新增行为，不声称与原 CLI 原样一致。
