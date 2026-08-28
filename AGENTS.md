# 项目协作约定

## Git 提交

- 提交标题遵循 Conventional Commits：`<type>(<scope>): <中文简述>`；`scope` 可省略。
- `type` 使用仓库既有类别，如 `feat`、`fix`、`refactor`、`docs`、`chore`。
- 标题和正文都使用中文；标题说明改动结果，避免泛泛而谈。
- 提交必须包含详细正文：空一行后用列表说明改动内容、原因或影响，以及已执行的验证。

## 更新日志

- 修改功能、修复缺陷或调整用户可见行为后，必须同步更新 `CHANGELOG.md`。
- 更新日志按 Keep a Changelog 分组记录在当前版本中，准确说明改动内容及其影响；纯内部改动若会影响维护、兼容性或发布内容，也应记录。

示例：

```text
fix(chat): 修复输入框回车发送时意外换行

- 在多行输入框处理前消费普通 Enter，避免光标位于正文中间时插入换行。
- 保持 Shift+Enter 的换行行为及 IME 输入逻辑不变。
- 验证：cargo test --release plain_enter_is_consumed_before_multiline_editor_can_insert_a_newline
```
