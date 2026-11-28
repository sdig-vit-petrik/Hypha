// A replay participant with a Requests or a Calls port, for the lockstep tests of services
// (apps/hypha_tools/tests/service_lockstep.rs, docs/design/native-services.md §4, FSR-44).
//
//   HYPHA_USE_SIM_TIME=1 SERVICE_PEER_ROLE=<role> [SERVICE_PEER_TRIGGERED=1] \
//       service_lockstep_peer <parameters.toml>
//
// A cycle node on simulated time, run by hypha::runNode, at `[cycle] rate_hz` or, with
// SERVICE_PEER_TRIGGERED=1, on each sample of its input `tick`. Each line it prints is one
// fact of a step, stamped with the step's time, so two runs of one replay print the same
// lines. Roles (hypha_test_srvs/Echo):
//
//   server  [ports.servers.serve]: answers each request `<text>` with `<text>@<step ms>`.
//   client  [ports.clients.call]: calls `c<k>` whenever no call is in flight, and prints
//           each outcome it reads.
//   relay   both: defers a request, calls on with `<text>>`, and answers the deferred
//           request with the outcome's text and `<` when it comes: the middle of a chain.
//   pair    a client, and a server of a second cycle in the same node (one WaitSet), at
//           the same rate: [ports.clients.call] names the service of [ports.servers.serve].

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
void serve(hypha::Requests<Echo>& port, const hypha::CycleTime& time, const char* who)
{
    for (hypha::Request<Echo>& request : port.requests()) {
        const std::string text = request->text + "@" + std::to_string(millis(time));
        print(std::string(who) + " t=" + std::to_string(millis(time)) + " serve " + text);
        static_cast<void>(request.reply({.text = text}));
    }
}

/// The server of a pair, a second cycle in the node.
struct PairServer
{
    hypha::Requests<Echo>* port;

    void operator()(const hypha::CycleTime& time) const
    {
        serve(*this->port, time, "pair-server");
    }
};

class ServicePeer final : public hypha::CycleNode
{
  public:
    ServicePeer() : role_(environment("SERVICE_PEER_ROLE"))
    {
        const bool server = this->role_ == "server" || this->role_ == "relay";
        const bool client = this->role_ != "server";
        if (server) {
            this->requests_.emplace(*this, "serve");
        }
        if (client) {
            this->calls_.emplace(*this, "call");
        }
        if (this->role_ == "pair") {
            this->pairServer_.emplace(this->pairCycle_, "serve");
        }
        if (environment("SERVICE_PEER_TRIGGERED") == "1") {
            this->tick_.emplace(*this, "tick");
            this->setTrigger(*this->tick_);
        }
    }

    void setup(hypha::Node& node) override
    {
        print("start " + this->role_);
        if (this->pairServer_) {
            const double rate = hypha::config("cycle").required<double>("rate_hz");
            this->pairCycle_.setRate(hypha::Hz(rate));
            const auto started = this->pairCycle_.start(node, PairServer{&*this->pairServer_});
            if (!started) {
                this->failRun(started.error());
            }
        }
    }

    void step(const hypha::CycleTime& time) override
    {
        if (this->role_ == "server") {
            serve(*this->requests_, time, "server");
            return;
        }
        const std::string at = " t=" + std::to_string(millis(time));
        if (this->role_ == "relay") {
            this->relay(at);
            return;
        }
        for (const hypha::CallResult<Echo>& result : this->calls_->results()) {
            this->inFlight_ = false;
            const std::string what = result.ok() ? result.response().text : result.description();
            print(this->role_ + at + " result " + what);
        }
        if (this->inFlight_) {
            return;
        }
        const std::string       text = "c" + std::to_string(this->next_++);
        const hypha::CallStatus status = this->calls_->call({.text = text});
        this->inFlight_ = status == hypha::CallStatus::Sent;
        print(this->role_ + at + " call " + text + " " + hypha::toString(status));
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
    std::optional<Requests<Echo>>             requests_;
    std::optional<Calls<Echo>>                calls_;
    hypha::Cycle                              pairCycle_{"pair_server"};
    std::optional<hypha::Requests<Echo>>      pairServer_;
    hypha::Deferred<Echo>                     pending_;
    bool                                      inFlight_{false};
    uint32_t                                  next_{0};
};

}  // namespace

int main(int argc, char** argv)
{
    return hypha::runNode<ServicePeer>(argc, argv, "service_lockstep_peer");
}
