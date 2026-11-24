/// Periodic publication on the node executor also serves schema requests.
#include <hypha/hypha.h>

#include <std_msgs/Float64.h>

HYPHA_LOG_MODULE_INIT("speed_source")

namespace {
class SpeedSource final : public hypha::CycleNode
{
  public:
    hypha::Output<std_msgs::Float64> speed{*this, "speed"};

    void setup(hypha::Node&) override
    {
        const auto config = hypha::config();
        this->peak_ = config.optional<double>("peak", 25.0, hypha::constraints::positive());
        this->increment_ = config.optional<double>("step", 1.0, hypha::constraints::positive());
    }

    void step(const hypha::CycleTime&) override
    {
        this->speed.send({.data = this->current_});
        this->current_ += this->increment_;
        if (this->current_ > this->peak_) {
            this->current_ = 0.0;
        }
    }

  private:
    double peak_{0.0};
    double increment_{0.0};
    double current_{0.0};
};
}  // namespace

int main(int argc, char** argv)
{
    return hypha::runNode<SpeedSource>(argc, argv, "speed_source");
}
