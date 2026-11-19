/// The simulated clock. Publishes rosgraph_msgs/Clock on /clock: one message per
/// timer fire, each adding one step, with one forward jump from the TOML schedule.
/// Stops after the last message. It never goes back: a backward seek restarts the
/// participants of a replay instead (hypha play).
#include <hypha/hypha.h>
#include <hypha_msgs/rosgraph_msgs/Clock.h>

#include <cstdint>
#include <cstdio>
#include <exception>
#include <optional>

HYPHA_LOG_MODULE_INIT("sim_clock")

namespace {

/// The schedule the TOML file gives, all in nanoseconds and message counts.
struct ClockSchedule
{
    int64_t  stepNanos;
    int64_t  paceNanos;
    uint64_t messages;
    uint64_t forwardAfter;
    int64_t  forwardNanos;
};

ClockSchedule readSchedule()
{
    ClockSchedule schedule{};
    schedule.stepNanos = hypha::config().required<int64_t>("step_ns");
    schedule.paceNanos = hypha::config().required<int64_t>("pace_ns");
    schedule.messages = hypha::config().required<uint64_t>("messages");
    schedule.forwardAfter = hypha::config().required<uint64_t>("forward_after");
    schedule.forwardNanos = hypha::config().required<int64_t>("forward_ns");
    return schedule;
}

/// Publishes the next clock value. Holds the simulated time between timer fires.
class SimClock
{
  public:
    SimClock(const ClockSchedule& schedule, hypha::Publisher<rosgraph_msgs::Clock> publisher)
        : schedule_(schedule), publisher_(std::move(publisher))
    {
    }

    void tick()
    {
        if (this->published_ >= this->schedule_.messages) {
            hypha::requestStop();
            return;
        }
        ++this->published_;
        this->nanos_ += this->schedule_.stepNanos;
        if (this->published_ == this->schedule_.forwardAfter) {
            this->nanos_ += this->schedule_.forwardNanos;
        }
        rosgraph_msgs::Clock message;
        message.clock = hypha::toRosTime(hypha::Time::fromNanos(this->nanos_));
        const bool sent = this->publisher_.publish(message);
        const bool notSent = !sent;
        if (notSent) {
            this->unsent_ += 1U;
        }
        HYPHA_LOG_ERROR_WHEN_THROTTLE(
            notSent,
            false,
            5.0,
            "clock message {} not sent ({} so far)",
            this->published_,
            this->unsent_
        );
    }

    uint64_t published() const
    {
        return this->published_;
    }

    uint64_t unsent() const
    {
        return this->unsent_;
    }

    int64_t nanos() const
    {
        return this->nanos_;
    }

  private:
    ClockSchedule                          schedule_;
    hypha::Publisher<rosgraph_msgs::Clock> publisher_;
    uint64_t                               published_{0};
    uint64_t                               unsent_{0};
    int64_t                                nanos_{0};
};

}  // namespace

int main(int argc, char** argv)
{
    if (argc != 2) {
        std::fprintf(stderr, "usage: hypha_sim_clock <parameter file>\n");
        return 2;
    }
    try {
        hypha::Node         node("sim_clock", argv[1]);
        const ClockSchedule schedule = readSchedule();
        SimClock            clock(
            schedule, hypha::Node().advertise<rosgraph_msgs::Clock>(hypha::Topic{"/clock"})
        );
        hypha::Timer pacer = node.timer(
            hypha::Duration::fromNanos(schedule.paceNanos), hypha::bind<&SimClock::tick>(clock)
        );
        std::printf("sim_clock: ready\n");
        std::fflush(stdout);
        const hypha::RunResult result = node.run();
        std::printf(
            "summary,clock,messages=%llu,unsent=%llu,final_ns=%lld\n",
            static_cast<unsigned long long>(clock.published()),
            static_cast<unsigned long long>(clock.unsent()),
            static_cast<long long>(clock.nanos())
        );
        // A refused clock message means the twin lost a time step, so the run fails.
        const bool runOk = result.ok();
        const bool allSent = clock.unsent() == 0U;
        return (runOk && allSent) ? 0 : 1;
    } catch (const std::exception& error) {
        std::fprintf(stderr, "sim_clock: %s\n", error.what());
        return 1;
    }
}
