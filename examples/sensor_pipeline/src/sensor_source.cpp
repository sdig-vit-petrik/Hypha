/// hypha_sensor_source: a synthetic vehicle with an IMU, a LiDAR and odometry.
///
///   hypha_sensor_source examples/sensor_pipeline/config/source.toml

#include <hypha/hypha.h>
#include <hypha/transform_buffer.h>
#include <hypha_msgs/geometry_msgs/TransformStamped.h>
#include <hypha_msgs/sensor_msgs/Imu.h>
#include <hypha_msgs/sensor_msgs/PointCloud2.h>
#include <hypha_msgs/sensor_msgs/PointField.h>
#include <hypha_msgs/tf2_msgs/TFMessage.h>

#include "sensor_pipeline_qos.h"

#include <array>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <exception>
#include <numbers>
#include <optional>
#include <string>
#include <vector>

HYPHA_LOG_MODULE_INIT("sensor_source")

namespace {

/// One static transform of the TOML file: `[static.<name>]`.
struct StaticFrame
{
    std::string           child;
    std::array<double, 3> translation;
    std::array<double, 4> rotation;  // x, y, z, w
};

struct SourceSettings
{
    double                  tickHz;
    uint32_t                tfEveryTicks;
    uint32_t                cloudEveryTicks;
    uint32_t                cloudRings;
    uint32_t                cloudColumns;
    double                  speedMps;
    double                  yawRateRadps;
    std::optional<uint64_t> stopAfterTicks;
    std::string             odomFrame;
    std::string             baseFrame;
    StaticFrame             lidar;
    StaticFrame             imu;
};

/// A count of ticks; zero is refused, as a period of zero ticks means nothing.
uint32_t requiredTicks(const std::string& key)
{
    const uint32_t ticks = hypha::config().required<uint32_t>(key);
    if (ticks == 0U) {
        throw hypha::ConfigError("hypha: parameter '" + key + "': must be at least 1");
    }
    return ticks;
}

StaticFrame readStaticFrame(const std::string& table)
{
    const hypha::ConfigView view{table};
    StaticFrame             frame;
    frame.child = view.required<std::string>("child", hypha::constraints::nonempty());
    frame.translation = view.requiredArray<double, 3>("translation");
    frame.rotation = view.requiredArray<double, 4>("rotation");
    return frame;
}

SourceSettings readSettings()
{
    SourceSettings settings;
    settings.tickHz = hypha::config().required<double>("tick_hz", hypha::constraints::positive());
    settings.tfEveryTicks = requiredTicks("tf_every_ticks");
    settings.cloudEveryTicks = requiredTicks("cloud_every_ticks");
    const bool cloudOnTfTick = settings.cloudEveryTicks % settings.tfEveryTicks == 0U;
    if (!cloudOnTfTick) {
        throw hypha::ConfigError(
            "hypha: parameter 'cloud_every_ticks': must be a multiple of tf_every_ticks, so "
            "every cloud has a /tf message at its stamp"
        );
    }
    settings.cloudRings = requiredTicks("cloud_rings");
    settings.cloudColumns = requiredTicks("cloud_columns");
    settings.speedMps = hypha::config().required<double>("speed_mps", hypha::constraints::finite());
    settings.yawRateRadps =
        hypha::config().required<double>("yaw_rate_radps", hypha::constraints::finite());
    const std::optional<double> stopAfterSeconds =
        hypha::config().optional<double>("stop_after_s", hypha::constraints::positive());
    if (stopAfterSeconds.has_value()) {
        settings.stopAfterTicks =
            static_cast<uint64_t>(std::llround(*stopAfterSeconds * settings.tickHz));
    }
    const hypha::ConfigView frames{"frames"};
    settings.odomFrame = frames.required<std::string>("odom", hypha::constraints::nonempty());
    settings.baseFrame = frames.required<std::string>("base", hypha::constraints::nonempty());
    settings.lidar = readStaticFrame("static.lidar");
    settings.imu = readStaticFrame("static.imu");
    return settings;
}

geometry_msgs::TransformStamped staticTransform(const std::string& parent, const StaticFrame& frame)
{
    geometry_msgs::TransformStamped transform;
    transform.header.stamp = hypha::toRosTime(hypha::now());
    transform.header.frame_id = parent;
    transform.child_frame_id = frame.child;
    transform.transform.translation.x = frame.translation[0];
    transform.transform.translation.y = frame.translation[1];
    transform.transform.translation.z = frame.translation[2];
    transform.transform.rotation.x = frame.rotation[0];
    transform.transform.rotation.y = frame.rotation[1];
    transform.transform.rotation.z = frame.rotation[2];
    transform.transform.rotation.w = frame.rotation[3];
    return transform;
}

void appendFloat(std::vector<uint8_t>& data, float value)
{
    std::array<uint8_t, sizeof(float)> bytes{};
    std::memcpy(bytes.data(), &value, sizeof(float));
    for (const uint8_t byte : bytes) {
        data.push_back(byte);
    }
}

sensor_msgs::PointField floatField(const char* name, uint32_t offset)
{
    sensor_msgs::PointField field;
    field.name = name;
    field.offset = offset;
    field.datatype = sensor_msgs::PointField::FLOAT32;
    field.count = 1;
    return field;
}

/// Publishes every output of one tick. Lives on the run() thread: the timer calls it.
class SensorSource
{
  public:
    SensorSource(
        const SourceSettings&                      settings,
        hypha::Publisher<sensor_msgs::Imu>         imu,
        hypha::Publisher<sensor_msgs::PointCloud2> cloud,
        hypha::Publisher<tf2_msgs::TFMessage>      tf
    )
        : settings_(settings), imu_(std::move(imu)), cloud_(std::move(cloud)), tf_(std::move(tf))
    {
    }

    void tick()
    {
        const bool stopReached = this->settings_.stopAfterTicks.has_value()
                              && this->ticks_ >= *this->settings_.stopAfterTicks;
        if (stopReached) {
            hypha::requestStop();
            return;
        }
        const hypha::Time         now = hypha::now();
        const hypha_msgs::RosTime stamp = hypha::toRosTime(now);
        const double seconds = static_cast<double>(this->ticks_) / this->settings_.tickHz;
        const double yaw = this->settings_.yawRateRadps * seconds;

        this->publishImu(stamp, yaw);
        const bool tfTick = this->ticks_ % this->settings_.tfEveryTicks == 0U;
        if (tfTick) {
            this->publishTf(stamp, seconds, yaw);
        }
        const bool cloudTick = this->ticks_ % this->settings_.cloudEveryTicks == 0U;
        if (cloudTick) {
            this->publishCloud(stamp);
        }
        ++this->ticks_;
    }

    uint64_t ticks() const
    {
        return this->ticks_;
    }

    uint64_t unsent() const
    {
        return this->unsent_;
    }

    uint64_t clouds() const
    {
        return this->clouds_;
    }

  private:
    void publishImu(const hypha_msgs::RosTime& stamp, double yaw)
    {
        sensor_msgs::Imu imu;
        imu.header.stamp = stamp;
        imu.header.frame_id = this->settings_.imu.child;
        imu.orientation.z = std::sin(yaw / 2.0);
        imu.orientation.w = std::cos(yaw / 2.0);
        imu.angular_velocity.z = this->settings_.yawRateRadps;
        imu.linear_acceleration.x = this->settings_.speedMps * this->settings_.yawRateRadps;
        imu.linear_acceleration.z = 9.81;
        this->count(this->imu_.publish(imu), "imu");
    }

    void publishTf(const hypha_msgs::RosTime& stamp, double seconds, double yaw)
    {
        // A constant speed along a constant turn: the arc of radius speed / yaw rate.
        const double                    distance = this->settings_.speedMps * seconds;
        const bool                      straight = this->settings_.yawRateRadps == 0.0;
        geometry_msgs::TransformStamped pose;
        pose.header.stamp = stamp;
        pose.header.frame_id = this->settings_.odomFrame;
        pose.child_frame_id = this->settings_.baseFrame;
        if (straight) {
            pose.transform.translation.x = distance;
        } else {
            const double radius = this->settings_.speedMps / this->settings_.yawRateRadps;
            pose.transform.translation.x = radius * std::sin(yaw);
            pose.transform.translation.y = radius * (1.0 - std::cos(yaw));
        }
        pose.transform.rotation.z = std::sin(yaw / 2.0);
        pose.transform.rotation.w = std::cos(yaw / 2.0);
        tf2_msgs::TFMessage message;
        message.transforms.push_back(pose);
        this->count(this->tf_.publish(message), "/tf");
    }

    /// A ring of beams around the sensor. The range of each beam changes with the cloud
    /// index, so each cloud has another centroid.
    void publishCloud(const hypha_msgs::RosTime& stamp)
    {
        const uint32_t           rings = this->settings_.cloudRings;
        const uint32_t           columns = this->settings_.cloudColumns;
        const uint32_t           pointStep = 4U * sizeof(float);
        sensor_msgs::PointCloud2 cloud;
        cloud.header.stamp = stamp;
        cloud.header.frame_id = this->settings_.lidar.child;
        cloud.height = rings;
        cloud.width = columns;
        cloud.fields.push_back(floatField("x", 0U));
        cloud.fields.push_back(floatField("y", 4U));
        cloud.fields.push_back(floatField("z", 8U));
        cloud.fields.push_back(floatField("intensity", 12U));
        cloud.is_bigendian = false;
        cloud.point_step = pointStep;
        cloud.row_step = pointStep * columns;
        cloud.is_dense = true;
        cloud.data.reserve(static_cast<size_t>(cloud.row_step) * rings);
        const double phase = 0.1 * static_cast<double>(this->clouds_);
        for (uint32_t ring = 0; ring < rings; ++ring) {
            const double ringFraction =
                rings > 1U ? static_cast<double>(ring) / static_cast<double>(rings - 1U) : 0.5;
            const double elevation = (-15.0 + 30.0 * ringFraction) * std::numbers::pi / 180.0;
            for (uint32_t column = 0; column < columns; ++column) {
                const double azimuth = 2.0 * std::numbers::pi * static_cast<double>(column)
                                     / static_cast<double>(columns);
                const double range = 10.0 + 4.0 * std::cos(azimuth - phase);
                appendFloat(
                    cloud.data, static_cast<float>(range * std::cos(elevation) * std::cos(azimuth))
                );
                appendFloat(
                    cloud.data, static_cast<float>(range * std::cos(elevation) * std::sin(azimuth))
                );
                appendFloat(cloud.data, static_cast<float>(range * std::sin(elevation)));
                appendFloat(cloud.data, static_cast<float>(ring));
            }
        }
        this->count(this->cloud_.publish(cloud), "cloud");
        ++this->clouds_;
    }

    void count(bool sent, const char* what)
    {
        if (sent) {
            return;
        }
        ++this->unsent_;
        HYPHA_LOG_WARN_THROTTLE(1.0, "{} not sent ({} unsent so far)", what, this->unsent_);
    }

    const SourceSettings&                      settings_;
    hypha::Publisher<sensor_msgs::Imu>         imu_;
    hypha::Publisher<sensor_msgs::PointCloud2> cloud_;
    hypha::Publisher<tf2_msgs::TFMessage>      tf_;
    uint64_t                                   ticks_{0};
    uint64_t                                   clouds_{0};
    uint64_t                                   unsent_{0};
};

}  // namespace

int main(int argc, char** argv)
{
    if (argc != 2) {
        std::fprintf(stderr, "usage: hypha_sensor_source <parameter file>\n");
        return hypha::kExitUsage;
    }
    try {
        hypha::Node          node("sensor_source", argv[1]);
        const SourceSettings settings = readSettings();
        SensorSource         source(
            settings,
            node.advertise<sensor_msgs::Imu>("imu"),
            node.advertise<sensor_msgs::PointCloud2>("cloud"),
            node.advertise<tf2_msgs::TFMessage>(hypha::Topic{"/tf"})
        );
        hypha::StaticTransformBroadcaster statics{node};
        const hypha::Duration             period =
            hypha::Duration::fromNanos(std::llround(1e9 / settings.tickHz));
        hypha::Timer ticker = node.timer(period, hypha::bind<&SensorSource::tick>(source));

        // runNode refuses a key nothing read; a custom main() checks it itself.
        const std::vector<std::string> unread = hypha::config().unreadKeys();
        if (!unread.empty()) {
            HYPHA_LOG_ERROR("unread parameter(s): {}", unread.front());
            return hypha::kExitConfiguration;
        }

        // Two sends, one latched message with both transforms.
        const bool lidarSent = statics.send(staticTransform(settings.baseFrame, settings.lidar));
        const bool imuSent = statics.send(staticTransform(settings.baseFrame, settings.imu));
        const bool staticsSent = lidarSent && imuSent;
        if (!staticsSent) {
            HYPHA_LOG_ERROR("/tf_static not sent: {}", statics.lastPublishStatus().name());
            return hypha::kExitRuntime;
        }

        std::printf("sensor_source: ready\n");
        std::fflush(stdout);
        const hypha::RunResult result = node.run();
        std::printf(
            "sensor_source: ticks=%llu clouds=%llu unsent=%llu timer_missed=%llu\n",
            static_cast<unsigned long long>(source.ticks()),
            static_cast<unsigned long long>(source.clouds()),
            static_cast<unsigned long long>(source.unsent()),
            static_cast<unsigned long long>(ticker.missedCount())
        );
        std::fflush(stdout);
        const bool rewound = result.status() == hypha::Status::ClockRewound;
        if (rewound) {
            return hypha::kExitRewind;
        }
        return result.ok() ? hypha::kExitClean : hypha::kExitRuntime;
    } catch (const hypha::ConfigError& error) {
        HYPHA_LOG_ERROR("{}", error.what());
        return hypha::kExitConfiguration;
    } catch (const std::exception& error) {
        HYPHA_LOG_ERROR("{}", error.what());
        return hypha::kExitRuntime;
    }
}
