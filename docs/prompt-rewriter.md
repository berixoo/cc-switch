# Claude 请求级提示词全文替换扩展

本扩展仅支持 **Claude Code CLI 和 Claude Desktop**。在 CC Switch 本地代理入口、供应商协议转换及重试循环之前，分别替换系统提示词和已识别的用户级 `CLAUDE.md` 文件片段，每个请求只执行一次。

用户级指令不是聊天输入。聊天消息、项目指令、工具结果和附件不是替换目标；源文件只读，替换全文仅保存到独立的扩展 JSON。扩展随源码构建，不是官方动态插件 ABI。

**Codex 提示词扩展支持已移除**：不捕获、不读取其指令文件、不改写其请求。CC Switch 官方 Codex 功能不受影响，Claude 使用 Responses、Chat、Gemini 等上游供应商的官方协议转换也保留。

## 构建与启用

沿用 [贡献指南](../CONTRIBUTING.md) 的平台依赖。在仓库根目录执行：

```bash
pnpm install --frozen-lockfile
```

```bash
pnpm tauri dev --features prompt-rewriter
```

```bash
pnpm tauri build --features prompt-rewriter --no-bundle
```

Windows 正式构建使用 MSVC 工具链和 C++ Build Tools。已有相应工具链时可在 Git Bash 中指定：

```bash
RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-msvc pnpm tauri build --features prompt-rewriter --no-bundle
```

用 MSVC 而不是仓库默认的 GNU 工具链，是为了产出**自包含单文件**：GNU 构建会把 `WebView2Loader.dll`（Tauri 的 WebView2 加载器）和 `liblzma-5.dll`（`zip → xz2 → lzma-sys`）留成外部依赖，MSVC 构建把两者静态链接进可执行文件。产物只需 `cc-switch.exe`，没有安装器或卸载程序；移除时直接删除该文件。

Cargo feature 默认关闭；普通构建不读取扩展配置或源文件、不捕获请求并隐藏编辑器。带 feature 的构建显示自定义构建身份，但运行时总开关、路由及两个目标仍默认关闭。

在“设置 → 路由”的提示词扩展面板选择 Claude Code（CLI）或 Claude Desktop。编辑后显式保存，草稿不影响代理，保存只影响后续新请求。关闭目标恢复透传，不写回源文件。

### 客户端必须实际经过代理

扩展开关不会接管直连流量。客户端必须通过 CC Switch 对应的官方路由连接模式发送请求，才能应用替换。更改连接模式后彻底重启客户端并用新会话验证。以客户端选中的地址和对应应用的代理入站记录一起确认，不以供应商名称推断客户端入口。

## 当前原文与替换全文

### 用户级指令文件

两条 Claude 路由默认只读 `~/.claude/CLAUDE.md`，支持官方已有的 Claude 自定义目录；也可分别在扩展里指定源文件绝对路径。自定义路径只写入扩展 JSON，不修改客户端配置、不创建源文件、不扫描主目录。

面板展示源路径、磁盘原文、读取状态和时间。空文件、缺失、读取错误分别表示；文件可读取不证明客户端已发送它。启动、保存及显式刷新时生成源快照，请求热路径不读盘。外部编辑源文件后需刷新；刷新不覆盖编辑草稿，“从当前原文载入编辑器”才会更新编辑起点。

### 系统提示词

点击获取后，等待所选 Claude 路由下一次请求，在入口改写前观察客户端的系统文本。此时未进入供应商尝试循环、未做模型映射、协议转换或 body overrides。Stack 模型可能已解码，但观察元数据保留解码前的客户端模型。

捕获独立于替换开关；未获取、等待中、已捕获空文本和错误分别显示。保存的替换文本不会冒充观察原文。观察包含原始模型、入站协议、请求标识和时间，`providerId` 为 null，只驻留内存，不写日志、配置或数据库。它不是最终出站文本，也不包含上游服务端隐藏指令。

## 请求范围

| 请求                                         | 扩展行为                |
| -------------------------------------------- | ----------------------- |
| Claude Code `/v1/messages`                   | Claude Code 独立配置    |
| Claude Desktop `/claude-desktop/v1/messages` | Claude Desktop 独立配置 |
| Claude 压缩请求                              | 跳过                    |
| Codex、其他应用、direct、模型列表、健康检查  | 不接入本扩展            |

两条路由独立保存设置、源路径和观察，默认可读取同一全局文件，但互不继承替换配置。

## 文件定位与全文替换

非空文件替换要求 **实际绝对路径、明确封装边界** 同时成立，并对正文做来源判定：与当前源文件逐字相同（或仅省略恰好一个终止 LF）时按当前内容识别；路径相同但正文是旧快照时按历史快照识别，同样替换。不会仅凭 `role=user`、路径字样、单个标签或文本相等就替换；不会把普通聊天当成文件。请求里完全没有该文件的封装时走请求级新增（见下节）。

当前支持严格 LF 换行的 Claude 封装：

```text
<system-reminder>
Contents of <源文件绝对路径> (user's private global instructions for all projects):

<完整源文件原文>
</system-reminder>
```

以及在真实 Claude Desktop 入站中观察到的固定前言形式：

```text
<system-reminder>
Codebase and user instructions are shown below. Be sure to adhere to these instructions. IMPORTANT: These instructions OVERRIDE any default behavior and you MUST follow them exactly as written.

Contents of <源文件绝对路径> (user's private global instructions for all projects):

<完整源文件原文>
</system-reminder>
```

固定前言形式可位于前导 user 消息的首个独立文本块，后续块可有仓库上下文或聊天，但整条消息必须全为文本块。只改首块的源正文，后续块不改。其他可编辑前导用户消息必须完全由指令封装组成。工具结果、图片、后续聊天历史、不完整或嵌套边界不会作为文件替换目标。

Claude 文件正文允许逐字相等或客户端仅省略恰好一个终止 LF，不做通用 trim、CRLF 转换、注释删除或内部空白归一化。项目路径中的指令不是全局源目标。

### 旧会话（历史快照）

恢复旧会话时，客户端重放的是会话开始那一刻的 `CLAUDE.md` 快照，不是磁盘上的当前内容。因此正文逐字相等不能作为唯一识别条件：

- 路径归一化后等于所选源文件、且封装完整闭合的历史快照，按同一文件识别并替换为当前替换全文。
- 同一载体里出现多份同路径封装时全部替换，不再整体放弃；系统目标也不再被文件目标的份数连带阻止。
- 载体范围：前导指令消息，以及任何只由文本块组成、且每个文本块都是 `<system-reminder>` 封装的消息——恢复会话时指令块可能出现在历史中间。带散文、工具结果、图片或混合块的普通聊天仍然不扫描。
- 封装内允许有限前言（≤1 KiB、不含标签），用于客户端在 `Contents of` 之前加的上下文标题。
- 截断、嵌套、夹带散文或路径不匹配的封装仍然不猜。面板原因会区分「请求里没有该源文件的封装」和「找到的封装属于其他路径」。
- 旧快照替换只发生在出站请求里，不改写客户端保存的历史，也不改源文件。

系统和文件共用载体时先分离，再原子应用编辑计划。无法可靠分离时保留原文并报告原因。独立 Anthropic `system` 字段不依赖用户文件是否命中；正文中用反引号提及 `<system-reminder>` 不作为真实边界，实际无法分离的文件区域仍受保护。

### 找不到封装时的请求级新增

当客户端根本没有发送该源文件的指令封装时（恢复或压缩过的会话、子代理等），改用请求级新增，仍然只修改代理请求：

- 源状态必须是 `Ready`，路径合法且绝对，内容已成功读取（空文件也算），替换值非空。
- 请求里没有任何该路径的封装，也没有"看起来像但无法安全分离"的残留时，前置一条独立 user 消息，文本为 `<cc-switch-user-instructions>\n<replacement>\n</cc-switch-user-instructions>`；不提升为 system/developer，也不伪装成客户端发送的源文件。
- 找到可识别的封装时一律原位替换，不再新增；两份以上同样全部替换。
- 找不到可用封装、但存在同文件的残缺或歧义载体时宁可不加，避免旧文本和新文本同时出现在同一请求里。
- 注入是幂等的：请求里已有同一段 `<cc-switch-user-instructions>` 时不再重复添加。协议是 Chat/Responses 时，注入消息排在开头的 system/developer 轮之后，不会破坏"system 必须在前"的约束。
- 缺失、读取失败、路径不合法或替换为空时不新增。空替换值不会制造空消息。

面板状态区分 `unknown`、`unidentified`、`identified`、`injected`；`ambiguous` 仅为兼容保留，同路径多份封装不再产生该状态。`identified` 表示找到该文件的指令段（当前内容或历史快照），是否真正替换以面板显示的「请求内系统替换」「请求内用户级指令替换」应用状态为准；`injected` 表示新增独立用户级指令，并不表示识别到了原文件。捕获是单次观察，不是"最近一次请求"的持续视图。

## 配置与官方功能隔离

```text
<CC Switch 当前配置目录>/
├── config.json
├── settings.json
├── cc-switch.db
└── extensions/
    └── prompt-rewriter.json
```

默认 Windows 扩展路径为 `%USERPROFILE%/.cc-switch/extensions/prompt-rewriter.json`，跟随官方配置目录覆盖。扩展不向官方配置增加字段、不新增数据库表、不写源文件或其他扩展；这不禁止 CC Switch 原有设置保存、路由配置或数据库升级行为。

加载缺失配置不创建文件，只有显式保存才原子写入扩展 JSON。备份需单独保留该文件；官方导出或同步没有扩展为携带它。

### v2 双路配置

```json
{
  "version": 2,
  "enabled": true,
  "routes": {
    "claudeCode": {
      "enabled": false,
      "system": { "enabled": false, "replacement": null },
      "userInstructions": {
        "enabled": false,
        "replacement": null,
        "sourcePath": null
      }
    },
    "claudeDesktop": {
      "enabled": true,
      "system": { "enabled": true, "replacement": "系统提示词替换全文。" },
      "userInstructions": {
        "enabled": true,
        "replacement": "用户级指令替换全文。",
        "sourcePath": null
      }
    }
  }
}
```

- 版本仍为 2。旧 v2 的 `routes.codex` 只作为已删除字段接受并忽略，不进入运行配置、状态或保存输出，不读取该项的源文件；两条 Claude 设置保留。
- 加载或重载不自动改写旧文件；下次显式保存才输出双路格式。旧配置缺少 `claudeCode` 时仍按全关闭默认值加载，不复制 Desktop 设置。
- 其他未知字段仍拒绝。v1 旧聊天消息规则语义不兼容，报错并保留文件，不自动迁移。
- `replacement: null` 表示未设置、继承原文；`""` 表示显式清空已有目标。`sourcePath: null` 使用默认文件，非空时必须为绝对路径。
- 两个目标只支持全文替换，没有匹配串、规则顺序或正则。

手动修改 JSON 后显式重新加载或重启，没有文件监听。同目录 reload 失败保留最后有效快照；切换目录失败禁用旧配置并清空观察，不将旧目录文本带入新目录。请求等待更新锁后重新解析目录，不用旧路径清掉刚发布的新配置；捕获也在更新锁内解析路径。真正切到未加载目录仍禁用旧快照，请求热路径不因此读盘。

## 时序、协议转换和预览

```text
入口解析、鉴权、Stack / 工具 / 压缩上下文识别
→ 固定扩展配置和源快照
→ 原文观察、定位、分离并原子执行一次改写
→ 使用同一已改写基线执行供应商尝试和重试
→ 官方模型映射、协议转换和允许的供应商处理
→ 序列化并发送
```

保存配置只影响后续请求。重试不重新观察或改写、不累积替换。会话指纹与 Copilot 分类使用原始入站内容。

后续供应商 body overrides 在深度合并后比较受保护载体：系统目标保护 `system`、`instructions`、`systemInstruction` 和 system/developer 消息；文件目标保守保护所有消息及系统载体。字段清单包含多种协议，是因为 Claude 可转发到不同上游。

冲突在发送前拒绝，并作为供应商配置错误交给既有故障转移机制；有下一家且额度允许时继续，否则返回错误，绝不发送冲突请求。相同值、仅采样参数允许；仅系统目标启用时允许普通用户消息覆盖。

官方协议转换器的既有规范化仍保留，包括 billing header 清理、特定工具约束。因此不承诺最终所有协议文本逐字节一致，也不修改供应商服务端隐藏指令。扩展内部的通用协议载体处理保留，但实时入口和面板预览都只有两条 Claude 路由、固定 Anthropic 格式。

本地 JSON 预览复用入口引擎和草稿，不保存、不请求上游、不执行供应商转换或覆盖，不是实时抓包。总开关和路由仅在预览副本临时开启，目标开关仍表示实际意图。

## 同步官方更新

自定义构建允许查看官方版本和发布说明，但禁止用官方二进制覆盖自身。更新方式为保留本地扩展改动、同步官方源码、带 feature 重建；当前 origin 仍为官方 `farion1231/cc-switch`。独立模块降低冲突范围，不保证更新永不冲突。

关于页仍照常检查并显示官方新版本；点击「更新源码并重新构建」会弹出说明对话框（复用官方 `ConfirmDialog` 样式），可直接跳转对应版本的发布说明，但不会执行安装。后端 `install_update_and_restart` 同样以 `ensure_custom_update_allowed` 拒绝，双保险。

主要实现：

- `src-tauri/src/extensions/prompt_rewriter/`：配置、源快照、捕获、引擎、适配器与 IPC。
- `src/extensions/prompt-rewriter/`：编辑器、类型与 API。
- `proxy/handlers.rs`：仅 Claude 两路入口接入；`proxy/forwarder.rs`：单次改写及转换后覆盖保护。
- Cargo feature、lib 注册、settings/misc 更新保护、AboutSection 身份、RoutingSection 插槽、五语言文案。

## 验证

```bash
pnpm typecheck
```

```bash
pnpm exec vitest run tests/components/PromptRewriterPanel.test.tsx --maxWorkers 2 --minWorkers 1
```

Rust 测试设置隔离的 `CC_SWITCH_TEST_HOME`，不使用个人配置：

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib prompt_rewriter --features prompt-rewriter
```

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib prompt_rewriter
```

```bash
cargo check --manifest-path src-tauri/Cargo.toml --all-targets --features prompt-rewriter
```

```bash
cargo check --manifest-path src-tauri/Cargo.toml --all-targets
```

### 2026-10-26 Claude-only 收敛阶段

用户明确取消 Codex 扩展支持。移除生产接入、专属路由、指令文件读取及封装解析、UI 和专属测试；保留官方 Codex 与 Claude 跨协议转换，将通用覆盖和 failover 回归改为 Claude 入站。旧 v2 的 Codex 配置兼容忽略，不修改真实配置。

此前稳定性测试已复现并修复前导后续文本块漏检、目录切换旧路径失效及供应商覆盖冲突中止 failover。SSE/工具续接和双路 failover 的 Desktop 测试夹具已补齐模型路由。

当前双路代码已完成：

- `pnpm typecheck`、面板组件测试 **46/46**、扩展前端及文档格式检查通过。
- Rust 扩展定向测试：feature-on **65/65**、feature-off **64/64**；包括旧 v2 配置兼容、Claude 流式工具续接、跨协议转换、覆盖冲突 failover 和配置目录竞争回归。
- 两套 `cargo check --all-targets`、`cargo fmt --check`、`git diff --check` 通过。Rust 有现有测试辅助函数 `named` 未使用提示。
- 生产 renderer 构建通过；有浏览器兼容数据过期、现有 Tailwind 类歧义及 bundle 大小提示，不影响构建成功。
- renderer-only mock IPC 界面已实际操作：下拉恰好两条 Claude 路由、切换独立路由、固定 Anthropic 预览、空源新增状态和预览返回正常，已查看截图。没有将其作为原生 Tauri 或真实上游端到端证据。

- 官方代理回归（feature-off，隔离测试目录与本地 mock 上游）：`proxy::forwarder::tests` **138/138**、`proxy::handlers::tests` **51/51** 通过。
- `pnpm tauri build --features prompt-rewriter --no-bundle` 通过。本机沿用已有 Windows GNU 工具链和本地 manifest shim，没有安装新工具链。
- 新便携包：`src-tauri/target/release/prompt-rewriter-claude-only-20261026.zip`，**16,622,265 bytes**；仅包含 `cc-switch.exe`、`WebView2Loader.dll`、`liblzma-5.dll`、`README.txt`。文件名单、ZIP 完整性及解压内容与构建文件逐字节一致性均已校验。
- EXE SHA256：`c62ee606d6a257231cc33f98a5f7d566604d966e5acd207c306e1d8f97298bd7`。

未跑全量测试，未启动新版原生应用或进行新版真实客户端上游复测；未自动停止、覆盖或重启现用程序，未修改真实客户端路由或个人配置。

### 2026-10-26 旧会话（历史快照）修正

恢复旧会话时客户端重放的是会话开始那一刻的 `CLAUDE.md` 快照，与磁盘当前内容不一致，旧实现因此定位不到目标并静默不替换；同一前导载体里出现两份同路径封装时还会整单放弃，系统目标一并失效。现在按绝对路径 + 完整封装识别，历史快照同样替换，多份封装全部替换；面板新增请求内系统替换、用户级指令替换的应用状态。

- Rust 扩展定向测试：feature-on **71/71**、feature-off **70/70**。
- `proxy::forwarder::tests`（feature-on）**138/138**；`cargo check --all-targets --features prompt-rewriter`、`cargo fmt --check`、`git diff --check` 通过。
- 前端 `pnpm typecheck`、面板组件 **46/46**、关于页更新弹窗组件测试 **2/2**、Prettier 通过。
- 便携包（自包含单文件）：`src-tauri/target/release/prompt-rewriter-claude-only-20261026-single-v2.zip`，**17,340,211 bytes**，包内只有 `cc-switch.exe` 和 `README.txt`；EXE SHA256 `344d1b1387493cdecb4e36616c4167ae8f1e3aad3d63013049896a3fcb9620c3`。
- 该 EXE 由 MSVC 工具链构建，导入表只剩 Windows 系统 DLL，`WebView2Loader.dll` 与 `liblzma-5.dll` 已静态链接；没有安装器或卸载程序，移除时直接删除该文件。
- 未做新版原生端到端复测；未自动停止、覆盖或重启现用程序，未修改真实客户端配置。

### 2026-10-26 注入路径稳定性加固

- 子代理识别扫描全部消息（`copilot_optimizer::detect_subagent`），前置注入不影响判定；请求分类只看最后一条消息。
- 压缩检测在改写前用原始 body 计算，20 KB 级别的系统替换不会让压缩请求被误判或漏判；压缩请求仍整体跳过。
- 协议转换收到的是一条普通 user 文本消息。Chat 载荷要求 system/developer 仍在首位，所以注入位置改为首条 system/developer 之后；Anthropic/Gemini 仍在最前。
- 注入幂等：请求里已有同一段 `<cc-switch-user-instructions>` 时不再重复添加，Gemini 的 `parts` 也计入检查。
- 会话指纹与 Copilot 分类使用原始入站内容；供应商 body override 校验发生在注入之后，比较两侧都含注入，不会误报冲突。
- 重试与 failover 复用同一改写基线，注入不累加。

已知成本：旧会话每个请求会多带一份替换全文（当前配置约 1.9 KB），替换文本越大开销越明显。

### 历史验证（不代表当前双路版本已验证）

- 用户已反馈此前 fix1 的 Claude 替换成功。此前 Codex 失败排查显示未进入代理；该扩展支持现已按用户要求移除，不再继续 Codex 客户端替换验证。
- fix1：组件 47/47，Rust feature-on 63/63、off 62/62，两套 all-targets check、renderer 与 Release 构建通过；未跑全量测试。便携 ZIP 16,614,832 bytes；EXE SHA256 `c0f767eea249af82bbe13143f8574b5b7d88e98528885b8cf6b0eec3d4d5758a`。
- 此前真实 Claude 入站离线重放两个目标均应用；Codex 仅本地会话重构离线重放，不是 HTTP 抓包。临时私有测试已移除，样本、密钥与配置不入 Git、不打包。两份离线重放均未向真实上游发请求。
- 此前 renderer-only mock UI 操作成功但不是原生 Tauri 全流程。更早原生调试曾超时，不能宣称原生全流程通过。原生调试会注册全局 deep link，隔离测试目录不隔离注册表；如再次启动隔离原生实例，应禁用其 deep-link schemes。

合成引擎、本地 mock 上游、renderer-only 界面、真实入站离线重放和真实客户端端到端是不同验证层级；HTTP 200 本身不证明两个目标逐字正确。未覆盖的真实客户端、供应商或原生流程需明确注明。
