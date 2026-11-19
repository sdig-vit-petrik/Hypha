// sensor_fusion_probe: the checker's reader. It prints each estimate and each node
// state of the fusion node for the number of seconds given on the command line:
//   estimate <ms> <value>
//   state <ms> <state>
// <ms> is the time since the probe started.

#include <hypha/hypha.h>
#include <hypha_msgs/std_msgs/Float64.h>
#include <hypha_msgs/std_msgs/UInt8.h>

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <exception>

namespace {

int64_t elapsedMillis(hypha::Time start)
{
    return (hypha::steadyNow() - start).nanos() / 1000000;
}

struct EstimatePrinter
{
    hypha::Time start;

    void operator()(const hypha::MessagePtr<std_msgs::Float64>& message) const
    {
        std::printf(
            "estimate %lld %.9f\n",
            static_cast<long long>(elapsedMillis(this->start)),
            message->data
        );
        std::fflush(stdout);
    }
};

struct StatePrinter
{
    hypha::Time start;

    void operator()(const hypha::MessagePtr<std_msgs::UInt8>& message) const
    {
        std::printf(
            "state %lld %u\n",
            static_cast<long long>(elapsedMillis(this->start)),
            static_cast<unsigned>(message->data)
        );
        std::fflush(stdout);
    }
};

}  // namespace

int main(int argc, char** argv)
{
    const bool wrongArguments = argc != 2;
    if (wrongArguments) {
        std::fprintf(stderr, "usage: sensor_fusion_probe <seconds>\n");
        return 2;
    }
    char*        end = nullptr;
    const double seconds = std::strtod(argv[1], &end);
    const bool   parsed = end != argv[1] && *end == '\0';
    const bool   validSeconds = parsed && seconds > 0.0;
    if (!validSeconds) {
        std::fprintf(stderr, "sensor_fusion_probe: seconds must be a positive number\n");
        return 2;
    }
    try {
        hypha::Node       node("sensor_fusion_probe");
        const hypha::Time start = hypha::steadyNow();
        // Every publisher notifies, so the run wakes per sample.
        hypha::Subscriber<std_msgs::Float64> estimates = hypha::Node().subscribe<std_msgs::Float64>(
            hypha::Topic{"/fusion/estimate"}, EstimatePrinter{start}
        );
        hypha::Subscriber<std_msgs::UInt8> states = hypha::Node().subscribe<std_msgs::UInt8>(
            hypha::Topic{"/sensor_fusion/input_health"}, StatePrinter{start}
        );
        const hypha::RunResult result = node.runFor(hypha::Duration::fromSeconds(seconds));
        return result.ok() ? 0 : 1;
    } catch (const std::exception& error) {
        std::fprintf(stderr, "sensor_fusion_probe: %s\n", error.what());
        return 1;
    }
}
