#pragma once

/// @file
/// autoware_vehicle_info_utils' VehicleInfoUtils: the ten vehicle parameters, required,
/// from the node's parameter file (`wheel_radius`, `wheel_width`, `wheel_base`,
/// `wheel_tread`, `front_overhang`, `rear_overhang`, `left_overhang`, `right_overhang`,
/// `vehicle_height`, `max_steer_angle`), as upstream's get_or_declare_parameter reads them.

#include <autoware_vehicle_info_utils/vehicle_info.hpp>

namespace autoware::vehicle_info_utils {

class VehicleInfoUtils
{
  public:
    template <typename NodeT>
    explicit VehicleInfoUtils(NodeT& node)
        : vehicle_info_(createVehicleInfo(
              node.template declare_parameter<double>("wheel_radius"),
              node.template declare_parameter<double>("wheel_width"),
              node.template declare_parameter<double>("wheel_base"),
              node.template declare_parameter<double>("wheel_tread"),
              node.template declare_parameter<double>("front_overhang"),
              node.template declare_parameter<double>("rear_overhang"),
              node.template declare_parameter<double>("left_overhang"),
              node.template declare_parameter<double>("right_overhang"),
              node.template declare_parameter<double>("vehicle_height"),
              node.template declare_parameter<double>("max_steer_angle")
          ))
    {
    }

    [[nodiscard]] VehicleInfo getVehicleInfo() const
    {
        return this->vehicle_info_;
    }

  private:
    VehicleInfo vehicle_info_;
};

}  // namespace autoware::vehicle_info_utils
