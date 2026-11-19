// A replay participant for the restart test of hypha_play
// (apps/hypha_tools/tests/play_restart.rs).
//
//   HYPHA_USE_SIM_TIME=1 replay_restart_peer
//
// A 50 Hz cycle node on simulated time, run by hypha::runNode. Its first step in each
// process prints "replay_restart_peer: first step, rejoined <0|1>": 1 in a process that
// a backward seek of the replay restarted.

#include "hypha/cycle_node.h"
#include "hypha/hypha.h"

#include <cstdio>
#include <cstdlib>

namespace {

class RestartPeer final : public hypha::CycleNode
{
  public:
    RestartPeer()
    {
        this->setRate(hypha::Hz(50.0));
    }

    void step(const hypha::CycleTime&) override
    {
        if (this->printed_) {
            return;
        }
        this->printed_ = true;
        const bool rejoined = std::getenv("HYPHA_REJOIN_AFTER_NANOS") != nullptr;
        std::printf("replay_restart_peer: first step, rejoined %d\n", rejoined ? 1 : 0);
        std::fflush(stdout);
    }

  private:
    bool printed_{false};
};

}  // namespace

int main(int argc, char** argv)
{
    return hypha::runNode<RestartPeer>(argc, argv, "replay_restart_peer");
}
