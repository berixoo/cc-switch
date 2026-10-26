# CC Switch · 自定义构建（提示词全文替换）

> 这是个人自定义构建（fork），**不是官方项目**。
> 在官方 CC Switch **4.0.20** 基础上增加请求级提示词全文替换，其余功能沿用官方。
> 官方仓库：[farion1231/cc-switch](https://github.com/farion1231/cc-switch) · 官网：[ccswitch.io](https://ccswitch.io)

> **当前状态**：替换功能已经基本实现、可以正常使用，但目前覆盖的客户端与应用范围还比较少。后续是否继续更新、扩展到哪些客户端，取决于实际使用效果——好用就继续做下去，遇到问题也欢迎直接反馈。

本构建**不定期**跟随官方上游更新，详见[同步上游](#不定期同步上游)。

## 功能概览

- **请求级提示词全文替换**：在 CC Switch 本地代理入口、供应商协议转换与重试之前改写一次，后续重试与故障转移复用同一份已改写请求。
- **两条独立路由**：Claude Code CLI、Claude Desktop，各自保存开关、替换全文与源路径，互不影响。
- **两类目标**：系统提示词全文替换；用户级指令文件（默认 `~/.claude/CLAUDE.md`）全文替换。只做整体替换，没有匹配串、规则顺序或正则。
- **旧会话同样生效**：恢复或压缩过的会话重放的是会话开始时的文件快照，会被识别并替换；请求里完全没有该文件的封装时，改为在请求内新增一条独立用户级指令（幂等，不会重复添加）。
- **只改代理请求**：不写回源文件，不修改官方配置或数据库。扩展配置单独放在 `<配置目录>/extensions/prompt-rewriter.json`，官方配置目录继续复用，供应商无需重配。
- **可核对**：面板显示每次请求里「系统替换 / 用户级指令替换」是否真的应用，以及未应用的原因。

## 下载

只从本仓库的 [Releases](../../releases) 获取，发布物**只有一个文件**：

```text
cc-switch.exe
```

- 自包含单文件：不需要 `WebView2Loader.dll` 或 `liblzma-5.dll`（已静态链接）。
- 需要系统的 WebView2 Runtime（Windows 11 自带，Windows 10 通常也已安装）。
- 没有安装器，也没有卸载程序：移除时直接删除该文件即可。
- 继续使用官方配置目录 `%USERPROFILE%\.cc-switch`，不要用官方卸载程序清理本构建，可能连带删掉共享配置。

## 使用

1. 退出正在运行的 CC Switch。
2. 运行本目录的 `cc-switch.exe`。
3. 打开 **设置 → 路由 → 提示词扩展**。
4. 选择路由：**Claude Code（CLI）** 或 **Claude Desktop**，两条路由分别配置。
5. **系统提示词**：点「获取」，然后在该路由的客户端里发一条消息；捕获到原文后编辑替换全文并保存。
6. **用户级指令**：默认读取 `~/.claude/CLAUDE.md`，可直接编辑替换全文；也可指定其他绝对路径。
7. 保存后只影响后续新请求；面板会显示每次请求是否真的应用。

客户端必须实际经过 CC Switch 路由，仅打开扩展开关不会接管直连流量。

## 构建

需要 Node.js、pnpm、Rust，以及 Windows 上的 MSVC 工具链与 C++ Build Tools。

```bash
pnpm install --frozen-lockfile
```

```bash
RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-msvc pnpm tauri build --features prompt-rewriter --no-bundle
```

用 **MSVC** 而不是仓库默认的 GNU 工具链，才能得到自包含单文件；GNU 构建会额外需要 `WebView2Loader.dll` 和 `liblzma-5.dll`。

Cargo feature 默认关闭，普通构建不读取扩展配置、不捕获请求、隐藏编辑器。

实现与验证细节见 [docs/prompt-rewriter.md](docs/prompt-rewriter.md)。

## 更新策略

关于页仍会正常检查和显示官方新版本，但**不会执行更新**：点击更新只会弹出说明，后端安装命令同样拒绝。用官方二进制覆盖会丢失本扩展。

升级方式只有一种 —— 同步上游源码后带 feature 重建，见下一节。

## 不定期同步上游

本构建不跟随官方每次发布：

- 上游有新版本时，会**不定期**把官方最新 `main` 合并进来，重新构建后更新本仓库的 Release；
- Release 说明里会标注对应的上游版本；
- 两次同步之间的官方更新不会自动出现，需要等待下一次同步或自行重建。

同步流程（`main` 受规则保护，必须走 PR）：

```bash
git remote add upstream https://github.com/farion1231/cc-switch.git
git fetch upstream
git checkout -b sync/upstream-$(date +%Y%m%d)
git merge upstream/main
git push -u origin sync/upstream-$(date +%Y%m%d)
gh pr create --base main --head sync/upstream-$(date +%Y%m%d)
```

若你的克隆里 `origin` 已经指向官方仓库（例如直接把官方仓库克隆下来再改的），把上面的 `upstream` 换成 `origin`、推送目标改成你自己的远程即可。

合并后删除分支：

```bash
git branch -d sync/upstream-XXXXXX && git push origin --delete sync/upstream-XXXXXX
```

若合并出现冲突，优先保留上游改动，再把扩展重新接回入口、转发与界面插槽。

## 一起完善

这个构建最初只是为了自用，功能还不算完整，实现也偏朴素，欢迎有兴趣的朋友一起开发、维护、把功能补全：

- **扩展覆盖范围**：接入更多 Claude 客户端与其他 Agent 工具，让替换不只局限于当前两条路由。
- **丰富替换方式**：按段落或标题局部替换、多文件组合、按项目区分等。
- **打磨兼容性**：不同上游协议、供应商与网关的边界情况，以及大上下文、工具调用、压缩会话等场景。
- **文档与测试**：补充使用说明、常见问题，添加可复现的用例与自动化测试。

参与方式很简单：从 `main` 开一个新分支，改完推送后提 PR（`main` 受规则保护，只接受 PR 合并）；暂时不想写代码也欢迎开 Issue 描述使用场景和遇到的问题，复现清楚的反馈同样很有价值。

## 已知限制

- 仅提供 Windows 发布包；其他平台的构建方式与上游一致。
- 只支持全文替换，没有匹配串、规则顺序或正则。
- 旧会话走请求级注入时，每个请求会多带一份替换全文（当前配置约 2 KB）。
- 验证以自动化测试与隔离 mock 上游为主，未覆盖所有真实供应商与客户端组合。

## 许可

与上游一致，MIT。原始项目与版权归 [farion1231/cc-switch](https://github.com/farion1231/cc-switch) 及其贡献者所有。
