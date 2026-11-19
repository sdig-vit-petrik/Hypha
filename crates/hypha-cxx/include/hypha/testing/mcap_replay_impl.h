#pragma once

/// @file
/// The member functions of testing::McapReplay. Included by hypha/testing/mcap_replay.h.

#include "hypha/testing/mcap_replay.h"

namespace hypha::testing {

inline int64_t McapReplay::periodOf(const std::string& path, Hz rate)
{
    const Duration period = hypha::detail::periodFromHz(rate);
    const bool     noPeriod = period.nanos() <= 0;
    if (noPeriod) {
        throw std::invalid_argument(
            "hypha: McapReplay '" + path + "': the rate " + std::to_string(rate.value())
            + " Hz gives no period from 1 ns through INT64_MAX ns"
        );
    }
    return period.nanos();
}

/// @return True when the port is fed.
template <typename M>
bool McapReplay::addFeed(
    Input<M>& port, void* window, Take take, Flush flush, bool mustBePresent
)
{
    this->refuseFeed(port);
    std::string topic = this->topicOf(port);
    const bool  absentAndAllowed =
        !topic.empty() && !mustBePresent && !McapReplay::holdsTopic(this->file_, topic);
    if (absentAndAllowed) {
        topic.clear();
    }
    const bool fed = !topic.empty();
    if (fed) {
        McapReplay::checkChannels<M>(this->file_, this->path_, topic);
        this->file_.select(topic);
    }
    this->feeds_.push_back(
        Feed{
            &port, topic, window, take, flush, &McapReplay::destroyWindow<M>,
            codec::dataType<M>()
        }
    );
    return fed;
}

inline bool McapReplay::holdsTopic(const detail::McapFile& file, const std::string& topic)
{
    const size_t count = file.channelCount();
    for (size_t index = 0; index < count; ++index) {
        if (detail::mcapText(file.channel(index).topic) == topic) {
            return true;
        }
    }
    return false;
}

inline bool McapReplay::due() const
{
    if (this->hasPending_) {
        return true;
    }
    const bool hasTail = this->end_.has_value() && !this->lastDeadlineReached_;
    return hasTail && this->nextDeadline_ <= *this->end_;
}

inline void McapReplay::refuseFeed(const Port& port) const
{
    if (this->started_) {
        throw std::logic_error(
            "hypha: McapReplay '" + this->path_
            + "': feed() after the replay started (stepDue() or next())"
        );
    }
    for (const Feed& feed : this->feeds_) {
        if (feed.port == &port) {
            throw std::logic_error(
                "hypha: McapReplay '" + this->path_ + "': the port of topic '" + feed.topic
                + "' is fed already"
            );
        }
    }
}

template <typename M>
std::string McapReplay::topicOf(const Input<M>& port) const
{
    const hypha::detail::TopicSource&        source = port.slot_->topic;
    const std::expected<std::string, Status> resolved = source.resolve();
    if (resolved.has_value()) {
        return *resolved;
    }
    const bool noNode = source.fromParam() && backend::nodeHandle() == nullptr;
    if (noNode) {
        throw std::logic_error(
            "hypha: McapReplay '" + this->path_ + "': feed() of " + source.describe()
            + " reads its topic and needs a Node that holds the parameter file"
        );
    }
    throw std::runtime_error(
        "hypha: McapReplay '" + this->path_ + "': the topic of " + source.describe()
        + " does not resolve: " + std::string(resolved.error().name())
    );
}

/// @brief Refuse a topic the file does not hold, and each channel of it that is not
///        ROS 1 or was recorded as another type name or MD5 than @p M has.
template <typename M>
void McapReplay::checkChannels(
    const detail::McapFile& file, const std::string& path, const std::string& topic
)
{
    const std::string prefix = "hypha: McapReplay '" + path + "': topic '" + topic + "': ";
    const std::string typeName = codec::dataType<M>();
    const std::string md5 = codec::md5Sum<M>();
    size_t            channels = 0;
    const size_t      count = file.channelCount();
    for (size_t index = 0; index < count; ++index) {
        const HyphaMcapChannel channel = file.channel(index);
        if (detail::mcapText(channel.topic) != topic) {
            continue;
        }
        ++channels;
        const std::string schemaEncoding = detail::mcapText(channel.schema_encoding);
        if (schemaEncoding != "ros1msg") {
            throw std::runtime_error(
                prefix + "its schema encoding is '" + schemaEncoding
                + "', not 'ros1msg': the replay decodes ROS 1 messages only"
            );
        }
        const std::string messageEncoding = detail::mcapText(channel.message_encoding);
        if (messageEncoding != "ros1") {
            throw std::runtime_error(
                prefix + "its message encoding is '" + messageEncoding + "', not 'ros1'"
            );
        }
        const std::string recordedType = detail::mcapText(channel.type_name);
        if (recordedType != typeName) {
            throw std::runtime_error(
                prefix + "recorded as " + recordedType + ", but the port reads " + typeName
            );
        }
        const std::string recordedMd5 = detail::mcapText(channel.md5);
        const bool        otherMd5 = !recordedMd5.empty() && recordedMd5 != md5;
        if (otherMd5) {
            throw std::runtime_error(
                prefix + "recorded as " + recordedType + " with MD5 " + recordedMd5
                + ", but the port's " + typeName + " has MD5 " + md5
            );
        }
    }
    if (channels == 0) {
        throw std::runtime_error(
            "hypha: McapReplay '" + path + "': the file holds no topic '" + topic + "'"
        );
    }
}

inline void McapReplay::start()
{
    if (this->started_) {
        return;
    }
    this->started_ = true;
    this->file_.rewind();
    this->advance();
    if (!this->hasPending_) {
        std::string topics;
        for (const Feed& feed : this->feeds_) {
            const bool listed = !feed.topic.empty();
            if (listed) {
                topics += (topics.empty() ? "'" : ", '") + feed.topic + "'";
            }
        }
        throw std::runtime_error(
            "hypha: McapReplay '" + this->path_ + "': the file holds no message on any fed "
            "topic (" + (topics.empty() ? std::string("none fed") : topics) + ")"
        );
    }
    // The first whole multiple of the period strictly after the earliest fed sample.
    const int64_t gridBelow = (this->pendingNanos_ / this->periodNanos_) * this->periodNanos_;
    const int64_t lastStart = std::numeric_limits<int64_t>::max() - this->periodNanos_;
    const bool    overflows = gridBelow > lastStart;
    if (overflows) {
        throw std::runtime_error(
            "hypha: McapReplay '" + this->path_ + "': the first deadline after "
            + std::to_string(this->pendingNanos_) + " ns does not fit framework time"
        );
    }
    this->nextDeadline_ = gridBelow + this->periodNanos_;
}

inline void McapReplay::advance()
{
    this->hasPending_ = this->file_.next(this->pending_);
    if (this->hasPending_) {
        this->pendingNanos_ = this->nanosOf(this->pending_.log_time_ns);
    }
}

inline void McapReplay::deliverPending(StepHarness& harness)
{
    const HyphaMcapChannel channel = this->file_.channel(this->pending_.channel_index);
    const std::string      topic = detail::mcapText(channel.topic);
    const Time             sentAt = Time::fromNanos(this->pendingNanos_);
    for (Feed& feed : this->feeds_) {
        if (feed.topic != topic) {
            continue;
        }
        const bool decoded = feed.take(feed, harness, this->pending_, sentAt);
        if (!decoded) {
            this->throwUndecodable(topic, this->pending_, feed.typeName);
        }
    }
}

inline int64_t McapReplay::nanosOf(uint64_t nanos) const
{
    const bool tooLate = nanos > static_cast<uint64_t>(std::numeric_limits<int64_t>::max());
    if (tooLate) {
        throw std::runtime_error(
            "hypha: McapReplay '" + this->path_ + "': the time " + std::to_string(nanos)
            + " ns does not fit framework time"
        );
    }
    return static_cast<int64_t>(nanos);
}

inline void McapReplay::throwUndecodable(
    const std::string& topic, const HyphaMcapMessage& message, const char* typeName
) const
{
    throw std::runtime_error(
        "hypha: McapReplay '" + this->path_ + "': topic '" + topic
        + "': the message logged at " + std::to_string(message.log_time_ns)
        + " ns does not decode as " + typeName
    );
}

template <typename M>
bool McapReplay::takeLatest(
    Feed& feed, StepHarness& harness, const HyphaMcapMessage& message, Time sentAt
)
{
    M          value{};
    const bool decoded = codec::decode(message.payload.data, message.payload.len, value);
    if (!decoded) {
        return false;
    }
    harness.deliver(*static_cast<Latest<M>*>(feed.port), std::move(value), sentAt);
    return true;
}

template <typename M>
bool McapReplay::takeArrival(
    Feed& feed, StepHarness&, const HyphaMcapMessage& message, Time sentAt
)
{
    M          value{};
    const bool decoded = codec::decode(message.payload.data, message.payload.len, value);
    if (!decoded) {
        return false;
    }
    Window<M>& window = *static_cast<Window<M>*>(feed.window);
    window.samples.push_back(std::move(value));
    window.newestSentAt = sentAt;
    return true;
}

template <typename M>
void McapReplay::flushArrivals(Feed& feed, StepHarness& harness)
{
    Window<M>& window = *static_cast<Window<M>*>(feed.window);
    if (window.samples.empty()) {
        return;
    }
    harness.deliver(
        *static_cast<Arrivals<M>*>(feed.port), std::move(window.samples), window.newestSentAt
    );
    window.samples.clear();
}

inline void McapReplay::keep(Feed&, StepHarness&)
{
}

template <typename M>
void McapReplay::destroyWindow(void* window)
{
    delete static_cast<Window<M>*>(window);
}

}  // namespace hypha::testing
