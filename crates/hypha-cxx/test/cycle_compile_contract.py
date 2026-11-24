"""Compile the public cycle contract, including cases that must fail."""

import argparse
import concurrent.futures
import pathlib
import subprocess
import tempfile


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--compiler", required=True)
    parser.add_argument("--include", action="append", default=[])
    parser.add_argument("--flag", action="append", default=[])
    args = parser.parse_args()

    common = r"""#include <hypha/hypha.h>
#include <hypha/cycle_node.h>
#include <std_msgs/Float64.h>
#include <std_msgs/String.h>
#include <deque>
#include <expected>
#include <optional>
#include <string>
using Float64 = std_msgs::Float64;
using String = std_msgs::String;
"""

    cases = {
        "class_with_ports": (
            r"""
struct Node final : hypha::CycleNode {
  hypha::Latest<Float64> speed{*this, "topic_speed"};
  hypha::Arrivals<String> commands{*this, "topic_commands"};
  hypha::Output<Float64> status{*this, "topic_status"};
  void step(const hypha::CycleTime&) override {}
};
int run(int argc, char** argv) { return hypha::runNode<Node>(argc, argv, "node"); }
""",
            None,
        ),
        "ports_take_tags_without_the_namespace": (
            r"""
struct Node final : hypha::CycleNode {
  explicit Node(bool required) : required_(required) {}
  bool required_;
  Latest<Float64> speed{*this, "speed", optionalTopic, unsupervised, hostClock};
  Latest<Float64> shared{*this, "shared", hypha::sharing(speed), advisory(!required_)};
  Arrivals<String> commands{*this, "commands", perStep(16), limitMax(hypha::Duration::fromMillis(65535))};
  Output<Float64> status{*this, "status", latched, diagnostic(false), perStep(2), maxPayloadDynamic(1024)};
  void step(const hypha::CycleTime&) override {}
};
void endpoints(hypha::Node& node) {
  auto pub = node.advertise<Float64>("out", hypha::optionalTopic, hypha::latched(true), hypha::maxPayloadDynamic(0));
  auto fixed = node.advertise<Float64>(hypha::Topic{"/out"}, hypha::latched);
  auto sub = node.subscribe<Float64>("in", {}, hypha::optionalTopic);
  (void)pub; (void)fixed; (void)sub;
}
""",
            None,
        ),
        "require_silence_limit_does_not_compile": (
            r"""
void bind(hypha::Cycle& cycle) {
  hypha::Latest<Float64> port(cycle, "speed", hypha::requireSilenceLimit);
  (void)port;
}
""",
            ("requireSilenceLimit",),
        ),
        "unsupervised_does_not_apply_to_an_output": (
            r"""
void bind(hypha::Cycle& cycle) {
  hypha::Output<Float64> port(cycle, "status", hypha::unsupervised);
  (void)port;
}
""",
            ("OutputOption",),
        ),
        "staging_and_last_sent_are_gone": (
            r"""
void use(hypha::Latest<Float64>& latest, hypha::Output<Float64>& out) {
  latest.stage(hypha::InputVerdict::Usable, Float64{});
  (void)out.lastSent();
}
""",
            ("stage", "lastSent"),
        ),
        "the_umbrella_header_does_not_define_the_harness": (
            r"""
void use(hypha::Cycle& cycle) {
  hypha::testing::StepHarness harness{cycle};
}
""",
            ("StepHarness",),
        ),
        "a_step_that_is_not_callable_with_cycle_time_does_not_compile": (
            r"""
struct WrongStep { void operator()(int) const {} };
void startWrong(hypha::Cycle& cycle, hypha::Node& node) {
  (void)cycle.start(node, WrongStep{});
}
""",
            ("invocable",),
        ),
        "per_step_does_not_apply_to_latest": (
            r"""
void bind(hypha::Cycle& cycle) {
  hypha::Latest<Float64> port(cycle, "speed", hypha::perStep(4));
  (void)port;
}
""",
            ("LatestOption",),
        ),
        "host_clock_does_not_apply_to_arrivals": (
            r"""
void bind(hypha::Cycle& cycle) {
  hypha::Arrivals<Float64> port(cycle, "speed", hypha::hostClock);
  (void)port;
}
""",
            ("ArrivalsOption",),
        ),
        "a_sharing_port_has_no_optional_topic": (
            r"""
void bind(hypha::Cycle& cycle, hypha::Latest<Float64>& first) {
  hypha::Latest<Float64> port(cycle, "second", hypha::sharing(first), hypha::optionalTopic);
  (void)port;
}
""",
            ("FixedTopicOption",),
        ),
        "an_input_is_not_latched": (
            r"""
void bind(hypha::Cycle& cycle) {
  hypha::Latest<Float64> port(cycle, "speed", hypha::latched);
  (void)port;
}
""",
            ("LatestOption",),
        ),
        "a_subscriber_takes_only_optional_topic": (
            r"""
void endpoints(hypha::Node& node) {
  auto sub = node.subscribe<Float64>("in", {}, hypha::latched);
  (void)sub;
}
""",
            ("SubscriberOption",),
        ),
        "a_tag_given_twice": (
            r"""
void bind(hypha::Cycle& cycle) {
  hypha::Arrivals<Float64> port(cycle, "speed", hypha::perStep(4), hypha::perStep(8));
  (void)port;
}
""",
            ("given twice",),
        ),
        "qos_policies_and_options": (
            r"""
namespace policies {
inline constexpr hypha::Qos shallow{.name = "shallow", .publishers = 1, .subscribers = 2, .maxDepth = 2};
inline constexpr hypha::Qos camera{
    .name = "camera", .publishers = 1, .subscribers = 4, .maxDepth = 2, .maxPayloadDynamic = 8'000'000};
inline constexpr hypha::Qos positional{"positional", 2, 4, 64};
}
template <>
struct hypha::QosOf<String> {
  static constexpr hypha::Qos value = policies::camera;
};
static_assert(hypha::QosOf<Float64>::value.name == "default");
static_assert(hypha::QosOf<Float64>::value.publishers == 8);
static_assert(hypha::QosOf<Float64>::value.subscribers == 16);
static_assert(hypha::QosOf<Float64>::value.maxDepth == 32);
static_assert(hypha::QosOf<Float64>::value.maxPayloadDynamic == 0);
static_assert(hypha::QosOf<String>::value.name == "camera");
static_assert(hypha::QosOf<String>::value.maxPayloadDynamic == 8'000'000);
static_assert(hypha::QosOf<tf2_msgs::TFMessage>::value.name == "tf");
static_assert(hypha::QosOf<tf2_msgs::TFMessage>::value.publishers == 32);
static_assert(hypha::QosOf<tf2_msgs::TFMessage>::value.subscribers == 32);
static_assert(hypha::QosOf<tf2_msgs::TFMessage>::value.maxDepth == 32);
struct Node final : hypha::CycleNode {
  Latest<Float64> speed{*this, "speed", policies::shallow};
  Arrivals<Float64> queued{*this, "queued", policies::positional, perStep(64)};
  Arrivals<String> frames{*this, "frames", perStep(2)};
  Output<String> image{*this, "image", policies::camera, maxPayloadDynamic(1 << 20)};
  void step(const hypha::CycleTime&) override {}
};
void endpoints(hypha::Node& node) {
  auto pub = node.advertise<Float64>("out", policies::shallow, hypha::maxPayloadDynamic(64));
  auto fixed = node.advertise<Float64>(hypha::Topic{"/out"}, policies::shallow);
  auto sub = node.subscribe<Float64>("in", {}, policies::shallow);
  auto literal = node.subscribe<Float64>(hypha::Topic{"/in"}, {}, policies::shallow);
  (void)pub; (void)fixed; (void)sub; (void)literal;
}
""",
            None,
        ),
        "a_sharing_port_has_no_qos": (
            r"""
void bind(hypha::Cycle& cycle, hypha::Latest<Float64>& first) {
  hypha::Latest<Float64> port(cycle, "second", hypha::sharing(first), hypha::defaultQos);
  (void)port;
}
""",
            ("SharingOption",),
        ),
        "the_default_per_step_must_fit_the_policy_of_the_type": (
            r"""
template <>
struct hypha::QosOf<String> {
  static constexpr hypha::Qos value{.name = "shallow", .publishers = 1, .subscribers = 2, .maxDepth = 2};
};
void bind(hypha::Cycle& cycle) {
  hypha::Arrivals<String> port(cycle, "commands");
  (void)port;
}
""",
            ("default perStep",),
        ),
        "max_payload_is_replaced_by_max_payload_dynamic": (
            r"""
void endpoints(hypha::Node& node) {
  auto pub = node.advertise<String>("out", hypha::maxPayload(1024));
  (void)pub;
}
""",
            ("maxPayload",),
        ),
        "growable_is_gone": (
            r"""
void endpoints(hypha::Node& node) {
  auto pub = node.advertise<String>("out", hypha::growable);
  (void)pub;
}
""",
            ("growable",),
        ),
        "service_application_runner": (
            r"""
struct App final : hypha::ApplicationNode {
  void setup(hypha::Node&) override {}
};
int run(int argc, char** argv) { return hypha::runNode<App>(argc, argv, "service"); }
""",
            None,
        ),
        "cycle_start_cannot_be_overridden": (
            r"""
struct App final : hypha::CycleNode {
  void step(const hypha::CycleTime&) override {}
  std::expected<void, std::string> start(hypha::Node&) { return {}; }
};
""",
            ("final",),
        ),
        "ports_that_share_a_topic": (
            r"""
void share(hypha::Cycle& cycle) {
  hypha::Latest<Float64> latest(cycle, hypha::Topic{"/speed"});
  hypha::Arrivals<Float64> arrivals(cycle, hypha::sharing(latest));
  hypha::Latest<Float64> limited(cycle, "limited", hypha::sharing(arrivals));
  (void)limited;
}
""",
            None,
        ),
        "a_node_forwards_constructor_arguments": (
            r"""
struct Node final : hypha::CycleNode {
  Node(int gain, std::string label) : gain_(gain), label_(std::move(label)) {}
  void step(const hypha::CycleTime&) override {}
  int gain_;
  std::string label_;
};
int run(int argc, char** argv) { return hypha::runNode<Node>(argc, argv, "node", 3, "a"); }
""",
            None,
        ),
        "dynamic_ports_in_a_deque": (
            r"""
struct Relay {
  std::deque<hypha::Latest<Float64>>* inputs;
  void operator()(const hypha::CycleTime&) const {}
};
std::expected<void, std::string> start(hypha::Node& node, hypha::Cycle& cycle) {
  std::deque<hypha::Latest<Float64>> inputs;
  inputs.emplace_back(cycle, "topic_a");
  return cycle.start(node, Relay{&inputs});
}
""",
            None,
        ),
        "latest_of_another_message_type": (
            r"""
void bind(hypha::Cycle& cycle, hypha::Latest<String>& other) {
  hypha::Latest<Float64> port(cycle, hypha::sharing(other));
  (void)port;
}
""",
            ("Float64", "String"),
        ),
        "arrivals_of_another_message_type": (
            r"""
void bind(hypha::Cycle& cycle, hypha::Arrivals<String>& other) {
  hypha::Arrivals<Float64> port(cycle, hypha::sharing(other));
  (void)port;
}
""",
            ("Float64", "String"),
        ),
        "output_without_a_topic": (
            r"""
void bind(hypha::Cycle& cycle) {
  hypha::Output<Float64> port(cycle);
  (void)port;
}
""",
            ("Output",),
        ),
        "latest_without_a_topic": (
            r"""
void bind(hypha::Cycle& cycle) {
  hypha::Latest<Float64> port(cycle);
  (void)port;
}
""",
            ("Latest",),
        ),
        "cycle_node_without_step": (
            r"""
struct Node final : hypha::CycleNode {
};
Node instance;
""",
            ("abstract",),
        ),
        "publish_result_cannot_be_discarded": (
            r"""
#pragma GCC diagnostic error "-Wunused-result"
void publishOne(hypha::Publisher<Float64>& p) {
  p.publish(Float64{1.0});
}
""",
            "ignoring return value",
        ),
        "raw_publish_result_cannot_be_discarded": (
            r"""
#pragma GCC diagnostic error "-Wunused-result"
#include "hypha/raw_api.h"
#include <vector>
void sendRaw(const hypha::RawPublisher& p) {
  p.publish(std::vector<uint8_t>{0x01});
}
""",
            "ignoring return value",
        ),
        "publish_loan_result_cannot_be_discarded": (
            r"""
#pragma GCC diagnostic error "-Wunused-result"
#include "hypha/raw_api.h"
void sendLoan(hypha::RawPublisher& p) {
  std::expected<hypha::LoanedBuffer, hypha::Status> loaned = p.loan(8);
  if (!loaned) {
    return;
  }
  p.publishLoan(std::move(*loaned));
}
""",
            "ignoring return value",
        ),
        "loan_result_cannot_be_discarded": (
            r"""
#pragma GCC diagnostic error "-Wunused-result"
#include "hypha/raw_api.h"
void reserveLoan(const hypha::RawPublisher& p) {
  p.loan(8);
}
""",
            "ignoring return value",
        ),
    }

    include_flags = [part for path in args.include for part in ("-I", path)]
    with tempfile.TemporaryDirectory(prefix="hypha-cycle-contract-") as temporary:
        temp = pathlib.Path(temporary)

        def check(item):
            name, (body, diagnostic) = item
            source = temp / f"{name}.cpp"
            source.write_text(common + body, encoding="utf-8")
            result = subprocess.run(
                [
                    args.compiler,
                    "-std=c++23",
                    "-fsyntax-only",
                    "-DHYPHA_NO_LOGGING",
                    *args.flag,
                    *include_flags,
                    str(source),
                ],
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                check=False,
            )
            accepted = result.returncode == 0
            if diagnostic is None:
                valid = accepted
            else:
                tokens = diagnostic if isinstance(diagnostic, tuple) else (diagnostic,)
                valid = not accepted and all(token in result.stdout for token in tokens)
            if valid:
                return True, f"PASS {name}"
            expectation = (
                "compile successfully" if diagnostic is None else f"fail with {diagnostic!r}"
            )
            return False, f"FAIL {name}: expected to {expectation}\n{result.stdout}"

        with concurrent.futures.ThreadPoolExecutor() as pool:
            results = list(pool.map(check, cases.items()))
        for _, line in results:
            print(line)
        return 0 if all(valid for valid, _ in results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
