#pragma once

/// @file
/// The member functions of testing::StepHarness. Included by hypha/testing.h.

#include "hypha/testing.h"
#include "hypha/testing/step_harness_judging.h"

namespace hypha::testing {

/// Predicts whether a start enables @p topic, from the parameter file alone. It follows
/// `hypha_param_topic` (crates/hypha_node/src/param_topic.rs), which needs a node and so
/// cannot run here; TheHarnessEnablesTheOptionalPortsThatAStartEnables keeps the two equal.
/// A port that requires its topic, or sits on a literal, is not checked: a start refuses a
/// missing, empty or invalid one, the harness enables it.
inline bool StepHarness::startWouldEnable(const detail::TopicSource& topic)
{
    const bool alwaysEnabled = !topic.fromParam() || !topic.paramOptional;
    if (alwaysEnabled) {
        return true;
    }
    const std::string entry = topic.describe();
    const bool plainEntry = backend::readConfigValue(entry).first != HYPHA_PARAM_RESULT_ABSENT;
    if (plainEntry) {
        throw ConfigError("hypha: the entry " + entry + " must be a table with a `topic` key");
    }
    const std::optional<std::string> name = ConfigView(entry).optional<std::string>("topic");
    const bool                       switchedOff = !name.has_value() || name->empty();
    if (switchedOff) {
        return false;
    }
    if (validateTopicName(*name) != Status::Ok) {
        throw ConfigError(detail::failureText(entry, Status::InvalidTopic));
    }
    return true;
}

/// Decides every port before it changes one, so a refusal leaves the cycle as it was.
inline void StepHarness::wirePorts()
{
    std::vector<bool> enabled;
    for (const Port* port : this->cycle_.ports_) {
        const detail::TopicSource* topic = port->topicSource();
        enabled.push_back(topic != nullptr && StepHarness::startWouldEnable(*topic));
    }
    for (size_t index = 0; index < enabled.size(); ++index) {
        this->cycle_.ports_[index]->enabledByHarness_ = enabled[index];
    }
}

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

/// The buffer of @p port, made empty on first use: a step that looks up a transform nobody
/// delivered gets the answer of a started cycle's empty TF input. The library refuses a
/// cache that a start refuses.
inline const HyphaTransformBuffer* StepHarness::transformsFor(const Port& port, int64_t cacheNanos)
{
    for (const FedTransforms& held : this->transforms_) {
        if (held.port == &port) {
            return held.buffer.get();
        }
    }
    HyphaTransformBuffer* created = nullptr;
    const uint64_t        cache = cacheNanos > 0 ? static_cast<uint64_t>(cacheNanos) : 0U;
    const ::HyphaStatus   status = backend::createFedTransformBuffer(cache, &created);
    if (status != HYPHA_STATUS_OK) {
        throw std::invalid_argument(
            "hypha: a Transforms port needs a positive cache: " + std::string(Status(status).name())
        );
    }
    this->transforms_.push_back(FedTransforms{&port, backend::UniqueTransformBuffer(created), {}});
    return created;
}

inline void
StepHarness::deliver(Transforms& port, std::vector<geometry_msgs::TransformStamped> transforms)
{
    this->feed(port, std::move(transforms), false);
}

inline void StepHarness::deliverStatic(
    Transforms& port, std::vector<geometry_msgs::TransformStamped> transforms
)
{
    this->feed(port, std::move(transforms), true);
}

inline void StepHarness::feed(
    Transforms& port, std::vector<geometry_msgs::TransformStamped> transforms, bool isStatic
)
{
    tf2_msgs::TFMessage message;
    message.transforms = std::move(transforms);
    std::vector<uint8_t> payload;
    codec::encode(message, payload);
    const HyphaTransformBuffer* buffer = this->transformsFor(port, port.cache_.nanos());
    const ::HyphaStatus         status = backend::transformBufferFeed(buffer, payload, isStatic);
    if (status != HYPHA_STATUS_OK) {
        throw std::runtime_error(
            "hypha: deliver() on a Transforms port refused: " + std::string(Status(status).name())
        );
    }
    if (isStatic) {
        for (FedTransforms& held : this->transforms_) {
            if (held.port == &port) {
                held.statics.push_back(std::move(payload));
            }
        }
    }
}

inline void StepHarness::clearAfterStep(Port& port, void (*forget)(Port&))
{
    this->clears_.emplace_back(&port, forget);
}

inline bool StepHarness::cleared(const Port& port) const
{
    for (const std::pair<Port*, void (*)(Port&)>& clear : this->clears_) {
        if (clear.first == &port) {
            return true;
        }
    }
    return false;
}

/// Applies the clears of the step that returned: an input forgets its delivery and value; a
/// Transforms port gets a new buffer fed its static transforms again, as a started cycle's
/// TF input forgets the dynamic ones alone.
inline void StepHarness::applyClears()
{
    for (const std::pair<Port*, void (*)(Port&)>& clear : this->clears_) {
        Port* const port = clear.first;
        if (clear.second != nullptr) {
            this->dropPending(*port);
            std::erase_if(this->deliveries_, [port](const Delivery& item) {
                return item.port == port;
            });
            clear.second(*port);
            continue;
        }
        for (FedTransforms& held : this->transforms_) {
            if (held.port != port) {
                continue;
            }
            const int64_t         cacheNanos = static_cast<Transforms*>(port)->cache_.nanos();
            HyphaTransformBuffer* created = nullptr;
            ::HyphaStatus         status =
                backend::createFedTransformBuffer(static_cast<uint64_t>(cacheNanos), &created);
            held.buffer = backend::UniqueTransformBuffer(created);
            for (const std::vector<uint8_t>& payload : held.statics) {
                if (status == HYPHA_STATUS_OK) {
                    status = backend::transformBufferFeed(created, payload, true);
                }
            }
            if (status != HYPHA_STATUS_OK) {
                throw std::runtime_error(
                    "hypha: clear() of a Transforms port refused: "
                    + std::string(Status(status).name())
                );
            }
        }
    }
}

template <typename M>
void StepHarness::keepSamples(Port*)
{
}

template <typename M>
void StepHarness::countSamples(Port* port)
{
    Arrivals<M>& arrivals = *static_cast<Arrivals<M>*>(port);
    arrivals.heldReport_.delivered_total += arrivals.sampleCount_;
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

template <typename M>
void StepHarness::setOnce(Arrivals<M>& port)
{
    this->pending_.push_back(
        Pending{&port, &StepHarness::countSamples<M>, &StepHarness::dropSamples<M>}
    );
}

/// The source of @p port accepts @p count samples; the next step's capture counts them in
/// received(), as a capture counts what the source accepted until then.
template <typename M>
void StepHarness::accept(Input<M>& port, uint64_t count)
{
    for (Intake& intake : this->intakes_) {
        if (intake.report == &port.heldReport_) {
            intake.accepted += count;
            return;
        }
    }
    this->intakes_.push_back(Intake{&port.heldReport_, count});
}

/// A port that never had a sample has accepted none: both counters read zero again.
template <typename M>
void StepHarness::startCountersOver(Input<M>& port)
{
    std::erase_if(this->intakes_, [&port](const Intake& intake) {
        return intake.report == &port.heldReport_;
    });
    port.heldReport_ = HyphaCycleHeldReport{};
}

inline void StepHarness::takeIntakes()
{
    for (Intake& intake : this->intakes_) {
        intake.report->received_total += intake.accepted;
        intake.accepted = 0;
    }
}

/// Clears the last step's messages and routes every send() of the cycle's ports here.
inline void StepHarness::beginStep(Time now)
{
    for (Store& store : this->stores_) {
        store.destroy(store.sent);
    }
    this->stores_.clear();
    this->clears_.clear();
    this->running_ = std::move(this->pending_);
    this->pending_.clear();
    for (Pending& item : this->running_) {
        item.begin(item.port);
    }
    this->takeIntakes();
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

template <typename M>
void StepHarness::setStagnant(Input<M>& port, bool stagnant)
{
    StepHarness::requireEnabled(port, "setStagnant()");
    HyphaSupervisionReport& report = port.facts_.supervision;
    const uint8_t           others = report.faults & ~HYPHA_FAULT_STAGNANT;
    report.faults = static_cast<uint8_t>(others | (stagnant ? HYPHA_FAULT_STAGNANT : 0U));
    StepHarness::rejudge(port);
}

template <typename M>
const std::vector<M>& StepHarness::sent(const Output<M>& port) const
{
    for (const Store& store : this->stores_) {
        if (store.port == &port) {
            return *static_cast<const std::vector<M>*>(store.sent);
        }
    }
    static const std::vector<M> none;
    return none;
}

template <typename M>
void StepHarness::setNeverArrived(Latest<M>& port)
{
    this->forget(port);
    this->startCountersOver(port);
    port.hasHeld_ = false;
    port.fresh_ = false;
    port.verdict_ = InputVerdict::NeverArrived;
    StepHarness::judge(port, HYPHA_CYCLE_VERDICT_NEVER_ARRIVED, false);
}

template <typename M>
void StepHarness::setNeverArrived(Arrivals<M>& port)
{
    this->forget(port);
    this->startCountersOver(port);
    port.sampleCount_ = 0;
    port.verdict_ = InputVerdict::NeverArrived;
    StepHarness::judge(port, HYPHA_CYCLE_VERDICT_NEVER_ARRIVED, false);
}

}  // namespace hypha::testing
