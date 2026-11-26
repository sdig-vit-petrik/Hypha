// The shape of twist_mux as one triggered cycle: velocity inputs that each trigger the
// step, in priority order, and locks that mask the velocities below their priority. The
// step asks no clock and never which port fired. The priorities come from the test.

#include "hypha/cycle_node.h"
#include "hypha/hypha.h"
#include "hypha/testing.h"

#include "test_types.h"

#include <gtest/gtest.h>
#include <geometry_msgs/Twist.h>
#include <hypha_msgs/NodeState.h>
#include <std_msgs/Bool.h>

#include <algorithm>
#include <chrono>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <utility>
#include <vector>

using geometry_msgs::Twist;
using std_msgs::Bool;
using hypha::Time;
using namespace hypha::literals;

namespace {

/// A port of the mux with its priority.
struct Prioritized
{
    std::string name;
    int64_t     priority;
};

class TwistMux final : public hypha::CycleNode
{
  public:
    TwistMux(const std::vector<Prioritized>& velocityInputs, const std::vector<Prioritized>& lockInputs)
    {
        for (const Prioritized& input : velocityInputs) {
            this->velocities_.push_back(
                {input.name, input.priority, std::make_unique<Arrivals<Twist>>(*this, input.name, advisory)}
            );
        }
        for (const Prioritized& input : lockInputs) {
            this->locks_.push_back(
                {input.name, input.priority, std::make_unique<Latest<Bool>>(*this, input.name)}
            );
        }
        std::ranges::stable_sort(this->velocities_, std::greater{}, &Velocity::priority);
        for (const Velocity& velocity : this->velocities_) {
            this->setTrigger(*velocity.port);
        }
    }

    void step(const hypha::CycleTime&) override
    {
        const int64_t locked = this->lockedPriority();
        // Highest priority first: the first usable velocity that no lock masks wins, and
        // forwards only its own sample.
        for (const Velocity& velocity : this->velocities_) {
            if (velocity.priority < locked || !velocity.port->usable()) {
                continue;
            }
            if (!velocity.port->samples().empty()) {
                this->out.send(velocity.port->samples().front());
            }
            return;
        }
    }

    /// The velocity port named @p name.
    Arrivals<Twist>& velocity(const std::string& name)
    {
        return *this->find(this->velocities_, name).port;
    }

    /// The lock port named @p name.
    Latest<Bool>& lock(const std::string& name)
    {
        return *this->find(this->locks_, name).port;
    }

    Output<Twist> out{*this, "cmd_vel_out"};

  private:
    struct Velocity
    {
        std::string                      name;
        int64_t                          priority;
        std::unique_ptr<Arrivals<Twist>> port;
    };

    struct Lock
    {
        std::string                   name;
        int64_t                       priority;
        std::unique_ptr<Latest<Bool>> port;
    };

    template <typename Entry>
    static Entry& find(std::vector<Entry>& entries, const std::string& name)
    {
        const auto found = std::ranges::find_if(entries, [&](const Entry& entry) {
            return entry.name == name;
        });
        if (found == entries.end()) {
            throw std::invalid_argument("no port " + name);
        }
        return *found;
    }

    /// The highest priority a lock masks: a lock that is not usable counts as locked.
    int64_t lockedPriority() const
    {
        int64_t locked = 0;
        for (const Lock& lock : this->locks_) {
            if (!lock.port->usable() || (*lock.port)->data) {
                locked = std::max(locked, lock.priority);
            }
        }
        return locked;
    }

    std::vector<Velocity> velocities_;
    std::vector<Lock>     locks_;
};

std::string topicFor(const std::string& name)
{
    return hypha_test::testTopic("/mux_" + name);
}

/// The parameter file of the mux: 500 ms on each velocity, 1 s on the lock.
std::string parameters()
{
    std::string text;
    for (const auto& [port, silenceMs] :
         std::vector<std::pair<std::string, int>>{{"joystick", 500}, {"navigation", 500}, {"e_stop", 1000}}) {
        text += "[ports.inputs." + port + "]\ntopic='" + topicFor(port) + "'\nsilence_ms = "
                + std::to_string(silenceMs) + "\n";
    }
    return text + "[ports.outputs.cmd_vel_out]\ntopic='" + topicFor("out") + "'\n";
}

const std::vector<Prioritized> kVelocities{{"navigation", 10}, {"joystick", 100}};
const std::vector<Prioritized> kStop{{"e_stop", 255}};

Twist twist(double x)
{
    Twist message;
    message.linear.x = x;
    return message;
}

constexpr int64_t kNow = 10'000'000'000;

hypha::CycleTime at(int64_t nanos)
{
    hypha::CycleTime time;
    time.now = Time::fromNanos(nanos);
    return time;
}

Time before(int64_t millis)
{
    return Time::fromNanos(kNow - millis * 1'000'000);
}

/// The x of each twist the last step sent.
std::vector<double> sentX(const hypha::testing::StepHarness& harness, const TwistMux& mux)
{
    std::vector<double> xs;
    for (const Twist& message : harness.sent(mux.out)) {
        xs.push_back(message.linear.x);
    }
    return xs;
}

class TwistMuxHarness : public ::testing::Test
{
  protected:
    void SetUp() override
    {
        this->path = std::filesystem::temp_directory_path() / "hypha_twist_mux_harness.toml";
        std::ofstream(this->path) << parameters();
        this->context = std::make_unique<hypha::ConfigContext>(this->path.string());
    }

    void TearDown() override
    {
        this->context.reset();
        std::filesystem::remove(this->path);
    }

    std::unique_ptr<hypha::ConfigContext> context;
    std::filesystem::path                 path;
};

}  // namespace

TEST_F(TwistMuxHarness, HigherPriorityUsableSuppressesLower)
{
    TwistMux                    mux(kVelocities, kStop);
    hypha::testing::StepHarness harness{mux};
    harness.set(mux.lock("e_stop"), Bool{});
    harness.deliver(mux.velocity("joystick"), {}, before(100));
    harness.deliver(mux.velocity("navigation"), {twist(2.0)}, before(0));
    harness.step(mux, at(kNow));
    EXPECT_TRUE(sentX(harness, mux).empty());
}

TEST_F(TwistMuxHarness, ExpiredHigherForwardsLower)
{
    TwistMux                    mux(kVelocities, kStop);
    hypha::testing::StepHarness harness{mux};
    harness.set(mux.lock("e_stop"), Bool{});
    harness.deliver(mux.velocity("joystick"), {}, before(600));
    harness.deliver(mux.velocity("navigation"), {twist(2.0)}, before(0));
    harness.step(mux, at(kNow));
    EXPECT_EQ(sentX(harness, mux), std::vector<double>{2.0});
}

TEST_F(TwistMuxHarness, ATieAtOneInstantForwardsOnlyTheHigher)
{
    // Two steps at one instant, in either port order: the joystick's tied sample counts for
    // the navigation step, so only the joystick is forwarded.
    for (const bool joystickFirst : {false, true}) {
        TwistMux                    mux(kVelocities, kStop);
        hypha::testing::StepHarness harness{mux};
        harness.set(mux.lock("e_stop"), Bool{});
        std::vector<double> forwarded;
        const auto          fire = [&](const std::string& firing, const std::string& other, double x) {
            harness.deliver(mux.velocity(firing), {twist(x)}, before(0));
            harness.deliver(mux.velocity(other), {}, before(0));
            harness.step(mux, at(kNow));
            const std::vector<double> sent = sentX(harness, mux);
            forwarded.insert(forwarded.end(), sent.begin(), sent.end());
        };
        if (joystickFirst) {
            fire("joystick", "navigation", 1.0);
            fire("navigation", "joystick", 2.0);
        } else {
            fire("navigation", "joystick", 2.0);
            fire("joystick", "navigation", 1.0);
        }
        EXPECT_EQ(forwarded, std::vector<double>{1.0}) << "joystick first: " << joystickFirst;
    }
}

TEST_F(TwistMuxHarness, ExpiredAndNeverArrivedLocksCountAsLocked)
{
    TwistMux                    mux(kVelocities, kStop);
    hypha::testing::StepHarness harness{mux};
    harness.deliver(mux.velocity("joystick"), {}, before(600));
    harness.setStale(mux.lock("e_stop"));
    harness.deliver(mux.velocity("navigation"), {twist(2.0)}, before(0));
    harness.step(mux, at(kNow));
    EXPECT_TRUE(sentX(harness, mux).empty()) << "a stale lock";
    harness.setNeverArrived(mux.lock("e_stop"));
    harness.deliver(mux.velocity("navigation"), {twist(2.0)}, before(0));
    harness.step(mux, at(kNow));
    EXPECT_TRUE(sentX(harness, mux).empty()) << "a lock that never arrived";
    harness.set(mux.lock("e_stop"), Bool{});
    harness.deliver(mux.velocity("navigation"), {twist(2.0)}, before(0));
    harness.step(mux, at(kNow));
    EXPECT_EQ(sentX(harness, mux), std::vector<double>{2.0}) << "an open lock";
}

TEST_F(TwistMuxHarness, ALockMasksOnlyLowerPriorities)
{
    const std::vector<Prioritized> middle{{"e_stop", 50}};
    TwistMux                       mux(kVelocities, middle);
    hypha::testing::StepHarness    harness{mux};
    Bool                           engaged;
    engaged.data = true;
    harness.set(mux.lock("e_stop"), engaged);
    harness.deliver(mux.velocity("joystick"), {}, before(600));
    harness.deliver(mux.velocity("navigation"), {twist(2.0)}, before(0));
    harness.step(mux, at(kNow));
    EXPECT_TRUE(sentX(harness, mux).empty()) << "navigation is below the lock";
    harness.deliver(mux.velocity("joystick"), {twist(1.0)}, before(0));
    harness.deliver(mux.velocity("navigation"), {}, before(0));
    harness.step(mux, at(kNow));
    EXPECT_EQ(sentX(harness, mux), std::vector<double>{1.0}) << "the joystick is above it";
}

TEST_F(TwistMuxHarness, AFallbackForwardsNothing)
{
    TwistMux                    mux(kVelocities, kStop);
    hypha::testing::StepHarness harness{mux};
    harness.set(mux.lock("e_stop"), Bool{});
    harness.deliver(mux.velocity("joystick"), {}, before(600));
    harness.deliver(mux.velocity("navigation"), {}, before(100));
    harness.step(mux, at(kNow));
    EXPECT_TRUE(sentX(harness, mux).empty());
}

namespace {

/// A node for the mux on the bus, under simulated time when @p simulated.
class TwistMuxBus : public ::testing::Test
{
  protected:
    void start(bool simulated)
    {
        this->path = std::filesystem::temp_directory_path() / "hypha_twist_mux_bus.toml";
        std::ofstream(this->path) << parameters();
        if (simulated) {
            ASSERT_TRUE(hypha::enableSimTime(Time::fromNanos(kNow)));
        }
        this->node.emplace("twist_mux_shape", this->path.string());
    }

    void TearDown() override
    {
        this->node.reset();
        static_cast<void>(hypha::disableSimTime());
        std::filesystem::remove(this->path);
    }

    template <typename M>
    hypha::Publisher<M> publisherOf(const std::string& port)
    {
        return hypha::Node().advertise<M>(hypha::Topic{topicFor(port)});
    }

    /// Runs the node in 1 ms slices until @p done holds, or a real-time bound passes.
    bool runUntil(const std::function<bool()>& done)
    {
        const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(10);
        while (!done()) {
            if (std::chrono::steady_clock::now() >= deadline || !this->node->runFor(1_ms).ok()) {
                return false;
            }
        }
        return true;
    }

    std::optional<hypha::Node> node;
    std::filesystem::path      path;
};

/// The node's state, as `~/state` reports it now.
uint8_t nodeStateNow()
{
    const std::expected<hypha_msgs::NodeState, hypha::Status> state = hypha::nodeState();
    EXPECT_TRUE(state.has_value());
    return state.has_value() ? state->state : hypha_msgs::NodeState::DEGRADED;
}

struct Collect
{
    std::vector<double>* xs;

    void operator()(const hypha::Subscriber<Twist>::ConstPtr& message) const
    {
        this->xs->push_back(message->linear.x);
    }
};

}  // namespace

TEST_F(TwistMuxBus, ATieAtOneInstantOnTheBusForwardsOnlyTheHigher)
{
    this->start(true);
    TwistMux   mux(kVelocities, kStop);
    const auto started = mux.start(*this->node);
    ASSERT_TRUE(started.has_value()) << started.error();
    std::vector<double> forwarded;
    auto output = hypha::Node().subscribe<Twist>(hypha::Topic{topicFor("out")}, Collect{&forwarded});
    auto lock = this->publisherOf<Bool>("e_stop");
    auto joystick = this->publisherOf<Twist>("joystick");
    auto navigation = this->publisherOf<Twist>("navigation");
    ASSERT_TRUE(lock.publish(Bool{}));
    // Both stamped at one simulated instant; the navigation port fires first.
    ASSERT_TRUE(hypha::setSimTime(Time::fromNanos(kNow + 100'000'000)));
    ASSERT_TRUE(navigation.publish(twist(2.0)));
    ASSERT_TRUE(joystick.publish(twist(1.0)));
    ASSERT_TRUE(hypha::setSimTime(Time::fromNanos(kNow + 200'000'000)));
    ASSERT_TRUE(this->runUntil([&] {
        return !forwarded.empty();
    }));
    ASSERT_TRUE(this->node->runFor(20_ms).ok());
    EXPECT_EQ(forwarded, std::vector<double>{1.0});
}

TEST_F(TwistMuxBus, IdleVelocityInputsDoNotDegradeState)
{
    this->start(false);
    TwistMux mux(kVelocities, kStop);
    ASSERT_TRUE(mux.start(*this->node));
    auto lock = this->publisherOf<Bool>("e_stop");
    ASSERT_TRUE(lock.publish(Bool{}));
    // A fallback after the 500 ms of the velocities captures the lock and the idle inputs.
    ASSERT_TRUE(this->node->runFor(700_ms).ok());
    EXPECT_EQ(nodeStateNow(), hypha_msgs::NodeState::INPUTS_USABLE);
}

TEST_F(TwistMuxBus, AMissingLockShowsWaiting)
{
    this->start(false);
    TwistMux mux(kVelocities, kStop);
    ASSERT_TRUE(mux.start(*this->node));
    ASSERT_TRUE(this->node->runFor(700_ms).ok());
    EXPECT_EQ(nodeStateNow(), hypha_msgs::NodeState::WAITING);
}
