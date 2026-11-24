/// Frozen speed readings with explicit missing/stale verdicts and cycle outputs.
#include <hypha/hypha.h>

#include <std_msgs/Bool.h>
#include <std_msgs/Float64.h>

#include <cmath>

HYPHA_LOG_MODULE_INIT("speed_monitor")

namespace {
enum class Warning : uint32_t { OverLimit = 1, InputStale = 2, NeverSeen = 4 };

/// Sets or clears one warning bit. The enumerator is the bit.
void setWarning(uint32_t& warnings, Warning warning, bool on)
{
    const uint32_t bit = static_cast<uint32_t>(warning);
    warnings = on ? (warnings | bit) : (warnings & ~bit);
}

bool hasWarning(uint32_t warnings, Warning warning)
{
    return (warnings & static_cast<uint32_t>(warning)) != 0;
}
}  // namespace

namespace {
class SpeedMonitor final : public hypha::CycleNode
{
  public:
    hypha::Latest<std_msgs::Float64> speed{*this, "speed"};
    hypha::Output<std_msgs::Bool>    overLimit{*this, "status"};
    hypha::Output<std_msgs::Float64> debug{*this, "debug"};

    void setup(hypha::Node&) override
    {
        this->limit_ =
            hypha::config().optional<double>("speed_limit", 20.0, hypha::constraints::finite());
        this->debugEnabled_ = hypha::config().optional<bool>("debug_enabled", false);
    }

    void step(const hypha::CycleTime&) override
    {
        uint32_t warnings = 0;
        // usable() reports the age verdict. It does not check the value.
        const bool usable = this->speed.usable();
        const bool finite = usable && std::isfinite(this->speed->data);
        const bool nonFinite = usable && !finite;
        if (!usable) {
            setWarning(
                warnings, Warning::InputStale, this->speed.verdict() == hypha::InputVerdict::Stale
            );
            setWarning(
                warnings,
                Warning::NeverSeen,
                this->speed.verdict() == hypha::InputVerdict::NeverArrived
            );
        }
        HYPHA_LOG_ERROR_WHEN_THROTTLE(!usable, false, 5.0, "speed unavailable [0x{:x}]", warnings);
        HYPHA_LOG_ERROR_WHEN_THROTTLE(
            nonFinite,
            false,
            5.0,
            "speed value {} is not finite: treated as unavailable",
            this->speed->data
        );
        if (!finite) {
            ++this->unavailableCycles_;
            // No Bool can express not-available. Do not publish a false all-clear.
            return;
        }
        const double value = this->speed->data;
        setWarning(warnings, Warning::OverLimit, value > this->limit_);
        HYPHA_LOG_WARN_WHEN(
            hasWarning(warnings, Warning::OverLimit),
            false,
            "over limit: {} > {}",
            value,
            this->limit_
        );
        HYPHA_LOG_INFO_THROTTLE(5.0, "speed {} m/s [0x{:x}]", value, warnings);
        this->overLimit.send({.data = hasWarning(warnings, Warning::OverLimit)});
        if (this->debugEnabled_) {
            this->debug.send({.data = value});
        }
    }

  private:
    double   limit_{0.0};
    bool     debugEnabled_{false};
    uint64_t unavailableCycles_{0};
};
}  // namespace

int main(int argc, char** argv)
{
    return hypha::runNode<SpeedMonitor>(argc, argv, "speed_monitor");
}
