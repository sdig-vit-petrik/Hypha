#pragma once

/// @file
/// testing::StepHarness: run the step of a cycle that was not started, with no node and
/// no bus, setting what its inputs show and reading what its outputs sent.
///
///     Controller controller;
///     hypha::testing::StepHarness harness{controller};
///     harness.set(controller.speed, std_msgs::Float64{4.0});
///     harness.step(controller);
///     ASSERT_EQ(harness.sent(controller.command).size(), 1U);
///
/// What a harness step shares with a started cycle:
///  - which optionalTopic ports are enabled, predicted from the parameter file as a start
///    decides it: a port is enabled when its table sets a non-empty `topic`; a missing
///    table, a missing `topic` and `topic = ""` leave it disabled. The constructor throws
///    ConfigError for a name a start would refuse, or for an entry that is a plain string
///    where a start requires a table;
///  - the verdict of a delivered input, by the rule of the capture, from `silence_ms`
///    (`silence_ms = 0` and the tag unsupervised are no limit, and Input::untimed() says so);
///  - the counters of an input: received() counts in the next step each sample given to
///    deliver() or set(), deliveredCount() each one that step takes as usable;
///    setNeverArrived() sets both to zero;
///  - the answers of a Transforms port, from the transforms given to deliver() and
///    deliverStatic(): the library code of a cycle's TF input looks them up;
///  - the bound of perStep() on the sends of a step;
///  - clear(): later steps find an input NeverArrived, and a Transforms port static-only;
///  - what a disabled port does: deliver(), set(), setStale() and setStagnant() on a
///    disabled input are refused, since a started cycle gives it no sample and never judges
///    it (setNeverArrived() is true of it and stays allowed); a disabled output counts the
///    sends of a step and keeps none.
/// What it leaves out:
///  - a port that requires its topic, or sits on a Topic literal, is enabled without a
///    check of its table or name, where a start refuses a missing, empty or invalid one;
///  - an enabled output reports a subscriber (hasSubscribers() is true): no transport counts one;
///  - the node, the transport (no subscriber or publisher is opened, no sample crosses a
///    bus), the clock, and every key of a table but `topic` and `silence_ms`. A `topic`
///    that is not text throws ParameterError, where a start refuses it with
///    InvalidArgument; a start reads an array `topic` as a disabled port.
///
/// The parameter file is read when the harness is built, for `topic`, and in deliver(), for
/// `silence_ms`: a Node or a ConfigContext holds it before the harness exists; without one
/// every optionalTopic port is disabled. step() moves no clock: hypha::now() returns the
/// process clock, so a test of code that calls it sets simulated time itself. Not part of
/// hypha/hypha.h. Declare the harness after the cycle and its ports, which it points to. A
/// send the step refused and caught still fails step().

#include "hypha/backend/handles.h"
#include "hypha/backend/ros1_codec.h"
#include "hypha/config.h"
#include "hypha/cycle_arrivals.h"
#include "hypha/cycle_latest.h"
#include "hypha/cycle_node.h"
#include "hypha/cycle_output.h"
#include "hypha/cycle_runtime.h"
#include "hypha/cycle_transforms.h"

#include <cstddef>
#include <cstdint>
#include <optional>
#include <stdexcept>
#include <string>
#include <type_traits>
#include <utility>
#include <vector>

namespace hypha::testing {

/// @brief Runs the step of a cycle that was not started: no node, no bus.
///
/// Cycle::start() throws while a harness lives. Every send() of a step appends a copy to
/// sent(port); the next step() clears it.
class StepHarness final : private detail::CaptureStore
{
  public:
    /// @throws std::logic_error when @p cycle has started or another harness holds it.
    /// @throws ConfigError when an optional port's table is a plain string, or its `topic`
    ///         is a name a start would refuse.
    /// @throws ParameterError when that `topic` is not text.
    explicit StepHarness(Cycle& cycle) : cycle_(cycle)
    {
        const bool taken = cycle.started_ || cycle.harnessLive_;
        if (taken) {
            throw std::logic_error(
                "hypha: cycle '" + cycle.name()
                + "': a StepHarness needs a cycle that was not started"
            );
        }
        this->wirePorts();
        cycle.harnessLive_ = true;
    }

    StepHarness(const StepHarness&) = delete;
    StepHarness& operator=(const StepHarness&) = delete;
    StepHarness(StepHarness&&) = delete;
    StepHarness& operator=(StepHarness&&) = delete;

    ~StepHarness()
    {
        for (Store& store : this->stores_) {
            store.destroy(store.sent);
        }
        for (Port* port : this->cycle_.ports_) {
            port->enabledByHarness_ = false;
        }
        this->cycle_.harnessLive_ = false;
    }

    /// @brief The port receives @p value, sent at @p sentAt. Each later step() judges it as
    ///        a capture does: usable while (step time - sentAt) <= silence_ms, stale after.
    /// @throws std::logic_error when the port is disabled, or reads a table and no
    ///         configuration is loaded.
    /// @throws ParameterError when `silence_ms` is missing, or is neither 0 (no limit) nor
    ///         a whole number of milliseconds from 1 to the limitMax() of the port.
    template <typename M>
    void deliver(Latest<M>& port, detail::MessageOf<M> value, Time sentAt)
    {
        StepHarness::requireEnabled(port, "deliver()");
        const int64_t silenceNanos = StepHarness::silenceOf(port);
        this->forget(port);
        this->accept(port, 1);
        port.candidate_ = detail::sampleOf<M>(std::move(value));
        port.supervision_.silenceNanos_ = silenceNanos;
        this->deliveries_.push_back(
            Delivery{&port, sentAt, silenceNanos, true, &StepHarness::judgeDelivery<M>}
        );
    }

    /// @brief The port receives @p samples, oldest first, for the next step only; the newest
    ///        was sent at @p newestSentAt. The refusals are those of deliver(Latest&).
    template <typename M>
    void deliver(Arrivals<M>& port, std::vector<detail::MessageOf<M>> samples, Time newestSentAt)
    {
        StepHarness::requireEnabled(port, "deliver()");
        const int64_t silenceNanos = StepHarness::silenceOf(port);
        this->forget(port);
        this->accept(port, samples.size());
        port.sampleCount_ = samples.size();
        port.stamps_.assign(samples.size(), detail::SampleStamp{});
        port.samples_ = detail::samplesOf<M>(std::move(samples));
        port.supervision_.silenceNanos_ = silenceNanos;
        this->deliverOnce(port);
        this->deliveries_.push_back(
            Delivery{&port, newestSentAt, silenceNanos, true, &StepHarness::judgeArrivals<M>}
        );
    }

    /// @brief The port holds @p value: usable, and fresh in the next step only.
    /// @throws std::logic_error when the port is disabled.
    template <typename M>
    void set(Latest<M>& port, detail::MessageOf<M> value)
    {
        StepHarness::requireEnabled(port, "set()");
        this->forget(port);
        this->accept(port, 1);
        port.held_ = detail::sampleOf<M>(std::move(value));
        port.heldStamp_ = detail::SampleStamp{};
        port.hasHeld_ = true;
        port.verdict_ = InputVerdict::Usable;
        port.ageNanos_ = 0;
        StepHarness::judge(port, HYPHA_CYCLE_VERDICT_USABLE, false);
        this->freshNext(port);
    }

    /// @brief The port is stale; lastKnown() keeps the value last set.
    /// @throws std::logic_error when the port is disabled.
    template <typename M>
    void setStale(Latest<M>& port)
    {
        StepHarness::requireEnabled(port, "setStale()");
        this->forget(port);
        port.fresh_ = false;
        port.verdict_ = InputVerdict::Stale;
        StepHarness::judge(port, HYPHA_CYCLE_VERDICT_STALE, true);
    }

    /// @brief The port has no value at all.
    template <typename M>
    void setNeverArrived(Latest<M>& port)
    {
        this->forget(port);
        this->startCountersOver(port);
        port.hasHeld_ = false;
        port.fresh_ = false;
        port.verdict_ = InputVerdict::NeverArrived;
        StepHarness::judge(port, HYPHA_CYCLE_VERDICT_NEVER_ARRIVED, false);
    }

    /// @brief The port delivers @p samples, oldest first, to the next step only.
    /// @throws std::logic_error when the port is disabled.
    template <typename M>
    void set(Arrivals<M>& port, std::vector<detail::MessageOf<M>> samples)
    {
        StepHarness::requireEnabled(port, "set()");
        this->forget(port);
        this->accept(port, samples.size());
        port.sampleCount_ = samples.size();
        port.stamps_.assign(samples.size(), detail::SampleStamp{});
        port.samples_ = detail::samplesOf<M>(std::move(samples));
        port.verdict_ = InputVerdict::Usable;
        StepHarness::judge(port, HYPHA_CYCLE_VERDICT_USABLE, false);
        this->setOnce(port);
    }

    /// @brief The port has no sample and is stale.
    /// @throws std::logic_error when the port is disabled.
    template <typename M>
    void setStale(Arrivals<M>& port)
    {
        StepHarness::requireEnabled(port, "setStale()");
        this->forget(port);
        port.sampleCount_ = 0;
        port.verdict_ = InputVerdict::Stale;
        StepHarness::judge(port, HYPHA_CYCLE_VERDICT_STALE, true);
    }

    /// @brief The port has no sample and never had one.
    template <typename M>
    void setNeverArrived(Arrivals<M>& port)
    {
        this->forget(port);
        this->startCountersOver(port);
        port.sampleCount_ = 0;
        port.verdict_ = InputVerdict::NeverArrived;
        StepHarness::judge(port, HYPHA_CYCLE_VERDICT_NEVER_ARRIVED, false);
    }

    /// @brief The Transforms port receives @p transforms on `/tf`: each later step looks
    ///        them up as a started cycle's TF input does, which keeps `cache` of history
    ///        per frame. A transform that input refuses is left out, and a lookup on its
    ///        frame answers TransformError::Refused.
    void deliver(Transforms& port, std::vector<geometry_msgs::TransformStamped> transforms);

    /// @brief As deliver(Transforms&), on `/tf_static`: transforms that hold at every time.
    void deliverStatic(Transforms& port, std::vector<geometry_msgs::TransformStamped> transforms);

    /// @brief The port's alive value has (or has not) stopped changing.
    /// @throws std::logic_error when the port is disabled.
    template <typename M>
    void setStagnant(Input<M>& port, bool stagnant);

    /// @brief Run the step of @p node once.
    void step(CycleNode& node, CycleTime time = {})
    {
        this->step(NodeStep{&node}, time);
    }

    /// @brief Run @p step once, as the cycle would: void operator()(const CycleTime&).
    template <typename Step>
        requires(!std::is_base_of_v<CycleNode, std::remove_cvref_t<Step>>)
    void step(Step&& step, CycleTime time = {})
    {
        this->beginStep(time.now);
        const EndStep endStep{this};
        std::forward<Step>(step)(time);
        this->applyClears();
        this->rethrowLatched();
    }

    /// @brief What the last step sent on @p port, in send order.
    template <typename M>
    const std::vector<M>& sent(const Output<M>& port) const;

    /// @brief True when the last step called clear() on @p port.
    bool cleared(const Port& port) const;

  private:
    struct Store
    {
        const Port* port;
        void*       sent;
        void (*destroy)(void*);
    };

    struct Pending
    {
        Port* port;
        void (*begin)(Port*);
        void (*finish)(Port*);
    };

    /// A port that deliver() gave a value or samples: judged again in every step.
    /// `silenceNanos` is zero without a limit.
    struct Delivery
    {
        Port*   port;
        Time    sentAt;
        int64_t silenceNanos;
        bool    unseen;
        void (*judge)(Delivery&, Time, std::vector<Pending>&);
    };

    /// The samples a port accepted since the last step, which the next step's capture
    /// adds to received(). `report` is the held report of the port.
    struct Intake
    {
        HyphaCycleHeldReport* report;
        uint64_t              accepted;
    };

    /// The transforms a Transforms port holds, in a buffer the harness feeds.
    struct FedTransforms
    {
        const Port*                       port;
        backend::UniqueTransformBuffer    buffer;
        std::vector<std::vector<uint8_t>> statics;
    };

    struct NodeStep
    {
        CycleNode* node;

        void operator()(const CycleTime& time) const
        {
            this->node->step(time);
        }
    };

    struct EndStep
    {
        explicit EndStep(StepHarness* owner) : harness(owner)
        {
        }

        StepHarness* harness;

        EndStep(const EndStep&) = delete;
        EndStep& operator=(const EndStep&) = delete;

        ~EndStep()
        {
            this->harness->endStep();
        }
    };

    // Defined in hypha/testing/step_harness_impl.h.
    void        wirePorts();
    static bool startWouldEnable(const detail::TopicSource& topic);
    void*       storeFor(const Port& port, void* (*make)(), void (*destroy)(void*)) override;
    const HyphaTransformBuffer* transformsFor(const Port& port, int64_t cacheNanos) override;
    void                        clearAfterStep(Port& port, void (*forget)(Port&)) override;
    void                        applyClears();
    void
    feed(Transforms& port, std::vector<geometry_msgs::TransformStamped> transforms, bool isStatic);
    template <typename M>
    static void requireEnabled(const Input<M>& port, const char* call);
    template <typename M>
    static int64_t  silenceOf(const Input<M>& port);
    static bool     withinSilence(int64_t silenceNanos, int64_t sentNanos, int64_t nowNanos);
    static uint64_t ageOf(int64_t sentNanos, int64_t nowNanos);
    template <typename M>
    static void judgeDelivery(Delivery& delivery, Time now, std::vector<Pending>& running);
    template <typename M>
    static void judgeArrivals(Delivery& delivery, Time now, std::vector<Pending>& running);
    template <typename M>
    static void judge(Input<M>& port, uint32_t verdict, bool stale);
    template <typename M>
    static void rejudge(Input<M>& port);
    template <typename M>
    static void makeFresh(Port* port);
    template <typename M>
    static void keepFresh(Port*);
    template <typename M>
    static void endFresh(Port* port);
    template <typename M>
    static void keepSamples(Port*);
    template <typename M>
    static void countSamples(Port* port);
    template <typename M>
    static void dropSamples(Port* port);
    template <typename M>
    void forget(Input<M>& port);
    void dropPending(const Port& port);
    template <typename M>
    void freshNext(Latest<M>& port);
    template <typename M>
    void deliverOnce(Arrivals<M>& port);
    template <typename M>
    void setOnce(Arrivals<M>& port);
    template <typename M>
    void accept(Input<M>& port, uint64_t count);
    template <typename M>
    void startCountersOver(Input<M>& port);
    void takeIntakes();
    void beginStep(Time now);
    void rethrowLatched();
    void endStep() noexcept;

    Cycle&                                         cycle_;
    std::vector<Store>                             stores_;
    std::vector<Pending>                           pending_;
    std::vector<Pending>                           running_;
    std::vector<Delivery>                          deliveries_;
    std::vector<Intake>                            intakes_;
    std::vector<FedTransforms>                     transforms_;
    std::vector<std::pair<Port*, void (*)(Port&)>> clears_;
};

}  // namespace hypha::testing

#include "hypha/testing/step_harness_impl.h"
