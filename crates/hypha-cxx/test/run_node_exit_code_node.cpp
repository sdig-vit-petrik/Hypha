// A node run by hypha::runNode whose exit code a CTest case checks
// (run_node_exit_code.cmake). The parameter file chooses the end of its run:
//
//   [k8] ending = "stop"   the first timer tick requests a stop: exit 0
//   [k8] ending = "throw"  the first timer tick throws, which fails the run: exit 70
//
// A missing file or a key nobody reads is refused before the run: exit 78.

#include "hypha/hypha.h"

#include <stdexcept>
#include <string>

namespace {

class ExitCodeNode final : public hypha::ApplicationNode
{
  public:
    ExitCodeNode() : throws_(hypha::config().required<std::string>("k8.ending") == "throw")
    {
    }

    void setup(hypha::Node& node) override
    {
        this->timer_ =
            node.timer(hypha::Duration::fromMillis(10), hypha::bind<&ExitCodeNode::tick>(*this));
    }

  private:
    void tick()
    {
        if (this->throws_) {
            throw std::runtime_error("the tick refused");
        }
        hypha::requestStop();
    }

    bool         throws_;
    hypha::Timer timer_;
};

}  // namespace

int main(int argc, char** argv)
{
    return hypha::runNode<ExitCodeNode>(argc, argv, "run_node_exit_code_node");
}
