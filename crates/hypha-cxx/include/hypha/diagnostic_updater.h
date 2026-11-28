#pragma once

/// @file
/// DiagnosticUpdater: the named status tasks of a node, published as one
/// `diagnostic_msgs/DiagnosticArray` for a ROS `diagnostic_aggregator`. It is ROS's
/// `diagnostic_updater::Updater` without the timer and the node handle: the node calls
/// update() in its step, and the array leaves on a port the node declares, so its topic
/// (`/diagnostics`) is a line of the parameter file, as every topic is. See
/// `docs/features.md` ("Application status").
///
/// ```cpp
/// hypha::Output<diagnostic_msgs::DiagnosticArray> diagnostics{*this, "diagnostics"};
/// hypha::DiagnosticUpdater                        updater{diagnostics};
/// // setup():  updater.setHardwareID("lidar");
/// //           updater.add("Connection", this, &Node::diagnoseConnection);
/// // step():   updater.update();
/// ```

#include "hypha/diagnostic_status.h"
#include "hypha/message_stamp.h"
#include "hypha/node.h"
#include "hypha_base/clock.h"
#include "hypha_base/time.h"

#include <diagnostic_msgs/DiagnosticArray.h>

#include <cmath>
#include <cstdint>
#include <exception>
#include <functional>
#include <optional>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

namespace hypha {

/// @brief A named check. run() fills the status of the task; it returns, or throws, and a
///        throw becomes an ERROR status that names the task and the reason.
class DiagnosticTask
{
  public:
    explicit DiagnosticTask(std::string name) : name_(std::move(name))
    {
    }

    DiagnosticTask(const DiagnosticTask&) = delete;
    DiagnosticTask& operator=(const DiagnosticTask&) = delete;
    virtual ~DiagnosticTask() = default;

    const std::string& name() const
    {
        return this->name_;
    }

    virtual void run(DiagnosticStatusWrapper& status) = 0;

  private:
    std::string name_;
};

/// @brief A task that calls a function.
class FunctionDiagnosticTask final : public DiagnosticTask
{
  public:
    using Function = std::function<void(DiagnosticStatusWrapper&)>;

    FunctionDiagnosticTask(std::string name, Function function)
        : DiagnosticTask(std::move(name)), function_(std::move(function))
    {
    }

    void run(DiagnosticStatusWrapper& status) override
    {
        this->function_(status);
    }

  private:
    Function function_;
};

/// @brief Several tasks reported as one: the worst level, the messages joined, all the
///        values.
class CompositeDiagnosticTask : public DiagnosticTask
{
  public:
    using DiagnosticTask::DiagnosticTask;

    /// @brief Add @p task. It must outlive this task.
    void addTask(DiagnosticTask* task)
    {
        if (task == nullptr) {
            throw std::invalid_argument("hypha: CompositeDiagnosticTask::addTask: null task");
        }
        this->tasks_.push_back(task);
    }

    void run(DiagnosticStatusWrapper& status) override
    {
        DiagnosticStatusWrapper combined;
        DiagnosticStatusWrapper original;
        original.summary(status);
        for (DiagnosticTask* task : this->tasks_) {
            status.summary(original);
            task->run(status);
            combined.mergeSummary(status);
        }
        status.summary(combined);
    }

  private:
    std::vector<DiagnosticTask*> tasks_;
};

/// @brief The tasks of one node, run every period, and the array they make.
///
/// One thread calls add(), update() and the other members: the step. A task that
/// another thread feeds (FrequencyStatus::tick()) is safe against that.
class DiagnosticUpdater
{
  public:
    using Function = FunctionDiagnosticTask::Function;

    /// @brief Publish on @p sink: an `Output<DiagnosticArray>` (it sends in the step that
    ///        calls update()), or a `Publisher<DiagnosticArray>` (a refused publish throws
    ///        std::runtime_error with the status). @p sink must outlive the updater.
    /// @param nodeName Prefix of every task name, as ROS writes `<node>: <task>`. Empty: the
    ///        name of the node, read at the first update.
    template <typename Sink>
    explicit DiagnosticUpdater(Sink& sink, std::string nodeName = {})
        : send_([&sink](const diagnostic_msgs::DiagnosticArray& array) {
            if constexpr (requires { sink.send(array); }) {
                sink.send(array);
            } else {
                if (!sink.publish(array)) {
                    throw std::runtime_error(
                        "hypha: diagnostics: publish refused: "
                        + std::string(sink.lastPublishStatus().name())
                    );
                }
            }
        })
        , nodeName_(std::move(nodeName))
    {
    }

    DiagnosticUpdater(const DiagnosticUpdater&) = delete;
    DiagnosticUpdater& operator=(const DiagnosticUpdater&) = delete;

    /// @brief The `hardware_id` of every status; ROS's default is "unspecified".
    void setHardwareID(std::string hardwareId)
    {
        this->hardwareId_ = std::move(hardwareId);
    }

    /// @brief Time between two arrays; default 1 s (ROS's `diagnostic_period`).
    /// @throws std::invalid_argument unless positive and finite.
    void setPeriod(Duration period)
    {
        if (period.nanos() <= 0) {
            throw std::invalid_argument("hypha: DiagnosticUpdater: the period must be positive");
        }
        this->period_ = period;
    }

    void setPeriod(double seconds)
    {
        this->setPeriod(Duration::fromSeconds(seconds));
    }

    /// @brief Add a task. A name may repeat, as in ROS. @p task must outlive the updater.
    void add(DiagnosticTask& task)
    {
        this->tasks_.push_back({task.name(), [&task](DiagnosticStatusWrapper& status) {
                                    task.run(status);
                                }});
    }

    void add(const std::string& name, Function function)
    {
        this->tasks_.push_back({name, std::move(function)});
    }

    template <typename T>
    void add(const std::string& name, T* object, void (T::*method)(DiagnosticStatusWrapper&))
    {
        this->add(name, [object, method](DiagnosticStatusWrapper& status) {
            (object->*method)(status);
        });
    }

    /// @brief Remove the tasks of this name.
    /// @return Whether there was one.
    bool removeByName(const std::string& name)
    {
        const size_t before = this->tasks_.size();
        std::erase_if(this->tasks_, [&name](const Entry& entry) {
            return entry.name == name;
        });
        return this->tasks_.size() != before;
    }

    /// @brief Run the tasks and send the array when a period has passed since the last one;
    ///        the first array leaves one period after the first call, as ROS's timer does.
    ///        @p now is the framework time (`CycleTime::now`); the other form reads
    ///        hypha::now(), which in a step is the step's time.
    void update(Time now)
    {
        const bool armed = this->next_.has_value();
        if (!armed || now < *this->next_ - this->period_) {
            // First call, or time went back: start the period again from here.
            this->next_ = now + this->period_;
            return;
        }
        if (now >= *this->next_) {
            this->forceUpdate(now);
        }
    }

    void update()
    {
        this->update(hypha::now());
    }

    /// @brief Run the tasks and send the array now.
    void forceUpdate(Time now)
    {
        diagnostic_msgs::DiagnosticArray array;
        array.header.stamp = toRosTime(now);
        for (const Entry& entry : this->tasks_) {
            array.status.push_back(this->runTask(entry, now));
        }
        this->next_ = now + this->period_;
        this->send_(array);
    }

    void forceUpdate()
    {
        this->forceUpdate(hypha::now());
    }

    /// @brief Send one status of @p level and @p message for each task, without running it:
    ///        what ROS's `broadcast` does when the node knows all its tasks are in one state.
    void broadcast(int8_t level, const std::string& message, Time now)
    {
        diagnostic_msgs::DiagnosticArray array;
        array.header.stamp = toRosTime(now);
        for (const Entry& entry : this->tasks_) {
            DiagnosticStatusWrapper status;
            this->fill(status, entry, now);
            status.summary(level, message);
            array.status.push_back(this->checked(std::move(status)));
        }
        this->send_(array);
    }

    void broadcast(int8_t level, const std::string& message)
    {
        this->broadcast(level, message, hypha::now());
    }

  private:
    struct Entry
    {
        std::string name;
        Function    function;
    };

    void fill(DiagnosticStatusWrapper& status, const Entry& entry, Time now)
    {
        if (this->nodeName_.empty()) {
            this->nodeName_ = hypha::nodeName();
        }
        if (this->nodeName_.empty()) {
            throw std::logic_error(
                "hypha: DiagnosticUpdater: the process has no node name; pass one to the updater"
            );
        }
        status.name = this->nodeName_ + ": " + entry.name;
        status.hardware_id = this->hardwareId_;
        status.setNow(now);
    }

    /// A level outside the four of the message is the task's fault, not a level.
    static diagnostic_msgs::DiagnosticStatus checked(DiagnosticStatusWrapper status)
    {
        const bool known = status.level >= diagnostic_msgs::DiagnosticStatus::OK
                        && status.level <= diagnostic_msgs::DiagnosticStatus::STALE;
        if (!known) {
            status.message =
                "invalid level " + std::to_string(status.level) + ": " + status.message;
            status.level = diagnostic_msgs::DiagnosticStatus::ERROR;
        }
        return status;
    }

    diagnostic_msgs::DiagnosticStatus runTask(const Entry& entry, Time now)
    {
        DiagnosticStatusWrapper status;
        this->fill(status, entry, now);
        // A task that sets nothing is a fault, as in ROS.
        status.summary(diagnostic_msgs::DiagnosticStatus::ERROR, "No message was set");
        try {
            entry.function(status);
        } catch (const std::exception& error) {
            status.clear();
            status.summary(
                diagnostic_msgs::DiagnosticStatus::ERROR,
                std::string("Exception in diagnostic task: ") + error.what()
            );
        } catch (...) {
            status.clear();
            status.summary(
                diagnostic_msgs::DiagnosticStatus::ERROR, "Exception in diagnostic task: unknown"
            );
        }
        return checked(std::move(status));
    }

    std::function<void(const diagnostic_msgs::DiagnosticArray&)> send_;
    std::string                                                  nodeName_;
    std::string                                                  hardwareId_{"unspecified"};
    Duration            period_{Duration::fromSeconds(1.0)};
    std::optional<Time> next_;
    std::vector<Entry>  tasks_;
};

}  // namespace hypha
