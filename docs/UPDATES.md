# 更新与镜像下载

0.2.11 起，版本检查通过 GitHub API 镜像，更新 ZIP 通过文件镜像下载。客户端不直接请求 GitHub 域名，不维护额外的更新清单。

## Release 附件

每个版本只发布两个附件：

- `PortalLoginer-版本号-Setup.exe`：手动安装或覆盖升级。
- `PortalLoginer-版本号-windows-x86_64-update.zip`：程序静默更新使用。

版本号、文件大小及 SHA-256 从镜像转发的 GitHub Release API 读取。发布先上传草稿全部附件，再公开为最新版本。Release 附件不进入 Git 版本历史。

## 完整镜像链路

1. 并发经镜像请求 `https://api.github.com/repos/Small-SaltedFish/Portal_Loginer_Client/releases/latest`。例如 `https://gh-proxy.org/https://api.github.com/repos/Small-SaltedFish/Portal_Loginer_Client/releases/latest`。不同镜像对 API 支持不同，仅接受有效响应。
2. 检查正式版本、平台、固定项目路径、文件名、大小及 SHA-256，选择有效最高版本，同版本择快；请求附带缓存刷新参数。
3. 经文件镜像读取真实 ZIP 最多 64 KiB 片段测速，择优下载，失败换站。支持续传，拒绝错误网页、错误 Range、损坏文件和异常降级。
4. 所有镜像请求禁止跳回 GitHub 域名；绕过普通 HTTP/SOCKS 代理和代理环境变量。TUN 仍由系统路由控制。
5. 镜像全部不可用时，只能使用此前缓存的较新版本继续下载；无可用缓存时显示镜像、HTTP 错误或超时，不把检查失败当作已是最新版。

不使用私钥或额外更新签名，SHA-256 用于文件完整性校验。API 代理能力参考 [GH-Proxy 文档](https://gh-proxy.com/docs/github-accelerator)。

## 安装与恢复

- 正常联网后约每天检查一次并随机错开；失败 15 分钟后重试，可手动检查。
- 等待认证完成、设置关闭、全屏或演示结束后安装，与安装和卸载共用互斥锁。
- 再次校验文件，只解压固定七个文件，拒绝路径穿越、符号链接、额外文件及超限解压。
- 保留上一版、原子替换文件、确认新服务版本，失败回滚。独立守护及开机检查处理未完成事务。
- 保留账号密码和设置，恢复托盘，不自动打开设置窗口，不再次请求管理员授权。

## 发布

执行 `dev.ps1 test`、`dev.ps1 build`、`package-dev.ps1 -SkipBuild`、`package-update.ps1`。提交源码后运行 `publish-release.ps1 -Publish`。

打包生成的本地清单仅供构建校验，不上传。旧版本若因直连检查失败，可手动覆盖安装 0.2.11 切换镜像入口。程序暂未做系统代码签名。
