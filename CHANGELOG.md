# Changelog

## Unreleased - 2026-08-05

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
