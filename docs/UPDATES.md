# 更新与镜像下载

从 0.2.11 起，更新检查、清单与更新 ZIP 全程通过镜像访问 GitHub Releases，不调用 GitHub API，不把更新文件放进源码目录。

## Release 附件

每个版本只发布三个附件：

- `PortalLoginer-版本号-Setup.exe`：手动安装或覆盖升级。
- `PortalLoginer-版本号-windows-x86_64-update.zip`：客户端静默更新使用。
- `update-manifest.json`：版本、平台、更新包大小、SHA-256 及下载地址。

发布先创建草稿、上传全部附件，再公开为最新版本，避免出现有版本号却缺清单或包的短暂状态。Release 附件不进入 Git 版本历史。

## 镜像链路

1. 并发通过 GHFast、ghproxy.net、GH-Proxy 读取最新清单。例如 `https://gh-proxy.org/https://github.com/Small-SaltedFish/Portal_Loginer_Client/releases/latest/download/update-manifest.json`。
2. 校验固定项目、平台、版本、下载路径和大小，选择有效最高版本；同版本选择响应较快的镜像。请求附带缓存刷新参数。
3. 对真实 ZIP 最多 64 KiB 片段测速，按吞吐量排序，失败换站。拒绝错误网页和无效 Range。
4. 支持中断续传，完整文件必须匹配清单中的 SHA-256 和大小。镜像重定向不允许跳回 GitHub 域名。
5. 镜像全部不可用时，只能使用此前缓存的较新版本继续下载；没有可用缓存则报告镜像、HTTP 错误或超时，不把失败当作已是最新版。

更新请求绕过普通 HTTP/SOCKS 代理和代理环境变量，TUN 仍由系统路由控制。镜像可用性每次实测。不使用私钥或额外更新签名，SHA-256 用于文件完整性校验。

## 安装与恢复

- 正常联网后低频检查，约每天一次并随机错开；失败 15 分钟后重试。关闭自动更新后仍可手动检查、下载并安装。
- 等待认证事务完成、设置关闭、全屏或演示结束后安装；与安装、卸载共用互斥锁。
- 再次校验文件，只解压固定七个文件，拒绝路径穿越、符号链接、额外文件及超限解压。
- 保留上一版、逐文件原子替换，检查新服务版本；失败恢复旧版，独立守护与开机检查处理未完成事务。
- 保留账号密码和设置，恢复托盘，不自动打开设置窗口，不再次请求管理员授权。

## 发布

依次执行 `dev.ps1 test`、`dev.ps1 build`、`package-dev.ps1 -SkipBuild`、`package-update.ps1`。提交源码后运行 `publish-release.ps1 -Publish`。

旧版仍按旧入口检查；如果旧版访问失败，需要手动覆盖安装 0.2.11 一次，之后使用全链路镜像。程序暂未做系统代码签名。
