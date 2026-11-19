// hypha_fusion_sensors: one composed cycle per [[sensors]] entry, at base_hz * rate_factor. Each
// cycle publishes Float64 on the entry's topic. A sensor publishes nothing while its
// silence window covers the cycle time. The window is [silence_start_s, silence_end_s),
// in seconds since this process started, and an entry without it is never silent.
// The file is the root parameter file: it holds base_hz and the list.

#include <hypha/cycle_runtime.h>
#include <hypha/hypha.h>
#include <hypha_msgs/std_msgs/Float64.h>

#include <cstdint>
#include <cstdio>
#include <exception>
#include <memory>
#include <optional>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

namespace {

struct SilenceWindow
{
    double fromSeconds;
    double untilSeconds;
};

/// @brief The silence window of entry @p index. Both keys or neither: one key alone throws.
std::optional<SilenceWindow> silenceOf(const hypha::ConfigView& entry)
{
    const std::optional<double> fromSeconds =
        entry.optional<double>("silence_start_s", hypha::constraints::finite());
    const std::optional<double> untilSeconds =
        entry.optional<double>("silence_end_s", hypha::constraints::finite());
    const bool neitherSet = !fromSeconds.has_value() && !untilSeconds.has_value();
    if (neitherSet) {
        return std::nullopt;
    }
    const bool bothSet = fromSeconds.has_value() && untilSeconds.has_value();
    if (!bothSet) {
        throw std::runtime_error(
            entry.name() + ": silence_start_s and silence_end_s must be set together"
        );
    }
    const bool validWindow = *fromSeconds >= 0.0 && *untilSeconds >= *fromSeconds;
    if (!validWindow) {
        throw std::runtime_error(entry.name() + ": silence times must be nonnegative and ordered");
    }
    return SilenceWindow{*fromSeconds, *untilSeconds};
}

struct SensorStep
{
    hypha::Publisher<std_msgs::Float64> publisher;
    hypha::Time                         processStart;
    size_t                              index;
    double                              value;
    std::optional<SilenceWindow>        silence;
    uint64_t*                           published;

    void operator()(const hypha::CycleTime& time)
    {
        const double elapsedSeconds = (time.now - this->processStart).seconds();
        const bool   inSilence = this->silence.has_value()
                              && elapsedSeconds >= this->silence->fromSeconds
                              && elapsedSeconds < this->silence->untilSeconds;
        if (inSilence) {
            return;
        }
        std_msgs::Float64 message;
        message.data = this->value;
        const bool sent = this->publisher.publish(message);
        if (!sent) {
            throw std::runtime_error("sensor " + std::to_string(this->index) + ": publish refused");
        }
        ++*this->published;
    }
};

/// Prints the cumulative count of published samples once per second, so the checker can
/// compare what the sensors sent with what the fusion node received.
struct PublishReport
{
    hypha::Time     processStart;
    const uint64_t* published;

    void operator()(const hypha::CycleTime& time) const
    {
        const double elapsedSeconds = (time.now - this->processStart).seconds();
        std::printf(
            "sensors elapsed_s=%.3f published=%llu\n",
            elapsedSeconds,
            static_cast<unsigned long long>(*this->published)
        );
        std::fflush(stdout);
    }
};

}  // namespace

int main(int argc, char** argv)
{
    const bool wrongArguments = argc != 2;
    if (wrongArguments) {
        std::fprintf(stderr, "usage: hypha_fusion_sensors <parameter_file>\n");
        return 2;
    }
    try {
        hypha::Node  node("sensor_fusion_sensors", argv[1]);
        const auto   sensors = hypha::config().list("sensors");
        const size_t count = sensors.size();
        const double baseHz =
            hypha::config().required<double>("base_hz", hypha::constraints::positive());
        const bool noSensors = count == 0;
        if (noSensors) {
            std::fprintf(stderr, "hypha_fusion_sensors: the list 'sensors' is empty\n");
            return 1;
        }

        const hypha::Time                          processStart = hypha::now();
        uint64_t                                   published = 0;
        std::vector<std::unique_ptr<hypha::Cycle>> cycles;
        for (size_t index = 0; index < count; ++index) {
            const auto        entry = sensors.at(index);
            const std::string topic =
                entry.required<std::string>("topic", hypha::constraints::nonempty());
            const double rateFactor =
                entry.required<double>("rate_factor", hypha::constraints::positive());
            auto cycle = std::make_unique<hypha::Cycle>("sensor_" + std::to_string(index));
            cycle->setRate(hypha::Hz(baseHz * rateFactor));
            SensorStep step{
                hypha::Node().advertise<std_msgs::Float64>(hypha::Topic{topic}),
                processStart,
                index,
                10.0 + static_cast<double>(index),
                silenceOf(entry),
                &published
            };
            hypha::require(cycle->start(node, std::move(step)));
            cycles.push_back(std::move(cycle));
        }
        hypha::Cycle reporter("sensor_report");
        reporter.setRate(hypha::Hz(1.0));
        hypha::require(reporter.start(node, PublishReport{processStart, &published}));

        std::printf("sensors started count=%zu\n", count);
        std::fflush(stdout);
        const hypha::RunResult result = node.run();
        return result.ok() ? 0 : 1;
    } catch (const std::exception& error) {
        std::fprintf(stderr, "hypha_fusion_sensors: %s\n", error.what());
        return 1;
    }
}
