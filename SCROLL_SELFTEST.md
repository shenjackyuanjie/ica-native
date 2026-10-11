# 滚动与图片加载回归自测

## 隔离原则

- 滚动自测位于 [scroll_regression.rs](src/app/chat/scroll_regression.rs)，图片格式回落与错误槽绘制自测位于 [image_preview_regression.rs](src/app/chat/image_preview_regression.rs)，均仅由 `#[cfg(test)]` 模块声明引入；GIF 和真实静态加载器链的回归也都在 `#[cfg(test)]` 下。正常发布二进制不会编译这些测试，没有自测启动参数、后台测试任务或运行时分支。
- 使用无窗口 egui、内存构造的消息和 `bytes://` 图片；不读取本地配置，不连接 Bridge，不访问真实聊天，也不发起网络图片请求。
- GIF 并发测试通过通道和队列标记控制解码开始、完成与失效，不依靠耗时 GIF 或睡眠猜测结果。

## 覆盖的回归

| 场景 | 验收条件 |
| --- | --- |
| 正文、引用和窄窗图片槽 | Pending（有／无尺寸）、Ready、长短错误及模拟淘汰重载下高度保持不变；方图、横图、竖图和小图均不撑宽 |
| 连续图片状态变化 | 行高、后续消息 top、总内容高度和中部阅读锚点在暖机后变化不超过 1 点 |
| 视口上方冷行校准 | 修正错误的估算高度后，同一可见消息不被拖动 |
| 冷首屏上滚／点击 | 注入确定的冷行高度误差，用户滚轮或列表内按下鼠标均立即取消底部校准锁；布局稳定后不吸回底部 |
| 窗口变窄 | 长正文重新换行后，保留原消息及行内阅读位置，不沿用旧像素偏移 |
| 普通滚轮及尾帧 | 5 次 100 点输入只产生一次总计 500 点的位移，尾帧结束后阅读锚点不继续漂移 |
| 历史前插 | 通过真实数组事件插入跨日期消息，首帧及后续帧中同一消息的绘制位置变化不超过 1 点 |
| 小图对齐与命中 | 保留自然 16×12 尺寸、左右对齐；点击图像打开图片，槽内留白不打开 |
| GIF 源字节释放与格式变化 | 已有帧元数据时不重新探测缺失源字节；真实静态加载器不接管 `#frame`，base URI 仍能解码与上传 |
| 预览格式回落与错误槽 | PNG 后缀下的 GIF 必须实际画出帧；旧 GIF 元数据遇静态图必须清理并实际出图；过期链接保留专用文案与固定槽高 |
| GIF 后台任务 | 冷加载立即 Pending、完成唤醒；URI 去重、队列满可重试、forget／forget_all 后旧成功和错误结果不复活 |
| GIF 原有容量规则 | 帧索引及帧时长正确；错误缓存有界；180 帧上限和活跃缓存宽限仍成立 |

几何测试每组合循环 90 帧，真实列表状态变化循环 60 次，普通滚轮还检查 40 个收尾帧。所有几何断言容差为 1 个 egui 逻辑点，不是物理像素。

底部锁回归还执行过变异校验：临时禁用生产代码的输入解锁条件后，滚轮和点击两项测试均失败；恢复条件后通过。临时变异没有保留，测试不会被帧末“自然解锁”掩盖。窗口缩窄测试也先复现了原阅读消息漂移，修复布局键失效后的锚点恢复后才通过。

## 执行方式

以下命令在 `ica-native` 项目目录执行。

### 定向验证

```powershell
cargo test --release --locked scroll_regression -- --test-threads=1
cargo test --release --locked image_loader -- --test-threads=1
```

### 重复定向验证

```powershell
foreach ($round in 1..10) {
    Write-Host "回归轮次 $round / 10"
    cargo test --release --locked scroll_regression -- --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw "滚动回归第 $round 轮失败" }
    cargo test --release --locked image_loader -- --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw "图片加载回归第 $round 轮失败" }
}
```

### 全量测试与构建

项目现有的部分磁盘缓存、贴纸测试需要可写临时目录。下面只为本次测试指定构建目录中的临时位置，并恢复当前 PowerShell 的环境，不修改系统权限或用户配置。

```powershell
$previousTemp = $env:TEMP
$previousTmp = $env:TMP
$testTemp = Join-Path (Get-Location) 'target/scroll-selftest-temp'
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
cargo fmt --all --check
cargo clippy --release --all-targets --locked -- -D warnings
cargo build --release --locked
```

## 本次验证结果

在 Windows、Rust 1.99、更新后的锁文件上执行：

| 检查 | 结果 |
| --- | --- |
| `cargo fmt --all --check` | 通过 |
| `cargo clippy --release --all-targets --locked -- -D warnings` | 通过，无警告 |
| 串行全量测试 | 247 通过，0 失败，1 项原有字体桌面测试忽略 |
| 默认并行全量测试 | 247 通过，0 失败，1 项原有字体桌面测试忽略 |
| 10 轮滚动／预览回归 | 每轮 13 项通过 |
| 10 轮图片加载回归 | 每轮 17 项通过 |
| 输入解锁变异校验 | 禁用条件时滚轮和点击两项均失败；恢复后通过 |
| `cargo build --release --locked` | 正常发布路径构建通过 |

## 验证边界与人工复查

这些测试验证项目自身的几何、输入与异步失效规则，不度量 GPU 上传、显示器刷新率或真实网络延迟，也不能替代桌面上对主观滚动手感的检查。没有为了测试而启动真实客户端或打开真实会话。

稳定预览槽是本次采用的最小几何修复：正文高 240 点、引用高 96 点，各状态保持一致；小图和横图因此可能留白，但不会拉伸或放大。图像动作仍只命中实际图片区域。

人工复查可重点观察：冷打开图片较多的会话时立即上滚是否被拉回、慢图片／GIF 加载和缓存重载时阅读位置是否稳定、拉取旧消息首帧是否跳动、自己发送的小图是否右对齐，以及窄窗下点击图像和留白是否正确。
