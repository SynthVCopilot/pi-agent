#ifndef PI_AGENT_H
#define PI_AGENT_H

/*
 * pi_agent.dll public C ABI.
 *
 * Text arguments are UTF-8, NUL-terminated strings. Every non-NULL `char*`
 * returned by this library is allocated by the library and must be released
 * exactly once with pi_string_free. Opaque handles must be released with their
 * matching destroy function and must not be used concurrently.
 */

#ifdef _WIN32
#  ifdef PI_AGENT_BUILD
#    define PI_AGENT_API __declspec(dllexport)
#  else
#    define PI_AGENT_API __declspec(dllimport)
#  endif
#else
#  define PI_AGENT_API
#endif

#ifdef __cplusplus
extern "C" {
#endif

typedef struct PiAgent PiAgent;
typedef struct PiBridge PiBridge;
typedef struct PiJob PiJob;

/* Current exported ABI. */
PI_AGENT_API char* pi_agent_version(void);
PI_AGENT_API PiAgent* pi_agent_create(void);
PI_AGENT_API PiAgent* pi_agent_create_json(const char* config_json_utf8);
PI_AGENT_API char* pi_config_check(const char* config_json_utf8);
PI_AGENT_API void pi_agent_destroy(PiAgent* handle);
PI_AGENT_API char* pi_agent_send(PiAgent* handle, const char* input_utf8);
PI_AGENT_API char* pi_components_json(void);

PI_AGENT_API PiBridge* pi_bridge_connect(const char* bridge_repo_dir_utf8);
PI_AGENT_API char* pi_bridge_call(
    PiBridge* handle,
    const char* tool_utf8,
    const char* args_json_utf8);
PI_AGENT_API void pi_bridge_destroy(PiBridge* handle);
PI_AGENT_API char* pi_agent_send_with_bridge(
    PiAgent* agent_handle,
    PiBridge* bridge_handle,
    const char* input_utf8);

PI_AGENT_API void pi_string_free(char* string);

/* FFmpeg component and asynchronous job ABI.
 * Configuration is process-wide: config.json is used until a successful
 * pi_agent_create_json call supplies the process override.
 */
PI_AGENT_API char* pi_components_status_json(void);
PI_AGENT_API PiJob* pi_component_action_start(
    const char* component_id,
    const char* action);
PI_AGENT_API PiJob* pi_ffmpeg_job_start(const char* request_json);
PI_AGENT_API char* pi_job_status_json(PiJob* job);
PI_AGENT_API void pi_job_cancel(PiJob* job);
PI_AGENT_API void pi_job_destroy(PiJob* job);

#ifdef __cplusplus
}
#endif

#endif /* PI_AGENT_H */
