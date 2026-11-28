#pragma once

/// @file
/// The options of a port and an endpoint: typed tags in its declaration.
///
///     Latest<Status>  status{*this, "status", optionalTopic};
///     Arrivals<Imu>   imu{*this, "imu", perStep(64), advisory(!imuRequired)};
///     Arrivals<Twist> cmd{*this, "cmd", newestOnly};
///     Output<TF>      tf{*this, "tf_static", latched};
///     Output<Scan>    scan{*this, "scan", my::policies::lidar, maxPayloadDynamic(1 << 20)};
///
/// A tag a port kind does not accept, or one given twice, does not compile. `tag(bool)`
/// takes a flag from a run-time value. A `hypha::Qos` value replaces `QosOf<M>::value`.

#include "hypha/hypha_ffi.h"
#include "hypha/qos.h"
#include "hypha_base/time.h"

#include <concepts>
#include <cstddef>
#include <cstdint>
#include <optional>
#include <stdexcept>

namespace hypha {

/// @brief The port table may leave out the topic, or set `topic = ""`: the port is then
///        disabled. Without it an unset topic fails start().
struct OptionalTopic
{
    bool on = true;

    constexpr OptionalTopic operator()(bool value) const
    {
        return {value};
    }
};

/// @brief The input does not count for the health of the node: `~/state` and
///        Cycle::inputsOk() ignore its faults.
struct Advisory
{
    bool on = true;

    constexpr Advisory operator()(bool value) const
    {
        return {value};
    }
};

/// @brief The input runs without a silence limit: it is never stale (Input::untimed()). The
///        table must then not set `silence_ms`. It may be advisory too. A deployment that
///        chooses no limit sets `silence_ms = 0` in the table instead.
struct Unsupervised
{
    bool on = true;

    constexpr Unsupervised operator()(bool value) const
    {
        return {value};
    }
};

/// @brief Latest only: the data is stamped on another clock. Under simulated time a step
///        takes the newest sample whatever its stamp and judges `silence_ms` by host time.
struct HostClock
{
    bool on = true;

    constexpr HostClock operator()(bool value) const
    {
        return {value};
    }
};

/// @brief An output or publisher keeps its last message for late subscribers.
struct Latched
{
    bool on = true;

    constexpr Latched operator()(bool value) const
    {
        return {value};
    }
};

/// @brief An output for diagnostics: a send that finds the loan pool exhausted is
///        dropped and counted. Without it any refused send fails the run.
struct Diagnostic
{
    bool on = true;

    constexpr Diagnostic operator()(bool value) const
    {
        return {value};
    }
};

/// @brief Arrivals: the samples one step examines (default HYPHA_DEFAULT_ARRIVALS_PER_STEP).
///        Output: the messages one step may send (default 1). Zero throws
///        std::invalid_argument; an Arrivals perStep above the policy's `maxDepth` (32 for the
///        default policy) is refused when the port is constructed, the same way, and runNode()
///        exits 70. A count read from the parameter file is checked where it is read, with
///        constraints::range(1, maxDepth), to be refused as a ParameterError (exit 78).
struct PerStep
{
    uint32_t count;
};

/// @brief Arrivals only: ROS's `queue_size` 1. A step examines only the newest pending
///        sample; the older ones are dropped and counted as work-limit drops. A trigger steps
///        once on the newest of the samples due together, on host time every sample pending
///        when the node gets its turn, under simulated time the samples of one stamp, so a
///        replay steps the same on every run. The queue stays perStep() deep, nothing is lost
///        in transport, and the topic's policy is not changed (execution-model.md §6.6).
struct NewestOnly
{
    bool on = true;

    constexpr NewestOnly operator()(bool value) const
    {
        return {value};
    }
};

/// @brief The largest message of an output or publisher, in serialized bytes, for a
///        type whose size varies. A larger one is not sent (TooLarge). Zero is growable.
struct MaxPayloadDynamic
{
    uint64_t bytes;
};

/// @brief The largest value the port table may give `silence_ms` and `stagnation_ms`.
///        Without it one hour. A larger value fails start().
struct LimitMax
{
    Duration max;
};

inline constexpr OptionalTopic optionalTopic{};
inline constexpr Advisory      advisory{};
inline constexpr Unsupervised  unsupervised{};
inline constexpr HostClock     hostClock{};
inline constexpr Latched       latched{};
inline constexpr Diagnostic    diagnostic{};
inline constexpr NewestOnly    newestOnly{};

constexpr PerStep perStep(uint32_t count)
{
    return {count};
}

constexpr MaxPayloadDynamic maxPayloadDynamic(uint64_t bytes)
{
    return {bytes};
}

constexpr LimitMax limitMax(Duration max)
{
    return {max};
}

namespace detail {

template <typename T, typename... Allowed>
concept OneOf = (std::same_as<T, Allowed> || ...);

/// @brief True when no type appears twice in the pack.
template <typename... T>
inline constexpr bool distinctTags = true;

template <typename T, typename... Rest>
inline constexpr bool distinctTags<T, Rest...> =
    (!std::same_as<T, Rest> && ...) && distinctTags<Rest...>;

/// @brief A tag for a port whose topic code fixes or shares.
template <typename T>
concept FixedTopicOption = !std::same_as<T, OptionalTopic>;

/// @brief A tag for a port that reads the topic of another port: neither optionalTopic
///        nor a Qos.
template <typename T>
concept SharingOption = FixedTopicOption<T> && !std::same_as<T, Qos>;

/// @brief True when the pack holds the tag @p Tag.
template <typename Tag, typename... T>
inline constexpr bool hasTag = (std::same_as<Tag, T> || ...);

}  // namespace detail

/// @brief The tags a Latest input takes.
template <typename T>
concept LatestOption =
    detail::OneOf<T, OptionalTopic, Advisory, Unsupervised, LimitMax, HostClock, Qos>;

/// @brief The tags an Arrivals input takes.
template <typename T>
concept ArrivalsOption =
    detail::OneOf<T, OptionalTopic, Advisory, Unsupervised, LimitMax, PerStep, NewestOnly, Qos>;

/// @brief The tags an Output takes.
template <typename T>
concept OutputOption =
    detail::OneOf<T, OptionalTopic, Latched, Diagnostic, PerStep, MaxPayloadDynamic, Qos>;

/// @brief The tags Node::advertise() takes.
template <typename T>
concept PublisherOption = detail::OneOf<T, OptionalTopic, Latched, MaxPayloadDynamic, Qos>;

/// @brief The tags Node::subscribe() takes.
template <typename T>
concept SubscriberOption = detail::OneOf<T, OptionalTopic, Qos>;

namespace detail {

/// @brief The settings of an input port that its tags give.
struct InputSettings
{
    bool                    optionalTopic{false};
    bool                    countsForHealth{true};
    bool                    unsupervised{false};
    bool                    hostClock{false};
    bool                    newestOnly{false};
    std::optional<uint32_t> perStep{};
    Duration                limitMax{Duration::fromSeconds(3600)};
    std::optional<Qos>      qos{};

    void apply(Qos tag)
    {
        this->qos = tag;
    }

    void apply(OptionalTopic tag)
    {
        this->optionalTopic = tag.on;
    }

    void apply(Advisory tag)
    {
        this->countsForHealth = !tag.on;
    }

    void apply(Unsupervised tag)
    {
        this->unsupervised = tag.on;
    }

    void apply(HostClock tag)
    {
        this->hostClock = tag.on;
    }

    void apply(NewestOnly tag)
    {
        this->newestOnly = tag.on;
    }

    void apply(PerStep tag)
    {
        if (tag.count == 0) {
            throw std::invalid_argument("hypha: perStep() needs a positive count");
        }
        this->perStep = tag.count;
    }

    void apply(LimitMax tag)
    {
        if (tag.max.nanos() <= 0) {
            throw std::invalid_argument("hypha: limitMax() needs a positive duration");
        }
        this->limitMax = tag.max;
    }
};

/// @brief The settings of an output, a publisher or a subscriber that its tags give.
struct EndpointSettings
{
    bool                    optionalTopic{false};
    bool                    latch{false};
    bool                    diagnostic{false};
    uint32_t                perStep{1};
    std::optional<Qos>      qos{};
    std::optional<uint64_t> maxPayloadDynamic{};

    void apply(Qos tag)
    {
        this->qos = tag;
    }

    void apply(OptionalTopic tag)
    {
        this->optionalTopic = tag.on;
    }

    void apply(Latched tag)
    {
        this->latch = tag.on;
    }

    void apply(Diagnostic tag)
    {
        this->diagnostic = tag.on;
    }

    void apply(PerStep tag)
    {
        if (tag.count == 0) {
            throw std::invalid_argument("hypha: perStep() needs a positive count");
        }
        this->perStep = tag.count;
    }

    void apply(MaxPayloadDynamic tag)
    {
        this->maxPayloadDynamic = tag.bytes;
    }
};

/// @brief Fold @p tags into settings. A tag given twice does not compile.
template <typename Settings, typename... Tags>
Settings settingsOf(const Tags&... tags)
{
    static_assert(distinctTags<Tags...>, "hypha: a port option is given twice");
    Settings settings{};
    (settings.apply(tags), ...);
    return settings;
}

}  // namespace detail

}  // namespace hypha
