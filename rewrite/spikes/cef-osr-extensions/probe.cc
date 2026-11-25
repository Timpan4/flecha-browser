#include "include/capi/cef_app_capi.h"
#include "include/capi/cef_browser_capi.h"
#include "include/capi/cef_client_capi.h"
#include "include/capi/cef_devtools_message_observer_capi.h"

#include <atomic>
#include <cstdio>
#include <cstdlib>
#include <iostream>
#include <map>
#include <sstream>
#include <string>
#include <thread>

namespace {

// CEF handlers have static lifetime. Browser/context references are released
// after OnBeforeClose, before CefShutdown.
template <typename T>
T StaticHandler() {
  T handler{};
  handler.base.size = sizeof(T);
  handler.base.add_ref = [](cef_base_ref_counted_t*) {};
  handler.base.release = [](cef_base_ref_counted_t*) { return 0; };
  handler.base.has_one_ref = [](cef_base_ref_counted_t*) { return 0; };
  handler.base.has_at_least_one_ref = [](cef_base_ref_counted_t*) { return 1; };
  return handler;
}

void SetString(cef_string_t* field, const std::string& value) {
  cef_string_utf8_to_utf16(value.data(), value.size(), field);
}

std::map<std::string, cef_request_context_t*> contexts;
std::map<int, cef_browser_t*> browsers;
bool quitting = false;
std::thread commands;
std::string initial_url;
std::string profile_root;
bool chrome_style = false;
cef_client_t client = StaticHandler<cef_client_t>();
cef_registration_t* observer_registration = nullptr;

void Open(const std::string& context, const std::string& url) {
  if (!contexts.contains(context)) {
    cef_request_context_settings_t settings{};
    settings.size = sizeof(settings);
    if (context != "private")
      SetString(&settings.cache_path, profile_root + "/" + context);
    contexts.emplace(context, cef_request_context_create_context(&settings, nullptr));
    cef_string_clear(&settings.cache_path);
  }

  cef_window_info_t window{};
  window.size = sizeof(window);
  window.runtime_style = chrome_style ? CEF_RUNTIME_STYLE_CHROME : CEF_RUNTIME_STYLE_ALLOY;
  window.windowless_rendering_enabled = !chrome_style;
  cef_browser_settings_t settings{};
  settings.size = sizeof(settings);
  cef_string_t address{};
  SetString(&address, url);
  auto* request_context = contexts.at(context);
  request_context->base.base.add_ref(&request_context->base.base);
  const int queued = cef_browser_host_create_browser(
      &window, &client, &address, &settings, nullptr, request_context);
  cef_string_clear(&address);
  if (!queued) std::abort();
}

void RunCommand(const std::string& line) {
  if (line == "quit") {
    quitting = true;
    if (browsers.empty()) cef_quit_message_loop();
    const auto closing = browsers;
    for (const auto& [id, browser] : closing) {
      auto* host = browser->get_host(browser);
      host->close_browser(host, true);
      host->base.release(&host->base);
    }
    return;
  }
  std::istringstream input(line);
  std::string context, url;
  input >> context >> url;
  if (url.empty()) std::abort();
  Open(context, url);
}

struct Command : cef_task_t {
  explicit Command(std::string value) : line(std::move(value)) {
    base.size = sizeof(cef_task_t);
    base.add_ref = [](cef_base_ref_counted_t* self) { ++From(self)->refs; };
    base.release = [](cef_base_ref_counted_t* self) {
      auto* command = From(self);
      if (--command->refs != 0) return 0;
      delete command;
      return 1;
    };
    base.has_one_ref = [](cef_base_ref_counted_t* self) -> int { return From(self)->refs == 1; };
    base.has_at_least_one_ref = [](cef_base_ref_counted_t*) { return 1; };
    execute = [](cef_task_t* self) { RunCommand(static_cast<Command*>(self)->line); };
  }
  static Command* From(cef_base_ref_counted_t* self) {
    return static_cast<Command*>(reinterpret_cast<cef_task_t*>(self));
  }
  std::atomic<int> refs{1};
  std::string line;
};

}  // namespace

int main(int argc, char** argv) {
  const char* launcher = std::getenv("FLECHA_LAUNCHER");
  if (!launcher || std::string(launcher) != "isolated") return 1;
  const char* version = cef_api_hash(CEF_API_VERSION, 2);
  if (!version || std::string(version) != "8c29be240971eebee960ac92114d3efccb5780ff")
    return 1;
  cef_main_args_t args{argc, argv};
  auto app = StaticHandler<cef_app_t>();
  static auto process = StaticHandler<cef_browser_process_handler_t>();
  app.get_browser_process_handler = [](cef_app_t*) { return &process; };
  process.on_context_initialized = [](cef_browser_process_handler_t*) {
    contexts.emplace("global", cef_request_context_get_global_context());
    Open("global", initial_url);
    commands = std::thread([] {
      std::string line;
      while (std::getline(std::cin, line)) {
        // The CEF ABI consumes the task's reference.
        if (!cef_post_task(TID_UI, new Command(line))) std::abort();
        if (line == "quit") return;
      }
    });
  };

  int subprocess = cef_execute_process(&args, &app, nullptr);
  if (subprocess >= 0) return subprocess;
  const char* profile = std::getenv("PROBE_PROFILE");
  const char* url = std::getenv("PROBE_URL");
  if (!profile || !url) return 1;
  profile_root = profile;
  initial_url = url;
  chrome_style = std::getenv("PROBE_CHROME") != nullptr;
  static auto render = StaticHandler<cef_render_handler_t>();
  static auto life = StaticHandler<cef_life_span_handler_t>();
  static auto observer = StaticHandler<cef_dev_tools_message_observer_t>();
  client.get_render_handler = [](cef_client_t*) { return &render; };
  client.get_life_span_handler = [](cef_client_t*) { return &life; };
  render.get_view_rect = [](cef_render_handler_t*, cef_browser_t*, cef_rect_t* rect) {
    *rect = {0, 0, 640, 480};
  };
  render.on_paint = [](cef_render_handler_t*, cef_browser_t* browser, cef_paint_element_type_t type,
                       size_t, const cef_rect_t*, const void* buffer, int width, int height) {
    const auto* pixel = static_cast<const unsigned char*>(buffer);
    if (type == PET_VIEW && pixel[0] == 103 && pixel[1] == 71 && pixel[2] == 37) {
      std::printf("popup-painted %d %dx%d\n", browser->get_identifier(browser), width, height);
      std::fflush(stdout);
    }
    browser->base.release(&browser->base);
  };
  observer.on_dev_tools_message = [](cef_dev_tools_message_observer_t*, cef_browser_t* browser,
                                      const void* message, size_t size) {
    std::printf("cef-cdp %.*s\n", static_cast<int>(size), static_cast<const char*>(message));
    std::fflush(stdout);
    browser->base.release(&browser->base);
    return 1;
  };
  life.on_after_created = [](cef_life_span_handler_t*, cef_browser_t* browser) {
    browsers.emplace(browser->get_identifier(browser), browser);
    if (!observer_registration && std::getenv("PROBE_CEF_CDP")) {
      auto* host = browser->get_host(browser);
      observer_registration = host->add_dev_tools_message_observer(host, &observer);
      const std::string command = R"({"id":1,"method":"Extensions.loadUnpacked","params":{"path":"/mnt/visual/extension"}})";
      host->send_dev_tools_message(host, command.data(), command.size());
      host->base.release(&host->base);
    }
  };
  life.on_before_close = [](cef_life_span_handler_t*, cef_browser_t* browser) {
    const auto retained = browsers.extract(browser->get_identifier(browser));
    if (retained) retained.mapped()->base.release(&retained.mapped()->base);
    browser->base.release(&browser->base);
    if (quitting && browsers.empty()) cef_quit_message_loop();
  };

  cef_settings_t settings{};
  settings.size = sizeof(settings);
  SetString(&settings.root_cache_path, profile_root);
  SetString(&settings.cache_path, profile_root);
  SetString(&settings.resources_dir_path, "/mnt/cef");
  SetString(&settings.locales_dir_path, "/mnt/cef/locales");
  if (const char* policy = std::getenv("PROBE_POLICY")) SetString(&settings.chrome_policy_id, policy);
  settings.windowless_rendering_enabled = !chrome_style;
  if (!cef_initialize(&args, &settings, &app, nullptr)) return cef_get_exit_code();
  cef_string_clear(&settings.root_cache_path);
  cef_string_clear(&settings.cache_path);
  cef_string_clear(&settings.resources_dir_path);
  cef_string_clear(&settings.locales_dir_path);
  cef_string_clear(&settings.chrome_policy_id);
  cef_run_message_loop();
  commands.join();
  if (observer_registration) observer_registration->base.release(&observer_registration->base);
  for (const auto& [name, context] : contexts) context->base.base.release(&context->base.base);
  cef_shutdown();
}
