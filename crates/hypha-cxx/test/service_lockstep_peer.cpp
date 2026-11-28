// A replay participant with a Requests or a Calls port, for the lockstep tests of services
// (apps/hypha_tools/tests/service_lockstep.rs, docs/design/native-services.md §4, FSR-44).
//
//   HYPHA_USE_SIM_TIME=1 SERVICE_PEER_ROLE=<role> [SERVICE_PEER_TRIGGERED=1] \
//       [SERVICE_PEER_PAIR_RATE=<hz>] [SERVICE_PEER_OUTPUT=1] \
//       service_lockstep_peer <parameters.toml>
//
// A cycle node on simulated time, run by hypha::runNode, at `[cycle] rate_hz` or, with
// SERVICE_PEER_TRIGGERED=1, on each sample of its input `tick`. With SERVICE_PEER_OUTPUT=1
// each step also sends its step count on the output `out`. Each line it prints is one fact
// of a step, stamped with the step's time, so two runs of one replay print the same lines.
// Roles (hypha_test_srvs/Echo):
//
//   server  [ports.servers.serve]: answers each request `<text>` with `<text>@<step ms>`.
//   client  [ports.clients.call]: calls `c<k>` whenever no call is in flight, and prints
//           each outcome it reads.
//   mutual  both, in one cycle: serves as `server` and calls as `client`.
//   relay   both: defers a request, calls on with `<text>>`, and answers the deferred
//           request with the outcome's text and `<` when it comes: the middle of a chain.
//   pair    a client, and a server of a second cycle in the same node (one WaitSet), at
//           SERVICE_PEER_PAIR_RATE or `[cycle] rate_hz`.
//   twin    a client, and a second client in a second cycle of the node, both on the
//           port `call`: their requests tie on stamp and node.
//   plain   no cycle: a plain server of `serve` that answers `<text>@plain`; it takes no
//           part in the replay.

#include "hypha/cycle_node.h"
#include "hypha/hypha.h"
#include <hypha_test_srvs/Echo.h>

#include <std_msgs/UInt32.h>

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <optional>
#include <string>

namespace {

using Echo = hypha_test_srvs::Echo;

std::string environment(const char* name)
{
    const char* value = std::getenv(name);
    return value == nullptr ? std::string() : std::string(value);
}

long long millis(const hypha::CycleTime& time)
{
    return static_cast<long long>(time.now.nanos() / 1'000'000);
}

void print(const std::string& line)
{
    std::printf("%s\n", line.c_str());
    std::fflush(stdout);
}

/// Answer each request of @p port at @p time.
void serve(hypha::Requests<Echo>& port, const hypha::CycleTime& time, const std::string& who)
{
    for (hypha::Request<Echo>& request : port.requests()) {
        const std::string text = request->text + "@" + std::to_string(millis(time));
        print(who + " t=" + std::to_string(millis(time)) + " serve " + text);
        static_cast<void>(request.reply({.text = text}));
    }
}

/// Calls with `<prefix><k>` whenever no call is in flight, and prints what it reads.
struct Caller
{
    std::string prefix;
    bool        inFlight{false};
    uint32_t    next{0};

    void step(hypha::Calls<Echo>& port, const hypha::CycleTime& time, const std::string& who)
    {
        const std::string at = " t=" + std::to_string(millis(time));
        for (const hypha::CallResult<Echo>& result : port.results()) {
            this->inFlight = false;
            const std::string what = result.ok() ? result.response().text : result.description();
            print(who + at + " result " + what);
        }
        if (this->inFlight) {
            return;
        }
        const std::string       text = this->prefix + std::to_string(this->next++);
        const hypha::CallStatus status = port.call({.text = text});
        this->inFlight = status == hypha::CallStatus::Sent;
        print(who + at + " call " + text + " " + hypha::toString(status));
    }
};

/// The second cycle of a pair or a twin.
struct SecondCycle
{
    hypha::Requests<Echo>* server;
    hypha::Calls<Echo>*    client;
    Caller*                caller;

    void operator()(const hypha::CycleTime& time) const
    {
        if (this->server != nullptr) {
            serve(*this->server, time, "pair-server");
        } else {
            this->caller->step(*this->client, time, "twin");
        }
    }
};

class ServicePeer final : public hypha::CycleNode
{
  public:
    ServicePeer() : role_(environment("SERVICE_PEER_ROLE"))
    {
        const bool server =
            this->role_ == "server" || this->role_ == "relay" || this->role_ == "mutual";
        const bool client = this->role_ != "server";
        if (server) {
            this->requests_.emplace(*this, "serve");
        }
        if (client) {
            this->calls_.emplace(*this, "call");
        }
        if (this->role_ == "pair") {
            this->secondServer_.emplace(this->second_, "serve");
        }
        if (this->role_ == "twin") {
            this->secondClient_.emplace(this->second_, "call");
        }
        if (environment("SERVICE_PEER_TRIGGERED") == "1") {
            this->tick_.emplace(*this, "tick");
            this->setTrigger(*this->tick_);
        }
        if (environment("SERVICE_PEER_OUTPUT") == "1") {
            this->out_.emplace(*this, "out");
        }
    }

    void setup(hypha::Node& node) override
    {
        print("start " + this->role_);
        if (!this->secondServer_ && !this->secondClient_) {
            return;
        }
        const std::string pairRate = environment("SERVICE_PEER_PAIR_RATE");
        const double rate = pairRate.empty() ? hypha::config("cycle").required<double>("rate_hz")
                                             : std::stod(pairRate);
        this->second_.setRate(hypha::Hz(rate));
        hypha::Requests<Echo>* server = this->secondServer_ ? &*this->secondServer_ : nullptr;
        hypha::Calls<Echo>*    client = this->secondClient_ ? &*this->secondClient_ : nullptr;
        const auto started = this->second_.start(node, SecondCycle{server, client, &this->twin_});
        if (!started) {
            this->failRun(started.error());
        }
    }

    void step(const hypha::CycleTime& time) override
    {
        if (this->out_) {
            this->out_->send(std_msgs::UInt32{this->steps_++});
        }
        if (this->role_ == "server" || this->role_ == "mutual") {
            serve(*this->requests_, time, this->role_);
        }
        if (this->role_ == "server") {
            return;
        }
        if (this->role_ == "relay") {
            this->relay(" t=" + std::to_string(millis(time)));
            return;
        }
        this->caller_.step(*this->calls_, time, this->role_);
    }

  private:
    void relay(const std::string& at)
    {
        for (const hypha::CallResult<Echo>& result : this->calls_->results()) {
            const std::string what = result.ok() ? result.response().text : result.description();
            print("relay" + at + " result " + what);
            if (this->pending_) {
                static_cast<void>(this->pending_.reply({.text = what + "<"}));
            }
        }
        for (hypha::Request<Echo>& request : this->requests_->requests()) {
            if (this->pending_) {
                print("relay" + at + " refuse " + request->text);
                static_cast<void>(request.refuse("busy"));
                continue;
            }
            const hypha::CallStatus status = this->calls_->call({.text = request->text + ">"});
            print("relay" + at + " defer " + request->text + " " + hypha::toString(status));
            if (status == hypha::CallStatus::Sent) {
                this->pending_ = request.defer();
            } else {
                static_cast<void>(request.refuse(hypha::toString(status)));
            }
        }
    }

    std::string                               role_;
    std::optional<Arrivals<std_msgs::UInt32>> tick_;
    std::optional<Output<std_msgs::UInt32>>   out_;
    std::optional<Requests<Echo>>             requests_;
    std::optional<Calls<Echo>>                calls_;
    hypha::Cycle                              second_{"second"};
    std::optional<hypha::Requests<Echo>>      secondServer_;
    std::optional<hypha::Calls<Echo>>         secondClient_;
    hypha::Deferred<Echo>                     pending_;
    Caller                                    caller_{"c"};
    Caller                                    twin_{"t"};
    uint32_t                                  steps_{0};
};

/// A plain server that takes no part in the replay.
class PlainServer final : public hypha::ApplicationNode
{
  public:
    void setup(hypha::Node& node) override
    {
        this->server_ = node.advertiseService<Echo>("serve", [](hypha::Request<Echo>& request) {
            static_cast<void>(request.reply({.text = request->text + "@plain"}));
        });
        // After the server exists: the harness starts the replay once it reads this.
        print("start plain");
    }

  private:
    hypha::ServiceServer server_;
};

}  // namespace

int main(int argc, char** argv)
{
    if (environment("SERVICE_PEER_ROLE") == "plain") {
        return hypha::runNode<PlainServer>(argc, argv, "service_lockstep_peer");
    }
    return hypha::runNode<ServicePeer>(argc, argv, "service_lockstep_peer");
}
