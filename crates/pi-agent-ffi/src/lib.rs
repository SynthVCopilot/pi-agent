//! C-ABI 层：把 pi-agent 暴露成一组 `extern "C"` 函数，供 WinUI3 桌面壳
//! 通过 P/Invoke 进程内直调（DllImport("pi_agent")）。
//!
//! 约定：
//! - 入参字符串是 UTF-8、NUL 结尾的 C 字符串。
//! - 返回的字符串由本库分配，调用方**必须**用 `pi_string_free` 释放。
//! - 句柄由对应 create/connect 创建、destroy 销毁，各自单线程使用
//!   （桌面壳串行调用；并发调用同一句柄是未定义行为）。

use std::collections::HashMap;
use std::ffi::{c_char, CStr, CString};
use std::ptr;

use pi_agent_core::{
    AgentLoop, AgentProvider, ChatMessage, EchoProvider, NoTools, PiError, ToolCall,
    ToolDefinition, ToolExecutor, ToolResult,
};
use pi_agent_mcp::SynthVBridge;
use pi_agent_provider::{AudioToolConfig, PiConfig};

/// 不透明 agent 句柄。
pub struct PiAgent {
    provider: Box<dyn AgentProvider>,
    conversation: Vec<ChatMessage>,
    /// pi-audio 组件配置（有则为 agent 增加 audio_* 工具）。
    audio: Option<AudioToolConfig>,
}

/// 不透明桥句柄：自带 tokio 运行时，同步阻塞地驱动异步 MCP 客户端。
pub struct PiBridge {
    runtime: tokio::runtime::Runtime,
    bridge: SynthVBridge,
}

fn to_cstring(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(c) => c.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

fn err_json(msg: &str) -> *mut c_char {
    to_cstring(
        serde_json::json!({ "error": msg }).to_string(),
    )
}

unsafe fn cstr_to_str<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    CStr::from_ptr(p).to_str().ok()
}

/// 返回库版本。调用方需 `pi_string_free`。
#[no_mangle]
pub extern "C" fn pi_agent_version() -> *mut c_char {
    to_cstring(env!("CARGO_PKG_VERSION").to_string())
}

/// 创建一个占位 echo 后端的 agent 句柄（无配置时的默认）。
#[no_mangle]
pub extern "C" fn pi_agent_create() -> *mut PiAgent {
    Box::into_raw(Box::new(PiAgent {
        provider: Box::new(EchoProvider),
        conversation: Vec::new(),
        audio: None,
    }))
}

/// 按 JSON 配置创建 agent 句柄。配置形如：
/// `{"provider":"anthropic","anthropic":{"base_url":"…","auth_token":"…","model":"…"}}`。
/// 失败返回 NULL（详细原因可先用 `pi_config_check` 校验）。
///
/// # Safety
/// `config_json_utf8` 必须是 NUL 结尾的 UTF-8 字符串。
#[no_mangle]
pub unsafe extern "C" fn pi_agent_create_json(config_json_utf8: *const c_char) -> *mut PiAgent {
    let Some(text) = cstr_to_str(config_json_utf8) else {
        return ptr::null_mut();
    };
    let Ok(config) = PiConfig::from_json(text) else {
        return ptr::null_mut();
    };
    let Ok(provider) = config.build_provider() else {
        return ptr::null_mut();
    };
    Box::into_raw(Box::new(PiAgent {
        provider,
        conversation: Vec::new(),
        audio: config.audio.clone(),
    }))
}

/// 校验配置 JSON，返回 `{"ok":true,"provider":"…"}` 或 `{"error":"…"}`。
///
/// # Safety
/// `config_json_utf8` 必须是 NUL 结尾的 UTF-8 字符串。
#[no_mangle]
pub unsafe extern "C" fn pi_config_check(config_json_utf8: *const c_char) -> *mut c_char {
    let Some(text) = cstr_to_str(config_json_utf8) else {
        return err_json("入参不是合法 UTF-8");
    };
    match PiConfig::from_json(text) {
        Ok(config) => match config.build_provider() {
            Ok(p) => to_cstring(
                serde_json::json!({ "ok": true, "provider": p.id() }).to_string(),
            ),
            Err(e) => err_json(&e.to_string()),
        },
        Err(e) => err_json(&e.to_string()),
    }
}

/// 销毁 agent 句柄。
///
/// # Safety
/// `handle` 必须来自 create 且未被销毁过。
#[no_mangle]
pub unsafe extern "C" fn pi_agent_destroy(handle: *mut PiAgent) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

/// 跑一轮对话（阻塞直至本轮完成）。返回本轮新增消息的 JSON 数组文本；
/// 出错返回 `{"error":…}`。调用方需 `pi_string_free`。
///
/// # Safety
/// `handle` 必须有效；`input_utf8` 必须是 NUL 结尾的 UTF-8 字符串。
#[no_mangle]
pub unsafe extern "C" fn pi_agent_send(
    handle: *mut PiAgent,
    input_utf8: *const c_char,
) -> *mut c_char {
    let Some(agent) = handle.as_mut() else {
        return err_json("空句柄");
    };
    let Some(input) = cstr_to_str(input_utf8) else {
        return err_json("入参不是合法 UTF-8");
    };

    let loop_ = AgentLoop::new(agent.provider.as_ref(), &NoTools);
    match loop_.run_turn(&mut agent.conversation, input) {
        Ok(added) => match serde_json::to_string(&added) {
            Ok(json) => to_cstring(json),
            Err(e) => err_json(&format!("序列化失败: {e}")),
        },
        Err(e) => err_json(&e.to_string()),
    }
}

/// 返回内置组件目录 JSON。调用方需 `pi_string_free`。
#[no_mangle]
pub extern "C" fn pi_components_json() -> *mut c_char {
    match serde_json::to_string(&pi_agent_core::default_catalog()) {
        Ok(json) => to_cstring(json),
        Err(e) => err_json(&e.to_string()),
    }
}

/// 连接 synthv-agent-bridge：拉起 `node dist/src/cli.js`（工作目录 = 桥仓库根）
/// 并完成 MCP 握手。失败返回 NULL。
///
/// # Safety
/// `bridge_repo_dir_utf8` 必须是 NUL 结尾的 UTF-8 路径。
#[no_mangle]
pub unsafe extern "C" fn pi_bridge_connect(bridge_repo_dir_utf8: *const c_char) -> *mut PiBridge {
    let Some(dir) = cstr_to_str(bridge_repo_dir_utf8) else {
        return ptr::null_mut();
    };
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    else {
        return ptr::null_mut();
    };
    let connected = runtime.block_on(SynthVBridge::connect(dir, HashMap::new()));
    match connected {
        Ok(bridge) => Box::into_raw(Box::new(PiBridge { runtime, bridge })),
        Err(_) => ptr::null_mut(),
    }
}

/// 调桥的一个工具（如 `sv_status`），args 为 JSON 对象文本（可传 `{}`）。
/// 返回工具结果 JSON；出错返回 `{"error":…}`。调用方需 `pi_string_free`。
///
/// # Safety
/// `handle` 必须有效；两个字符串参数必须是 NUL 结尾的 UTF-8。
#[no_mangle]
pub unsafe extern "C" fn pi_bridge_call(
    handle: *mut PiBridge,
    tool_utf8: *const c_char,
    args_json_utf8: *const c_char,
) -> *mut c_char {
    let Some(b) = handle.as_ref() else {
        return err_json("空句柄");
    };
    let (Some(tool), Some(args_text)) = (cstr_to_str(tool_utf8), cstr_to_str(args_json_utf8))
    else {
        return err_json("入参不是合法 UTF-8");
    };
    let args: serde_json::Value = match serde_json::from_str(args_text) {
        Ok(v) => v,
        Err(e) => return err_json(&format!("args 不是合法 JSON: {e}")),
    };
    match b.runtime.block_on(b.bridge.call(tool, args)) {
        Ok(result) => to_cstring(result.to_string()),
        Err(e) => err_json(&e.to_string()),
    }
}

/// 断开桥并销毁句柄（连带 kill 掉 node 子进程）。
///
/// # Safety
/// `handle` 必须来自 `pi_bridge_connect` 且未被销毁过。
#[no_mangle]
pub unsafe extern "C" fn pi_bridge_destroy(handle: *mut PiBridge) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

/// 把六个 SynthV v3 工具暴露给模型、并把调用路由到桥的执行器。
struct BridgeTools<'a> {
    bridge: &'a PiBridge,
}

impl ToolExecutor for BridgeTools<'_> {
    fn tools(&self) -> Vec<ToolDefinition> {
        const OPEN_SCHEMA: &str = r#"{"type":"object","additionalProperties":true}"#;
        [
            ("sv_status", "读取 SynthV 桥连接、Session、能力状态。无参。"),
            ("sv_describe", "列出动作或返回单个 Query/Command/UI/Review 动作的紧凑 schema。"),
            ("sv_query", "只读投影；contextMode:\"writeIntent\" 才能为后续写铸出 Context。"),
            ("sv_command", "校验过的 edit/delete/clone/import/批处理写入。"),
            ("sv_ui", "选区、视口、剪贴板、对话框、吸附、坐标、播放控制。"),
            ("sv_review", "发布/查看侧栏预览，用户在 SynthV 内 Apply/Dismiss。"),
        ]
        .into_iter()
        .map(|(name, desc)| ToolDefinition {
            name: name.to_string(),
            description: desc.to_string(),
            input_schema_json: OPEN_SCHEMA.to_string(),
        })
        .collect()
    }

    fn execute(&self, call: &ToolCall) -> Result<ToolResult, PiError> {
        let args: serde_json::Value =
            serde_json::from_str(&call.arguments_json).unwrap_or(serde_json::json!({}));
        match self
            .bridge
            .runtime
            .block_on(self.bridge.bridge.call(&call.tool_name, args))
        {
            Ok(result) => {
                // MCP tools/call 返回 {content:[{type:"text",text}...], isError}；
                // 给模型喂拼接后的文本，减少无谓包装。
                let text = result
                    .get("content")
                    .and_then(|c| c.as_array())
                    .map(|blocks| {
                        blocks
                            .iter()
                            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| result.to_string());
                let is_error = result
                    .get("isError")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                Ok(ToolResult { tool_call_id: call.id.clone(), result_json: text, is_error })
            }
            Err(e) => Ok(ToolResult {
                tool_call_id: call.id.clone(),
                result_json: format!("{{\"error\":\"{e}\"}}"),
                is_error: true,
            }),
        }
    }
}

/// pi-audio 组件工具：audio_probe / audio_pair_diff，spawn 组件 venv 的 python 执行。
struct AudioTools<'a> {
    cfg: &'a AudioToolConfig,
}

impl AudioTools<'_> {
    fn run_cli(&self, cli_args: &[String]) -> Result<String, PiError> {
        let output = std::process::Command::new(&self.cfg.python)
            .arg(&self.cfg.script)
            .args(cli_args)
            .output()
            .map_err(|e| PiError::new(format!("启动 pi-audio 失败: {e}")))?;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stdout.is_empty() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail: String = stderr.chars().rev().take(400).collect::<String>()
                .chars().rev().collect();
            return Err(PiError::new(format!("pi-audio 无输出，stderr 尾部: {tail}")));
        }
        Ok(stdout)
    }
}

impl ToolExecutor for AudioTools<'_> {
    fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "audio_probe".into(),
                description: "音频探针：特征指纹(BPM/调/打击比/能量弧)；panns=true 加乐器构成、\
                              genre 倾向与有词/无词判别；notes=true 加音符统计(慢)。返回紧凑 JSON，\
                              风格命名由你(模型)结合事实完成。"
                    .into(),
                input_schema_json: r#"{"type":"object","properties":{"audio":{"type":"string","description":"音频文件绝对路径"},"panns":{"type":"boolean"},"notes":{"type":"boolean"}},"required":["audio"]}"#.into(),
            },
            ToolDefinition {
                name: "audio_pair_diff".into(),
                description: "有词/无词配对差分：提取人声贡献音符并单音化；midi 给出路径则导出单音\
                              人声轨（≤512 音符时可直接经 sv_command import_monophonic_score 进 SynthV）。"
                    .into(),
                input_schema_json: r#"{"type":"object","properties":{"vocal":{"type":"string","description":"有词版绝对路径"},"inst":{"type":"string","description":"无词版绝对路径"},"midi":{"type":"string","description":"可选：导出 MIDI 的绝对路径"}},"required":["vocal","inst"]}"#.into(),
            },
        ]
    }

    fn execute(&self, call: &ToolCall) -> Result<ToolResult, PiError> {
        let args: serde_json::Value =
            serde_json::from_str(&call.arguments_json).unwrap_or(serde_json::json!({}));
        let get = |k: &str| args.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let cli: Vec<String> = match call.tool_name.as_str() {
            "audio_probe" => {
                let Some(audio) = get("audio") else {
                    return Ok(ToolResult {
                        tool_call_id: call.id.clone(),
                        result_json: "{\"error\":\"缺少 audio 参数\"}".into(),
                        is_error: true,
                    });
                };
                let mut v = vec!["probe".to_string(), audio];
                if args.get("panns").and_then(|b| b.as_bool()).unwrap_or(false) {
                    v.push("--panns".into());
                }
                if args.get("notes").and_then(|b| b.as_bool()).unwrap_or(false) {
                    v.push("--notes".into());
                }
                v
            }
            "audio_pair_diff" => {
                let (Some(vocal), Some(inst)) = (get("vocal"), get("inst")) else {
                    return Ok(ToolResult {
                        tool_call_id: call.id.clone(),
                        result_json: "{\"error\":\"缺少 vocal/inst 参数\"}".into(),
                        is_error: true,
                    });
                };
                let mut v = vec!["pair-diff".to_string(), vocal, inst];
                if let Some(midi) = get("midi") {
                    // 模型可控参数：只取文件名主干、强制 .mid、经 safe_join 圈定在
                    // ~/.SynthVcopilot/output 下（统一数据根 + 禁止 .. 穿透）。
                    let stem = std::path::Path::new(&midi)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("vocal-mono")
                        .to_string();
                    let out_dir = pi_agent_core::output_dir();
                    let _ = std::fs::create_dir_all(&out_dir);
                    match pi_agent_core::safe_join(&out_dir, &format!("{stem}.mid")) {
                        Ok(safe) => {
                            v.push("--midi".into());
                            v.push(safe.to_string_lossy().into_owned());
                        }
                        Err(e) => {
                            return Ok(ToolResult {
                                tool_call_id: call.id.clone(),
                                result_json: format!("{{\"error\":\"midi 路径被拒绝: {e}\"}}"),
                                is_error: true,
                            })
                        }
                    }
                }
                v
            }
            other => {
                return Ok(ToolResult {
                    tool_call_id: call.id.clone(),
                    result_json: format!("{{\"error\":\"未知音频工具 {other}\"}}"),
                    is_error: true,
                })
            }
        };
        match self.run_cli(&cli) {
            Ok(json) => Ok(ToolResult {
                tool_call_id: call.id.clone(),
                result_json: json.clone(),
                is_error: json.contains("\"error\""),
            }),
            Err(e) => Ok(ToolResult {
                tool_call_id: call.id.clone(),
                result_json: format!("{{\"error\":\"{e}\"}}"),
                is_error: true,
            }),
        }
    }
}

/// 组合执行器：按工具名路由到桥工具或音频工具。
struct CompositeTools<'a> {
    parts: Vec<&'a dyn ToolExecutor>,
}

impl ToolExecutor for CompositeTools<'_> {
    fn tools(&self) -> Vec<ToolDefinition> {
        self.parts.iter().flat_map(|p| p.tools()).collect()
    }
    fn execute(&self, call: &ToolCall) -> Result<ToolResult, PiError> {
        for p in &self.parts {
            if p.tools().iter().any(|t| t.name == call.tool_name) {
                return p.execute(call);
            }
        }
        Ok(ToolResult {
            tool_call_id: call.id.clone(),
            result_json: format!("{{\"error\":\"没有名为 {} 的工具\"}}", call.tool_name),
            is_error: true,
        })
    }
}

/// 跑一轮对话；工具面 = SynthV 桥六工具(bridge 非 NULL 时) + pi-audio 工具(配置了 audio 时)。
/// 返回本轮新增消息 JSON 数组；出错返回 `{"error":…}`。调用方需 `pi_string_free`。
///
/// # Safety
/// 两个句柄各自必须有效或为 NULL；`input_utf8` 必须是 NUL 结尾的 UTF-8。
#[no_mangle]
pub unsafe extern "C" fn pi_agent_send_with_bridge(
    agent_handle: *mut PiAgent,
    bridge_handle: *mut PiBridge,
    input_utf8: *const c_char,
) -> *mut c_char {
    let Some(agent) = agent_handle.as_mut() else {
        return err_json("空 agent 句柄");
    };
    let Some(input) = cstr_to_str(input_utf8) else {
        return err_json("入参不是合法 UTF-8");
    };

    let bridge_tools = bridge_handle.as_ref().map(|bridge| BridgeTools { bridge });
    let audio_cfg = agent.audio.clone();
    let audio_tools = audio_cfg.as_ref().map(|cfg| AudioTools { cfg });
    let mut parts: Vec<&dyn ToolExecutor> = Vec::new();
    if let Some(b) = bridge_tools.as_ref() {
        parts.push(b);
    }
    if let Some(a) = audio_tools.as_ref() {
        parts.push(a);
    }

    let result = if parts.is_empty() {
        AgentLoop::new(agent.provider.as_ref(), &NoTools)
            .run_turn(&mut agent.conversation, input)
    } else {
        let composite = CompositeTools { parts };
        AgentLoop::new(agent.provider.as_ref(), &composite)
            .run_turn(&mut agent.conversation, input)
    };
    match result {
        Ok(added) => match serde_json::to_string(&added) {
            Ok(json) => to_cstring(json),
            Err(e) => err_json(&format!("序列化失败: {e}")),
        },
        Err(e) => err_json(&e.to_string()),
    }
}

/// 释放本库返回的字符串。
///
/// # Safety
/// `s` 必须是本库返回的指针，且只释放一次。
#[no_mangle]
pub unsafe extern "C" fn pi_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}
