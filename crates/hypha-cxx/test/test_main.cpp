#include <gtest/gtest.h>
#include <unistd.h>

#include <cstdlib>
#include <filesystem>
#include <string>
#include <system_error>

int main(int argc, char** argv)
{
    // ctest runs the gtest shards in parallel processes, so each process gets a bus
    // domain of its own: the caller's HYPHA_DOMAIN (or "t") plus the process id. Their
    // /clock and topics never meet, and the peers a test spawns inherit the domain. The
    // tests write parameter files under fixed names in the temporary directory, so each
    // process gets a temporary directory of its own as well.
    const std::string pid = std::to_string(::getpid());
    const char*       base = std::getenv("HYPHA_DOMAIN");
    const std::string domain = std::string(base != nullptr ? base : "t") + "_" + pid;
    std::error_code   error;
    const auto temporary = std::filesystem::temp_directory_path(error) / ("hypha_test_" + pid);
    if (error || !std::filesystem::create_directories(temporary, error)
        || ::setenv("HYPHA_DOMAIN", domain.c_str(), 1) != 0
        || ::setenv("TMPDIR", temporary.c_str(), 1) != 0) {
        return 1;
    }
    ::testing::InitGoogleTest(&argc, argv);
    const int result = RUN_ALL_TESTS();
    std::filesystem::remove_all(temporary, error);
    return result;
}
