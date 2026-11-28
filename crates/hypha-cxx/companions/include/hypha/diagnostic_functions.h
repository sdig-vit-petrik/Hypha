#pragma once

/// @file
/// The ready-made tasks of ROS's `diagnostic_updater` (`update_functions.h`,
/// `publisher.h`): FrequencyStatus (a rate check), TimeStampStatus (the age of a header
/// stamp), and TopicDiagnostic and HeaderlessTopicDiagnostic, which hold both and add
/// themselves to an updater. A node calls tick() where it sends or receives.
/// `diagnostic_updater::DiagnosedPublisher` has no counterpart: call tick() before send().

#include "hypha/diagnostic_updater.h"
#include "hypha/message_stamp.h"
#include "hypha_base/clock.h"
#include "hypha_base/time.h"
#include <hypha_msgs/ros_time.h>

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <limits>
#include <mutex>
#include <optional>
#include <stdexcept>
#include <string>
#include <vector>

namespace hypha {

/// @brief The limits of a FrequencyStatus. @p minFreq and @p maxFreq point at the values, so
///        that the node may change them (a parameter); they must outlive the task. The
///        frequency is acceptable between `*minFreq * (1 - tolerance)` and
///        `*maxFreq * (1 + tolerance)`, measured over @p windowSize updates.
class FrequencyStatusParam
{
  public:
    FrequencyStatusParam(
        const double* minFreq, const double* maxFreq, double tolerance = 0.1, int windowSize = 5
    )
        : minFreq_(minFreq), maxFreq_(maxFreq), tolerance_(tolerance), windowSize_(windowSize)
    {
        if (minFreq == nullptr || maxFreq == nullptr) {
            throw std::invalid_argument("hypha: FrequencyStatusParam: null frequency pointer");
        }
        if (windowSize < 1 || !std::isfinite(tolerance) || tolerance < 0.0) {
            throw std::invalid_argument(
                "hypha: FrequencyStatusParam: the window is below 1 or the tolerance is invalid"
            );
        }
    }

    double minFreq() const
    {
        return *this->minFreq_;
    }

    double maxFreq() const
    {
        return *this->maxFreq_;
    }

    double tolerance() const
    {
        return this->tolerance_;
    }

    int windowSize() const
    {
        return this->windowSize_;
    }

  private:
    const double* minFreq_;
    const double* maxFreq_;
    double        tolerance_;
    int           windowSize_;
};

/// @brief ERROR when no event came in the window, WARN when the rate is outside the limits.
///        tick() may be called from any thread.
class FrequencyStatus final : public DiagnosticTask
{
  public:
    explicit FrequencyStatus(
        const FrequencyStatusParam& param, std::string name = "Frequency Status"
    )
        : DiagnosticTask(std::move(name))
        , param_(param)
        , counts_(static_cast<size_t>(param.windowSize()), 0U)
        , times_(static_cast<size_t>(param.windowSize()))
    {
    }

    /// @brief One event happened, at @p at (default hypha::now()). The window of the rate
    ///        starts with the first event or the first update, whichever comes first.
    void tick(Time at)
    {
        const std::lock_guard<std::mutex> lock(this->mutex_);
        this->startWindow(at);
        this->count_ += 1U;
    }

    void tick()
    {
        this->tick(hypha::now());
    }

    /// @brief Forget the events and the window.
    void clear()
    {
        const std::lock_guard<std::mutex> lock(this->mutex_);
        this->count_ = 0U;
        this->started_ = false;
    }

    void run(DiagnosticStatusWrapper& status) override
    {
        const std::lock_guard<std::mutex> lock(this->mutex_);
        const Time                        now = status.now();
        this->startWindow(now);
        const uint64_t events = this->count_ - this->counts_[this->index_];
        const double   window = (now - this->times_[this->index_]).seconds();
        this->counts_[this->index_] = this->count_;
        this->times_[this->index_] = now;
        this->index_ = (this->index_ + 1U) % this->counts_.size();

        const bool   measured = window > 0.0;
        const double frequency = measured ? static_cast<double>(events) / window : 0.0;
        const double minimum = this->param_.minFreq();
        const double maximum = this->param_.maxFreq();
        const double tolerance = this->param_.tolerance();
        if (events == 0U) {
            status.summary(diagnostic_msgs::DiagnosticStatus::ERROR, "No events recorded.");
        } else if (!measured) {
            status.summary(diagnostic_msgs::DiagnosticStatus::WARN, "No time elapsed to measure.");
        } else if (frequency < minimum * (1.0 - tolerance)) {
            status.summary(diagnostic_msgs::DiagnosticStatus::WARN, "Frequency too low.");
        } else if (frequency > maximum * (1.0 + tolerance)) {
            status.summary(diagnostic_msgs::DiagnosticStatus::WARN, "Frequency too high.");
        } else {
            status.summary(diagnostic_msgs::DiagnosticStatus::OK, "Desired frequency met");
        }
        status.add("Events in window", events);
        status.add("Events since startup", this->count_);
        status.add("Duration of window (s)", window);
        status.add("Actual frequency (Hz)", frequency);
        if (minimum == maximum) {
            status.add("Target frequency (Hz)", minimum);
        }
        if (minimum > 0.0) {
            status.add("Minimum acceptable frequency (Hz)", minimum * (1.0 - tolerance));
        }
        if (std::isfinite(maximum)) {
            status.add("Maximum acceptable frequency (Hz)", maximum * (1.0 + tolerance));
        }
    }

  private:
    void startWindow(Time at)
    {
        if (this->started_) {
            return;
        }
        std::ranges::fill(this->counts_, this->count_);
        std::ranges::fill(this->times_, at);
        this->index_ = 0;
        this->started_ = true;
    }

    FrequencyStatusParam  param_;
    std::mutex            mutex_;
    uint64_t              count_{0U};
    std::vector<uint64_t> counts_;
    std::vector<Time>     times_;
    size_t                index_{0U};
    bool                  started_{false};
};

/// @brief The limits of a TimeStampStatus: the age (now minus the stamp, in seconds) of a
///        stamp is acceptable from @p minAcceptable to @p maxAcceptable. The default allows a
///        stamp up to 1 s ahead and 5 s old.
struct TimeStampStatusParam
{
    double minAcceptable{-1.0};
    double maxAcceptable{5.0};
};

/// @brief ERROR when a stamp since the last update was too old, too new, zero or invalid;
///        WARN when none came.
class TimeStampStatus final : public DiagnosticTask
{
  public:
    explicit TimeStampStatus(
        const TimeStampStatusParam& param = {}, std::string name = "Timestamp Status"
    )
        : DiagnosticTask(std::move(name)), param_(param)
    {
    }

    /// @brief A message with @p stamp arrived or left, now (@p now, default hypha::now()).
    void tick(Time stamp, Time now)
    {
        const std::lock_guard<std::mutex> lock(this->mutex_);
        if (stamp.nanos() == 0) {
            this->zeroSeen_ = true;
            return;
        }
        const double delta = (now - stamp).seconds();
        this->minDelta_ = this->seen_ ? std::min(this->minDelta_, delta) : delta;
        this->maxDelta_ = this->seen_ ? std::max(this->maxDelta_, delta) : delta;
        this->seen_ = true;
    }

    void tick(Time stamp)
    {
        this->tick(stamp, hypha::now());
    }

    /// @brief A header stamp. One that is not a time (`nsecs` of a second or more) is an
    ///        ERROR at the next update.
    void tick(const hypha_msgs::RosTime& stamp)
    {
        const std::optional<Time> time = fromRosTime(stamp);
        if (!time) {
            const std::lock_guard<std::mutex> lock(this->mutex_);
            this->invalidSeen_ = true;
            return;
        }
        this->tick(*time);
    }

    void run(DiagnosticStatusWrapper& status) override
    {
        const std::lock_guard<std::mutex> lock(this->mutex_);
        if (!this->seen_) {
            status.summary(diagnostic_msgs::DiagnosticStatus::WARN, "No data since last update.");
        } else if (this->minDelta_ < this->param_.minAcceptable) {
            status.summary(
                diagnostic_msgs::DiagnosticStatus::ERROR,
                "Timestamp too far in future seen since last update."
            );
        } else if (this->maxDelta_ > this->param_.maxAcceptable) {
            status.summary(
                diagnostic_msgs::DiagnosticStatus::ERROR,
                "Timestamp too far in past seen since last update."
            );
        } else {
            status.summary(diagnostic_msgs::DiagnosticStatus::OK, "Timestamps are reasonable.");
        }
        if (this->zeroSeen_) {
            status.mergeSummary(
                diagnostic_msgs::DiagnosticStatus::ERROR,
                "Timestamp of zero was seen since last update."
            );
        }
        if (this->invalidSeen_) {
            status.mergeSummary(
                diagnostic_msgs::DiagnosticStatus::ERROR,
                "Invalid timestamp seen since last update."
            );
        }
        status.add("Earliest timestamp delay since last update (s)", this->minDelta_);
        status.add("Latest timestamp delay since last update (s)", this->maxDelta_);
        this->seen_ = false;
        this->minDelta_ = 0.0;
        this->maxDelta_ = 0.0;
        this->zeroSeen_ = false;
        this->invalidSeen_ = false;
    }

  private:
    TimeStampStatusParam param_;
    std::mutex           mutex_;
    bool                 seen_{false};
    bool                 zeroSeen_{false};
    bool                 invalidSeen_{false};
    double               minDelta_{0.0};
    double               maxDelta_{0.0};
};

/// @brief The rate check of one topic, added to @p updater under @p name.
///        @p updater must outlive it.
class HeaderlessTopicDiagnostic : public CompositeDiagnosticTask
{
  public:
    HeaderlessTopicDiagnostic(
        std::string name, DiagnosticUpdater& updater, const FrequencyStatusParam& frequency
    )
        : CompositeDiagnosticTask(std::move(name)), frequency_(frequency)
    {
        this->addTask(&this->frequency_);
        updater.add(*this);
    }

    /// @brief One message went by.
    void tick()
    {
        this->frequency_.tick();
    }

    void clearWindow()
    {
        this->frequency_.clear();
    }

  private:
    FrequencyStatus frequency_;
};

/// @brief The rate and the stamp age of one topic, added to @p updater under @p name.
class TopicDiagnostic final : public HeaderlessTopicDiagnostic
{
  public:
    TopicDiagnostic(
        std::string                 name,
        DiagnosticUpdater&          updater,
        const FrequencyStatusParam& frequency,
        const TimeStampStatusParam& stamp = {}
    )
        : HeaderlessTopicDiagnostic(std::move(name), updater, frequency), stamp_(stamp)
    {
        this->addTask(&this->stamp_);
    }

    /// @brief One message went by, with this header stamp.
    void tick(const hypha_msgs::RosTime& stamp)
    {
        this->stamp_.tick(stamp);
        HeaderlessTopicDiagnostic::tick();
    }

    void tick(Time stamp)
    {
        this->stamp_.tick(stamp);
        HeaderlessTopicDiagnostic::tick();
    }

  private:
    TimeStampStatus stamp_;
};

}  // namespace hypha
