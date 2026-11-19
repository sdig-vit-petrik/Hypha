// A node's name comes from code or from the node.name parameter, and a name
// that breaks the identifier rule is refused.

#include "hypha/hypha.h"

#include <gtest/gtest.h>

#include <cstdio>
#include <filesystem>
#include <fstream>
#include <string>

namespace {

std::filesystem::path writeParameters(const std::string& text)
{
    const std::filesystem::path path =
        std::filesystem::temp_directory_path() / "hypha_node_identity_test.toml";
    std::ofstream file(path);
    file << text;
    return path;
}

}  // namespace

TEST(NodeIdentity, TheNodeNameParameterOverridesTheCodeName)
{
    const std::filesystem::path path = writeParameters("[node]\nname = \"ctrl_front_cxx\"\n");
    {
        hypha::Node node("speed_controller", path.string());
        EXPECT_EQ(hypha::nodeName(), "ctrl_front_cxx");
    }
    std::filesystem::remove(path);
}

TEST(NodeIdentity, AnInvalidNodeNameIsRefused)
{
    try {
        hypha::Node node("Speed-Controller");
        FAIL() << "an invalid node name created a node";
    } catch (const std::runtime_error& error) {
        EXPECT_NE(std::string(error.what()).find("InvalidArgument"), std::string::npos)
            << error.what();
    }
}
