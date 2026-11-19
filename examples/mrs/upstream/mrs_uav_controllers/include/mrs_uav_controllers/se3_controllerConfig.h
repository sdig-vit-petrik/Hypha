// hypha port: written by hand from cfg/se3_controller.cfg (dynamic_reconfigure is not
// used); every field has the default of the .cfg file.
#pragma once

namespace mrs_uav_controllers
{

struct se3_controllerConfig
{
  // Horizontal gains
  double kpxy      = 0.0;
  double kvxy      = 0.0;
  double kaxy      = 0.0;
  double kiwxy     = 0.0;
  double kibxy     = 0.0;
  double kiwxy_lim = 0.0;
  double kibxy_lim = 0.0;
  // Vertical gains
  double kpz = 0.0;
  double kvz = 0.0;
  double kaz = 0.0;
  // Attitude control
  double kq_roll_pitch = 0.0;
  double kq_yaw        = 0.0;
  // Mass estimator
  double km                = 0.0;
  bool   fuse_acceleration = false;
  double km_lim            = 0.0;
  // Output
  bool jerk_feedforward                     = true;
  bool pitch_roll_heading_rate_compensation = true;
  int  rotation_type                        = 0;
  int  preferred_output_mode                = 0;
};

}  // namespace mrs_uav_controllers
