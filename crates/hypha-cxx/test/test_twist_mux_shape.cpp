// The shape of twist_mux as one triggered cycle: velocity inputs that each trigger the
// step, in priority order, and locks that mask the velocities below their priority. The
// step asks no clock and never which port fired. The priority of each input is a key of its
// port table, read before the cycle starts. A ROS timeout of 0.0 (no timeout) is
// `silence_ms = 0` in the table: the node never reads `silence_ms` itself (#238).

#include "hypha/cycle_node.h"
#include "hypha/hypha.h"
#include "hypha/testing.h"
#include <hypha_msgs/NodeState.h>

#include "test_types.h"

#include <geometry_msgs/Twist.h>
#include <gtest/gtest.h>
#include <std_msgs/Bool.h>

#include <algorithm>
#include <chrono>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <functional>
#include <memory>
#include <optional>
#include <stdexcept>
#include <string>
#include <tuple>
#include <utility>
#include <vector>

using geometry_msgs::Twist;
using hypha::Time;
using std_msgs::Bool;
using namespace hypha::literals;

namespace {

class TwistMux final : public hypha::CycleNode
{
  public:
    TwistMux(
        const std::vector<std::string>& velocityInputs, const std::vector<std::string>& lockInputs
    )
    {
        for (const std::string& name : velocityInputs) {
            this->velocities_.push_back(
                {name, priorityOf(name), std::make_unique<Arrivals<Twist>>(*this, name, advisory)}
            );
        }
        // A lock is advisory: the node is healthy without it, as in ROS, and a lock that
        // never arrived or expired masks (fail safe).
        for (const std::string& name : lockInputs) {
            this->locks_.push_back(
                {name, priorityOf(name), std::make_unique<Latest<Bool>>(*this, name, advisory)}
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

    /// The `priority` key of the port's table.
    static int64_t priorityOf(const std::string& port)
    {
        return hypha::config("ports.inputs." + port)
            .required<int64_t>("priority", hypha::constraints::range(0, 255));
    }

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

    /// The highest priority a lock masks. A usable lock masks while its value is true; one
    /// that is not usable masks when it has a time limit (it expired, or never arrived), and
    /// an untimed lock (`silence_ms = 0`, ROS timeout 0.0) is free until its first message.
    int64_t lockedPriority() const
    {
        int64_t locked = 0;
        for (const Lock& lock : this->locks_) {
            const bool masks = lock.port->usable() ? (*lock.port)->data : !lock.port->untimed();
            if (masks) {
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

/// The parameter file of the mux: one table for each input, with its limit and its
/// priority. The joystick has 500 ms, the navigation @p navigationMs (0: a ROS timeout of
/// 0.0, no limit), the e-stop lock 1 s, and the pause lock no limit.
std::string parameters(int64_t lockPriority = 255, int navigationMs = 500)
{
    std::string text;
    for (const auto& [port, silenceMs, priority] :
         std::vector<std::tuple<std::string, int, int64_t>>{
             {"joystick", 500, 100},
             {"navigation", navigationMs, 10},
             {"e_stop", 1000, lockPriority},
             {"pause", 0, 200}
         }) {
        text += "[ports.inputs." + port + "]\ntopic='" + topicFor(port) + "'\nsilence_ms = "
              + std::to_string(silenceMs) + "\npriority = " + std::to_string(priority) + "\n";
    }
    return text + "[ports.outputs.cmd_vel_out]\ntopic='" + topicFor("out") + "'\n";
}

const std::vector<std::string> kVelocities{"navigation", "joystick"};
const std::vector<std::string> kStop{"e_stop", "pause"};

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
        this->load(parameters());
    }

    /// Serve @p text as the parameter file of the mux.
    void load(const std::string& text)
    {
        this->context.reset();
        std::ofstream(this->path) << text;
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
        const auto fire = [&](const std::string& firing, const std::string& other, double x) {
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
    this->load(parameters(50));
    TwistMux                    mux(kVelocities, kStop);
    hypha::testing::StepHarness harness{mux};
    Bool                        engaged;
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

TEST_F(TwistMuxHarness, AnUntimedLockIsFreeUntilItsFirstMessageAndNeverExpires)
{
    TwistMux                    mux(kVelocities, kStop);
    hypha::testing::StepHarness harness{mux};
    harness.set(mux.lock("e_stop"), Bool{});
    EXPECT_TRUE(mux.lock("pause").untimed()) << "from silence_ms = 0, as a start decides";
    EXPECT_FALSE(mux.lock("e_stop").untimed());
    harness.setNeverArrived(mux.lock("pause"));
    harness.deliver(mux.velocity("joystick"), {twist(1.0)}, before(0));
    harness.deliver(mux.velocity("navigation"), {}, before(0));
    harness.step(mux, at(kNow));
    EXPECT_EQ(sentX(harness, mux), std::vector<double>{1.0}) << "never set: free";
    Bool paused;
    paused.data = true;
    // Sent ten hours before the step: an untimed lock holds its value until it is replaced.
    harness.deliver(mux.lock("pause"), paused, before(36'000'000));
    harness.deliver(mux.velocity("joystick"), {twist(1.0)}, before(0));
    harness.step(mux, at(kNow));
    EXPECT_TRUE(mux.lock("pause").usable());
    EXPECT_TRUE(sentX(harness, mux).empty()) << "set long ago: still locked";
}

TEST_F(TwistMuxHarness, AVelocityWithRosTimeoutZeroIsUsableForEver)
{
    this->load(parameters(255, 0));
    TwistMux                    mux(kVelocities, kStop);
    hypha::testing::StepHarness harness{mux};
    harness.set(mux.lock("e_stop"), Bool{});
    harness.deliver(mux.velocity("joystick"), {}, before(600));
    harness.deliver(mux.velocity("navigation"), {twist(2.0)}, before(36'000'000));
    harness.step(mux, at(kNow));
    EXPECT_TRUE(mux.velocity("navigation").untimed());
    EXPECT_EQ(sentX(harness, mux), std::vector<double>{2.0}) << "a ten-hour-old sample";
}

namespace {

/// A node for the mux on the bus, under simulated time when @p simulated.
class TwistMuxBus : public ::testing::Test
{
  protected:
    void start(bool simulated, int navigationMs = 500)
    {
        this->path = std::filesystem::temp_directory_path() / "hypha_twist_mux_bus.toml";
        std::ofstream(this->path) << parameters(255, navigationMs);
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
    auto                output =
        hypha::Node().subscribe<Twist>(hypha::Topic{topicFor("out")}, Collect{&forwarded});
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

TEST_F(TwistMuxBus, AnUntimedLockAndVelocityThatNeverArrivedLeaveTheNodeHealthy)
{
    this->start(false, 0);
    TwistMux mux(kVelocities, kStop);
    ASSERT_TRUE(mux.start(*this->node));
    auto lock = this->publisherOf<Bool>("e_stop");
    ASSERT_TRUE(lock.publish(Bool{}));
    // The joystick's 500 ms are the only fallback: the navigation has no limit.
    ASSERT_TRUE(this->node->runFor(700_ms).ok());
    EXPECT_EQ(nodeStateNow(), hypha_msgs::NodeState::INPUTS_USABLE);
    EXPECT_TRUE(mux.lock("pause").untimed());
    EXPECT_TRUE(mux.velocity("navigation").untimed());
    EXPECT_EQ(mux.lock("pause").verdict(), hypha::InputVerdict::NeverArrived);
    const auto introspection = hypha::introspect();
    ASSERT_TRUE(introspection.has_value());
    for (const auto& input : introspection->input_bindings) {
        const bool untimed =
            input.topic == topicFor("pause") || input.topic == topicFor("navigation");
        EXPECT_EQ(input.silence_nanos == 0, untimed) << input.topic << " shows its limit";
    }
}

TEST_F(TwistMuxBus, ANeverArrivedTimedLockMasksWithoutDegradingState)
{
    this->start(false);
    TwistMux mux(kVelocities, kStop);
    ASSERT_TRUE(mux.start(*this->node));
    ASSERT_TRUE(this->node->runFor(700_ms).ok());
    EXPECT_EQ(nodeStateNow(), hypha_msgs::NodeState::INPUTS_USABLE) << "every input is advisory";
    EXPECT_EQ(mux.lock("e_stop").verdict(), hypha::InputVerdict::NeverArrived);
    EXPECT_FALSE(mux.lock("e_stop").untimed()) << "so it masks";
}
