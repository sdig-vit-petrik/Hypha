#ifndef ESTIMATION_MANAGER_PRIVATE_HANDLERS_H
#define ESTIMATION_MANAGER_PRIVATE_HANDLERS_H

#include <functional>
#include <string>
#include <mrs_lib/param_loader.h>

namespace mrs_uav_managers
{

namespace estimation_manager
{

typedef std::function<bool(const std::string)> loadConfigFile_t;  // hypha port: std::function replaced by std::function  // hypha port: boost::function replaced by std::function

struct PrivateHandlers_t
{
  std::unique_ptr<mrs_lib::ParamLoader> param_loader;
  loadConfigFile_t                      loadConfigFile;
};

}  // namespace estimation_manager

}  // namespace mrs_uav_managers

#endif  // ESTIMATION_MANAGER_PRIVATE_HANDLERS_H
