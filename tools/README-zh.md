# 简体中文语言支持（Chinese Simplified）

给 WinRust 添加 `简体中文` 界面语言。

## 改动清单

### 1. `common/src/lib.rs` — 语言基础设施

| 位置 | 改动 |
|---|---|
| `enum Lang` | 增加 `Zh` |
| `Lang::name()` | `Zh => "简体中文"` |
| `Lang::next()` | `Pt => Zh`、`Zh => En`（切换循环） |
| `Lang::code()` | 数组加 `"zh"` |
| `Lang::parse()` | `Some("zh") => Lang::Zh` |
| `lang()` | `4 => Lang::Zh` |
| `t!` 宏 | 新分支 `zh: $zh`；无 `zh:` 时中文回落英文 |
| `tf!` 宏 | 同上 |
| `dot!` 宏 | 中文使用小数点（与英/日一致） |

**向后兼容**：没有 `zh:` 的旧调用不受影响，中文会显示英文原文。

`lang.txt` 写 `zh` 即可切换（`%ProgramData%\WinRust\lang.txt`）。

### 2. 翻译应用

`tools/add-zh.py` 给所有 `t!`/`tf!` 调用补 `zh: "..."`：

```bash
python tools/add-zh.py --dry-run   # 预览
python tools/add-zh.py             # 实际修改
```

- 翻译表：`tools/zh-map.json`（763 条，英文原文 → 中文）
- 本次应用：**897 处调用**，覆盖**全部 UI 文案**
- 括号配对：902 处宏调用，0 处不配平

### 3. 翻译原则

| 原则 | 说明 |
|---|---|
| 术语统一 | `Bar` → 任务栏；`Display` → 显示器；`Process` → 进程 |
| 保持占位符 | `{}` `{:.0}` `{name}` 等原样保留，顺序不变 |
| 单位不译 | `MB/s` `Mbps` `GHz` `MT/s` 保持原样 |
| 代码标识不译 | `lsass.exe`、`svchost`、`WSearch`、`rs-du --purge` 等保持原文 |
| 路径不译 | `%ProgramData%\...`、`AppData\Local\Programs` 保持原文 |
| 按钮简洁 | 「应用」「取消」「确定」等短词优先 |

## 构建

需要 MinGW-w64（`windres`、`dlltool`、`gcc`）：

```bash
# 环境
export PATH="/path/to/mingw64/bin:$PATH"

# 构建
powershell -ExecutionPolicy Bypass -File scripts/build-plugins.ps1
cargo build --release
```

产物：`target/release/plugin-launcher.exe`

## 测试

```bash
# 切到中文
echo -n zh > "$PROGRAMDATA/WinRust/lang.txt"

# 重启各组件后生效
```

## 与上游合并的注意点

- 每次上游更新后，新增的 `t!` 调用需要补 `zh:`
- 用 `python tools/add-zh.py --dry-run` 可查出新增未翻译项
- `zh-map.json` 只需追加，不用重译已有的

## 已知限制

- 3 处标识符（`*en`、`run::KNOWN[i].2`、`self.1`）不是用户文案，保持原样
- 部分技术术语保留了英文（如 `ETW`、`DPC`、`NTFS`），更符合中文技术习惯
