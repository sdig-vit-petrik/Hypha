// hypha_fusion: one composed cycle at fusion_hz over sensor_count Latest inputs, the ports
// sensor_0 to sensor_<n-1>. The estimate is the mean of the usable inputs that are not
// ignored. Latest, not Arrivals: the estimate needs the newest value of each sensor.

#include <hypha/cycle_runtime.h>
#include <hypha/hypha.h>

#include <std_msgs/Float64.h>
#include <std_msgs/UInt8.h>

#include <cmath>
#include <cstdint>
#include <cstdio>
#include <deque>
#include <optional>
#include <string>

HYPHA_LOG_MODULE_INIT("sensor_fusion")

namespace {

using SensorInput = hypha::Latest<std_msgs::Float64>;

/// Every input is usable.
constexpr uint8_t STATE_INPUTS_USABLE = 0;
/// An input never arrived.
constexpr uint8_t STATE_WAITING = 1;
/// An input is stale, and none never arrived.
constexpr uint8_t STATE_DEGRADED = 2;

struct SensorEntry
{
    SensorInput input;
    std::string port;

    SensorEntry(hypha::Cycle& cycle, const std::string& portName)
        : input(cycle, portName), port(portName)
    {
    }
};

/// The node state this step's input verdicts imply. An ignored input still counts: ignoring
/// hides its supervision fault, not its age.
uint8_t stateOf(const std::deque<SensorEntry>& entries)
{
    bool waiting = false;
    bool stale = false;
    for (const SensorEntry& entry : entries) {
        const hypha::InputVerdict verdict = entry.input.verdict();
        if (verdict == hypha::InputVerdict::NeverArrived) {
            waiting = true;
        }
        if (verdict == hypha::InputVerdict::Stale) {
            stale = true;
        }
    }
    if (waiting) {
        return STATE_WAITING;
    }
    if (stale) {
        return STATE_DEGRADED;
    }
    return STATE_INPUTS_USABLE;
}

struct FusionStep
{
    std::deque<SensorEntry>*          entries;
    hypha::Output<std_msgs::Float64>* estimate;
    hypha::Output<std_msgs::UInt8>*   state;
    hypha::Time                       processStart;
    hypha::Time                       nextReport;

    void operator()(const hypha::CycleTime& time)
    {
        double sum = 0.0;
        size_t usable = 0;
        for (SensorEntry& entry : *this->entries) {
            // usable() reports the age verdict. It does not check the value. A NaN or infinite
            // value must not enter the mean.
            const bool inputUsable = entry.input.usable();
            const bool finite = inputUsable && std::isfinite(entry.input->data);
            const bool nonFinite = inputUsable && !finite;
            HYPHA_LOG_WARN_WHEN_THROTTLE(
                nonFinite,
                false,
                5.0,
                "input {} is not finite: excluded from the estimate",
                entry.port
            );
            const bool counts = finite && !entry.input.ignored();
            if (counts) {
                sum += entry.input->data;
                ++usable;
            }
        }
        const bool   haveValue = usable > 0;
        const double mean = haveValue ? sum / static_cast<double>(usable) : 0.0;
        const bool   meanFinite = std::isfinite(mean);
        const bool   sendEstimate = haveValue && meanFinite;
        if (sendEstimate) {
            this->estimate->send({.data = mean});
        }
        this->reportIfDue(time, usable);
    }

    /// Prints the input counters once per second: what the source accepted, and whether
    /// the transport dropped anything before it.
    void reportIfDue(const hypha::CycleTime& time, size_t usable)
    {
        const bool notDue = time.now < this->nextReport;
        if (notDue) {
            return;
        }
        this->nextReport = time.now + hypha::Duration::fromSeconds(1.0);
        this->state->send({.data = stateOf(*this->entries)});

        uint64_t received = 0;
        for (const SensorEntry& entry : *this->entries) {
            received += entry.input.received();
        }
        // The transport does not count the samples it drops.
        const std::string droppedText = "not-counted";
        const double      elapsedSeconds = (time.now - this->processStart).seconds();
        std::printf(
            "fusion elapsed_s=%.3f usable=%zu inputs=%zu received=%llu dropped=%s\n",
            elapsedSeconds,
            usable,
            this->entries->size(),
            static_cast<unsigned long long>(received),
            droppedText.c_str()
        );
        std::fflush(stdout);
    }
};

class SensorFusion final : public hypha::ApplicationNode
{
  public:
    void setup(hypha::Node& node) override
    {
        this->processStart_ = hypha::now();
        const hypha::ConfigView settings = hypha::config();
        const double            fusionHz =
            settings.required<double>("fusion_hz", hypha::constraints::positive());

        const auto sensorCount = settings.required<int64_t>("sensor_count");
        if (sensorCount < 1) {
            throw hypha::ParameterError("hypha: sensor_count must be at least 1");
        }

        this->cycle_.setRate(hypha::Hz(fusionHz));

        for (int64_t index = 0; index < sensorCount; ++index) {
            this->entries_.emplace_back(this->cycle_, "sensor_" + std::to_string(index));
        }
        std::printf("sensor_list count=%lld\n", static_cast<long long>(sensorCount));
        std::fflush(stdout);
        hypha::require(this->cycle_.start(
            node,
            FusionStep{
                &this->entries_, &this->estimate_, &this->state_, this->processStart_, hypha::Time{}
            }
        ));
    }

  private:
    // The cycle outlives its ports.
    hypha::Cycle                     cycle_{"fusion"};
    hypha::Output<std_msgs::Float64> estimate_{cycle_, "topic_estimate"};
    hypha::Output<std_msgs::UInt8>   state_{cycle_, "topic_state"};
    std::deque<SensorEntry>          entries_;
    hypha::Time                      processStart_{};
};

}  // namespace

int main(int argc, char** argv)
{
    return hypha::runNode<SensorFusion>(argc, argv, "sensor_fusion");
}
