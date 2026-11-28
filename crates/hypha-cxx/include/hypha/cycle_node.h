#pragma once

/// @file
/// The cycle API: Cycle and its ports, and CycleNode, the node whose work is one step.

#include "hypha/application_node.h"
#include "hypha/config.h"
#include "hypha/cycle_arrivals.h"
#include "hypha/cycle_input.h"
#include "hypha/cycle_latest.h"
#include "hypha/cycle_output.h"
#include "hypha/cycle_runtime.h"
#include "hypha/cycle_service.h"
#include "hypha/node.h"
#include "hypha/options.h"
#include "hypha/sequence.h"

#include <expected>
#include <optional>
#include <string>
#include <utility>

namespace hypha {

/// @brief A node whose work is one step: its ports are members, step() is a method.
///
/// The cycle takes the node's name. Its rate is `rate_hz` of the `[cycle]` table, or
/// setRate() in setup(); or it steps on each sample of its setTrigger() ports.
///
///     class Fusion : public hypha::CycleNode {
///         Latest<Imu>     imu{*this, "imu"};  // [ports.inputs.imu] silence_ms = 50
///         Arrivals<Fix>   gnss{*this, "gnss", optionalTopic, perStep(16)};
///         Output<Pose>    pose{*this, "pose"};
///         void step(const hypha::CycleTime& time) override;
///     };
class CycleNode
    : public ApplicationNode
    , public Cycle
{
  public:
    CycleNode() = default;

    template <typename M>
    using Latest = hypha::Latest<M>;
    template <typename M>
    using Arrivals = hypha::Arrivals<M>;
    template <typename M>
    using Output = hypha::Output<M>;
    template <typename Srv>
    using Requests = hypha::Requests<Srv>;
    template <typename Srv>
    using Calls = hypha::Calls<Srv>;

    static constexpr OptionalTopic optionalTopic{};
    static constexpr Advisory      advisory{};
    static constexpr Unsupervised  unsupervised{};
    static constexpr HostClock     hostClock{};
    static constexpr Latched       latched{};
    static constexpr Diagnostic    diagnostic{};

    static constexpr PerStep perStep(uint32_t count)
    {
        return hypha::perStep(count);
    }

    static constexpr MaxPayloadDynamic maxPayloadDynamic(uint64_t bytes)
    {
        return hypha::maxPayloadDynamic(bytes);
    }

    static constexpr LimitMax limitMax(Duration max)
    {
        return hypha::limitMax(max);
    }

    virtual void step(const CycleTime& time) = 0;

    /// @brief Start the cycle with the rate of `[cycle] rate_hz` or setRate().
    /// @throws ConfigError when `rate_hz` is invalid (a ParameterError) or set with setRate().
    virtual std::expected<void, std::string> start(Node& node) final;

  private:
    /// A refused cycle configuration throws ConfigError; any other refusal stays a runtime
    /// failure.
    std::expected<void, std::string> commit(Node& node) final
    {
        const std::expected<void, std::string> started = this->start(node);
        const bool configurationRefused = !started && this->startRefusedByConfiguration();
        if (configurationRefused) {
            throw ConfigError(started.error());
        }
        return started;
    }
};

namespace detail {

struct NodeStep
{
    CycleNode* node;

    void operator()(const CycleTime& time) const
    {
        this->node->step(time);
    }
};

}  // namespace detail

inline std::expected<void, std::string> CycleNode::start(Node& node)
{
    const std::optional<double> rate =
        config("cycle").optional<double>("rate_hz", constraints::positive());
    if (rate.has_value()) {
        if (this->hasRate()) {
            throw ConfigError(
                "hypha: cycle node: the rate is set by setRate() and by rate_hz of [cycle]; "
                "keep one"
            );
        }
        this->setRate(Hz(*rate));
    }
    return Cycle::start(node, detail::NodeStep{this});
}

}  // namespace hypha
