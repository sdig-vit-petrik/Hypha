/// mrs_lib utilities on Eigen: attitude conversions, heading, cyclic angles.
#include "check.h"

#include <mrs_lib/attitude_converter.h>
#include <mrs_lib/geometry/cyclic.h>
#include <mrs_lib/geometry/misc.h>
#include <mrs_lib/lkf.h>
#include <mrs_lib/math.h>
#include <mrs_lib/mutex.h>
#include <mrs_lib/profiler.h>
#include <mrs_lib/scope_timer.h>
#include <mrs_lib/utils.h>

#include <cmath>

namespace {
bool near(double a, double b, double tol = 1e-12)
{
    return std::abs(a - b) < tol;
}
}  // namespace

int main()
{
    using mrs_lib::AttitudeConverter;

    // roll 0.1, pitch -0.2, yaw 0.3 round-trips through quaternion and matrix.
    const Eigen::Quaterniond q = AttitudeConverter(0.1, -0.2, 0.3);
    const auto [r1, p1, y1] = AttitudeConverter(q).getIntrinsicRPY();
    static_cast<void>(r1);
    const mrs_lib::EulerAttitude fromQ = AttitudeConverter(q);
    MRS_CHECK(near(fromQ.roll(), 0.1) && near(fromQ.pitch(), -0.2) && near(fromQ.yaw(), 0.3));
    const Eigen::Matrix3d        R = AttitudeConverter(0.1, -0.2, 0.3);
    const mrs_lib::EulerAttitude fromR = AttitudeConverter(R);
    MRS_CHECK(near(fromR.roll(), 0.1) && near(fromR.pitch(), -0.2) && near(fromR.yaw(), 0.3));
    const geometry_msgs::Quaternion gq = AttitudeConverter(0.1, -0.2, 0.3);
    const mrs_lib::EulerAttitude    fromG = AttitudeConverter(gq);
    MRS_CHECK(near(fromG.roll(), 0.1) && near(fromG.pitch(), -0.2) && near(fromG.yaw(), 0.3));
    MRS_CHECK(near(gq.w, q.w()) && near(gq.x, q.x()) && near(gq.y, q.y()) && near(gq.z, q.z()));

    // The heading of a quaternion with yaw 1.0 and roll 0.2 is 1.0.
    MRS_CHECK(near(AttitudeConverter(0.2, 0.0, 1.0).getHeading(), 1.0));

    // setHeading keeps roll and pitch and gives the new heading.
    const Eigen::Quaterniond turned = AttitudeConverter(0.1, -0.2, 0.3).setHeading(2.0);
    MRS_CHECK(near(AttitudeConverter(turned).getHeading(), 2.0));
    const Eigen::Vector3d zBefore = Eigen::Matrix3d(AttitudeConverter(0.1, -0.2, 0.3)).col(2);
    const Eigen::Vector3d zAfter = Eigen::Matrix3d(AttitudeConverter(turned)).col(2);
    MRS_CHECK(near(zBefore.z(), zAfter.z(), 1e-9));
    MRS_CHECK(near(std::hypot(zBefore.x(), zBefore.y()), std::hypot(zAfter.x(), zAfter.y()), 1e-9));

    // Cyclic angles.
    MRS_CHECK(near(mrs_lib::geometry::radians::wrap(3 * M_PI), M_PI));
    MRS_CHECK(near(mrs_lib::geometry::sradians::diff(0.1, -0.1), 0.2));
    MRS_CHECK(near(mrs_lib::geometry::sradians::wrap(3 * M_PI / 2), -M_PI / 2));
    return mrsCheckResult();
}
