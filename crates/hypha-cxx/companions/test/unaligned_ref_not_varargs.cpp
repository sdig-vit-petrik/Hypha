// Must not compile: a mutable PointCloud2 iterator's field is a proxy, which printf's `...`
// would print as its own bytes. The test reads the compiler's refusal.
#include <hypha/hypha.h>

#include <sensor_msgs/PointCloud2.h>
#include <sensor_msgs/point_cloud2_iterator.h>

#include <cstdio>

void print(sensor_msgs::PointCloud2& cloud)
{
    sensor_msgs::PointCloud2Iterator<float> x(cloud, "x");
    std::printf("%f\n", *x);
}
