# 更新日志

## v0.2.2 — 操作界面与 RedLang 回执修复

- 默认界面落地五个工作页，账号与连接同页设置，私聊/群聊分开搜索和保存。保留红白绿及明暗主题，使用实际 React Bits 账号高光、状态卡、边框与计数；移除示意趋势和固定撤回进度。
- 队列显示真实发送与撤回结果，提交请求后保留任务，区分准备失败、待核对和撤回未全部成功。
- 缩小默认窗口与表单间距；白名单支持立即加入/移出，批量编辑折叠收纳，高级设置展示真实兼容能力。
- `record` 转为真正微信原生语音气泡，内嵌音频解码与 SILK 编码，无 FFmpeg/Python 运行依赖。目前支持微信4.2.0.12；单条60秒，长音频分段并共用输出回执。修正最终上传回包的结束标志判定。
- 兼容层统一同步与持久化模式的目标、CQ 与节点转译，保留 echo 和追踪字段，不静默丢弃节点前后的正文。
- 单次图文输出共用回执；撤回只处理此输出，禁止把两分钟标记或来源 ID 扩散到其他输出。延时从实际发送后计算。
- 音乐使用本次已解析音频；默认点击直接打开音频，可选原始歌曲页面。发送前匿名检查音频，拒绝登录或错误页面。此项不解除音源的授权限制，接收端播放需实际验证。
- 构建脚本同步前端到内嵌资源，防止源代码已更新而 EXE 仍含旧界面。用户运行 EXE 不需要 Node 或 pnpm。
- 详细动作支持与不支持范围见 redlang/COMPATIBILITY.md；不修改现有 QQ/TG 业务插件。

## 2026-11-04 — GUI 重构、通用 OneBot 兼容、音乐卡片修复

本次更新把 GUI 从旧的原生风格页面切换为 React + ReactBits 控制台，同时把此前针对电影功能的兼容逻辑收敛为通用 OneBot 指令语义。

### GUI / WebView2

- 新增 React + Vite 版控制台，使用 ReactBits 官方组件：
  - `MagicBento`：队列 / 已发送 / 失败 / 撤回窗口 KPI 网格
  - `Shredder`：队列碎纸交互，拖入碎纸口触发撤回
  - `SpotlightCard`：主面板、队列面板、日志面板
  - `StarBorder`：启动服务按钮
  - `CountUp`：送达数字滚动
- 红 / 白 / 绿配色，支持明暗双模式，默认跟随系统并记忆选择。
- 品牌 logo 使用 `nodesire7` 的 GitHub 头像。
- 新 GUI 作为默认首页 `/`；旧 GUI 保留在 `/classic`，完整配置页仍可用。
- 新增 GUI 后端接口：
  - `GET /api/jobs`：读取真实 outbox 任务（id、会话、群/私聊、状态、撤回状态、错误）
  - `POST /api/recall`：按 `message_id` 写入撤回请求，由服务端执行
- GUI 认证改为 HttpOnly Cookie：
  - 页面加载时下发 `gui_token`（`HttpOnly; SameSite=Strict`）
  - `/api/*` 支持 query token、`X-Token` 和 Cookie 三种方式
  - 修复刷新页面或直接打开 `/next` 后 token 丢失、所有接口 403、启动/停止看似无效的问题
- 前端职责收敛为开关与信息查看：启动/停止监听、微信/RedReply/服务状态、真实队列、日志、窗口控制；绑定与完整配置跳转 `/classic`。
- 前端源码位于 `design/gui-react/`，构建为单文件 HTML 后由 Rust 内嵌到 `assets/gui/next.html`。

### 通用 OneBot 兼容层

- 转发指令统一支持：
  - `send_private_forward_msg`
  - `send_group_forward_msg`
  - `send_forward_msg`（OneBot11 标准动作）
- 普通 `send_private_msg` / `send_group_msg` / `send_msg` 中出现 `type: "node"` 时，自动提升为对应合并转发。
- 群转发 `group_id` 为空但带显式 `user_id` 时，按私聊转发处理。
- 节点内容兼容：text / image / at / reply、字符串化 JSON 节点、CQ 码。
- 文本段内的 `[CQ:image,...]`、`[CQ:at,...]` 会展开为独立消息段，不再作为字面文字发送。
- 私聊 image + text 拆分为独立的图片 job 与文本 job，图片单独确认，避免整条输出被拖成 `uncertain`。
- 图片 job 保留 `reply` 段，继承触发批次，可随同批输出一起撤回。
- 撤回指令通用化：
  - `delete_msg`、`schedule_delete_msg`
  - 支持按输出 job id 撤回
  - 支持按触发消息 id / 批次 source 撤回整批输出
  - 文本、图片、合并卡、音乐卡使用同一套撤回路径
- 修复撤回执行器 `LIMIT 100` 窗口被已完成任务占满的问题：查询排除 `recalled/cancelled/recall_failed`，新任务不再被饿死。
- 移除电影专用兼容：
  - 不再按 `资源列表` + `magnet:` 文本猜测转发
  - 不再缓存“封面图”并命名为“电影资源”
  - 不再猜测“最近活跃批次”作为私聊目标

### 音乐卡片

- 原生 QQ 音乐卡片 `dataurl` 从 `http://` 升级为 `https://`，避免微信拒绝明文媒体。
- `<appinfo>` 从 `<appmsg>` 内部移到外部，与微信真实 QQ 音乐卡片结构一致。
- 播放页 `url` 改为官方微信分享页：`https://i.y.qq.com/v8/playsong.html?...&source=weixin`。
- 移除多余的 `lowurl` / `lowdataurl`。
- 已实测音频地址：`HTTP 200`、`audio/mpeg`、约 4.1 MB、支持 Range 206。
- `--music-self-test` 返回 `server_accepted: true`。

### 召回 / 发送链路

- 召回执行日志增加“撤回执行开始/结束”，便于定位卡住的任务。
- 触发级撤回兜底：当 `delete_msg` 传入的是触发消息 id 而不是输出 job id 时，按 `batches.source` 撤回该次触发的全部输出。
- 保持 120 秒可撤回窗口；超时任务明确失败，不再静默卡在 `requested`。

### 环境与现场操作（不在仓库内）

- 在 RedReply 使用的 Python 环境安装 `playwright 1.64.1`、`greenlet 3.5.6`、`pyee 14.0.0` 与 Chromium，修复 `查电影` 的 `@download` 因缺少 Playwright 直接抛异常、磁力/网盘信息不发送的问题。
- 对 `查电影` RedLang 包做过现场诊断与临时调整（下载依赖、返回流程、私聊转发参数），未纳入本仓库；原包与备份位于服务器 `C:\XLZPJ\Mirai\red\plus_dir\pkg_dir\查电影`。
- 远端部署构建：
  - GUI / 兼容层：`5FA649E9...`（音乐卡片结构修复）
  - 通用指令收敛：`0C7067C0...`
  - React GUI 首次接入：`743DCA10...`
  - Cookie 认证：`60E59A64...`

### 验证

- GUI `/` 返回新版页面（约 415 KB，含 `REACTBITS`）。
- Cookie 登录后：
  - `/api/status`：`ready=true`、`connected=true`、outbox `sent=245 / failed=9 / uncertain=20`
  - `/api/jobs`：60 条真实任务
  - `/api/state`：`binding=true`、`running=true`
- 音乐自检：`server_accepted: true`、`revoke_confirmed: true`。
- 服务状态：`running=true / ready=true / connected=true`。

### 已知限制

- 新 GUI 的配置/绑定页尚未迁移，仍通过 `/classic` 使用旧 GUI。
- 队列中的任务来自真实 outbox，但“碎纸撤回”后任务会先显示“等待撤回”，待服务端确认后才变为“已撤回”。
- 音乐卡片能否在微信内播放仍需在客户端点播验证；音频 URL 本身已验证可访问。
- GUI token 为每次启动随机生成，只通过 Cookie 与 WebView2 URL 传递，不写入配置文件。
