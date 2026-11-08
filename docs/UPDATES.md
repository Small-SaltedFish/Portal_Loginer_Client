# 自动更新接入

发布源固定为 [Small-SaltedFish/Portal_Loginer_Client](https://github.com/Small-SaltedFish/Portal_Loginer_Client)。原 Portal_Loginer 是协议参照，不作为客户端更新源。

## 当前已实现

`portal-update` 是不依赖平台的清单及文件验证库：

- 验证原始清单字节的 Ed25519 分离签名后才解析 JSON；清单最多 64 KiB。
- 限定仓库、HTTPS 下载地址、版本、平台、架构及固定 ZIP 文件名。
- 拒绝降级、非稳定版本、重复平台包、未知字段和超限包。
- 对流式文件校验精确长度和 SHA-256，拒绝截断、额外字节及内容篡改。
- `VerifiedUpdate` 只能由通过上述验证的清单构造。

运行 `cargo test -p portal-update --locked` 可验证。测试中的固定私钥仅用于合成样例，不是发布密钥。

## 发布协议

每个 Release 使用 `v<版本>` 标签，附 `update-manifest.json`、原始 64 字节 `update-manifest.sig` 和各平台专用更新 ZIP。签名覆盖清单原始 UTF-8 字节，发布后不能重新排版或换行。

清单结构：

```json
{
  "schema": 1,
  "repository": "Small-SaltedFish/Portal_Loginer_Client",
  "version": "0.3.0",
  "assets": [{
    "platform": "windows",
    "architecture": "x86_64",
    "name": "PortalLoginer-0.3.0-windows-x86_64-update.zip",
    "url": "https://github.com/Small-SaltedFish/Portal_Loginer_Client/releases/download/v0.3.0/PortalLoginer-0.3.0-windows-x86_64-update.zip",
    "size": 123456,
    "sha256": "填写对应更新包的64位十六进制摘要"
  }]
}
```

这是结构示例，不可作为真实发布清单。开发 ZIP、Setup.exe 和 Cleanup.exe 不作为此协议的更新载荷。

## 后续接入顺序

1. 生成项目发布签名密钥，独立备份私钥，将公钥编入客户端。私钥仅进入 GitHub Actions Secret，不入仓库或客户端。
2. 增加固定文件集合的更新载荷打包和签名流程，再发布首个可更新版本。
3. 服务每天随机分散检查一次；更新请求走用户正常网络和代理，认证请求继续直连。无网时退避。
4. 有认证事务、游戏或全屏时推迟安装。下载、验签、文件校验完成后启动独立更新进程。
5. 更新进程停止托盘、设置和服务，原子切换固定程序文件；保留上一个完整版本，启动失败自动恢复。普通升级由已授权的服务完成，不启动新的交互授权流程。
6. 接入关于页、更新进度和手动检查，完成下载中断、损坏包、异常退出和回滚测试。

当前没有生产发布密钥，也没有联网下载、静默安装或回滚入口，界面仍明确显示自动更新开发中。不能仅打开复选框就宣称已支持自动更新。

## 构建流程

- `Checks`：Linux 核心/验签测试，Windows 全部检查和测试。
- `Build Windows packages`：手动构建未签名安装包、独立清理器和开发 ZIP，作为 Actions 附件供检查，不自动发布 Release。
- NSIS 3.11 下载文件带固定 SHA-256；GitHub Actions 使用已核对的完整提交编号。
