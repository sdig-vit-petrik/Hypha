#pragma once

/// @file
/// How testing::StepHarness judges a port as the library's capture does: the silence limit,
/// the verdicts and the fresh bit. Included by hypha/testing/step_harness_impl.h.

#include "hypha/testing.h"

namespace hypha::testing {

/// A disabled input never receives a sample in a started cycle, and no step judges it, so a
/// test that gives it a value or a verdict tests a state that cannot occur: refuse it, and
/// say what enables the port.
template <typename M>
void StepHarness::requireEnabled(const Input<M>& port, const char* call)
{
    if (port.enabled()) {
        return;
    }
    const std::string table = port.slot_->topic.describe();
    throw std::logic_error(
        "hypha: " + std::string(call) + " on input " + table
        + ", which is disabled: the parameter file gives [" + table
        + "] no non-empty `topic`, so a started cycle would give it no sample; set `topic` "
          "there, in the file that a Node or a ConfigContext holds before the harness is built"
    );
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
        port.heldReport_.delivered_total += 1;
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
    } else if (delivery.unseen) {
        port.heldReport_.delivered_total += port.sampleCount_;
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
    Latest<M>& latest = *static_cast<Latest<M>*>(port);
    latest.fresh_ = true;
    latest.heldReport_.delivered_total += 1;
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

}  // namespace hypha::testing
