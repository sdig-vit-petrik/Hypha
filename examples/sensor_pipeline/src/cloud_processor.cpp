/// hypha_cloud_processor: an event-driven sensor node, an ApplicationNode started by
/// runNode, in the shape of a ported ROS 1 LiDAR node, with one worker thread that does the
/// heavy work.
///   hypha_cloud_processor examples/sensor_pipeline/config/processor.toml

#include <hypha/hypha.h>
#include <hypha/transform_buffer.h>

#include "sensor_pipeline_qos.h"

#include <geometry_msgs/PointStamped.h>
#include <geometry_msgs/TransformStamped.h>
#include <sensor_msgs/Imu.h>
#include <sensor_msgs/PointCloud2.h>
#include <sensor_msgs/PointField.h>

#include <algorithm>
#include <array>
#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <deque>
#include <exception>
#include <expected>
#include <mutex>
#include <optional>
#include <string>
#include <thread>
#include <utility>
#include <vector>

HYPHA_LOG_MODULE_INIT("cloud_processor")

namespace {

using ImuPtr = hypha::MessagePtr<sensor_msgs::Imu>;
using CloudPtr = hypha::MessagePtr<sensor_msgs::PointCloud2>;
using CentroidPublisher = hypha::Publisher<geometry_msgs::PointStamped>;
using TransformResult = std::expected<geometry_msgs::TransformStamped, hypha::TransformError>;
using Milliseconds = std::chrono::milliseconds;

struct ProcessorSettings
{
    int64_t                    reportPeriodMs;
    uint32_t                   handoverDepth;
    std::string                targetFrame;
    int64_t                    tfWaitMs;
    std::array<std::string, 2> staticCheck;  // target, source
    bool                       slowEnabled;
    int64_t                    slowWorkerDelayMs;
    int64_t                    slowStallMs;
};

int64_t requiredMillis(const hypha::ConfigView& view, const std::string& key)
{
    const int64_t millis = view.required<int64_t>(key);
    if (millis < 0) {
        throw hypha::ConfigError("hypha: parameter '" + view.name(key) + "': must not be negative");
    }
    return millis;
}

ProcessorSettings readSettings()
{
    const hypha::ConfigView root{};
    ProcessorSettings       settings;
    settings.reportPeriodMs = requiredMillis(root, "report_period_ms");
    if (settings.reportPeriodMs == 0) {
        throw hypha::ConfigError("hypha: parameter 'report_period_ms': must be at least 1");
    }
    settings.handoverDepth = root.required<uint32_t>("handover_depth");
    if (settings.handoverDepth == 0U) {
        throw hypha::ConfigError("hypha: parameter 'handover_depth': must be at least 1");
    }
    settings.targetFrame =
        root.required<std::string>("target_frame", hypha::constraints::nonempty());
    settings.tfWaitMs = requiredMillis(root, "tf_wait_ms");
    settings.staticCheck = root.requiredArray<std::string, 2>("static_check");
    const hypha::ConfigView slow{"slow"};
    settings.slowEnabled = slow.required<bool>("enabled");
    settings.slowWorkerDelayMs = requiredMillis(slow, "worker_delay_ms");
    settings.slowStallMs = requiredMillis(slow, "stall_ms");
    return settings;
}

/// The bounded FIFO between the run() thread (push) and the worker (pop). One mutex
/// guards every member; the condition variable wakes the worker.
class CloudHandover
{
  public:
    explicit CloudHandover(size_t depth) : depth_(depth)
    {
    }

    /// run() thread. Never waits: a full queue drops its oldest cloud.
    void push(CloudPtr cloud)
    {
        {
            const std::lock_guard<std::mutex> lock(this->mutex_);
            const bool                        full = this->clouds_.size() >= this->depth_;
            if (full) {
                this->clouds_.pop_front();
                ++this->dropped_;
            }
            this->clouds_.push_back(std::move(cloud));
        }
        this->wake_.notify_one();
    }

    /// Worker. Waits for a cloud. Empty only once stop() was called and the queue is
    /// drained.
    std::optional<CloudPtr> pop()
    {
        std::unique_lock<std::mutex> lock(this->mutex_);
        while (this->clouds_.empty()) {
            if (this->stopping_) {
                return std::nullopt;
            }
            this->wake_.wait(lock);
        }
        CloudPtr cloud = std::move(this->clouds_.front());
        this->clouds_.pop_front();
        return cloud;
    }

    /// Worker. Waits @p duration of host time, or less once stop() was called.
    void pause(Milliseconds duration)
    {
        std::unique_lock<std::mutex>                lock(this->mutex_);
        const std::chrono::steady_clock::time_point until =
            std::chrono::steady_clock::now() + duration;
        while (!this->stopping_) {
            const std::cv_status woke = this->wake_.wait_until(lock, until);
            if (woke == std::cv_status::timeout) {
                return;
            }
        }
    }

    /// Any thread. The worker drains the queue and returns.
    void stop()
    {
        {
            const std::lock_guard<std::mutex> lock(this->mutex_);
            this->stopping_ = true;
        }
        this->wake_.notify_all();
    }

    bool stopping() const
    {
        const std::lock_guard<std::mutex> lock(this->mutex_);
        return this->stopping_;
    }

    uint64_t dropped() const
    {
        const std::lock_guard<std::mutex> lock(this->mutex_);
        return this->dropped_;
    }

  private:
    mutable std::mutex      mutex_;
    std::condition_variable wake_;
    std::deque<CloudPtr>    clouds_;
    size_t                  depth_;
    bool                    stopping_{false};
    uint64_t                dropped_{0};
};

/// What the worker counts. The worker writes, the Reporter reads on the run() thread.
struct WorkerCounters
{
    std::atomic<uint64_t> processed{0};
    std::atomic<uint64_t> published{0};
    std::atomic<uint64_t> publishFailed{0};
    std::atomic<uint64_t> badCloud{0};
    std::atomic<uint64_t> invalidFrame{0};
    std::atomic<uint64_t> unknownFrame{0};
    std::atomic<uint64_t> notConnected{0};
    std::atomic<uint64_t> beforeBuffer{0};
    std::atomic<uint64_t> afterBuffer{0};
    std::atomic<uint64_t> refused{0};
};

void countLookupFailure(WorkerCounters& counters, hypha::TransformError error)
{
    switch (error) {
        case hypha::TransformError::InvalidFrame:
            counters.invalidFrame.fetch_add(1U, std::memory_order_relaxed);
            return;
        case hypha::TransformError::UnknownFrame:
            counters.unknownFrame.fetch_add(1U, std::memory_order_relaxed);
            return;
        case hypha::TransformError::NotConnected:
            counters.notConnected.fetch_add(1U, std::memory_order_relaxed);
            return;
        case hypha::TransformError::BeforeBuffer:
            counters.beforeBuffer.fetch_add(1U, std::memory_order_relaxed);
            return;
        case hypha::TransformError::AfterBuffer:
            counters.afterBuffer.fetch_add(1U, std::memory_order_relaxed);
            return;
        case hypha::TransformError::Refused:
            counters.refused.fetch_add(1U, std::memory_order_relaxed);
            return;
    }
}

const char* transformErrorName(hypha::TransformError error)
{
    switch (error) {
        case hypha::TransformError::InvalidFrame:
            return "InvalidFrame";
        case hypha::TransformError::UnknownFrame:
            return "UnknownFrame";
        case hypha::TransformError::NotConnected:
            return "NotConnected";
        case hypha::TransformError::BeforeBuffer:
            return "BeforeBuffer";
        case hypha::TransformError::AfterBuffer:
            return "AfterBuffer";
        case hypha::TransformError::Refused:
            return "Refused";
    }
    return "unknown";
}

struct Vector3
{
    double x;
    double y;
    double z;
};

/// The byte offset of the FLOAT32 field @p name, or none.
std::optional<uint32_t> floatFieldOffset(const sensor_msgs::PointCloud2& cloud, const char* name)
{
    for (const sensor_msgs::PointField& field : cloud.fields) {
        const bool matches =
            field.name == name && field.datatype == sensor_msgs::PointField::FLOAT32;
        if (matches) {
            return field.offset;
        }
    }
    return std::nullopt;
}

/// The little-endian FLOAT32 at @p offset, widened to double.
double floatAt(const std::vector<uint8_t>& data, size_t offset)
{
    float value = 0.0F;
    std::memcpy(&value, data.data() + offset, sizeof(float));
    return static_cast<double>(value);
}

/// The mean of the x, y, z fields of @p cloud, or none for a cloud this node cannot read:
/// big-endian, a missing field, a field past the point, or data shorter than its size.
std::optional<Vector3> centroidOf(const sensor_msgs::PointCloud2& cloud)
{
    const std::optional<uint32_t> xOffset = floatFieldOffset(cloud, "x");
    const std::optional<uint32_t> yOffset = floatFieldOffset(cloud, "y");
    const std::optional<uint32_t> zOffset = floatFieldOffset(cloud, "z");
    const bool fieldsPresent = xOffset.has_value() && yOffset.has_value() && zOffset.has_value();
    if (!fieldsPresent) {
        return std::nullopt;
    }
    const uint64_t points = static_cast<uint64_t>(cloud.width) * cloud.height;
    const uint64_t lastField = std::max({*xOffset, *yOffset, *zOffset}) + sizeof(float);
    const uint64_t rowBytes = static_cast<uint64_t>(cloud.point_step) * cloud.width;
    const uint64_t dataBytes = static_cast<uint64_t>(cloud.row_step) * cloud.height;
    const bool     littleEndian = !cloud.is_bigendian;
    const bool     hasPoints = points > 0U;
    const bool     fieldsInPoint = lastField <= cloud.point_step;
    const bool     pointsInRow = rowBytes <= cloud.row_step;
    const bool     dataComplete = dataBytes <= cloud.data.size();
    const bool readable = littleEndian && hasPoints && fieldsInPoint && pointsInRow && dataComplete;
    if (!readable) {
        return std::nullopt;
    }
    Vector3 sum{0.0, 0.0, 0.0};
    for (uint32_t row = 0; row < cloud.height; ++row) {
        for (uint32_t column = 0; column < cloud.width; ++column) {
            const size_t point = static_cast<size_t>(row) * cloud.row_step
                               + static_cast<size_t>(column) * cloud.point_step;
            sum.x += floatAt(cloud.data, point + *xOffset);
            sum.y += floatAt(cloud.data, point + *yOffset);
            sum.z += floatAt(cloud.data, point + *zOffset);
        }
    }
    const double count = static_cast<double>(points);
    return Vector3{sum.x / count, sum.y / count, sum.z / count};
}

/// @p point mapped by @p transform: rotate by the unit quaternion, then translate.
Vector3 applyTransform(const geometry_msgs::Transform& transform, const Vector3& point)
{
    const geometry_msgs::Quaternion& q = transform.rotation;
    // t = 2 q.xyz x p; p' = p + w t + q.xyz x t
    const double tx = 2.0 * (q.y * point.z - q.z * point.y);
    const double ty = 2.0 * (q.z * point.x - q.x * point.z);
    const double tz = 2.0 * (q.x * point.y - q.y * point.x);
    Vector3      rotated{
        point.x + q.w * tx + (q.y * tz - q.z * ty),
        point.y + q.w * ty + (q.z * tx - q.x * tz),
        point.z + q.w * tz + (q.x * ty - q.y * tx),
    };
    rotated.x += transform.translation.x;
    rotated.y += transform.translation.y;
    rotated.z += transform.translation.z;
    return rotated;
}

/// The worker thread's body. A named functor: std::thread keeps it for the thread's life.
/// It owns the result publisher; nothing else touches it.
struct CloudWorker
{
    CloudHandover*                handover;
    const hypha::TransformBuffer* tf;
    WorkerCounters*               counters;
    CentroidPublisher             centroids;
    std::string                   targetFrame;
    Milliseconds                  tfWait;
    std::optional<Milliseconds>   slowDelay;

    void operator()()
    {
        for (;;) {
            std::optional<CloudPtr> cloud = this->handover->pop();
            if (!cloud.has_value()) {
                return;
            }
            if (this->slowDelay.has_value()) {
                this->handover->pause(*this->slowDelay);
            }
            this->process(**cloud);
            this->counters->processed.fetch_add(1U, std::memory_order_relaxed);
        }
    }

    void process(const sensor_msgs::PointCloud2& cloud)
    {
        const std::optional<hypha::Time> stamp = hypha::fromRosTime(cloud.header.stamp);
        const std::optional<Vector3>     centroid = centroidOf(cloud);
        const bool                       usable = stamp.has_value() && centroid.has_value();
        if (!usable) {
            this->counters->badCloud.fetch_add(1U, std::memory_order_relaxed);
            return;
        }
        const TransformResult found = this->lookupAt(cloud.header.frame_id, *stamp);
        if (!found) {
            countLookupFailure(*this->counters, found.error());
            return;
        }
        const Vector3               mapped = applyTransform(found->transform, *centroid);
        geometry_msgs::PointStamped result;
        result.header.stamp = cloud.header.stamp;  // the source stamp, never the host time
        result.header.frame_id = this->targetFrame;
        result.point.x = mapped.x;
        result.point.y = mapped.y;
        result.point.z = mapped.z;
        const bool sent = this->centroids.publish(result);
        if (!sent) {
            this->counters->publishFailed.fetch_add(1U, std::memory_order_relaxed);
            HYPHA_LOG_WARN_THROTTLE(
                1.0, "centroid not sent: {}", this->centroids.lastPublishStatus().name()
            );
            return;
        }
        this->counters->published.fetch_add(1U, std::memory_order_relaxed);
    }

    /// The run() thread fills the buffer, so the /tf message at the cloud's stamp may still
    /// be in its queue: an AfterBuffer answer is asked again every 1 ms for at most
    /// tf_wait_ms of host time, as ROS lookupTransform() with a timeout does.
    TransformResult lookupAt(const std::string& sourceFrame, hypha::Time stamp)
    {
        TransformResult found = this->tf->lookup(this->targetFrame, sourceFrame, stamp);
        const std::chrono::steady_clock::time_point deadline =
            std::chrono::steady_clock::now() + this->tfWait;
        for (;;) {
            const bool tooEarly = !found && found.error() == hypha::TransformError::AfterBuffer;
            if (!tooEarly) {
                return found;
            }
            const bool waitedEnough = std::chrono::steady_clock::now() >= deadline;
            const bool stopping = this->handover->stopping();
            const bool giveUp = waitedEnough || stopping;
            if (giveUp) {
                return found;
            }
            this->handover->pause(Milliseconds(1));
            found = this->tf->lookup(this->targetFrame, sourceFrame, stamp);
        }
    }
};

/// IMU subscription callback, on the run() thread.
class ImuCounter
{
  public:
    void onImu(const ImuPtr& /*imu*/)
    {
        ++this->received_;
    }

    uint64_t received() const
    {
        return this->received_;
    }

  private:
    uint64_t received_{0};
};

/// Cloud subscription callback, on the run() thread: hands the cloud to the worker. The
/// MessagePtr is a shared pointer to an owning copy, so the worker may keep it.
class CloudForwarder
{
  public:
    explicit CloudForwarder(CloudHandover& handover) : handover_(handover)
    {
    }

    void onCloud(const CloudPtr& cloud)
    {
        ++this->received_;
        this->handover_.push(cloud);
    }

    uint64_t received() const
    {
        return this->received_;
    }

  private:
    CloudHandover& handover_;
    uint64_t       received_{0};
};

/// The timer callback, on the run() thread: prints the diagnostics every report_period_ms,
/// and the application prints them once more at exit. In slow mode it then blocks the
/// run() thread.
class Reporter
{
  public:
    Reporter(
        const ProcessorSettings&                           settings,
        const ImuCounter&                                  imuCounter,
        const CloudForwarder&                              cloudForwarder,
        const hypha::Subscriber<sensor_msgs::Imu>&         imu,
        const hypha::Subscriber<sensor_msgs::PointCloud2>& clouds,
        const CloudHandover&                               handover,
        const WorkerCounters&                              counters
    )
        : settings_(settings)
        , imuCounter_(imuCounter)
        , cloudForwarder_(cloudForwarder)
        , imu_(imu)
        , clouds_(clouds)
        , handover_(handover)
        , counters_(counters)
    {
    }

    /// Called from setup() once the timer and the transform buffer exist.
    void watch(const hypha::Timer& timer, const hypha::TransformBuffer& tf)
    {
        this->timer_ = &timer;
        this->tf_ = &tf;
    }

    void onTick()
    {
        this->print("report");
        if (this->settings_.slowEnabled) {
            std::this_thread::sleep_for(Milliseconds(this->settings_.slowStallMs));
        }
    }

    void print(const char* kind) const
    {
        const double seconds = static_cast<double>(hypha::now().nanos()) / 1e9;
        printSubscriber(
            kind,
            seconds,
            "imu",
            this->imuCounter_.received(),
            this->imu_.missingSequences(),
            this->imu_.metrics()
        );
        printSubscriber(
            kind,
            seconds,
            "cloud",
            this->cloudForwarder_.received(),
            this->clouds_.missingSequences(),
            this->clouds_.metrics()
        );
        const uint64_t timerMissed = this->timer_ != nullptr ? this->timer_->missedCount() : 0U;
        std::printf(
            "%s t=%.3f node timer_missed=%llu handover_dropped=%llu processed=%llu published=%llu "
            "publish_failed=%llu bad_cloud=%llu\n",
            kind,
            seconds,
            static_cast<unsigned long long>(timerMissed),
            static_cast<unsigned long long>(this->handover_.dropped()),
            load(this->counters_.processed),
            load(this->counters_.published),
            load(this->counters_.publishFailed),
            load(this->counters_.badCloud)
        );
        const TransformResult statics =
            this->tf_->lookupLatest(this->settings_.staticCheck[0], this->settings_.staticCheck[1]);
        const char* staticText = statics ? "found" : transformErrorName(statics.error());
        std::printf(
            "%s t=%.3f tf static_%s_%s=%s lookup_failed invalid_frame=%llu unknown_frame=%llu "
            "not_connected=%llu before_buffer=%llu after_buffer=%llu refused=%llu\n",
            kind,
            seconds,
            this->settings_.staticCheck[0].c_str(),
            this->settings_.staticCheck[1].c_str(),
            staticText,
            load(this->counters_.invalidFrame),
            load(this->counters_.unknownFrame),
            load(this->counters_.notConnected),
            load(this->counters_.beforeBuffer),
            load(this->counters_.afterBuffer),
            load(this->counters_.refused)
        );
        std::fflush(stdout);
    }

  private:
    static unsigned long long load(const std::atomic<uint64_t>& counter)
    {
        return static_cast<unsigned long long>(counter.load(std::memory_order_relaxed));
    }

    static void printSubscriber(
        const char*                     kind,
        double                          seconds,
        const char*                     name,
        uint64_t                        received,
        uint64_t                        missing,
        const hypha::SubscriberMetrics& metrics
    )
    {
        std::printf(
            "%s t=%.3f %s received=%llu missing=%llu sequence_gap=%llu invalid_frame=%llu "
            "corrupt_payload=%llu schema_mismatch=%llu invalid_timestamp=%llu loan_budget=%llu "
            "loan_held=%llu loan_refused=%llu\n",
            kind,
            seconds,
            name,
            static_cast<unsigned long long>(received),
            static_cast<unsigned long long>(missing),
            static_cast<unsigned long long>(metrics.sequenceGap),
            static_cast<unsigned long long>(metrics.invalidFrame),
            static_cast<unsigned long long>(metrics.corruptPayload),
            static_cast<unsigned long long>(metrics.schemaMismatch),
            static_cast<unsigned long long>(metrics.invalidTimestamp),
            static_cast<unsigned long long>(metrics.loanBudget),
            static_cast<unsigned long long>(metrics.loanHeld),
            static_cast<unsigned long long>(metrics.loanRefused)
        );
    }

    const ProcessorSettings&                           settings_;
    const ImuCounter&                                  imuCounter_;
    const CloudForwarder&                              cloudForwarder_;
    const hypha::Subscriber<sensor_msgs::Imu>&         imu_;
    const hypha::Subscriber<sensor_msgs::PointCloud2>& clouds_;
    const CloudHandover&                               handover_;
    const WorkerCounters&                              counters_;
    const hypha::Timer*                                timer_{nullptr};
    const hypha::TransformBuffer*                      tf_{nullptr};
};

/// The node. The constructor reads the parameters, so a bad file ends the run with exit 78
/// before the bus is joined. setup() creates the endpoints; started() starts the worker once
/// runNode has checked that no parameter is unread. The worker is joined in the destructor,
/// before any endpoint it uses is destroyed.
class CloudProcessorNode final : public hypha::ApplicationNode
{
  public:
    CloudProcessorNode()
        : settings_(readSettings())
        , handover_(settings_.handoverDepth)
        , cloudForwarder_(handover_)
        , reporter_(settings_, imuCounter_, cloudForwarder_, imu_, clouds_, handover_, counters_)
    {
    }

    CloudProcessorNode(const CloudProcessorNode&) = delete;
    CloudProcessorNode& operator=(const CloudProcessorNode&) = delete;

    ~CloudProcessorNode() override
    {
        this->handover_.stop();
        const bool workerStarted = this->worker_.joinable();
        if (!workerStarted) {
            return;  // start-up failed: there was no run to report
        }
        this->worker_.join();
        try {
            this->reporter_.print("final");
        } catch (const std::exception& error) {
            this->failRun(error.what());
        }
    }

    void setup(hypha::Node& node) override
    {
        // Typed endpoints declare QosOf<M>: the imu and lidar_cloud policies of
        // sensor_pipeline_qos.h. A topic a tool opened first refuses them (QosMismatch).
        this->imu_ = node.subscribe<sensor_msgs::Imu>(
            "imu", hypha::bind<&ImuCounter::onImu>(this->imuCounter_)
        );
        this->clouds_ = node.subscribe<sensor_msgs::PointCloud2>(
            "cloud", hypha::bind<&CloudForwarder::onCloud>(this->cloudForwarder_)
        );
        this->tf_.emplace(node);
        this->centroids_ = node.advertise<geometry_msgs::PointStamped>("centroid");
        this->timer_ = node.timer(
            hypha::Duration::fromMillis(this->settings_.reportPeriodMs),
            hypha::bind<&Reporter::onTick>(this->reporter_)
        );
        this->reporter_.watch(this->timer_, *this->tf_);
    }

    void started(hypha::Node& /*node*/) override
    {
        std::optional<Milliseconds> slowDelay;
        if (this->settings_.slowEnabled) {
            slowDelay = Milliseconds(this->settings_.slowWorkerDelayMs);
        }
        // The publisher moves into the worker: from here on the worker owns it alone.
        this->worker_ = std::thread(
            CloudWorker{
                &this->handover_,
                &*this->tf_,
                &this->counters_,
                std::move(this->centroids_),
                this->settings_.targetFrame,
                Milliseconds(this->settings_.tfWaitMs),
                slowDelay,
            }
        );
        std::printf(
            "cloud_processor: ready (slow mode %s)\n", this->settings_.slowEnabled ? "on" : "off"
        );
        std::fflush(stdout);
    }

  private:
    const ProcessorSettings                     settings_;
    CloudHandover                               handover_;
    WorkerCounters                              counters_;
    ImuCounter                                  imuCounter_;
    CloudForwarder                              cloudForwarder_;
    hypha::Subscriber<sensor_msgs::Imu>         imu_;
    hypha::Subscriber<sensor_msgs::PointCloud2> clouds_;
    std::optional<hypha::TransformBuffer>       tf_;
    CentroidPublisher                           centroids_;
    hypha::Timer                                timer_;
    Reporter                                    reporter_;
    std::thread                                 worker_;
};

}  // namespace

int main(int argc, char** argv)
{
    return hypha::runNode<CloudProcessorNode>(argc, argv, "cloud_processor");
}
