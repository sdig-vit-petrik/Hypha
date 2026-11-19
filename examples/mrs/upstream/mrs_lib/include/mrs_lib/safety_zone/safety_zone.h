// clang: TomasFormat
#ifndef MRS_LIB_SAFETYZONE_H
#define MRS_LIB_SAFETYZONE_H

#include <mrs_lib/safety_zone/polygon.h>
// hypha port: visualization_msgs is not generated; nothing here uses it.
#include <Eigen/Eigen>  // hypha port: Eigen3::Eigen puts eigen3/ on the include path
#include <vector>

namespace mrs_lib
{
class SafetyZone {
public:
  SafetyZone(Polygon outerBorder);
  ~SafetyZone();

  SafetyZone(const Eigen::MatrixXd& outerBorderMatrix);

  bool isPointValid(const double px, const double py);
  bool isPathValid(const double p1x, const double p1y, const double p2x, const double p2y);

  Polygon getBorder();

private:
  Polygon* outerBorder;

public:
  struct BorderError : public std::exception
  {
    const char* what() const throw() {
      return "SafeZone: Wrong configuration for the border";
    }
  };
};
}  // namespace mrs_lib

#endif  // MRS_LIB_SAFETYZONE_H
