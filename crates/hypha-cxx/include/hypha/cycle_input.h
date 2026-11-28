#pragma once

/// @file
/// Input: the state that Latest and Arrivals share: source, settings and supervision.

#include "hypha/backend/cycle.h"
#include "hypha/backend/session.h"
#include "hypha/backend/typed.h"
#include "hypha/cycle.h"
#include "hypha/cycle_port.h"
#include "hypha/cycle_runtime.h"
#include "hypha/cycle_sharing.h"
#include "hypha/cycle_supervision.h"
#include "hypha/detail/report.h"
#include "hypha/options.h"
#include "hypha/qos.h"
#include "hypha/source.h"

#include <cstdint>
#include <expected>
#include <functional>
#include <memory>
#include <optional>
#include <stdexcept>
#include <string>
#include <type_traits>
#include <utility>

namespace hypha {

namespace detail {

/// @brief The alive value of a message: an integral member, read through a pointer to member.
template <typename M, typename Counter>
struct MemberCounter
{
    Counter M::* field;

    uint64_t operator()(const M& message) const
    {
        return static_cast<uint64_t>(message.*field);
    }
};

}  // namespace detail

/// @brief An input port of messages @p M: the base of Latest and Arrivals.
///
/// The port declares the service policy of its topic: its Qos option, or QosOf<M>::value.
/// A perStep() above the policy's maxDepth is refused when the port is constructed.
template <typename M>
class Input : public Port
{
  public:
    InputVerdict verdict() const
    {
        return this->verdict_;
    }

    /// @brief True when the input is within its limits in this step: verdict() is Usable.
    bool usable() const
    {
        return this->verdict_ == InputVerdict::Usable;
    }

    uint64_t received() const
    {
        return this->heldReport_.received_total;
    }

    uint64_t deliveredCount() const
    {
        return this->heldReport_.delivered_total;
    }

    /// @brief True when the newest sample was sent longer than `silence_ms` ago, or none
    ///        arrived for that long. False while setIgnored(true) hides the fault.
    bool stale() const
    {
        return this->hasFault(HYPHA_FAULT_STALE);
    }

    /// @brief True when the sender publishes but its life sign (setAlive()) has not
    ///        changed for `stagnation_ms`. False while setIgnored(true) hides the fault.
    bool stagnant() const
    {
        return this->hasFault(HYPHA_FAULT_STAGNANT);
    }

    /// @brief True when the port has no time limit: tagged unsupervised, or `silence_ms = 0`
    ///        in its table (a ROS timeout of 0.0). It is never stale: NeverArrived before its
    ///        first sample, Usable from it on, until a later sample replaces the value. False
    ///        for a port with a limit, a disabled port, and before start(); in a
    ///        testing::StepHarness it is read from the parameter file, as a start decides it.
    /// @throws std::logic_error in a StepHarness without a loaded configuration.
    /// @throws ParameterError in a StepHarness when the table's `silence_ms` is missing or
    ///         outside its range.
    bool untimed() const
    {
        if (!this->enabled()) {
            return false;
        }
        if (this->supervision_.decided()) {
            return this->supervision_.untimed();
        }
        if (!this->enabledByHarness()) {
            return false;
        }
        const bool noTable = this->supervision_.name().empty() || this->settings_.unsupervised;
        return noTable
            || detail::tableSilenceNanos(this->supervision_.name(), this->settings_.limitMax) == 0;
    }

    /// @brief True when setIgnored(true) hides the fault of this port.
    bool ignored() const
    {
        return this->currentView().supervision.ignored != 0;
    }

    /// @brief Hide or count the fault of this port. Call in step() only, on a supervised port.
    /// @throws std::logic_error outside step(), or when the port is not supervised.
    void setIgnored(bool ignore)
    {
        this->supervision_.setIgnored(this->index_, ignore);
    }

    /// @brief Forget, when the step returns, the value and the pending samples this step
    ///        admitted (`cleared_total`): later steps see later samples only, NeverArrived
    ///        until one comes, Stale once the last one before is older than `silence_ms`.
    /// @throws std::logic_error outside step(), or on a trigger port.
    void clear()
    {
        if (detail::CaptureStore* const harness = this->harnessStore()) {
            harness->clearAfterStep(*this, [](Port& port) {
                static_cast<Input<M>&>(port).forgetCleared();
            });
            return;
        }
        this->supervision_.clear(this->index_);
    }

    /// @brief Declare the stagnation check: an end-to-end life sign, an integral member of
    ///        the message that the original source advances. Call before start() only.
    template <typename Counter>
    void setAlive(Counter M::* field)
    {
        static_assert(std::is_integral_v<Counter>, "setAlive() needs an integral member");
        this->refuseAfterStart("setAlive()");
        this->alive_ = detail::MemberCounter<M, Counter>{field};
    }

    /// @brief Declare the stagnation check with a named functor that returns an integral value.
    ///        Call before start() only. The port keeps the callable: a lambda that captures
    ///        by reference dangles, so use a named functor.
    template <typename Fn>
    void setAlive(Fn alive)
    {
        static_assert(
            std::is_integral_v<std::invoke_result_t<const Fn&, const M&>>,
            "setAlive() needs a functor that returns an integral value"
        );
        this->refuseAfterStart("setAlive()");
        this->alive_ = std::move(alive);
    }

    /// @brief True when the port reads a topic. False before start(), and for a disabled
    ///        port, which never has a value. In a testing::StepHarness it is true for a
    ///        port that the parameter file wires, as a start would.
    bool enabled() const
    {
        return this->slot_->source.enabled() || this->enabledByHarness();
    }

    /// @brief The cumulative health of this binding with its supervision report: of this
    ///        step in step(), else of the last finished step. InvalidState before start()
    ///        and after the cycle stops.
    std::expected<BindingHealth, Status> bindingHealth() const
    {
        HyphaCycleBindingView view{};
        const ::HyphaStatus   status = this->supervision_.view(this->liveCycle(), view);
        if (status != HYPHA_STATUS_OK) {
            return std::unexpected(Status(status));
        }
        const uint32_t faults = view.supervision.faults;
        return BindingHealth{
            view.health,
            (faults & HYPHA_FAULT_STALE) != 0,
            (faults & HYPHA_FAULT_STAGNANT) != 0,
            view.supervision.ignored != 0,
        };
    }

  protected:
    /// @brief A port on @p topic. @p name is its table, empty for none.
    /// @throws std::invalid_argument when the queue exceeds the maxDepth of the policy.
    Input(
        Cycle&                owner,
        detail::TopicSource   topic,
        std::string           name,
        detail::InputSettings settings,
        bool                  queued
    )
        : Port(owner)
        , slot_(std::make_shared<detail::SourceSlot<M>>())
        , settings_(settings)
        , supervision_(std::move(name), !topic.fromParam())
    {
        this->slot_->topic = std::move(topic);
        this->slot_->policy = backend::policyOf<M>(settings.qos);
        this->depth_ = queueDepth(settings, queued, this->slot_->policy);
        this->claimDepth();
    }

    /// @brief A port that reads the topic of another port.
    /// @throws std::invalid_argument when the queue exceeds the topic's maxDepth.
    Input(
        Cycle&                owner,
        std::string           name,
        SharedWith<M>         shared,
        detail::InputSettings settings,
        bool                  queued
    )
        : Port(owner)
        , slot_(std::move(shared.slot))
        , settings_(settings)
        , supervision_(std::move(name), true)
    {
        this->depth_ = queueDepth(settings, queued, this->slot_->policy);
        this->claimDepth();
    }

    /// @brief The queue of a port: perStep() when @p queued, else the maxDepth of @p policy.
    static uint32_t
    queueDepth(const detail::InputSettings& settings, bool queued, const Qos& policy)
    {
        if (!queued) {
            return policy.maxDepth;
        }
        return settings.perStep.value_or(HYPHA_DEFAULT_ARRIVALS_PER_STEP);
    }

    static detail::TopicSource
    portTopic(const std::string& name, const detail::InputSettings& settings)
    {
        return detail::TopicSource::ofPort(name, false, settings.optionalTopic);
    }

    virtual void forgetCleared() = 0;  ///< A testing::StepHarness step's clear() returned.

    /// @brief The library's ok bit of this binding: usable and not stagnant, or not
    ///        counted (disabled, advisory) or ignored. True before start().
    bool inputOk() const override
    {
        return this->currentView().ok != 0;
    }

    std::string describeFailure() const override
    {
        return this->supervision_.describe(this->slot_->topic);
    }

    void endStep() override
    {
    }

    void leaveStep() override
    {
        this->supervision_.endStep();
    }

    ::HyphaStatus bindSource(HyphaCycle* cycle, bool queued)
    {
        this->tableRefused_ = false;
        const ::HyphaStatus opened = this->ensureSource();
        if (opened != HYPHA_STATUS_OK) {
            return opened;
        }
        const ::HyphaStatus bound = this->bindInputSource(
            cycle, this->slot_->source.native(), queued, this->settings_, this->index_
        );
        if (bound != HYPHA_STATUS_OK) {
            return bound;
        }
        const bool          alive = static_cast<bool>(this->alive_);
        const ::HyphaStatus checked =
            this->supervision_.bind(cycle, this->index_, this->settings_, alive);
        // A missing or refused key of the table (NotAvailable, InvalidArgument) is configuration.
        this->tableRefused_ =
            checked == HYPHA_STATUS_NOT_AVAILABLE || checked == HYPHA_STATUS_INVALID_ARGUMENT;
        return checked;
    }

    bool bindRefusedByConfiguration() const override
    {
        return this->tableRefused_;
    }

    const detail::TopicSource* topicSource() const override
    {
        return &this->slot_->topic;
    }

    std::optional<uint64_t> aliveOf(const M& message) const
    {
        if (!this->alive_) {
            return std::nullopt;
        }
        return this->alive_(message);
    }

    ::HyphaStatus captureSupervised(const HyphaCycleImage* image, std::optional<uint64_t> alive)
    {
        const bool pushAlive = this->supervision_.bound() && alive.has_value();
        if (pushAlive) {
            const ::HyphaStatus pushed = backend::cycleImageAlive(image, this->index_, *alive);
            if (pushed != HYPHA_STATUS_OK) {
                return pushed;
            }
        }
        return this->supervision_.capture(image, this->index_);
    }

    /// @brief The first port of a topic to bind opens its source.
    ::HyphaStatus ensureSource()
    {
        detail::SourceSlot<M>& slot = *this->slot_;
        if (slot.source.valid()) {
            // Another port opened the subscriber before this port was constructed.
            if (this->depth_ > slot.openedDepth) {
                detail::error(
                    this->portText() + " needs a queue of " + std::to_string(this->depth_)
                    + " samples, but the port it shares the topic with opened it with "
                    + std::to_string(slot.openedDepth)
                    + "; construct both ports before either cycle starts"
                );
                return HYPHA_STATUS_RESOURCE_LIMIT;
            }
            return HYPHA_STATUS_OK;
        }
        const std::expected<std::string, Status> resolved = slot.topic.resolve();
        if (!resolved) {
            // A bad `topic` in the port's table: a restart cannot fix it.
            this->tableRefused_ = true;
            return static_cast<::HyphaStatus>(resolved.error());
        }
        std::expected<detail::InputSource, Status> opened = detail::openSource<M>(
            backend::defaultWaitSet(),
            *resolved,
            backend::declaredSubscriberOptions(slot.policy, slot.depth)
        );
        if (!opened) {
            return static_cast<::HyphaStatus>(opened.error());
        }
        slot.source = std::move(*opened);
        slot.openedDepth = slot.depth;
        return HYPHA_STATUS_OK;
    }

    std::string portText() const
    {
        const detail::TopicSource& topic = this->slot_->topic;
        return this->supervision_.describe(topic) + " on " + topic.describe();
    }

    /// @throws std::invalid_argument when the depth exceeds the maxDepth of the policy.
    void claimDepth()
    {
        detail::SourceSlot<M>& slot = *this->slot_;
        if (this->depth_ > slot.policy.maxDepth) {
            throw std::invalid_argument(
                "hypha: " + this->portText() + ": a queue of " + std::to_string(this->depth_)
                + " samples per step (perStep) exceeds the maxDepth of its QoS policy "
                + detail::describeQos(slot.policy)
            );
        }
        if (this->depth_ > slot.depth) {
            slot.depth = this->depth_;
        }
    }

    bool hasFault(uint8_t fault) const
    {
        const HyphaSupervisionReport report = this->currentView().supervision;
        return (report.faults & fault) != 0 && report.ignored == 0;
    }

    /// @brief The binding view of this step, else of the last finished step, else the
    ///        facts last set.
    HyphaCycleBindingView currentView() const
    {
        HyphaCycleBindingView view{};
        const ::HyphaStatus   read = this->supervision_.view(this->liveCycle(), view);
        return read == HYPHA_STATUS_OK ? view : this->facts_;
    }

    friend class testing::StepHarness;
    friend class testing::McapReplay;

    template <typename T>
    friend SharedWith<T> sharing(const Input<T>& port);

    std::shared_ptr<detail::SourceSlot<M>> slot_;
    detail::InputSettings                  settings_;
    uint32_t                               depth_{0};
    InputVerdict                           verdict_{InputVerdict::NeverArrived};
    bool                                   tableRefused_{false};
    HyphaCycleBindingView                  facts_{
        .health = {}, .held = {}, .supervision = {}, .ok = 1, .age_nanos = 0
    };
    uint32_t                          index_{0};
    detail::InputSupervision          supervision_;
    std::function<uint64_t(const M&)> alive_;
    HyphaCycleHeldReport              heldReport_{};
};

}  // namespace hypha
