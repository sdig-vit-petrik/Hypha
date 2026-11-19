// Several threads that construct a hypha::Node at the same moment share one
// bus node: the check for an existing session and the creation are one step.

#include "hypha/hypha.h"

#include <gtest/gtest.h>
#include <unistd.h>

#include <atomic>
#include <memory>
#include <string>
#include <thread>
#include <vector>

namespace {

int aliveNodesOfThisProcess()
{
    const int32_t                      ownPid = static_cast<int32_t>(::getpid());
    const std::vector<hypha::NodeInfo> nodes = hypha::listNodes();
    int                                count = 0;
    for (const hypha::NodeInfo& node : nodes) {
        const bool ownAndAlive = node.pid == ownPid && node.alive;
        if (ownAndAlive) {
            count += 1;
        }
    }
    return count;
}

/// One thread of the race: wait for the start signal, then construct a node.
class StartNode
{
  public:
    StartNode(std::atomic<bool>& startSignal, std::unique_ptr<hypha::Node>& nodeSlot, int nodeIndex)
        : go(startSignal), slot(nodeSlot), index(nodeIndex)
    {
    }

    void operator()()
    {
        while (!this->go.load()) {
        }
        this->slot = std::make_unique<hypha::Node>("race_node_" + std::to_string(this->index));
    }

  private:
    std::atomic<bool>&            go;
    std::unique_ptr<hypha::Node>& slot;
    int                           index;
};

}  // namespace

TEST(NodeCreationRace, threadsStartingTogetherCreateOneBusNode)
{
    constexpr int threadCount = 4;
    constexpr int rounds = 20;
    // Other tests in this binary may hold bus nodes of their own.
    const int nodesBefore = aliveNodesOfThisProcess();
    for (int round = 0; round < rounds; ++round) {
        std::atomic<bool>                         go{false};
        std::vector<std::unique_ptr<hypha::Node>> nodes(threadCount);
        std::vector<std::thread>                  threads;
        for (int index = 0; index < threadCount; ++index) {
            threads.emplace_back(StartNode(go, nodes[index], index));
        }
        go.store(true);
        for (std::thread& thread : threads) {
            thread.join();
        }

        EXPECT_EQ(aliveNodesOfThisProcess(), nodesBefore + 1) << "round " << round;

        nodes.clear();
        EXPECT_EQ(aliveNodesOfThisProcess(), nodesBefore) << "round " << round;
    }
}
