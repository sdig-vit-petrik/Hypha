// A node stops on SIGINT and SIGTERM. The handlers node creation installs must
// still be in place after a run: the executor must not replace them with its own.

#include "test_types.h"

#include <gtest/gtest.h>

#include <csignal>

using namespace hypha::literals;

namespace {

using SignalHandler = void (*)(int);

SignalHandler installedHandler(int signal)
{
    struct sigaction action{};
    sigaction(signal, nullptr, &action);
    return action.sa_handler;
}

}  // namespace

TEST(StopSignal, ARunKeepsTheStopHandlersOfTheNode)
{
    hypha::Node         node("stop_signal");
    const SignalHandler termination = installedHandler(SIGTERM);
    const SignalHandler interruption = installedHandler(SIGINT);

    ASSERT_TRUE(node.runFor(10_ms).ok());

    EXPECT_EQ(installedHandler(SIGTERM), termination);
    EXPECT_EQ(installedHandler(SIGINT), interruption);
}
