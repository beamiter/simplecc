# Changelog

## Unreleased - 2026-08-05

### `:SimpleCCHealth` 变成真正的体检

- 此前它把十来行 echo 到消息区,内容基本是"守护进程在不在跑",而且把
  `~/.simplecc.json` 说成用户配置 —— 那是一个它从来不读的文件。现在报告渲染
  到 `buftype=nofile` 的 scratch buffer,每行都是
  `[LEVEL] 事实 — 该怎么办`,分五节:ENVIRONMENT / BINARY / CONFIG /
  RUNTIME / CONTEXT。
- BINARY 节里是这套插件最常见的故障:插件管理器更新了 Vim 文件,旁边的
  `lib/simplecc-daemon` 从来没重新编译过,两半说着不同的协议,而唯一的症状是
  某个功能安静地什么都不做。现在直接比较守护进程的 mtime 与
  `src/**/*.rs`、`Cargo.toml`、`autoload/**/*.vim`、`plugin/*.vim` 里最新的一个,
  过期就报 ERROR 并给出 `run ./install.sh, then :SimpleCCRestart`。
- CONFIG 节校验真正生效的那个配置文件(`ActiveConfigPath()`):JSON 能不能解析、
  `g:simplecc_config_path` 指的文件在不在、每个 `languageServers` 条目有没有
  `command` / `filetypes`,以及 command 到底能不能解析 —— 除了 $PATH 还会查
  `:SimpleCCInstall` 装到的托管目录,那里的服务端不在 $PATH 上却完全正常。
- CONTEXT 节回答"为什么在我的文件里没反应":buftype、filetype、这个 buffer 有
  没有被 didOpen 过、服务端看到的 changedtick 与当前是否一致、以及
  `omnifunc`/`tagfunc`/`formatexpr` 当前指向哪里。
- `test/health_doctor.vim`:五个小节都在;把假守护进程 `touch` 到 2001 年必须
  报"比插件旧"并带上 install.sh,`touch` 到明天必须不报错;JSON 语法错、
  command 不可执行(且提示里带服务端名字)、缺 `filetypes`、缺 `command`、
  `g:simplecc_config_path` 指向不存在的文件,各自都有对应的一行;以及报告确实
  落在一个不可编辑的 scratch buffer 里。

### omnifunc / tagfunc / formatexpr 接到语言服务器上

- 这个插件的能力此前只能通过它自己的命令和映射触达。`<C-x><C-o>`、`<C-]>`、
  `gq` —— Vim 用户不看 README 也会按的三个键,也是 tag stack、`completeopt`、
  `formatoptions` 和一堆第三方插件赖以工作的接口 —— 表现得就像根本没装 LSP
  客户端。现在 `didOpen` 发出的那一刻(即"这个 buffer 确实有服务端"的时刻)
  会把 `omnifunc` / `tagfunc` / `formatexpr` 指过去。
- 每个钩子在做不到更好时都把键还给 Vim:守护进程没跑、buffer 没有服务端、
  服务端没声明对应能力、或者 `tagfunc` 拿到的不是光标下的标识符。因此守护
  进程挂掉时退化成关键字补全、tags 文件和内建格式化,而不是一个死键。
  插入模式下由 `formatoptions` 的 a/t 触发的自动折行永远走 Vim 自己的实现,
  不会每敲一个字符就往服务端跑一次。
- `tagfunc` 必须同步返回列表,这是插件里唯一一处等待:走
  `core#Request()`(而不是 `SendWithCb()`,后者会让 `OnDefinition()` 也跳一次,
  而在 tagfunc 里跳窗口是 E1299),`sleep` 让 channel 回调得以运行,超时由
  `g:simplecc_tagfunc_timeout`(默认 1000ms)封顶,超时即回落到 tags 文件。
  返回行号而不是搜索模式:服务端给的位置本来就是精确的,再搜一次名字会在
  同名多处的文件里找错地方。
- 为此 `serverStatus running` 事件带上服务端能力(守护进程新增
  `LspClient::editor_capabilities()`)。Vim 无法同步询问一个运行中的服务端,
  而 `formatexpr` 必须在发请求*之前*就知道对方是否支持 rangeFormatting ——
  否则 rust-analyzer 这类不支持的服务端会让 `gq` 从"重排注释"变成"报错"。
  能力表为空(旧守护进程)一律按"支持"处理,行为与此前一致。
- 新增 `g:simplecc_native_options`(0 不设 / 1 仅在 buffer 自己没设时设 /
  2 总是覆盖,默认 1)与 `g:simplecc_tagfunc_timeout`。
- `test/native_options.vim`:三个选项在附着时被设上、`<C-x><C-o>` 确实发出了
  completion 请求、`<C-]>` 从服务端拿到 tag 条目而手打的 `:tag` 与插入模式
  补全回落到 tags 文件、服务端不支持 rangeFormatting 时 `gq` 必须还给 Vim、
  旧守护进程不上报能力时不能一刀切关掉、以及守护进程停掉后三个钩子都还给 Vim。

### inlay hint 按视口请求,语义高亮批量落属性

- `textDocument/inlayHint` 本来就是范围请求,而这里每次都填 `0 .. line('$')`。
  两万行的文件里,服务端要为整篇计算 hint,这个进程要把它们全部解码并逐条
  `prop_add()`,只为渲染屏幕上那二十来条。现在请求视口上下各
  `g:simplecc_inlay_margin`(默认 100)行,滚动时由 `WinScrolled` 补齐 ——
  `OnWinScrolled()` 此前只在开了语义高亮且文件够大时才做事。
- 语义高亮改用 `prop_add_list()`:按属性类型攒好位置,每种类型一次调用,
  取代每个 token 一次 `prop_add()`(每次还各带一个 try/catch)。5000 行的
  Rust 文件一次回复几万个 token,那正是每次编辑后卡顿的来源。批量调用是
  全有或全无的,因此失败时逐条回退,不会因为一个越界位置丢掉整类高亮。
- inlay hint 不能同样批量:`prop_add_list()` 明确不支持 `text` 字段,而且不是
  报错而是静默忽略 —— 照搬会让每条 hint 的文字凭空消失。这一点写在代码注释里。
- `test/viewport_hints.vim`:两千行文件里跳到第 1000 行,断言请求范围既不从
  文件头开始也不到文件尾;`g:simplecc_inlay_margin = 0` 时正好是视口;滚动会
  重新请求;以及一次回复里同类型的多个 token 必须全部落到属性上(批量写错时
  只会剩第一个)。改动前前三条断言全灭。

### workspace edit 支持文件的新建 / 重命名 / 删除

- 服务端发来的 `documentChanges` 里只要出现一个资源操作,守护进程就整条拒绝
  (`resource create/rename/delete operations are not supported`),于是
  rust-analyzer 的 "move to submodule"、TypeScript 的 "move to a new file"、
  以及所有连带改文件名的 "rename symbol" 全部什么都不做 —— 连改好的那部分
  文本编辑也一起丢掉。更早一步:客户端能力里从未声明
  `workspace.workspaceEdit.resourceOperations`,服务端因此本来就不该提供这
  类重构,能收到已经算是服务端宽容。
- 客户端能力补上 `resourceOperations: [create, rename, delete]` 与
  `failureHandling: abort`;`WorkspaceEdit` 新增按线序排列的 `operations`,
  把资源操作与文本编辑放在同一条时间线上 —— 顺序是有含义的:重命名重构先改
  文件内容再挪文件,反过来就会写进一个已经不存在的路径。原有的扁平
  `changes` 字段保留不变,因此 Vim 端比守护进程新或旧都不会失败。
- Vim 端按序执行,任何一步失败即停止并把失败原因回给服务端。`create` 只有在
  服务端明写 `overwrite` 时才会覆盖已存在的文件;`rename` 会先把未保存的
  buffer 落盘,再把窗口里的 buffer 换成新路径(否则下一次 `:write` 会把刚被
  挪走的文件重新写回来);`delete` 会 wipe 掉对应 buffer,`BufUnload` 顺带把
  `didClose` 发出去。新增 `g:simplecc_resource_operations`(默认 1)可整体
  关掉,关掉时整条 edit 报失败而不是只应用一半。
- 顺带修掉两个一直存在的问题:`bufnr({path})` 是按*模式*匹配的,路径里带
  `.` `*` `[` 的文件会匹配不到自己或匹配到别人,现在一律走
  `BufnrForPath()` 逐字比较;多文件 edit 不再用 `:edit` 打开目标文件 ——
  那会把用户的窗口拖到重构最后碰到的那个文件上,而且当前 buffer 刚被同一条
  edit 改过时会直接 E37 失败,改用 `bufadd()` + `bufload()`。
- `test/resource_operations.vim`:重命名重构(改 importer + 挪文件 + buffer
  跟着换名)、create 后紧跟一条填充 edit、`ignoreIfExists` 不许截断已有文件、
  delete 连带 wipe buffer、不存在的删除目标要报错、`g:simplecc_resource_operations = 0`
  必须整条拒绝、以及只发 `changes` 的旧守护进程仍然可用。改动前前四条断言全灭。

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

### `:SimpleCCStop` 会取消已排队的自动重启

- 守护进程意外死亡后 core 会排一个退避重启定时器;此时 `:SimpleCCStop` 因为
  `!IsRunning()` 提前返回,永远走不到唯一能取消该定时器的 `core#Stop()`,
  于是守护进程照常复活。崩溃循环里退避会涨到 5000ms 上限,而那正是用户最
  可能去按 `:SimpleCCStop` 的时刻。现在无论进程是否在跑都会调用
  `core#Stop()`(无 job 时它本就安全空转),并把状态清空。
- `test/daemon_restart.vim` 增加一段:连续崩溃把退避拉宽到 800ms,在窗口内
  调用 `simplecc#Stop()`,断言两秒后守护进程仍然是停的。

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
