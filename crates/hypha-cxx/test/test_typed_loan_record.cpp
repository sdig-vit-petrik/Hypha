// `hypha record` stores what a typed loan sends: clouds shrunk at the publish are recorded
// with the bytes publish(const M&) would send for the same messages, length included.

#include <hypha/hypha.h>
#include <hypha/testing/mcap_file.h>

#include <gtest/gtest.h>
#include <poll.h>
#include <sensor_msgs/PointCloud2.h>
#include <signal.h>
#include <spawn.h>
#include <sys/wait.h>
#include <unistd.h>

#include <chrono>
#include <cstdio>
#include <cstring>
#include <filesystem>
#include <optional>
#include <string>
#include <vector>

extern char** environ;

namespace {

sensor_msgs::PointCloud2 cloudHead()
{
    sensor_msgs::PointCloud2 head;
    head.header.frame_id = "lidar";
    head.height = 1;
    head.fields = {
        {.name = "x", .offset = 0, .datatype = sensor_msgs::PointField::FLOAT32, .count = 1}
    };
    head.point_step = 4;
    head.is_dense = true;
    return head;
}

/// One `hypha record` child process with its standard output read through a pipe.
class Recorder
{
  public:
    Recorder(const std::string& topic, const std::string& output)
    {
        int pipeEnds[2] = {-1, -1};
        EXPECT_EQ(pipe(pipeEnds), 0);
        posix_spawn_file_actions_t actions;
        posix_spawn_file_actions_init(&actions);
        posix_spawn_file_actions_adddup2(&actions, pipeEnds[1], STDOUT_FILENO);
        posix_spawn_file_actions_addclose(&actions, pipeEnds[0]);
        std::vector<std::string> args{
            HYPHA_TEST_HYPHA_TOOL,
            "record",
            "--topics",
            topic,
            "--compression",
            "none",
            "-o",
            output
        };
        std::vector<char*> argv;
        for (std::string& arg : args) {
            argv.push_back(arg.data());
        }
        argv.push_back(nullptr);
        EXPECT_EQ(posix_spawn(&this->pid_, argv[0], &actions, nullptr, argv.data(), environ), 0);
        posix_spawn_file_actions_destroy(&actions);
        close(pipeEnds[1]);
        this->out_ = pipeEnds[0];
    }

    Recorder(const Recorder&) = delete;
    Recorder& operator=(const Recorder&) = delete;

    ~Recorder()
    {
        if (this->pid_ > 0) {
            kill(this->pid_, SIGKILL);
            waitpid(this->pid_, nullptr, 0);
        }
        close(this->out_);
    }

    /// @brief True once the recorder said it follows a topic, within @p seconds.
    bool waitUntilFollowing(hypha::Node& node, double seconds)
    {
        const auto deadline =
            std::chrono::steady_clock::now() + std::chrono::duration<double>(seconds);
        std::string text;
        while (std::chrono::steady_clock::now() < deadline) {
            (void)node.runFor(hypha::Duration::fromMillis(10));
            pollfd ready{this->out_, POLLIN, 0};
            if (poll(&ready, 1, 0) <= 0) {
                continue;
            }
            char       chunk[512];
            const auto got = read(this->out_, chunk, sizeof chunk);
            if (got <= 0) {
                return false;
            }
            text.append(chunk, static_cast<size_t>(got));
            if (text.find("Following") != std::string::npos) {
                return true;
            }
        }
        return false;
    }

    /// @brief Stop it with SIGINT, serving @p node meanwhile; its exit status.
    std::optional<int> stop(hypha::Node& node)
    {
        kill(this->pid_, SIGINT);
        for (int round = 0; round < 1000; ++round) {
            int        status = 0;
            const auto done = waitpid(this->pid_, &status, WNOHANG);
            if (done == this->pid_) {
                this->pid_ = -1;
                return WIFEXITED(status) ? std::optional<int>(WEXITSTATUS(status)) : std::nullopt;
            }
            (void)node.runFor(hypha::Duration::fromMillis(10));
        }
        return std::nullopt;
    }

  private:
    pid_t pid_{-1};
    int   out_{-1};
};

}  // namespace

TEST(TypedLoanRecord, ARecorderStoresTheBytesOfCloudsShrunkAtThePublish)
{
    const std::string           topic = "/typed_loan_record/" + std::to_string(getpid()) + "/cloud";
    const std::filesystem::path output =
        std::filesystem::temp_directory_path()
        / ("typed_loan_record_" + std::to_string(getpid()) + ".mcap");
    std::filesystem::remove(output);
    hypha::Node node("typed_loan_record");
    auto        pub = node.advertise<sensor_msgs::PointCloud2>(hypha::Topic{topic});
    Recorder    recorder(topic, output.string());
    ASSERT_TRUE(recorder.waitUntilFollowing(node, 30.0)) << "the recorder never followed " << topic;

    std::vector<std::vector<uint8_t>> expected;
    for (uint32_t frame = 0; frame < 3; ++frame) {
        // Room for 16 points; this revolution ends after 5 + 4 * frame of them.
        const uint32_t points = 5 + 4 * frame;
        auto           loan = pub.loan(cloudHead(), 16 * sizeof(float));
        ASSERT_TRUE(loan.has_value()) << loan.error().name();
        sensor_msgs::PointCloud2 whole = cloudHead();
        for (uint32_t i = 0; i < points; ++i) {
            const float x = static_cast<float>(frame * 100 + i);
            loan->put(i, x);
            whole.data.resize(whole.data.size() + sizeof x);
            std::memcpy(whole.data.data() + i * sizeof x, &x, sizeof x);
        }
        loan->message().width = points;
        loan->message().row_step = points * 4;
        whole.width = points;
        whole.row_step = points * 4;
        whole.header.seq = frame;
        expected.push_back(hypha::codec::encode(whole));
        ASSERT_TRUE(pub.publish(std::move(*loan), points * sizeof(float)));
        (void)node.runFor(hypha::Duration::fromMillis(50));
    }
    (void)node.runFor(hypha::Duration::fromMillis(300));
    ASSERT_EQ(recorder.stop(node), std::optional<int>(0));

    hypha::detail::McapFile file(output.string());
    file.select(topic);
    std::vector<std::vector<uint8_t>> recorded;
    HyphaMcapMessage                  message{};
    while (file.next(message)) {
        recorded.emplace_back(message.payload.data, message.payload.data + message.payload.len);
    }
    EXPECT_EQ(recorded, expected);
    std::filesystem::remove(output);
}
