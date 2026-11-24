// A replay participant for the lockstep wait messages of hypha_play (#148).
//
//   HYPHA_USE_SIM_TIME=1 replay_loop_peer <parameters.toml>
//
// A cycle node on simulated time, run by hypha::runNode, triggered by its input `in`: each
// step sends one std_msgs/UInt32 on `out`. The parameter file names the node and the topics
// ([node] name, [ports.inputs.in] topic and silence_ms, [ports.outputs.out] topic). Two of
// them, each reading the other's output, form a loop of triggered cycles with no rate cycle
// as its delay element: the lockstep holds it for ever (replay-flow-control.md §6.4).

#include "hypha/cycle_node.h"
#include "hypha/hypha.h"

#include <std_msgs/UInt32.h>

#include <cstdint>

namespace {

class LoopPeer final : public hypha::CycleNode
{
  public:
    LoopPeer()
    {
        this->setTrigger(this->in);
    }

    void step(const hypha::CycleTime&) override
    {
        std_msgs::UInt32 message;
        message.data = this->steps_;
        this->steps_ += 1U;
        this->out.send(message);
    }

  private:
    Arrivals<std_msgs::UInt32> in{*this, "in"};
    Output<std_msgs::UInt32>   out{*this, "out"};
    uint32_t                   steps_{0U};
};

}  // namespace

int main(int argc, char** argv)
{
    return hypha::runNode<LoopPeer>(argc, argv, "replay_loop_peer");
}
