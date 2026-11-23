#pragma once

/// @file
/// autoware_utils_logging's LoggerLevelConfigure (autoware_utils 1.11.0) on hypha: the
/// `~/config_logger` service of logging_demo/ConfigLogger is offered, as upstream does, and
/// every request is refused with `success = false` and a warning. Upstream sets an rcutils
/// logger level at run time; hypha sets levels from the parameter file's `[log]` table only.

#include <logging_demo/srv/config_logger.hpp>
#include <rclcpp/rclcpp.hpp>

#include <memory>

namespace autoware_utils_logging {

template <class NodeT = rclcpp::Node>
class BasicLoggerLevelConfigure
{
  public:
    using ConfigLogger = logging_demo::srv::ConfigLogger;

    explicit BasicLoggerLevelConfigure(NodeT* node)
        : logger_(node->get_logger())
        , service_(node->template create_service<ConfigLogger>(
              "~/config_logger",
              [this](
                  const std::shared_ptr<ConfigLogger::Request>&  request,
                  const std::shared_ptr<ConfigLogger::Response>& response
              ) {
                  response->success = false;
                  RCLCPP_WARN_STREAM(
                      this->logger_,
                      "Logger level [" << request->level << "] for " << request->logger_name
                                       << " refused: hypha sets log levels from [log] in the "
                                          "parameter file"
                  );
              }
          ))
    {
    }

  private:
    rclcpp::Logger                                    logger_;
    typename rclcpp::Service<ConfigLogger>::SharedPtr service_;
};

using LoggerLevelConfigure = BasicLoggerLevelConfigure<rclcpp::Node>;

}  // namespace autoware_utils_logging
