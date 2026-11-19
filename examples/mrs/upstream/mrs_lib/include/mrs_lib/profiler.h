// hypha port: the upstream Profiler only publishes mrs_msgs/ProfilerUpdate for MRS
// monitoring tools, which the port does not run. This file keeps its API and
// publishes nothing; the upstream src/profiler/profiler.cpp is not vendored.
#ifndef PROFILER_H
#define PROFILER_H

#include <ros/ros.h>

#include <string>

namespace mrs_lib
{

class Routine {
public:
  void end(void) {
  }
};

class Profiler {
public:
  Profiler() = default;
  Profiler(ros::NodeHandle&, std::string, bool) {
  }
  Routine createRoutine(std::string, double, double, ros::TimerEvent) {
    return Routine{};
  }
  Routine createRoutine(std::string) {
    return Routine{};
  }
};

}  // namespace mrs_lib

#endif
