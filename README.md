# codex-switch

`codex-switch` 是一个用于切换本地 Codex 账号配置的小工具。它会从多个账号目录里读取 `auth.json`，查询每个账号的 API 余量，并支持在 TUI 中手动选择账号，或使用 `auto` 模式自动切换到当前余量最多的账号。

## 配置

第一次运行时会自动创建配置文件：

```toml
~/.config/codexswitch/config.toml
```

配置内容示例：

```toml
codex_config_dir = "/Users/you/.codex"
key_dir = "/Users/you/Documents/Codex"
```

- `codex_config_dir`: 当前 Codex 使用的配置目录，切换账号时会把选中账号目录里的文件复制到这里。
- `key_dir`: 存放多个账号配置的目录。

账号目录结构示例：

```text
/Users/you/Documents/Codex
├── account-a
│   └── auth.json
├── account-b
│   └── auth.json
└── account-c
    └── auth.json
```

每个账号目录里的 `auth.json` 需要包含 `tokens.access_token` 和 `tokens.account_id`。

## 使用

启动 TUI：

```bash
codex-switch
```

快捷键：

- `↑` / `↓`: 移动选择账号
- `Enter`: 切换到当前选中的账号
- `q`: 退出

列表右侧显示 API 余量：

```text
5h 100% 7d 84%
```

底部 `Details` 区域会显示当前选中账号的详细信息：

```text
email: name@example.com
plan_type: plus
5h 100% reset in 5h 0min
7d 84% reset at 05.12 12:00
```

`reset_after_seconds <= 24h` 时显示 `reset in xxh xxmin`；超过 24 小时时显示本地时间 `reset at MM.DD HH:MM`。

## 自动切换

不打开 TUI，自动切换到最合适的账号：

```bash
codex-switch auto
```

`auto` 会等待所有账号的用量查询完成，然后按当前排序规则选择第一名并切换。

排序规则：

1. 绿色账号优先
2. 橙色账号其次
3. 白色账号其次
4. 红色账号最后
5. 同颜色内，`5h` 余量多的在上面
6. `5h` 余量相同，再按 `7d` 余量多的在上面
7. 最后按账号名稳定排序

颜色含义：

- 白色: 用量还在获取中
- 绿色: 成功获取用量，且 `5h` / `7d` 余量都不少于 `1%`
- 橙色: 成功获取用量，但 `5h` 或 `7d` 余量小于 `1%`
- 红色: 无法获取 API 用量

## 构建

开发运行：

```bash
cargo run
```

构建 release：

```bash
cargo build --release
```

安装到本机 Cargo bin：

```bash
cargo install --path .
```
