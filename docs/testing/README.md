# 测试指南

ica-native 的自动化测试统一通过 `cargo test` 执行，包括业务规则、协议兼容、错误处理和无窗口 GUI 回归。回归是测试目的，不是另一套执行机制；项目没有运行时自测开关，也不会为测试启动真实客户端或连接真实 Bridge。

## 文件组织

- 生产文件只声明测试模块，测试实现放在所属业务目录的 `tests/` 下。文件级 `mod`、`use` 和 `pub use` 都放在文件开头，测试声明与条件编译属性也不例外。
- 根模块使用 `tests/mod.rs`；叶子模块通过 `#[path]` 引入同目录 `tests/` 下的对应文件，保持原有子模块边界和私有项访问，不为了搬测试而扩大生产代码可见性。
- `src/app/test_support.rs` 提供应用级夹具，其他用例不依赖具体测试模块中的通用构造器。场景专属消息、假纹理、输入帧和断言辅助函数留在各自测试文件。
- 当前为二进制 crate，这些测试由 `src/main.rs` 的测试目标统一编译；业务目录下的 `tests/` 不是 Cargo 顶层集成测试目录，不需要新增 `lib.rs`。

例如，`src/app/chat/message_card.rs` 在文件开头声明：

```rust
#[cfg(test)]
#[path = "tests/message_card.rs"]
mod tests;
```

对应文件为 `src/app/chat/tests/message_card.rs`，测试名仍位于 `app::chat::message_card::tests::` 下。定向执行应使用实际模块路径，不通过添加无关的函数名前缀来凑筛选条件。

## 执行要求

| 类别 | 执行方式与边界 |
| --- | --- |
| 常规自动化测试 | 默认 `cargo test`；不需要真实账号、Bridge、麦克风或 GPU 窗口。HTTP 用例只使用随机回环端口，磁盘和贴纸用例需要可写临时目录。 |
| 系统字体环境测试 | 默认忽略；需要本机安装中文字体和具备相应轮廓的 emoji 字体，字体方案变更时显式运行。 |
| 人工复查 | 在桌面上检查真实显示与操作手感，不计入自动化测试通过数，也不替代自动化断言。 |

应用级夹具使用内存配置、假 Bridge 命令通道和不加载贴纸的存储，构造时关闭真实连接及 Noticer、Agent 上下文监听。测试 GUI 权限状态可单独启用；端到端 HTTP 用例需要自行显式启动服务并选择随机回环端口。

## 全量测试

以下命令均在仓库根目录执行。Windows 下如需固定可写临时目录，只覆盖当前 PowerShell 的环境并在结束后恢复，不修改系统配置或权限：

```powershell
$previousTemp = $env:TEMP
$previousTmp = $env:TMP
$testTemp = Join-Path (Get-Location) 'target/test-temp'
New-Item -ItemType Directory -Force -Path $testTemp | Out-Null
try {
    $env:TEMP = (Resolve-Path -LiteralPath $testTemp).Path
    $env:TMP = $env:TEMP
    cargo test --release --locked -- --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw '串行全量测试失败' }
    cargo test --release --locked
    if ($LASTEXITCODE -ne 0) { throw '默认并行全量测试失败' }
} finally {
    $env:TEMP = $previousTemp
    $env:TMP = $previousTmp
}
```

其他平台可直接执行相同的 Cargo 命令，并确保系统临时目录可写。

## 定向测试

```powershell
# 枚举当前实际注册的测试
cargo test --release --locked -- --list

# 所有聊天测试，包括消息卡片、输入、操作与无窗口场景
cargo test --release --locked app::chat::

# 滚动、图片预览、通知栏和图片加载器分别执行
cargo test --release --locked app::chat::tests::scroll:: -- --test-threads=1
cargo test --release --locked app::chat::tests::image_preview:: -- --test-threads=1
cargo test --release --locked app::chat::tests::notices:: -- --test-threads=1
cargo test --release --locked image_loader:: -- --test-threads=1

# 关系网模型与布局
cargo test --release --locked app::relation_network::

# 字体方案变更时，在具备所需字体的桌面环境显式执行
cargo test --release --locked app::tests::system_fonts_render_chinese_and_thinking_face -- --ignored --exact
```

## 格式检查和构建

```powershell
cargo fmt --all --check
cargo clippy --release --all-targets --locked -- -D warnings
cargo build --release --locked
```

执行结果记录在对应改动的交付说明或提交正文中，不在长期指南中维护容易过期的“本次通过数”。滚动与图片加载的具体覆盖范围、重复验证及人工复查见 [专项说明](scroll-and-images.md)。
