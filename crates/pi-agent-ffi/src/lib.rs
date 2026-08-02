//! C-ABI 层：把 pi-agent-core 暴露成一组 `extern "C"` 函数，供 WinUI3 桌面壳
//! 通过 P/Invoke 进程内直调（DllImport("pi_agent")）。
//!
//! 约定：
//! - 入参字符串是 UTF-8、NUL 结尾的 C 字符串。
//! - 返回的字符串由本库 malloc，调用方**必须**用 `pi_string_free` 释放。
//! - 句柄由 `pi_agent_create` 创建、`pi_agent_destroy` 销毁。

use std::ffi::{c_char, CStr, CString};
use std::ptr;

use pi_agent_core::{AgentLoop, AgentProvider, ChatMessage, EchoProvider, NoTools};

/// 不透明 agent 句柄。
pub struct PiAgent {
    provider: Box<dyn AgentProvider>,
    conversation: Vec<ChatMessage>,
}

fn to_cstring(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(c) => c.into_raw(),
        Err(_) => ptr::null_mut(),
    }
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

/// 创建一个 agent 句柄（当前用占位 echo 后端；真实 provider 后续填充）。
#[no_mangle]
pub extern "C" fn pi_agent_create() -> *mut PiAgent {
    let agent = PiAgent {
        provider: Box::new(EchoProvider),
        conversation: Vec::new(),
    };
    Box::into_raw(Box::new(agent))
}

/// 销毁句柄。
///
/// # Safety
/// `handle` 必须来自 `pi_agent_create` 且未被销毁过。
#[no_mangle]
pub unsafe extern "C" fn pi_agent_destroy(handle: *mut PiAgent) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

/// 跑一轮对话。返回本轮新增消息的 JSON 数组文本；出错返回 `{"error":...}`。
/// 调用方需 `pi_string_free` 释放返回值。
///
/// # Safety
/// `handle` 必须有效；`input_utf8` 必须是 NUL 结尾的 UTF-8 字符串。
#[no_mangle]
pub unsafe extern "C" fn pi_agent_send(
    handle: *mut PiAgent,
    input_utf8: *const c_char,
) -> *mut c_char {
    let Some(agent) = handle.as_mut() else {
        return to_cstring("{\"error\":\"空句柄\"}".to_string());
    };
    let Some(input) = cstr_to_str(input_utf8) else {
        return to_cstring("{\"error\":\"入参不是合法 UTF-8\"}".to_string());
    };

    let loop_ = AgentLoop::new(agent.provider.as_ref(), &NoTools);
    match loop_.run_turn(&mut agent.conversation, input) {
        Ok(added) => match serde_json::to_string(&added) {
            Ok(json) => to_cstring(json),
            Err(e) => to_cstring(format!("{{\"error\":\"序列化失败: {e}\"}}")),
        },
        Err(e) => to_cstring(format!("{{\"error\":\"{e}\"}}")),
    }
}

/// 返回内置组件目录的 JSON 数组（ffmpeg / whisper / 音高 / 人声分离 / 乐器 / 曲风 / 拍点 / Sound→MIDI）。
/// 调用方需 `pi_string_free`。
#[no_mangle]
pub extern "C" fn pi_components_json() -> *mut c_char {
    match serde_json::to_string(&pi_agent_core::default_catalog()) {
        Ok(json) => to_cstring(json),
        Err(e) => to_cstring(format!("{{\"error\":\"{e}\"}}")),
    }
}

/// 释放本库返回的字符串。
///
/// # Safety
/// `s` 必须是本库某个返回字符串的函数所返回的指针，且只释放一次。
#[no_mangle]
pub unsafe extern "C" fn pi_string_free(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}
