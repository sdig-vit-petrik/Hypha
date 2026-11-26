#pragma once

/// @file
/// The member functions of testing::StepHarness. Included by hypha/testing.h.

#include "hypha/testing.h"

namespace hypha::testing {

inline void* StepHarness::storeFor(const Port& port, void* (*make)(), void (*destroy)(void*))
{
    for (Store& store : this->stores_) {
        if (store.port == &port) {
            return store.sent;
        }
    }
    this->stores_.push_back(Store{&port, make(), destroy});
    return this->stores_.back().sent;
}

/// @brief The silence limit of @p port in nanoseconds; zero when it has none.
template <typename M>
int64_t StepHarness::silenceOf(const Input<M>& port)
{
    const std::string& table = port.supervision_.name();
    const bool         noTable = table.empty() || port.settings_.unsupervised;
    if (noTable) {
        return 0;
    }
    const bool noConfiguration = backend::configHandle() == nullptr;
    if (noConfiguration) {
        throw std::logic_error(
            "hypha: deliver() on input '" + table
            + "' reads its silence_ms and needs a loaded configuration: a Node or a "
              "ConfigContext"
        );
    }
    const ConfigView             ports("ports.inputs." + table);
    const std::optional<int64_t> millis = ports.optional<int64_t>("silence_ms");
    if (!millis.has_value()) {
        return 0;
    }
    constexpr int64_t nanosPerMilli = 1'000'000;
    const int64_t     millisMax = port.settings_.limitMax.nanos() / nanosPerMilli;
    const bool        outOfRange = *millis < 1 || *millis > millisMax;
    if (outOfRange) {
        throw ParameterError(
            "hypha: parameter '" + ports.name("silence_ms") + "' is outside its range [1, "
            + std::to_string(millisMax) + "] ms"
        );
    }
    return *millis * nanosPerMilli;
}

/// The rule of `value_usable_within` in crates/hypha_cycle/src/capture/clock.rs. Keep it
/// equal to that function.
inline bool StepHarness::withinSilence(int64_t silenceNanos, int64_t sentNanos, int64_t nowNanos)
{
    const bool sentInTheFuture = sentNanos > nowNanos;
    if (sentInTheFuture) {
        return false;
    }
    const bool hasLimit = silenceNanos != 0;
    if (!hasLimit) {
        return true;
    }
    const uint64_t age = StepHarness::ageOf(sentNanos, nowNanos);
    return age <= static_cast<uint64_t>(silenceNanos);
}

inline uint64_t StepHarness::ageOf(int64_t sentNanos, int64_t nowNanos)
{
    const bool sentInTheFuture = sentNanos > nowNanos;
    if (sentInTheFuture) {
        return 0;
    }
    return static_cast<uint64_t>(nowNanos) - static_cast<uint64_t>(sentNanos);
}

/// Judges the value of a delivery at @p now as the capture of a Latest port does.
template <typename M>
void StepHarness::judgeDelivery(Delivery& delivery, Time now, std::vector<Pending>& running)
{
    Latest<M>&    port = *static_cast<Latest<M>*>(delivery.port);
    const int64_t sent = delivery.sentAt.nanos();
    const bool    usable = StepHarness::withinSilence(delivery.silenceNanos, sent, now.nanos());
    const bool    takesTheValue = usable && delivery.unseen;
    if (takesTheValue) {
        std::swap(port.held_, port.candidate_);
        port.heldStamp_ = detail::SampleStamp{static_cast<uint64_t>(sent), 0, 0};
        port.hasHeld_ = true;
        port.fresh_ = true;
        delivery.unseen = false;
        running.push_back(Pending{&port, &StepHarness::keepFresh<M>, &StepHarness::endFresh<M>});
    }
    port.verdict_ = usable ? InputVerdict::Usable : InputVerdict::Stale;
    port.ageNanos_ = StepHarness::ageOf(sent, now.nanos());
    StepHarness::judge(
        port, usable ? HYPHA_CYCLE_VERDICT_USABLE : HYPHA_CYCLE_VERDICT_STALE, !usable
    );
}

template <typename M>
void StepHarness::judgeArrivals(Delivery& delivery, Time now, std::vector<Pending>&)
{
    Arrivals<M>&  port = *static_cast<Arrivals<M>*>(delivery.port);
    const int64_t sent = delivery.sentAt.nanos();
    const bool    usable = StepHarness::withinSilence(delivery.silenceNanos, sent, now.nanos());
    const bool    refusesTheSamples = delivery.unseen && !usable;
    if (refusesTheSamples) {
        port.sampleCount_ = 0;
    }
    delivery.unseen = false;
    port.verdict_ = usable ? InputVerdict::Usable : InputVerdict::Stale;
    StepHarness::judge(
        port, usable ? HYPHA_CYCLE_VERDICT_USABLE : HYPHA_CYCLE_VERDICT_STALE, !usable
    );
}

template <typename M>
void StepHarness::judge(Input<M>& port, uint32_t verdict, bool stale)
{
    port.facts_.health.capture.verdict = verdict;
    HyphaSupervisionReport& report = port.facts_.supervision;
    const uint8_t           others = report.faults & ~HYPHA_FAULT_STALE;
    report.faults = static_cast<uint8_t>(others | (stale ? HYPHA_FAULT_STALE : 0U));
    StepHarness::rejudge(port);
}

/// The ok bit, by the library's rule.
template <typename M>
void StepHarness::rejudge(Input<M>& port)
{
    const HyphaCycleBindingView& facts = port.facts_;
    const bool usable = facts.health.capture.verdict == HYPHA_CYCLE_VERDICT_USABLE;
    const bool stagnant = (facts.supervision.faults & HYPHA_FAULT_STAGNANT) != 0;
    const bool ok = !port.settings_.countsForHealth || (usable && !stagnant);
    port.facts_.ok = ok ? 1 : 0;
}

template <typename M>
void StepHarness::makeFresh(Port* port)
{
    static_cast<Latest<M>*>(port)->fresh_ = true;
}

template <typename M>
void StepHarness::keepFresh(Port*)
{
}

template <typename M>
void StepHarness::endFresh(Port* port)
{
    static_cast<Latest<M>*>(port)->fresh_ = false;
}

template <typename M>
void StepHarness::keepSamples(Port*)
{
}

template <typename M>
void StepHarness::dropSamples(Port* port)
{
    static_cast<Arrivals<M>*>(port)->sampleCount_ = 0;
}

template <typename M>
void StepHarness::forget(Input<M>& port)
{
    this->dropPending(port);
    std::vector<Delivery> kept;
    for (const Delivery& item : this->deliveries_) {
        if (item.port != &port) {
            kept.push_back(item);
        }
    }
    this->deliveries_ = std::move(kept);
    port.supervision_.silenceNanos_ = 0;
}

inline void StepHarness::dropPending(const Port& port)
{
    std::vector<Pending> kept;
    for (const Pending& item : this->pending_) {
        if (item.port != &port) {
            kept.push_back(item);
        }
    }
    this->pending_ = std::move(kept);
}

template <typename M>
void StepHarness::freshNext(Latest<M>& port)
{
    this->pending_.push_back(Pending{&port, &StepHarness::makeFresh<M>, &StepHarness::endFresh<M>});
}

template <typename M>
void StepHarness::deliverOnce(Arrivals<M>& port)
{
    this->pending_.push_back(
        Pending{&port, &StepHarness::keepSamples<M>, &StepHarness::dropSamples<M>}
    );
}

/// Clears the last step's messages and routes every send() of the cycle's ports here.
inline void StepHarness::beginStep(Time now)
{
    for (Store& store : this->stores_) {
        store.destroy(store.sent);
    }
    this->stores_.clear();
    this->running_ = std::move(this->pending_);
    this->pending_.clear();
    for (Pending& item : this->running_) {
        item.begin(item.port);
    }
    for (Delivery& delivery : this->deliveries_) {
        delivery.judge(delivery, now, this->running_);
    }
    for (Port* port : this->cycle_.ports_) {
        port->endStep();
        static_cast<void>(port->takeStepFailure());
        port->harness_ = this;
    }
}

/// A send refused in the step fails it, also when the step caught the exception.
inline void StepHarness::rethrowLatched()
{
    for (Port* port : this->cycle_.ports_) {
        const std::optional<::HyphaStatus> failure = port->takeStepFailure();
        if (failure.has_value()) {
            throw detail::StepFailure(
                *failure,
                "hypha: the step latched a failure: " + std::string(Status(*failure).name())
            );
        }
    }
}

inline void StepHarness::endStep() noexcept
{
    for (Port* port : this->cycle_.ports_) {
        port->harness_ = nullptr;
        static_cast<void>(port->takeStepFailure());
    }
    for (Pending& item : this->running_) {
        item.finish(item.port);
    }
    this->running_.clear();
}

}  // namespace hypha::testing
