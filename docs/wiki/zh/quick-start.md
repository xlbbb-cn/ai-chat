本页把你从「刚下载完」带到「模型能回答，而且知道自己在哪个工作区」。模型参数、工具与技能各有单独页面。

## 1. 安装

### Microsoft Store（Windows，推荐）

打开 <https://apps.microsoft.com/detail/9MV82J9G95GS>（Store ID `9MV82J9G95GS`）点「获取」；也可以直接打开协议链接 `ms-windows-store://pdp/?productid=9MV82J9G95GS`。

商店版是 MSIX 包：由商店负责安装与更新，不需要管理员权限，也不需要单独装 WebView2（要求 Windows 10 2004+ / Windows 11）。和 GitHub 安装包版有两处差别：

- 应用配置被重定向到 `%LOCALAPPDATA%\Packages\XLBBB.RabbitChatStudio\LocalCache\Roaming\com.leonard.aichat`，与安装包版**不共享**；工作区里的 `.data` 目录两者共享。
- 商店版的「检查更新」会回答「已是最新」——版本更新由商店推送（见 [更新](更新)）。

### 从 GitHub Release 安装

打开 [Latest release](https://github.com/xlbbb-cn/ai-chat/releases/latest) 选择对应平台：

| 平台 | 文件 |
| --- | --- |
| Windows | `Rabbit-Chat-Studio-Windows-x64-setup.exe`（NSIS） |
| macOS Apple Silicon | `Rabbit-Chat-Studio-macOS-Apple-Silicon.dmg` |
| macOS Intel | `Rabbit-Chat-Studio-macOS-Intel.dmg` |
| Linux x86_64 | `Rabbit-Chat-Studio-Linux-x86_64.AppImage`、`.deb` 或 `.rpm` |

### 包管理器（暂未提供）

目前没有包管理器渠道：winget、Homebrew、Scoop 清单都还没发布，`winget install`、`brew install --cask`、`scoop install` 都找不到这个包。请先用上面的 Microsoft Store 或 GitHub Release 安装包；清单补齐后会加到本页。

### 首次启动提示

- **商店版**：无需额外操作，首次启动即可用。如果你之前用过安装包版，会看到一次「首次运行」，因为两者配置分开。
- **macOS**：构建只是 ad-hoc 签名（没有 Apple 开发者证书）。右键应用 → **打开**，或到「系统设置 → 隐私与安全性 → 仍要打开」。
- **Linux AppImage**：先给它执行权限。

  ```bash
  chmod +x Rabbit-Chat-Studio-Linux-x86_64.AppImage
  ./Rabbit-Chat-Studio-Linux-x86_64.AppImage
  ```

- **Windows**：缺少 WebView2 时，安装 [Microsoft Edge WebView2 运行时](https://developer.microsoft.com/microsoft-edge/webview2/)。从源码构建若遇到 GNU 链接器报错，请改用 MSVC 工具链。

## 2. 配置模型

打开「设置 → API 与模型」。

### 必填项

| 字段 | 说明 | 示例 |
| --- | --- | --- |
| 基础地址 | OpenAI 兼容 API 的根地址，通常以 `/v1` 结尾 | `https://api.openai.com/v1` |
| API 密钥 | 服务商提供的 Key；本地服务可以随便填 | `sk-...` |
| 模型 | 当前会话使用的模型 id | `gpt-4o-mini` |

常见基础地址：

| 服务 | 基础地址 |
| --- | --- |
| OpenAI | `https://api.openai.com/v1` |
| DeepSeek | `https://api.deepseek.com/v1` |
| OpenRouter | `https://openrouter.ai/api/v1` |
| Ollama（本地） | `http://localhost:11434/v1` |
| LM Studio（本地） | `http://localhost:1234/v1` |
| vLLM（本地） | `http://localhost:8000/v1` |

> 写到 `/v1` 为止；不要自己加 `/chat/completions`，应用会拼接端点。

### 获取模型列表

点「从 API 获取模型」（两个请求进行中按钮会显示转圈）：

1. 应用请求 `GET {base_url}/models`，把返回的 id 合并进**模型列表**。
2. 同时加载能力目录：把三个公开数据源（models.dev、OpenRouter、`basellm/llm-metadata`）合并成一份扁平文件 `<app_data>/model_metadata.json`，每周刷新一次。首次构建约下载 7 MB，之后 7 天内直接用缓存。
3. 匹配到的能力（工具、推理、视觉、文件、音频、上下文窗口）以徽标显示，并存进 `config.model_metadata`。模型名会被规范化（小写、扁平化），先精确匹配，再按相似度匹配。
4. 服务商若在 `/models` 里报告了上下文窗口，以它为准；否则用目录里的值。

获取失败不影响手动使用：在「手动添加模型」里填模型 id 后点「添加」。

### 上下文窗口

上下文窗口用于用量条和智能体的上下文预算，取值优先级：

1. `config.model_context_lengths[model]`（从 `/models` 自动填入，或手动填写）；
2. 旧字段 `model_settings.max_tokens`；
3. 全局保守默认值。

服务商没有报告、目录里也匹配不到时，请手动填真实值（例如 `128000`），否则长对话会比必要的时间更早触发压缩。

### 保存

点右上角「保存修改」。新配置从下一轮对话生效，不需要重启。

## 3. 第一轮对话

1. 回到主窗口，在输入框输入内容。
2. **Enter** 发送；**Shift+Enter** 换行。
3. 回复流式输出期间，发送按钮变成**停止**，点它可取消本轮。
4. 支持推理的模型会在回答上方显示可折叠的**思考过程**。
5. 每一轮结束时，页脚会给出这一轮的花费（tokens，已知价格时还有金额）和延迟。
6. 输入框中：`@` 引用工作区文件（追加 `#12-24` 指定行范围），`/` 插入技能引用，`+agent` 嵌入子智能体角色，`@@name` 指向 `.data/tmp` 里的文件。图片、PDF、音频只有在所选模型具备对应能力徽标时才能正常附带。
7. 往输入框粘贴或拖入文件，会被复制到 `.data/tmp` 并以 `@` 提及的形式附上。
8. 回复可以保存为 Markdown 或导出成 PDF 文件；回复里的绘图可另存为 PNG（静态）或 HTML（动态）。

更多细节见 [界面与聊天](界面与聊天)。

## 4. 选择工作区

点**标题栏的工作区名称** →「选择其他文件夹…」，挑一个目录。选定即生效；最近用过的工作区就在这个下拉菜单里。

- 不设置时，默认工作区是应用数据目录下的 `workspace/`。
- 选定后，应用会在其中创建隐藏的 `.data/`，用来保存会话数据库、技能、记忆和待办。
- 切换工作区会同时切换这些状态：历史、技能启用情况、记忆和待办，并落到一个新会话上。
- 如果保存的路径已经不存在（目录被移动或卸载），应用会回退到应用数据目录里的工作区，而不是启动失败；再到设置里重新选一次目录即可。
- 工作区若是 Git 仓库，记得把 `.data/` 加进 `.gitignore`。

完整布局见 [工作区与本地数据](工作区与本地数据)。

## 5. 按需开启能力

下面这些都不是第一轮对话所必需的：

| 能力 | 入口 | 教程 |
| --- | --- | --- |
| 文件访问与命令执行 | **工具** | [文件与命令](文件与命令) |
| 可复用工作流（技能） | **技能** | [技能](技能) |
| 外部 MCP 工具 | **MCP** | [MCP服务器](MCP服务器) |
| 子智能体与编排 | **智能体** | [子智能体](子智能体) |
| 记忆 / 待办 / 定时器 | **工具** | [记忆待办与定时器](记忆待办与定时器) |

## 6. 数据放在哪

应用级文件（配置、MCP、子智能体、`agents/` 定义、`backgrounds/`、`experiences/`、日志）在系统应用数据目录：

| 平台 | 路径 |
| --- | --- |
| Windows | `%APPDATA%\com.leonard.aichat\`（Roaming） |
| macOS | `~/Library/Application Support/com.leonard.aichat/` |
| Linux | `~/.local/share/com.leonard.aichat/` |

工作区级文件在 `<workspace>/.data/`：

```text
<workspace>/.data/
  chat.db                 会话 / 请求监控 / 交互日志 / 智能体任务
  memory/{user,repo}/…    记忆文件
  todos/<session>/…       待办清单
  skills/<name>/skill.md  技能
  skills.status           本工作区的技能启用状态
  timers.json             本工作区待触发的延时定时器
  scripts/                run_cmd 脚本库
  tmp/                    输入框粘贴 + PDF 提及缓存
  deliver/<session>/…     子智能体交付物，每个批次一个目录
```

## 7. 下一步

- 让模型读写代码：先看 [工具总览](工具总览)，再开启**文件操作**。
- 跑构建/测试：见 [文件与命令](文件与命令) 里的 `run_cmd` / `run_shell`。
- 让它按固定流程办事：写一个[技能](技能)。
- 接入外部系统：配置 [MCP服务器](MCP服务器)。
- 并行处理大任务：配置[子智能体](子智能体)。

## 快速排错

| 现象 | 检查 |
| --- | --- |
| `401 Unauthorized` | API 密钥是否正确/过期；组织头是否需要 |
| `404 Not Found` | 基础地址的 `/v1` 多了或少了；是不是自己加了 `/chat/completions` |
| 模型列表为空 | 服务商可能没实现 `/models`；手动添加模型 |
| 首次「获取模型」转圈很久 | 能力目录正在下载（约 7 MB，之后缓存 7 天） |
| 附件被跳过 | 所选模型缺少视觉/文件/音频能力 |
| 回复慢或中断 | 服务商限流、代理或上下文长度；见 [监控与日志](监控与日志) |
