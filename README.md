# codex-switch

`codex-switch` 是一个用于切换本地 Codex 账号配置的小工具。它会从多个 profile 目录里读取 `auth.json`，查询普通账号的 API 余量，并支持在 TUI 中手动选择账号或 API profile，也可以使用 `auto` 模式自动切换到当前余量最多的普通账号。

## 安装

推荐使用 Nix flake 安装：

```bash
nix profile install github:ShangYJQ/codex-switch#codex-switch
```

如果已经 clone 到本地，也可以在项目根目录运行：

```bash
nix profile install .#codex-switch
```

安装完成后确认命令可用：

```bash
codex-switch --version
```

临时运行，不安装到 profile：

```bash
nix run github:ShangYJQ/codex-switch#codex-switch -- --version
```

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

- `codex_config_dir`: 当前 Codex 使用的配置目录，切换 profile 时会把选中目录里的 `auth.json` 和对应的配置模板复制到这里。
- `key_dir`: 存放多个账号配置的目录。

账号目录结构示例：

```text
/Users/you/Documents/Codex
├── .account-config.toml
├── .api-config.toml
├── account-a
│   └── auth.json
├── account-b
│   └── auth.json
├── account-c
│   └── auth.json
└── API
    └── auth.json
```

## Profile 机制

`key_dir` 下每个子目录都是一个 profile。profile 目录里只需要放 `auth.json`，实际使用哪份 `config.toml` 由 `key_dir` 根目录下的两个模板决定：

- `.account-config.toml`: 普通账号切换时使用的默认 Codex 配置。
- `.api-config.toml`: API profile 切换时使用的默认 Codex 配置。

切换普通账号时会复制：

```text
key_dir/<account>/auth.json      -> codex_config_dir/auth.json
key_dir/.account-config.toml     -> codex_config_dir/config.toml
```

切换 API profile 时会复制：

```text
key_dir/API/auth.json            -> codex_config_dir/auth.json
key_dir/.api-config.toml         -> codex_config_dir/config.toml
```

profile 类型判断规则：

1. 目录名是 `API` / `api` / `Api` 时，视为 API profile。
2. 否则如果 `auth.json` 里有非空的 `OPENAI_API_KEY` 字段，也视为 API profile。
3. 其他目录视为普通账号。

普通账号会请求 `5h` / `7d` 用量并参与余量排序。API profile 不查询 `5h` / `7d`，列表右侧会显示 `.api-config.toml` 中的 `model_provider` 和 `model`，例如：

```text
API codex gpt-5.5
```

TUI 顶部会显示当前正在使用的 profile：

```text
current: account-a
```

当前 profile 的匹配方式：

- 普通账号使用 `tokens.account_id` 和 `codex_config_dir/auth.json` 中的 `tokens.account_id` 匹配。
- API profile 使用 `OPENAI_API_KEY` 和 `codex_config_dir/auth.json` 中的 `OPENAI_API_KEY` 匹配。
- 匹配不到时显示 `current: --`。

## 使用

启动 TUI：

```bash
codex-switch
```

查看版本：

```bash
codex-switch --version
codex-switch -V
```

查看帮助：

```bash
codex-switch --help
codex-switch ping --help
```

快捷键：

- `↑` / `↓`: 移动选择账号
- `Enter`: 切换到当前选中的账号
- `q`: 退出

普通账号列表右侧显示 API 余量：

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

## 自动切换

不打开 TUI，自动切换到最合适的账号：

```bash
codex-switch auto
```

`auto` 会等待所有普通账号的用量查询完成，然后按当前排序规则选择第一名并切换。API profile 没有 `5h` / `7d` 余量语义，不参与 `auto` 自动切换。

## 账号检测

不打开 TUI，检测所有普通账号是否可以正常返回用量 API：

```bash
codex-switch ping
```

`ping` 会等待所有普通账号的用量查询完成。全部正常时输出：

```text
all accounts ok
```

如果有账号无法正常返回用量 API，会逐行输出异常账号的邮箱；如果无法从 `auth.json` 解析邮箱，则输出 profile 名和路径。API profile 不参与 `ping` 检测。

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

使用 Nix 构建：

```bash
nix build
./result/bin/codex-switch --version
```

开发运行：

```bash
cargo run
```

构建 release：

```bash
cargo build --release
```
