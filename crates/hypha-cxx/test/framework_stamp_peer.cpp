// The C++ side of the framework-stamp round trip with the Rust SDK, run by
// crates/hypha/tests/framework_stamp_cross_language.rs.
//
//   framework_stamp_peer publish <topic> <md5> <sim_nanos> <override_nanos>
//       Enables simulated time at <sim_nanos>, waits for one subscriber, then sends
//       two frames: the first stamped with the simulated clock, the second with
//       <override_nanos> set on the loan.
//
//   framework_stamp_peer receive <topic> <expected_first> <expected_second>
//       Subscribes, runs until two frames arrive, and checks that their framework
//       stamps are the two expected values, in order.
//
// Exit status 0 means the check held. Any other status names the failure on stderr.

#include "hypha/hypha.h"
#include "hypha_base/clock.h"

#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <string>
#include <thread>
#include <vector>

namespace {

constexpr const char* DATA_TYPE = "std_msgs/UInt8";
constexpr const char* DEFINITION = "uint8 data\n";
constexpr auto        WAIT_LIMIT = std::chrono::seconds(10);

int fail(const char* message)
{
    std::fprintf(stderr, "framework_stamp_peer: %s\n", message);
    return 1;
}

bool parseNanos(const char* text, int64_t& out)
{
    char*           end = nullptr;
    const long long value = std::strtoll(text, &end, 10);
    if (end == text || *end != '\0') {
        return false;
    }
    out = static_cast<int64_t>(value);
    return true;
}

int publishMode(
    const std::string& topic, const std::string& md5, int64_t simNanos, int64_t overrideNanos
)
{
    if (!hypha::enableSimTime(hypha::Time::fromNanos(simNanos))) {
        return fail("cannot enable simulated time");
    }
    hypha::Node node("framework_stamp_peer_publish");
    auto        created =
        hypha::rawPublisher(topic, {.dataType = DATA_TYPE, .md5 = md5, .definition = DEFINITION});
    if (!created) {
        return fail("cannot create the publisher");
    }
    hypha::RawPublisher publisher = *created;

    // Frames sent before a subscriber connects are not delivered to it.
    const auto deadline = std::chrono::steady_clock::now() + WAIT_LIMIT;
    while (publisher.subscriberCount().value_or(0) == 0) {
        if (std::chrono::steady_clock::now() >= deadline) {
            return fail("no subscriber connected in time");
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(1));
    }

    // The first frame carries the framework clock, which is simulated time here.
    const std::vector<uint8_t> first{7};
    if (publisher.publish(first) != HYPHA_STATUS_OK) {
        return fail("the clock-stamped frame was refused");
    }

    // The second frame carries an explicit stamp set on the loan.
    auto loaned = publisher.loan(1);
    if (!loaned) {
        return fail("the loan was refused");
    }
    loaned->data()[0] = 8;
    if (loaned->setFrameworkSent(hypha::Time::fromNanos(overrideNanos)) != HYPHA_STATUS_OK) {
        return fail("the framework stamp override was refused");
    }
    if (publisher.publishLoan(std::move(*loaned)) != HYPHA_STATUS_OK) {
        return fail("the overridden frame was refused");
    }

    // Keep the publisher up long enough for the subscriber to take both frames.
    (void)node.runFor(hypha::Duration::fromMillis(200));
    return 0;
}

int receiveMode(const std::string& topic, int64_t expectedFirst, int64_t expectedSecond)
{
    std::vector<int64_t> stamps;
    hypha::Node          node("framework_stamp_peer_receive");
    auto                 subscription = hypha::subscribeRaw(
        topic,
        [&stamps](const hypha::RawSample& sample) {
            stamps.push_back(sample.frameworkSent.nanos());
        },
        8
    );
    if (!subscription) {
        return fail("cannot subscribe");
    }

    const auto deadline = std::chrono::steady_clock::now() + WAIT_LIMIT;
    while (stamps.size() < 2 && std::chrono::steady_clock::now() < deadline) {
        (void)node.runFor(hypha::Duration::fromMillis(1));
    }
    if (stamps.size() != 2) {
        std::fprintf(stderr, "framework_stamp_peer: received %zu of 2 frames\n", stamps.size());
        return 1;
    }
    if (stamps[0] != expectedFirst || stamps[1] != expectedSecond) {
        std::fprintf(
            stderr,
            "framework_stamp_peer: stamps were %lld and %lld, expected %lld and %lld\n",
            static_cast<long long>(stamps[0]),
            static_cast<long long>(stamps[1]),
            static_cast<long long>(expectedFirst),
            static_cast<long long>(expectedSecond)
        );
        return 1;
    }
    return 0;
}

}  // namespace

int main(int argc, char** argv)
{
    if (argc == 6 && std::string(argv[1]) == "publish") {
        int64_t simNanos = 0;
        int64_t overrideNanos = 0;
        if (!parseNanos(argv[4], simNanos) || !parseNanos(argv[5], overrideNanos)) {
            return fail("publish needs integer nanosecond arguments");
        }
        return publishMode(argv[2], argv[3], simNanos, overrideNanos);
    }
    if (argc == 5 && std::string(argv[1]) == "receive") {
        int64_t first = 0;
        int64_t second = 0;
        if (!parseNanos(argv[3], first) || !parseNanos(argv[4], second)) {
            return fail("receive needs integer nanosecond arguments");
        }
        return receiveMode(argv[2], first, second);
    }
    std::fprintf(
        stderr,
        "usage: framework_stamp_peer publish <topic> <md5> <sim> <override>\n"
        "       framework_stamp_peer receive <topic> <first> <second>\n"
    );
    return 2;
}
