# Changelog

## Unreleased - 2026-08-05

### code action 的 context.diagnostics 真的送到服务端

- 编辑器发出的是扁平的 `DiagnosticItem` 形状(`line`/`character`/`end_line`/
  `end_character`),而 `lsp_types::Diagnostic` 要求嵌套且必填的 `range`;
  守护进程此前直接 `from_value::<Vec<Diagnostic>>(...).unwrap_or_default()`,
  于是解析以 ``missing field `range` `` 失败后被 `unwrap_or_default()` 吞掉,
  转发给服务端的永远是 `"context": {"diagnostics": []}`。上一版“选中范围后
  quickfix action 能出现”的说法因此只兑现了 `refactor.extract` 那一半。
- 新增 `types::parse_context_diagnostics()`:扁平形状转成 LSP 形状,已经是
  LSP 形状的条目原样透传,数字形态的 `code` 还原为数字;无法解析的条目写到
  stderr 而不是静默丢弃,下一次形状不匹配不会再无声无息。
- 新增三个单元测试,直接喂入 `RangeDiagnostics()` 产生的那串 JSON,
  跨越 Vim 与守护进程的边界锁死这一类 bug。

### code action 与格式化支持范围

- `:SimpleCCAction` 与 `:SimpleCCFormat` 加上 `-range`。选中若干行再调用时,
  code action 会带上真实范围与范围内的诊断作为 `context.diagnostics` —— 这是
  `refactor.extract`(提取函数、提取变量、TypeScript 的 move to a new file)
  以及绑定在诊断上的 quickfix action 能出现的前提;此前请求永远是
  `end_line == line` 且 `diagnostics: []`,这一整类重构根本无法触达。
- 新增 `textDocument/rangeFormatting`(守护进程 `Request::RangeFormatting` +
  `LspClient::range_formatting`),按服务端能力位判断;不支持时明确报错,而不是
  悄悄把整个 buffer 重排。不带范围的调用行为完全不变。
- `:SimpleCCSelExpand` / `:SimpleCCSelShrink` 也加上 `-range`:从上一次展开
  留下的可视选区里再次调用,不会再因为 Vim 自动插入的 `'<,'>` 而抛 E481。
- 新增 visual 模式的 `<Plug>(simplecc-code-action)` / `(simplecc-format)`,
  以及 normal + visual 的 `<Plug>(simplecc-selection-expand)` /
  `(simplecc-selection-shrink)`;默认 `<leader>ca`、`<leader>fm` 现在同时映射
  到 visual 模式。
- 修正文档漂移:README 与 help 一直写着 `<leader>f`,实际映射是 `<leader>fm`;
  README 命令表补上 `:SimpleCCHealth`。
- 新增 `test/range_requests.vim`,用记录请求的 fake daemon 断言无范围与有范围
  两条路径各自发出的报文,并回归 E481。

### 空回复现在会清除,而不是被忽略

- inlay hint 与 semantic token 的空回复此前都在 `prop_remove` 之前提前 return。
  把 `let x = compute();` 注释掉,`: i32` 会留在注释上,而且 `RestoreInlayHints()`
  每次 `CursorHold` 都会从缓存里把它重新贴回去;全选删除后,已删除代码的
  semantic token 高亮同样留在 buffer 里。
- `:SimpleCCSemanticTokens` 与 1000ms 防抖后台刷新现在区分开来:只有用户主动
  发起的请求才会往消息行写 `No semantic tokens`,后台刷新不会再制造
  hit-enter 提示。
- 新增 `test/stale_props.vim` 回归两种空回复与缓存恢复路径。

### 守护进程交给 simplecore 托管

- 进程生命周期改由已经 vendored、sha256 锁定并有回归测试的
  `autoload/simplecc/core.vim` 负责。此前 `OnBackendExit` 只在
  `:SimpleCCRestart` 已经排队时才重启,被 OOM kill 或 panic 掉的守护进程永远
  不会回来:之后每个 `gd`/`K`/`<leader>ca` 都只回答 `[SimpleCC] not initialized`,
  补全悄悄退化成 buffer 词。而 `doc/simplecc.txt` 早就在承诺指数退避重启和
  crash-loop 断路器 —— 现在这些承诺是真的了。
- 守护进程恢复后会自动重放 `initialize` 与所有已打开 buffer 的 `didOpen`,
  整个 LSP 会话在用户无感知的情况下重建。
- 带回调的请求改走 `core#Request()`,有超时:卡死的守护进程不再让回调永远悬空。
- `:SimpleCCStop` 仍然先发 `shutdown` 并等待 ack 再终止进程,语言服务器不会被
  遗留成孤儿进程;ack 超时 2 秒后照样终止,不会挂住命令。
- 新增 `g:simplecc_auto_restart`、`g:simplecc_max_restarts`、
  `g:simplecc_request_timeout`。
- `:SimpleCCHealth` 现在直接输出 supervisor 的 uptime / crash / restart /
  断路器状态,并改为报告 `ActiveConfigPath()`(此前报的 `~/.simplecc.json`
  是一个插件根本不会读的路径)。
- 新增 `test/daemon_restart.vim` 与 `test/fake_daemon_crash.sh`:守护进程在
  会话中途非正常退出后,不做任何用户操作,`g:simplecc_status` 必须自己回到
  `ready`;并覆盖 `g:simplecc_auto_restart = 0` 与显式重启。

### 诊断按路径与来源服务器存储

- 诊断改用解析后的文件路径作 key。此前用的是语言服务器发来的 URI 原文,而
  Vim 的 `PercentEncodePath()` 会转义 `A-Za-z0-9-._~/:` 之外的一切,Rust `url`
  crate 却放过 `@ ( ) + , ; = & ' ! $ *`,于是同一个文件落进两个桶:sign 和
  virtual text 照常渲染(它们走 uri → path → bufnr),但 `simplecc#DiagCounts()`、
  `:SimpleCCDiagnostics`、`:SimpleCCDiag`、`[d`/`]d` 对任何
  `node_modules/@types/...` 之类的路径一律回答"没有诊断"。
- 诊断事件开始携带 `server` 字段,存储结构变为 `path -> server -> items`。
  一个 filetype 配两个服务器(pyright + ruff-lsp)时,双方的诊断不再互相覆盖;
  某个服务器重新发布只替换它自己那一份。pull diagnostics 也归到同一个来源,
  不会与 push 的结果重复入列。
- 新增 `g:simplecc_diag_sources`(默认 `[]` 即全部),可在不停用服务器的前提下
  只显示指定来源的诊断;`:SimpleCCHealth` 会列出当前的发布方与过滤器。
- 新增 `test/diagnostics_store.vim`:用含 `@()+,;=&'!$*` 的真实路径回归编码
  分歧,并覆盖双服务器共存、单服务器清空、来源过滤与 buffer 关闭清理。

### 补全排序遵循服务端意图

- 守护进程现在按 `sortText` 排序之后再截断到 `maxItems`。此前是按服务端数组
  顺序直接截断,而 rust-analyzer / gopls / tsserver 的相关度全部编码在
  `sortText` 里,于是被丢掉的往往正是最相关的候选,留在菜单里的第一项近乎随机。
- Vim 侧开始使用 `filterText`:当它与实际插入文本不同(postfix 补全、属性补全)
  时给该项加上 `equal`,Vim 自己的前缀过滤不会再在下一次按键时把它删掉。
- `preselect` 项被移到菜单首位,该次菜单去掉 `noselect`,`<CR>` 可直接接受。
- 新增 `g:simplecc_complete_sort`(默认 1)可整体退回原先的行为。
- 新增 `test/completion_items.vim` 与 `lsp::types` 单测,覆盖排序、截断、
  `filterText` 与 `preselect`。

### 精确级别诊断导航

- `:SimpleCCNextDiag [severity]` / `:SimpleCCPrevDiag [severity]` 可临时只在
  `error/warning/info/hint` 中跳转，也可显式用 `all` 浏览全部诊断；无参数仍完全
  沿用 `g:simplecc_diag_min_severity` 的可见范围。
- 非法级别会原地报错而不移动光标；位置相同的诊断增加稳定 tie-break，回绕结果
  不再依赖 Vim `sort()` 的非稳定顺序。

### 按需诊断详情

- 新增 `:SimpleCCDiag` 与 `<Plug>(simplecc-show-diagnostic)`:即使关闭自动
  diagnostic float,也可显式查看当前行的全部可见诊断,完整展示 severity、source、
  字符串或整数 code,并保留多行 message。
- 手动与 `CursorHold` 自动浮窗现在复用 `g:simplecc_diag_min_severity`,多条诊断按
  severity、位置稳定排序;隐藏的 info/hint 不会只在浮窗里意外重现。
- fake-daemon 冒烟测试覆盖 source/code 类型归一、按需弹窗与严重级过滤。

### 诊断工作流升级

- `:SimpleCCDiagnostics[!] [severity]` 现在同时覆盖两种常用视角:无 `!` 保持
  向后兼容,打开当前 split 自己的 location list;加 `!` 汇总客户端当前已知的
  全工作区诊断到 quickfix。可用 `all/error/warning/info/hint` 做精确严重级筛选,
  结果按路径与位置稳定排序。
- 筛选结果为空时也会替换并清空对应 location/quickfix list,不会把上一次的
  旧诊断留在界面上伪装成新查询结果。
- `[d`/`]d` 现在与 `g:simplecc_diag_min_severity` 的可见范围一致,能在同一行的
  多个诊断间按 UTF-16 位置移动,并按排序后的首尾正确回绕。此前会跳进已隐藏的
  hint/info,同一行诊断也无法逐个访问,向前回绕还依赖服务端原始顺序。
- Vim 冒烟测试通过真实 fake-daemon `diagnostics` 事件覆盖当前文件/工作区筛选、
  location list/quickfix 分流以及可见严重级导航。

### 全套统一

- `.simplecore/` 回来了。10 个仓库里的 supervisor(`autoload/<plugin>/core.vim`
  与三个测试文件)本来就是一套 vendored bundle,但源头目录早已丢失,而每个
  Makefile 都还在引用 `../.simplecore/vendor.sh`。现在 bundle 有了源头,而且
  每个仓库带一份 `.simplecore.manifest` 记录各文件的 sha256,`make core-verify`
  会校验它,`check` 依赖它——手改 vendored 文件会在改它的那个仓库里直接失败,
  不需要 `.simplecore/` 在场。
- 安装器抽成共享的 `install-common.sh`,各仓库的 `install.sh` 只剩配置。
  由此补齐的能力:构建前检查 cargo/rustc 与 MSRV(此前 3 个仓库缺,用户看到的
  是一屏 trait 解析错误);原子替换(此前 2 个仓库是就地覆写,Vim 还开着旧 daemon
  时会 ETXTBSY);Windows 的 `.exe` 后缀;安装前用 `--self-test` 验证刚构建的
  二进制;以及生成 helptags。
- `make check` 现在是每个仓库统一的完整门禁。simplemarkdown 与 simpleminimap
  此前叫 `make test`,旧名字保留为别名。
- daemon 的命令行统一为 `--version` / `--help` / `--self-test`。

### 工具链

- `rust-version` 统一到 1.88(此前 1.85 与 1.88 各半)。实测:1.88 能构建全部
  10 个仓库,1.85 只能构建 5 个。
- `cargo update`:全部为补丁级更新。

  注意:这次更新让 `ignore` 从 0.4.27 升到 0.4.30+,而后者用了 let-chains。
  simplefinder 与 simpletree 此前声明的 1.85 在更新前是真实可用的,更新后不再成立
  ——这是这次依赖刷新付出的代价,不是发现了旧的错误声明。
- MSRV 提到 1.88 后,clippy 的 `collapsible_if` 开始建议用 let-chains 合并
  (该 lint 受 MSRV 门控)。已按建议合并,语义不变。

### 本插件

- `--version`/`--help`:此前 daemon 会把任何参数当成没有参数,`--version`
  的结果是直接启动一个 daemon 而不是打印版本。
- `--self-test`:校验内置语言服务器表自洽——每个 server 声明的 filetype
  都必须能解析回一个 server。解析不回来意味着那个 filetype 静默地没有补全。

### 性能:缓冲区词补全

补全在插入模式下每次按键都会跑,而 `CollectBufferWords()` 过去会把整个 buffer
读进来并逐行做关键字切分。前缀匹配不到任何词时提前退出永远不触发——而"匹配不到"
恰恰是敲一个新标识符时的常态。60000 行的文件实测:

| | 优化前 | 优化后 |
|---|---|---|
| 前缀有匹配 | 6.78 ms | **0.39 ms** |
| 前缀无匹配(敲新名字) | **968 ms** | **1.54 ms** |

- 加子串预筛:以 `prefix` 开头的词只可能出现在包含 `prefix` 的行上,所以先用
  `stridx()` 排除绝大多数行,不必为它们付出正则切分的代价。结果与逐行切分完全
  一致,而"无匹配"场景快 12 倍。
- 扫描有上界:新增 `g:simplecc_complete_buffer_max_lines`(默认 2000),从光标向
  两侧展开,所以被截掉的只会是离光标最远的候选。此前无论 buffer 多大都会全扫。
- 不再预先构造覆盖全 buffer 的行号列表,也不再整体 `getbufline()`,只取光标附近
  的窗口。
- 新增 `test/buffer_words.vim`:验证匹配、距离排序、去重、上限、以及扫描上界确实
  生效(把上界去掉该测试会失败)。

### 修复

- `registry` 的 `root_patterns_choose_the_nearest_marker_without_crossing_workspace`
  在 macOS 上一直失败:`std::env::temp_dir()` 返回 `/var/folders/...`,而它是
  `/private/var/...` 的符号链接,`server_root_path()` 会做 canonicalize,两边对不上。
  期望值改为同样 canonicalize(与相邻的那个测试一致)。simplecc 的 CI 至少从
  2026-07-21 起就因此挂着。

### 构建与 CI 修复

- clippy 的 `collapsible_if` 属于按 MSRV 放开的 lint;声明升到 1.88 后它开始生效,12 处已合并为 let-chain。
- `rust-version` 由 1.85 更正为 1.88:依赖 `time`/`zip` 实际要求 1.88,原先的声明按字面根本编译不过。新增 CI 的 MSRV 作业按声明版本构建,防止再次漂移。
- 修复 `doc/simplecc.txt` 中重复的 help tag(`:SimpleCCLog`、`:SimpleCCRestart`),`helptags` 会因此报错并让 `install.sh` 失败。

### 修复

- `WsSymbolFilter` 因为在 Vim9 lambda 块里写了跨行字典字面量而触发 E723,
  整个函数其实从未编译成功——`:SimpleCCWorkspaceSymbol` 的实时过滤一按键就会
  抛错。字典已提到具名函数中。新增的 `make defcompile` 就是为了让这类
  "惰性编译藏起来的错误" 在测试期暴露。
- 新增 `:SimpleCCHealth`:一次性列出 daemon 路径与状态、workspace root、
  当前语言服务器、各服务器重启次数、in-flight 请求数、诊断与打开文档数、
  `+popupwin`/`+textprop` 可用性以及用户配置文件位置。

### 可靠性:统一 daemon 监督层 (simplecore)

- 进程生命周期改由 vendored `simplecore` 监督层接管(`autoload/simplecc/core.vim`,
  从 `.simplecore/` 同步,请勿直接编辑)。九个插件共用同一份实现:
  - 存活判定一律走 `job_status()`。`job_start()` 即使 exec 失败也会返回 job
    对象,所以 `job != null` 并不能说明进程还活着。
  - 代际守卫:被替换掉的旧 daemon 的 `exit_cb` 迟到时,不会再清掉接替它的新
    进程的状态。
  - 停止栅栏:显式停止后仍在管道里的事件会被丢弃,不会把刚拆掉的状态又写回去。
  - 指数退避自动重启;同一时间窗内反复崩溃则熔断,只报错一次而不是无限重启。
    手动 `:SimpleCCRestart` 会重新合闸。
  - 请求按 id 关联并支持超时,卡死的 daemon 不会让回调永远悬着。
- 新增 `:SimpleCCHealth`、`:SimpleCCRestart`、`:SimpleCCLog`,全套插件命名一致。

### 测试

- 新增 `tests/vim_core.vim`:监督层回归套件(存活判定、代际守卫、停止栅栏、
  退避重启、崩溃熔断、请求超时、协议握手、raw/json 两种编解码),由
  `tests/fake_daemon.py` 驱动——一个可以按需应答/静默/乱码/崩溃/忽略 SIGTERM
  的假 daemon。
- 新增 `make defcompile`:强制编译所有 Vim9 `def`。Vim9 惰性编译会把冷分支里的
  语法/类型错误一直藏到用户真正踩中为止。
- `make check` 现在包含以上两项。

## 0.2.0 - 2026-07-25

- 迁移到 Rust edition 2024，最低 Rust 版本提升到 1.85。
- 依赖大版本升级：which 6 → 8、dirs 5 → 6、zip 2 → 8；其余依赖统一刷新。
- 行为无变化；更新后请重新运行 `./install.sh` 重建 daemon。
