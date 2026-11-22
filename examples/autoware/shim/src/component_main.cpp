/// @file
/// The program of one registered component: `<program> <parameter_file.toml>`. Exit codes
/// as hypha's runNode (hypha/application_node.h): 0 clean, 2 usage, 78 refused
/// configuration or an unread key, 75 simulated time went back, 70 anything else.

#include <hypha/application_node.h>

#include <rclcpp/rclcpp.hpp>
#include <rclcpp_components/register_node_macro.hpp>

#include <exception>
#include <iostream>
#include <memory>
#include <optional>
#include <string>
#include <vector>

namespace {

int exitCodeOf(const hypha::RunResult& result)
{
    if (result.status() == hypha::Status::ClockRewound) {
        return hypha::kExitRewind;
    }
    return result.ok() ? hypha::kExitClean : hypha::kExitRuntime;
}

}  // namespace

int main(int argc, char** argv)
{
    const std::vector<rclcpp_components::Component>& components = rclcpp_components::registry();
    if (components.size() != 1) {
        std::cerr << "component runner: " << components.size()
                  << " components linked, expected one\n";
        return hypha::kExitRuntime;
    }
    if (argc != 2) {
        std::cerr << "usage: " << argv[0] << " <parameter_file.toml>  (" << components[0].className
                  << ")\n";
        return hypha::kExitUsage;
    }
    const std::string                   parameterFile(argv[1]);
    std::optional<hypha::ConfigContext> configuration;
    try {
        // Loaded first and alone: a file that is missing or does not parse is a
        // configuration fault, which a restart cannot fix.
        configuration.emplace(parameterFile);
    } catch (const std::exception& error) {
        std::cerr << "parameter file refused: " << error.what() << '\n';
        return hypha::kExitConfiguration;
    }
    std::shared_ptr<rclcpp::Node> node;
    try {
        // Start-up: a refused parameter, endpoint or QoS is a configuration fault.
        rclcpp::init(argc, argv);
        node = components[0].factory(rclcpp::NodeOptions());
        const std::vector<std::string> unread = hypha::config().unreadKeys();
        if (!unread.empty()) {
            std::cerr << "unread parameter(s): " << unread.front() << '\n';
            return hypha::kExitConfiguration;
        }
    } catch (const hypha::ConfigError& error) {
        std::cerr << "configuration refused: " << error.what() << '\n';
        return hypha::kExitConfiguration;
    } catch (const std::invalid_argument& error) {
        std::cerr << "configuration refused: " << error.what() << '\n';
        return hypha::kExitConfiguration;
    } catch (const std::exception& error) {
        std::cerr << "start-up failed: " << error.what() << '\n';
        return hypha::kExitRuntime;
    }
    try {
        rclcpp::spin(node);
    } catch (const std::exception& error) {
        std::cerr << "run failed: " << error.what() << '\n';
        return hypha::kExitRuntime;
    }
    const std::optional<hypha::RunResult> result = rclcpp::last_spin_result();
    return result.has_value() ? exitCodeOf(*result) : hypha::kExitRuntime;
}
