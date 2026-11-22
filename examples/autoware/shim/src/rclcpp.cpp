#include <rclcpp/rclcpp.hpp>
#include <rclcpp_components/register_node_macro.hpp>

#include <cstdarg>
#include <cstdio>
#include <mutex>
#include <stdexcept>
#include <string>
#include <vector>

HYPHA_LOG_MODULE_INIT("rclcpp")

namespace rclcpp {
namespace {

struct Context
{
    std::mutex                      mutex;
    std::optional<std::string>      parameterFile;
    std::optional<hypha::RunResult> lastSpin;
};

Context& context()
{
    static Context instance;
    return instance;
}

/// The parameter file named by ROS arguments: `--params-file <file>`, with or without
/// `--ros-args`; any other ROS argument is refused.
std::optional<std::string> parameterFileOf(const std::vector<std::string>& args)
{
    std::optional<std::string> file;
    for (size_t i = 0; i < args.size(); ++i) {
        const std::string& arg = args[i];
        if (arg == "--ros-args" || arg == "--") {
            continue;
        }
        if (arg == "--params-file") {
            if (i + 1 >= args.size()) {
                throw std::invalid_argument("rclcpp: --params-file without a file");
            }
            file = args[++i];
            continue;
        }
        throw std::invalid_argument(
            "rclcpp: the shim takes only --params-file <file.toml>, not '" + arg + "'"
        );
    }
    return file;
}

void recordParameterFile(const std::string& file)
{
    Context&                          ctx = context();
    const std::lock_guard<std::mutex> lock(ctx.mutex);
    if (ctx.parameterFile.has_value() && *ctx.parameterFile != file) {
        throw std::invalid_argument(
            "rclcpp: one process reads one parameter file; '" + file + "' after '"
            + *ctx.parameterFile + "'"
        );
    }
    ctx.parameterFile = file;
}

}  // namespace

namespace detail {

std::string formatPrintf(const char* format, ...)
{
    va_list args;
    va_start(args, format);
    va_list copy;
    va_copy(copy, args);
    const int length = std::vsnprintf(nullptr, 0, format, copy);
    va_end(copy);
    if (length < 0) {
        va_end(args);
        return std::string("(log format error) ") + format;
    }
    std::string text(static_cast<size_t>(length) + 1, '\0');
    (void)std::vsnprintf(text.data(), text.size(), format, args);
    va_end(args);
    text.resize(static_cast<size_t>(length));
    return text;
}

std::string portName(const std::string& rosName)
{
    std::string name = rosName.starts_with("~/") ? rosName.substr(2) : rosName;
    if (name.empty() || name.starts_with('/') || name.starts_with('~')) {
        throw std::invalid_argument("rclcpp: '" + rosName + "' names no port");
    }
    for (char& c : name) {
        if (c == '/') {
            c = '.';
        }
    }
    return name;
}

std::string hyphaNodeName(const std::string& rosName)
{
    std::string name;
    for (size_t i = 0; i < rosName.size(); ++i) {
        const char c = rosName[i];
        const bool upper = c >= 'A' && c <= 'Z';
        const bool lower = (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9');
        if (upper) {
            const bool wordStart = i > 0 && !name.empty() && name.back() != '_';
            if (wordStart) {
                name.push_back('_');
            }
            name.push_back(static_cast<char>(c - 'A' + 'a'));
        } else if (lower || c == '_') {
            name.push_back(c);
        } else {
            throw std::invalid_argument(
                "rclcpp: node name '" + rosName + "' has a character hypha does not take"
            );
        }
    }
    const bool startsWithLetter = !name.empty() && name.front() >= 'a' && name.front() <= 'z';
    if (!startsWithLetter || name.size() > 64) {
        throw std::invalid_argument("rclcpp: node name '" + rosName + "' names no hypha node");
    }
    return name;
}

std::string resolveParameterFile(const NodeOptions& options)
{
    const std::optional<std::string> fromOptions = parameterFileOf(options.arguments());
    if (fromOptions.has_value()) {
        recordParameterFile(*fromOptions);
        return *fromOptions;
    }
    Context&                          ctx = context();
    const std::lock_guard<std::mutex> lock(ctx.mutex);
    return ctx.parameterFile.value_or(std::string{});
}

void reportPublishFailure(const std::string& topic, hypha::Status status)
{
    HYPHA_LOG_ERROR_THROTTLE(1.0, "[rclcpp] publish on '{}' failed: {}", topic, status.name());
}

}  // namespace detail

void init(int argc, char const* const* argv)
{
    std::vector<std::string> args;
    for (int i = 1; i < argc; ++i) {
        args.emplace_back(argv[i]);
    }
    const bool positional = !args.empty() && !args.front().starts_with('-');
    if (positional) {
        if (args.size() > 1) {
            throw std::invalid_argument("rclcpp: one argument, the parameter file");
        }
        recordParameterFile(args.front());
        return;
    }
    const std::optional<std::string> file = parameterFileOf(args);
    if (file.has_value()) {
        recordParameterFile(*file);
    }
}

bool ok()
{
    return hypha::ok();
}

void shutdown()
{
    hypha::requestStop();
}

void spin(const std::shared_ptr<Node>& node)
{
    hypha::RunResult                  result = node->hypha_node().run();
    Context&                          ctx = context();
    const std::lock_guard<std::mutex> lock(ctx.mutex);
    ctx.lastSpin = std::move(result);
}

void spin_some(const std::shared_ptr<Node>& node)
{
    // A run always takes its first dispatch turn (docs/execution-model.md section 4), so
    // the shortest bounded run serves what is ready and returns.
    const hypha::RunResult result = node->hypha_node().runFor(hypha::Duration::fromNanos(1));
    if (!result.ok()) {
        throw std::runtime_error(
            std::string("rclcpp: spin_some: ") + std::string(result.status().name())
        );
    }
}

std::optional<hypha::RunResult> last_spin_result()
{
    Context&                          ctx = context();
    const std::lock_guard<std::mutex> lock(ctx.mutex);
    return ctx.lastSpin;
}

}  // namespace rclcpp

namespace rclcpp_components {

std::vector<Component>& registry()
{
    static std::vector<Component> components;
    return components;
}

}  // namespace rclcpp_components
