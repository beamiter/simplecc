# Changelog

## Unreleased - 2026-08-01

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
