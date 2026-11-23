#pragma once

/// @file
/// The part of ROS 2's diagnostic_updater (jazzy) the ported nodes use, on the shim's
/// rclcpp: an Updater that runs its tasks every period, on a timer of the node, and
/// publishes one DiagnosticArray on /diagnostics, each task's status named
/// `<node>: <task>` with the updater's hardware id, as ROS does; and the
/// DiagnosticStatusWrapper a task fills (summary, add). The `diagnostic_updater.period`
/// parameter of ROS is not read: the period is the one code gives.

#include <diagnostic_msgs/msg/diagnostic_array.hpp>
#include <diagnostic_msgs/msg/diagnostic_status.hpp>
#include <diagnostic_msgs/msg/key_value.hpp>
#include <rclcpp/rclcpp.hpp>

#include <chrono>
#include <functional>
#include <sstream>
#include <string>
#include <type_traits>
#include <utility>
#include <vector>

namespace diagnostic_updater {

/// A DiagnosticStatus a task fills.
class DiagnosticStatusWrapper : public diagnostic_msgs::msg::DiagnosticStatus
{
  public:
    void summary(unsigned char lvl, const std::string& msg)
    {
        this->level = lvl;
        this->message = msg;
    }

    void summary(const diagnostic_msgs::msg::DiagnosticStatus& src)
    {
        this->summary(src.level, src.message);
    }

    /// A key and its value, formatted as ROS's does: a bool as True or False, anything
    /// else through a stream.
    template <class T>
    void add(const std::string& key, const T& val)
    {
        diagnostic_msgs::msg::KeyValue entry;
        entry.key = key;
        if constexpr (std::is_same_v<T, bool>) {
            entry.value = val ? "True" : "False";
        } else {
            std::ostringstream text;
            text << val;
            entry.value = text.str();
        }
        this->values.push_back(entry);
    }

    void add(const std::string& key, const char* val)
    {
        this->add(key, std::string(val));
    }
};

using TaskFunction = std::function<void(DiagnosticStatusWrapper&)>;

class Updater
{
  public:
    explicit Updater(rclcpp::Node* node, double period = 1.0)
        : node_(node)
        , publisher_(
              node->create_publisher<diagnostic_msgs::msg::DiagnosticArray>("/diagnostics", 1)
          )
    {
        this->setPeriod(period);
    }

    void setHardwareID(const std::string& hwid)
    {
        this->hardware_id_ = hwid;
    }

    void add(const std::string& name, TaskFunction function)
    {
        this->tasks_.push_back({name, std::move(function)});
    }

    template <class T>
    void add(const std::string& name, T* object, void (T::*method)(DiagnosticStatusWrapper&))
    {
        this->add(name, [object, method](DiagnosticStatusWrapper& stat) {
            (object->*method)(stat);
        });
    }

    /// The period of the updates, in seconds.
    void setPeriod(double period)
    {
        this->timer_ = rclcpp::create_timer(
            this->node_, this->node_->get_clock(), rclcpp::Duration::from_seconds(period), [this] {
                this->update();
            }
        );
    }

    /// Runs the tasks and publishes now.
    void force_update()
    {
        this->update();
    }

  private:
    struct Task
    {
        std::string  name;
        TaskFunction function;
    };

    void update()
    {
        diagnostic_msgs::msg::DiagnosticArray array;
        array.header.stamp = this->node_->now();
        for (const Task& task : this->tasks_) {
            DiagnosticStatusWrapper status;
            status.name = std::string(this->node_->get_name()) + ": " + task.name;
            status.hardware_id = this->hardware_id_;
            task.function(status);
            array.status.push_back(status);
        }
        this->publisher_->publish(array);
    }

    rclcpp::Node*                                                       node_;
    rclcpp::Publisher<diagnostic_msgs::msg::DiagnosticArray>::SharedPtr publisher_;
    rclcpp::TimerBase::SharedPtr                                        timer_;
    std::string                                                         hardware_id_;
    std::vector<Task>                                                   tasks_;
};

}  // namespace diagnostic_updater
