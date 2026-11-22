#pragma once

/// @file
/// The rclcpp subset of the Autoware port, on the hypha C++ SDK. The seed of hypha_rclcpp
/// (#59); README.md of examples/autoware lists what it maps and what it refuses.
///
/// - One `rclcpp::Node` holds one `hypha::Node`. Several share the process session and
///   its one parameter file (`--params-file`, or the first program argument). A CamelCase
///   node name becomes snake_case for hypha; get_name() keeps the ROS name.
/// - A relative topic name is a port: `output` reads `[ports.outputs.output]`,
///   `~/crop_box_polygon` reads `[ports.outputs.crop_box_polygon]`, `a/b` reads
///   `[ports.outputs."a.b"]`. An absolute name (`/tf`) is a literal topic.
/// - `declare_parameter` reads the parameter file once (`hypha::config()`); parameters
///   never change, so a set-parameters callback is kept and never called.
/// - Callbacks run on the thread of `rclcpp::spin`, as with a single-threaded executor.

#include <hypha/hypha.h>
#include <hypha/message_stamp.h>
#include <hypha_log/hypha_log.h>

#include <builtin_interfaces/msg/time.hpp>

#include <chrono>
#include <cstdint>
#include <functional>
#include <map>
#include <memory>
#include <optional>
#include <sstream>
#include <stdexcept>
#include <string>
#include <type_traits>
#include <utility>
#include <variant>
#include <vector>

/// rcl's time point: nanoseconds.
using rcl_time_point_value_t = int64_t;

namespace rclcpp {

// ---------------------------------------------------------------------------------------
// Time

/// The clock kinds of rclcpp. hypha has one framework clock, simulated or not; a Time
/// keeps the kind it was given and means framework time either way.
enum class ClockType { RCL_ROS_TIME, RCL_SYSTEM_TIME, RCL_STEADY_TIME };
using rcl_clock_type_t = ClockType;
inline constexpr ClockType RCL_ROS_TIME = ClockType::RCL_ROS_TIME;

class Duration
{
  public:
    Duration() = default;

    Duration(int32_t seconds, uint32_t nanoseconds)
        : nanos_(static_cast<int64_t>(seconds) * 1000000000 + static_cast<int64_t>(nanoseconds))
    {
    }

    template <class Rep, class Period>
    Duration(std::chrono::duration<Rep, Period> value)  // NOLINT: implicit, as in rclcpp
        : nanos_(std::chrono::duration_cast<std::chrono::nanoseconds>(value).count())
    {
    }

    static Duration from_seconds(double seconds)
    {
        return from_nanoseconds(hypha::Duration::fromSeconds(seconds).nanos());
    }

    static Duration from_nanoseconds(int64_t nanoseconds)
    {
        Duration d;
        d.nanos_ = nanoseconds;
        return d;
    }

    int64_t nanoseconds() const
    {
        return this->nanos_;
    }

    double seconds() const
    {
        return static_cast<double>(this->nanos_) * 1e-9;
    }

    hypha::Duration toHypha() const
    {
        return hypha::Duration::fromNanos(this->nanos_);
    }

    auto operator<=>(const Duration&) const = default;

  private:
    int64_t nanos_{0};
};

/// A point in framework time (simulated under HYPHA_USE_SIM_TIME), in nanoseconds.
class Time
{
  public:
    Time() = default;

    explicit Time(int64_t nanoseconds, ClockType clockType = ClockType::RCL_ROS_TIME)
        : nanos_(nanoseconds), clockType_(clockType)
    {
    }

    Time(int32_t seconds, uint32_t nanoseconds)
        : nanos_(static_cast<int64_t>(seconds) * 1000000000 + static_cast<int64_t>(nanoseconds))
    {
    }

    /// From a message stamp. @throws std::invalid_argument for nsecs of one second or more.
    Time(const builtin_interfaces::msg::Time& stamp)  // NOLINT: implicit, as in rclcpp
    {
        const std::optional<hypha::Time> value = hypha::fromRosTime(stamp);
        if (!value.has_value()) {
            throw std::invalid_argument("rclcpp::Time: a stamp with nsecs of one second or more");
        }
        this->nanos_ = value->nanos();
    }

    /// @throws std::out_of_range for a time before the epoch or after 2106.
    operator builtin_interfaces::msg::Time() const  // NOLINT: implicit, as in rclcpp
    {
        return hypha::toRosTime(hypha::Time::fromNanos(this->nanos_));
    }

    int64_t nanoseconds() const
    {
        return this->nanos_;
    }

    double seconds() const
    {
        return static_cast<double>(this->nanos_) * 1e-9;
    }

    ClockType get_clock_type() const
    {
        return this->clockType_;
    }

    hypha::Time toHypha() const
    {
        return hypha::Time::fromNanos(this->nanos_);
    }

    Duration operator-(const Time& other) const
    {
        return Duration::from_nanoseconds(this->nanos_ - other.nanos_);
    }

    Time operator-(const Duration& d) const
    {
        return Time(this->nanos_ - d.nanoseconds());
    }

    Time operator+(const Duration& d) const
    {
        return Time(this->nanos_ + d.nanoseconds());
    }

    bool operator==(const Time& other) const
    {
        return this->nanos_ == other.nanos_;
    }

    auto operator<=>(const Time& other) const
    {
        return this->nanos_ <=> other.nanos_;
    }

  private:
    int64_t   nanos_{0};
    ClockType clockType_{ClockType::RCL_ROS_TIME};
};

/// The framework clock: hypha::now(), simulated under HYPHA_USE_SIM_TIME.
class Clock
{
  public:
    using SharedPtr = std::shared_ptr<Clock>;

    Time now() const
    {
        return Time(hypha::now().nanos());
    }
};

// ---------------------------------------------------------------------------------------
// Logging

class Logger
{
  public:
    explicit Logger(std::string name) : name_(std::move(name))
    {
    }

    const char* get_name() const
    {
        return this->name_.c_str();
    }

    Logger get_child(const std::string& suffix) const
    {
        return Logger(this->name_ + "." + suffix);
    }

  private:
    std::string name_;
};

inline Logger get_logger(const std::string& name)
{
    return Logger(name);
}

namespace detail {
/// printf-style formatting into a string.
std::string formatPrintf(const char* format, ...) __attribute__((format(printf, 1, 2)));

inline std::string formatPrintf(const std::string& text)
{
    return text;
}
}  // namespace detail

// ---------------------------------------------------------------------------------------
// QoS

enum class ReliabilityPolicy { Reliable, BestEffort };
enum class DurabilityPolicy { Volatile, TransientLocal };

/// The rclcpp QoS a node asks for. On hypha, a topic's queue is its policy
/// (`hypha::QosOf<M>`): a depth above that policy's maxDepth is refused. Transient local
/// is a latched publisher. Reliable and best effort are both served by the shared-memory
/// queue, which keeps the newest `maxDepth` samples.
class QoS
{
  public:
    QoS(size_t depth) : depth_(depth)  // NOLINT: implicit, as rclcpp's KeepLast
    {
    }

    QoS& keep_last(size_t depth)
    {
        this->depth_ = depth;
        return *this;
    }

    QoS& reliable()
    {
        this->reliability_ = ReliabilityPolicy::Reliable;
        return *this;
    }

    QoS& best_effort()
    {
        this->reliability_ = ReliabilityPolicy::BestEffort;
        return *this;
    }

    QoS& transient_local()
    {
        this->durability_ = DurabilityPolicy::TransientLocal;
        return *this;
    }

    QoS& durability_volatile()
    {
        this->durability_ = DurabilityPolicy::Volatile;
        return *this;
    }

    size_t depth() const
    {
        return this->depth_;
    }

    ReliabilityPolicy reliability() const
    {
        return this->reliability_;
    }

    DurabilityPolicy durability() const
    {
        return this->durability_;
    }

  private:
    size_t            depth_;
    ReliabilityPolicy reliability_{ReliabilityPolicy::Reliable};
    DurabilityPolicy  durability_{DurabilityPolicy::Volatile};
};

class SensorDataQoS : public QoS
{
  public:
    SensorDataQoS() : QoS(5)
    {
        this->best_effort();
    }
};

/// Accepted and without effect: no QoS parameter is declared, so nothing overrides one.
class QosOverridingOptions
{
  public:
    static QosOverridingOptions with_default_policies()
    {
        return {};
    }
};

struct PublisherOptions
{
    QosOverridingOptions qos_overriding_options{};
    /// Shim only: the port may be left unwired; a missing table disables the publisher,
    /// and publishing on it does nothing. The debug publishers of the port use it.
    bool hypha_optional_topic{false};
};

struct SubscriptionOptions
{
    QosOverridingOptions qos_overriding_options{};
};

// ---------------------------------------------------------------------------------------
// Parameters

/// A parameter of a set-parameters callback. The shim never calls such a callback.
class Parameter
{
  public:
    using Value = std::variant<bool, int64_t, double, std::string>;

    Parameter(std::string name, Value value) : name_(std::move(name)), value_(std::move(value))
    {
    }

    const std::string& get_name() const
    {
        return this->name_;
    }

    double as_double() const
    {
        return this->get_value<double>();
    }

    int64_t as_int() const
    {
        return this->get_value<int64_t>();
    }

    bool as_bool() const
    {
        return this->get_value<bool>();
    }

    std::string as_string() const
    {
        return this->get_value<std::string>();
    }

    template <class T>
    T get_value() const
    {
        if constexpr (std::is_same_v<T, bool> || std::is_same_v<T, std::string>) {
            return std::get<T>(this->value_);
        } else if constexpr (std::is_integral_v<T>) {
            return static_cast<T>(std::get<int64_t>(this->value_));
        } else {
            return static_cast<T>(std::get<double>(this->value_));
        }
    }

  private:
    std::string name_;
    Value       value_;
};

}  // namespace rclcpp

namespace rcl_interfaces::msg {
struct SetParametersResult
{
    bool        successful{false};
    std::string reason;
};
}  // namespace rcl_interfaces::msg

namespace rclcpp {

namespace node_interfaces {
struct OnSetParametersCallbackHandle
{
    using SharedPtr = std::shared_ptr<OnSetParametersCallbackHandle>;
    using Callback =
        std::function<rcl_interfaces::msg::SetParametersResult(const std::vector<Parameter>&)>;
    Callback callback;
};
}  // namespace node_interfaces

class NodeOptions
{
  public:
    /// ROS arguments; the shim reads `--params-file <file>` (a hypha TOML file) and
    /// refuses any other ROS argument.
    NodeOptions& arguments(const std::vector<std::string>& args)
    {
        this->arguments_ = args;
        return *this;
    }

    const std::vector<std::string>& arguments() const
    {
        return this->arguments_;
    }

  private:
    std::vector<std::string> arguments_;
};

namespace detail {

/// The port name of a relative ROS name: `~/` dropped, `/` becomes `.`.
std::string portName(const std::string& rosName);

/// The hypha name of a ROS node name: CamelCase words become snake_case
/// (`VoxelGridDownsampleFilter` is `voxel_grid_downsample_filter`).
/// @throws std::invalid_argument for a name hypha cannot hold.
std::string hyphaNodeName(const std::string& rosName);

/// The parameter file of this process: set once by init() or a node's
/// `--params-file`. @throws std::invalid_argument for a second, different file.
std::string resolveParameterFile(const NodeOptions& options);

/// Log an endpoint failure once per second per call site.
void reportPublishFailure(const std::string& topic, hypha::Status status);

template <class M>
size_t checkedDepth(const std::string& name, const QoS& qos)
{
    const uint32_t maxDepth = hypha::QosOf<M>::value.maxDepth;
    if (qos.depth() == 0 || qos.depth() > maxDepth) {
        throw std::invalid_argument(
            "rclcpp: '" + name + "': queue depth " + std::to_string(qos.depth()) + " is outside 1.."
            + std::to_string(maxDepth) + " of the hypha policy '"
            + std::string(hypha::QosOf<M>::value.name) + "' of its message type"
        );
    }
    return qos.depth();
}

}  // namespace detail

template <class M>
class Publisher
{
  public:
    using SharedPtr = std::shared_ptr<Publisher>;
    using ConstSharedPtr = std::shared_ptr<const Publisher>;

    explicit Publisher(hypha::Publisher<M> publisher) : publisher_(std::move(publisher))
    {
    }

    /// Publishes @p message. A failure (too large, pool exhausted, transport) is logged
    /// with its status and counted in failed_publish_count(); a disabled optional
    /// publisher does nothing.
    void publish(const M& message)
    {
        if (!this->publisher_.enabled()) {
            return;
        }
        if (!this->publisher_.publish(message)) {
            ++this->failed_;
            detail::reportPublishFailure(
                this->publisher_.topic(), this->publisher_.lastPublishStatus()
            );
        }
    }

    void publish(const std::unique_ptr<M>& message)
    {
        this->publish(*message);
    }

    /// The subscribers on the machine now. A count the transport cannot give reads as 1:
    /// work is skipped only for a counted zero. A disabled publisher has none.
    size_t get_subscription_count() const
    {
        if (!this->publisher_.enabled()) {
            return 0;
        }
        return this->publisher_.subscriberCount().value_or(1);
    }

    std::string get_topic_name() const
    {
        return this->publisher_.topic();
    }

    /// Shim only.
    uint64_t failed_publish_count() const
    {
        return this->failed_;
    }

  private:
    hypha::Publisher<M> publisher_;
    uint64_t            failed_{0};
};

template <class M>
class Subscription
{
  public:
    using SharedPtr = std::shared_ptr<Subscription>;

    explicit Subscription(hypha::Subscriber<M> subscriber) : subscriber_(std::move(subscriber))
    {
    }

    std::string get_topic_name() const
    {
        return this->subscriber_.topic();
    }

  private:
    hypha::Subscriber<M> subscriber_;
};

class Node : public std::enable_shared_from_this<Node>
{
  public:
    using SharedPtr = std::shared_ptr<Node>;
    using OnSetParametersCallbackHandle = node_interfaces::OnSetParametersCallbackHandle;

    explicit Node(const std::string& name, const NodeOptions& options = NodeOptions())
        : name_(name)
        , hyphaNode_(
              std::make_unique<hypha::Node>(
                  detail::hyphaNodeName(name), detail::resolveParameterFile(options)
              )
          )
        , clock_(std::make_shared<Clock>())
        , logger_(name)
    {
    }

    /// Only the root namespace: hypha topics are absolute or private, and a port's topic
    /// comes from the parameter file. @throws std::invalid_argument for another namespace.
    Node(const std::string& name, const std::string& namespace_, const NodeOptions& options)
        : Node(name, options)
    {
        if (!namespace_.empty() && namespace_ != "/") {
            throw std::invalid_argument(
                "rclcpp: node '" + name + "': namespace '" + namespace_
                + "' is not supported; set the topics in the parameter file"
            );
        }
    }

    virtual ~Node() = default;

    Node(const Node&) = delete;
    Node& operator=(const Node&) = delete;

    template <class... Args>
    static SharedPtr make_shared(Args&&... args)
    {
        return std::make_shared<Node>(std::forward<Args>(args)...);
    }

    const char* get_name() const
    {
        return this->name_.c_str();
    }

    const char* get_namespace() const
    {
        return "/";
    }

    Logger get_logger() const
    {
        return this->logger_;
    }

    Clock::SharedPtr get_clock() const
    {
        return this->clock_;
    }

    Time now() const
    {
        return this->clock_->now();
    }

    // -- parameters: read once from the parameter file --------------------------------

    /// The value of @p name, or @p defaultValue when the file does not hold it.
    /// @throws hypha::ParameterError for a value that does not read as T.
    template <class T>
    T declare_parameter(const std::string& name, const T& defaultValue)
    {
        return this->remember(name, hypha::config().optional<T>(name, defaultValue));
    }

    std::string declare_parameter(const std::string& name, const char* defaultValue)
    {
        return this->remember(
            name, hypha::config().optional<std::string>(name, std::string(defaultValue))
        );
    }

    template <class T>
    std::vector<T> declare_parameter(const std::string& name, const std::vector<T>& defaultValue)
    {
        return hypha::config().optionalArray<T>(name).value_or(defaultValue);
    }

    /// The value of @p name. @throws hypha::ParameterError when the file does not hold it.
    template <class T>
    T declare_parameter(const std::string& name)
    {
        if constexpr (requires { typename T::value_type; } && !std::is_same_v<T, std::string>) {
            return hypha::config().requiredArray<typename T::value_type>(name);
        } else {
            return this->remember(name, hypha::config().required<T>(name));
        }
    }

    /// The value a declare_parameter call read. A list parameter is not kept.
    /// @throws std::invalid_argument for a name no declare_parameter call read.
    Parameter get_parameter(const std::string& name) const
    {
        const auto found = this->parameters_.find(name);
        if (found == this->parameters_.end()) {
            throw std::invalid_argument("rclcpp: parameter '" + name + "' is not declared");
        }
        return Parameter(name, found->second);
    }

    /// Kept and never called: parameters do not change while a hypha node runs.
    OnSetParametersCallbackHandle::SharedPtr
    add_on_set_parameters_callback(OnSetParametersCallbackHandle::Callback callback)
    {
        auto handle = std::make_shared<OnSetParametersCallbackHandle>();
        handle->callback = std::move(callback);
        this->parameterCallbacks_.push_back(handle);
        return handle;
    }

    // -- endpoints ----------------------------------------------------------------------

    template <class M>
    typename Publisher<M>::SharedPtr
    create_publisher(const std::string& name, const QoS& qos, const PublisherOptions& options = {})
    {
        (void)detail::checkedDepth<M>(name, qos);
        const bool latched = qos.durability() == DurabilityPolicy::TransientLocal;
        return std::make_shared<Publisher<M>>(
            this->advertise<M>(name, latched, options.hypha_optional_topic)
        );
    }

    /// A callback takes `std::shared_ptr<const M>` (`M::ConstPtr`) or `const M&`.
    template <class M, class Callback>
    typename Subscription<M>::SharedPtr create_subscription(
        const std::string& name,
        const QoS&         qos,
        Callback&&         callback,
        const SubscriptionOptions& /*options*/ = {}
    )
    {
        (void)detail::checkedDepth<M>(name, qos);
        using Ptr = std::shared_ptr<const M>;
        std::function<void(const Ptr&)> adapted;
        if constexpr (std::is_invocable_v<Callback&, const Ptr&>) {
            adapted = [cb = std::forward<Callback>(callback)](const Ptr& message) mutable {
                cb(message);
            };
        } else {
            static_assert(
                std::is_invocable_v<Callback&, const M&>,
                "rclcpp shim: a subscription callback takes std::shared_ptr<const M> or const M&"
            );
            adapted = [cb = std::forward<Callback>(callback)](const Ptr& message) mutable {
                cb(*message);
            };
        }
        if (name.starts_with('/')) {
            return std::make_shared<Subscription<M>>(
                this->hyphaNode_->subscribe<M>(hypha::Topic{name}, std::move(adapted))
            );
        }
        return std::make_shared<Subscription<M>>(
            this->hyphaNode_->subscribe<M>(detail::portName(name), std::move(adapted))
        );
    }

    /// Shim only: the hypha node under this one, for the shim's TF and debug helpers.
    hypha::Node& hypha_node()
    {
        return *this->hyphaNode_;
    }

  private:
    template <class T>
    T remember(const std::string& name, T value)
    {
        if constexpr (std::is_same_v<T, bool> || std::is_same_v<T, std::string>) {
            this->parameters_.insert_or_assign(name, Parameter::Value(value));
        } else if constexpr (std::is_integral_v<T>) {
            this->parameters_.insert_or_assign(name, Parameter::Value(static_cast<int64_t>(value)));
        } else if constexpr (std::is_floating_point_v<T>) {
            this->parameters_.insert_or_assign(name, Parameter::Value(static_cast<double>(value)));
        }
        return value;
    }

    template <class M>
    hypha::Publisher<M> advertise(const std::string& name, bool latched, bool optional)
    {
        if (name.starts_with('/')) {
            const hypha::Topic topic{name};
            return latched ? this->hyphaNode_->advertise<M>(topic, hypha::latched)
                           : this->hyphaNode_->advertise<M>(topic);
        }
        const std::string port = detail::portName(name);
        if (latched && optional) {
            return this->hyphaNode_->advertise<M>(port, hypha::latched, hypha::optionalTopic);
        }
        if (latched) {
            return this->hyphaNode_->advertise<M>(port, hypha::latched);
        }
        if (optional) {
            return this->hyphaNode_->advertise<M>(port, hypha::optionalTopic);
        }
        return this->hyphaNode_->advertise<M>(port);
    }

    std::string                                           name_;
    std::unique_ptr<hypha::Node>                          hyphaNode_;
    Clock::SharedPtr                                      clock_;
    Logger                                                logger_;
    std::vector<OnSetParametersCallbackHandle::SharedPtr> parameterCallbacks_;
    std::map<std::string, Parameter::Value>               parameters_;
};

// ---------------------------------------------------------------------------------------
// Timers

/// An rclcpp timer on a hypha timer of its node: framework time (simulated under
/// HYPHA_USE_SIM_TIME), whatever clock is passed. It may be created and controlled from
/// a callback (hypha#90); on a simulated clock the node does not own, creating one inside
/// a callback is refused. A refused call throws, as rclcpp's do.
namespace detail {
inline void requireTimer(const std::expected<void, hypha::Status>& result, const char* call)
{
    if (!result.has_value()) {
        throw std::runtime_error(
            std::string("rclcpp: timer ") + call + ": " + std::string(result.error().name())
        );
    }
}
}  // namespace detail

class TimerBase
{
  public:
    using SharedPtr = std::shared_ptr<TimerBase>;

    explicit TimerBase(hypha::Timer timer) : timer_(std::move(timer))
    {
    }

    /// No call until reset().
    void cancel()
    {
        detail::requireTimer(this->timer_.cancel(), "cancel");
    }

    /// The next call one period from now; ends a cancel.
    void reset()
    {
        detail::requireTimer(this->timer_.reset(), "reset");
    }

    bool is_canceled() const
    {
        return this->state().canceled;
    }

    /// The time until the next call; `nanoseconds::max()` while canceled, as rclcpp.
    std::chrono::nanoseconds time_until_trigger() const
    {
        const hypha::TimerStatus status = this->state();
        if (!status.untilDue.has_value()) {
            return std::chrono::nanoseconds::max();
        }
        return std::chrono::nanoseconds(status.untilDue->nanos());
    }

  private:
    hypha::TimerStatus state() const
    {
        auto read = this->timer_.state();
        if (!read.has_value()) {
            throw std::runtime_error(
                std::string("rclcpp: timer state: ") + std::string(read.error().name())
            );
        }
        return *read;
    }

    hypha::Timer timer_;
};

/// `rclcpp::create_timer(node, clock, period, callback)`: a callback that takes no argument.
template <class NodePtr, class Rep, class Period, class Callback>
TimerBase::SharedPtr create_timer(
    const NodePtr& node,
    const Clock::SharedPtr& /*clock*/,
    std::chrono::duration<Rep, Period> period,
    Callback&&                         callback
)
{
    const auto nanos = std::chrono::duration_cast<std::chrono::nanoseconds>(period).count();
    return std::make_shared<TimerBase>(node->hypha_node().timer(
        hypha::Duration::fromNanos(nanos), std::function<void()>(std::forward<Callback>(callback))
    ));
}

template <class NodePtr, class Callback>
TimerBase::SharedPtr create_timer(
    const NodePtr& node, const Clock::SharedPtr& clock, const Duration& period, Callback&& callback
)
{
    return create_timer(
        node,
        clock,
        std::chrono::nanoseconds(period.nanoseconds()),
        std::forward<Callback>(callback)
    );
}

// ---------------------------------------------------------------------------------------
// Process

/// Records the parameter file: `--params-file <file>` or the first argument that is no
/// option. Other ROS arguments are refused.
void init(int argc, char const* const* argv);
bool ok();
void shutdown();
/// Runs every callback of the process on this thread until shutdown() or a signal.
void spin(const std::shared_ptr<Node>& node);
/// The outcome of the last spin(), for the exit code of the runner.
std::optional<hypha::RunResult> last_spin_result();

}  // namespace rclcpp

// ---------------------------------------------------------------------------------------
// Logging macros: printf-format, then hypha logging with the logger name in front.

#define HYPHA_RCLCPP_LOG(LEVEL, logger, ...)                                                       \
    do {                                                                                           \
        const std::string hyphaRclcppText = ::rclcpp::detail::formatPrintf(__VA_ARGS__);           \
        HYPHA_LOG_##LEVEL("[{}] {}", (logger).get_name(), hyphaRclcppText);                        \
    } while (false)

/// Throttled per call site in host time, not in the given clock's time.
#define HYPHA_RCLCPP_LOG_THROTTLE(LEVEL, logger, clock, ms, ...)                                   \
    do {                                                                                           \
        (void)(clock);                                                                             \
        const std::string hyphaRclcppText = ::rclcpp::detail::formatPrintf(__VA_ARGS__);           \
        HYPHA_LOG_##LEVEL##_THROTTLE(                                                              \
            static_cast<double>(ms) / 1000.0, "[{}] {}", (logger).get_name(), hyphaRclcppText      \
        );                                                                                         \
    } while (false)

#define RCLCPP_DEBUG(logger, ...) HYPHA_RCLCPP_LOG(DEBUG, logger, __VA_ARGS__)
#define RCLCPP_INFO(logger, ...) HYPHA_RCLCPP_LOG(INFO, logger, __VA_ARGS__)
#define RCLCPP_WARN(logger, ...) HYPHA_RCLCPP_LOG(WARN, logger, __VA_ARGS__)
#define RCLCPP_ERROR(logger, ...) HYPHA_RCLCPP_LOG(ERROR, logger, __VA_ARGS__)
#define RCLCPP_FATAL(logger, ...) HYPHA_RCLCPP_LOG(FATAL, logger, __VA_ARGS__)

/// Stream forms: `RCLCPP_INFO_STREAM(logger, "a " << b)`.
#define HYPHA_RCLCPP_LOG_STREAM(LEVEL, logger, expr)                                               \
    do {                                                                                           \
        std::ostringstream hyphaRclcppStream;                                                      \
        hyphaRclcppStream << expr;                                                                 \
        HYPHA_LOG_##LEVEL("[{}] {}", (logger).get_name(), hyphaRclcppStream.str());                \
    } while (false)

#define RCLCPP_DEBUG_STREAM(logger, expr) HYPHA_RCLCPP_LOG_STREAM(DEBUG, logger, expr)
#define RCLCPP_INFO_STREAM(logger, expr) HYPHA_RCLCPP_LOG_STREAM(INFO, logger, expr)
#define RCLCPP_WARN_STREAM(logger, expr) HYPHA_RCLCPP_LOG_STREAM(WARN, logger, expr)
#define RCLCPP_ERROR_STREAM(logger, expr) HYPHA_RCLCPP_LOG_STREAM(ERROR, logger, expr)

#define RCLCPP_DEBUG_THROTTLE(logger, clock, ms, ...)                                              \
    HYPHA_RCLCPP_LOG_THROTTLE(DEBUG, logger, clock, ms, __VA_ARGS__)
#define RCLCPP_INFO_THROTTLE(logger, clock, ms, ...)                                               \
    HYPHA_RCLCPP_LOG_THROTTLE(INFO, logger, clock, ms, __VA_ARGS__)
#define RCLCPP_WARN_THROTTLE(logger, clock, ms, ...)                                               \
    HYPHA_RCLCPP_LOG_THROTTLE(WARN, logger, clock, ms, __VA_ARGS__)
#define RCLCPP_ERROR_THROTTLE(logger, clock, ms, ...)                                              \
    HYPHA_RCLCPP_LOG_THROTTLE(ERROR, logger, clock, ms, __VA_ARGS__)

/// Throttled stream forms, throttled as the printf ones.
#define HYPHA_RCLCPP_LOG_STREAM_THROTTLE(LEVEL, logger, clock, ms, expr)                           \
    do {                                                                                           \
        (void)(clock);                                                                             \
        std::ostringstream hyphaRclcppStream;                                                      \
        hyphaRclcppStream << expr;                                                                 \
        HYPHA_LOG_##LEVEL##_THROTTLE(                                                              \
            static_cast<double>(ms) / 1000.0,                                                      \
            "[{}] {}",                                                                             \
            (logger).get_name(),                                                                   \
            hyphaRclcppStream.str()                                                                \
        );                                                                                         \
    } while (false)

#define RCLCPP_DEBUG_STREAM_THROTTLE(logger, clock, ms, expr)                                      \
    HYPHA_RCLCPP_LOG_STREAM_THROTTLE(DEBUG, logger, clock, ms, expr)
#define RCLCPP_INFO_STREAM_THROTTLE(logger, clock, ms, expr)                                       \
    HYPHA_RCLCPP_LOG_STREAM_THROTTLE(INFO, logger, clock, ms, expr)
#define RCLCPP_WARN_STREAM_THROTTLE(logger, clock, ms, expr)                                       \
    HYPHA_RCLCPP_LOG_STREAM_THROTTLE(WARN, logger, clock, ms, expr)
#define RCLCPP_ERROR_STREAM_THROTTLE(logger, clock, ms, expr)                                      \
    HYPHA_RCLCPP_LOG_STREAM_THROTTLE(ERROR, logger, clock, ms, expr)
