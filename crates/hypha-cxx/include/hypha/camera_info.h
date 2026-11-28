#pragma once

/// @file
/// The calibration of a camera as sensor_msgs::CameraInfo, in TOML: a `[camera_info]` table
/// of the node's own parameter file, or a file of its own that `set_camera_info` rewrites.
/// The keys are those of the ROS calibration file (`camera_calibration`,
/// `camera_info_manager`); a matrix is a flat row-major array whose length is checked
/// against its shape. Hypha's parameter-file loader reads it (no parser here), and a
/// calibration that does not hold what a camera needs is refused naming the key, as a bad
/// parameter is:
///
/// ```toml
/// image_width = 640
/// image_height = 480
/// camera_name = "narrow_stereo"
/// distortion_model = "plumb_bob"
/// distortion_coefficients = [-0.3635, 0.1619, -0.0001, 0.0003, 0.0]
/// camera_matrix = [430.2, 0.0, 306.7,  0.0, 430.5, 227.2,  0.0, 0.0, 1.0]         # 3 x 3
/// rectification_matrix = [1.0, 0.0, 0.0,  0.0, 1.0, 0.0,  0.0, 0.0, 1.0]          # 3 x 3
/// projection_matrix = [432.0, 0.0, 306.0, 0.0,  0.0, 432.0, 227.0, 0.0,  0.0, 0.0, 1.0, 0.0]
/// binning_x = 0          # optional, 0 when absent
/// binning_y = 0          # optional, 0 when absent
/// [roi]                  # optional, every key optional, 0 and false when absent
/// x_offset = 0
/// y_offset = 0
/// height = 0
/// width = 0
/// do_rectify = false
/// ```

#include "hypha/config.h"
#include "hypha/status.h"

#include <sensor_msgs/CameraInfo.h>

#include <algorithm>
#include <cmath>
#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <format>
#include <fstream>
#include <optional>
#include <stdexcept>
#include <string>
#include <system_error>
#include <vector>

namespace hypha {

namespace detail::camera {

/// The reads of one calibration, each refusal prefixed with what is read (the file, or the
/// table of the parameter file).
class Reader
{
  public:
    Reader(const ConfigView& view, std::string where) : view_(view), where_(std::move(where))
    {
    }

    [[noreturn]] void fail(const std::string& reason) const
    {
        throw ConfigError("hypha: camera calibration " + this->where_ + ": " + reason);
    }

    /// Runs @p read and turns its ParameterError into a refusal of this calibration.
    template <typename Read>
    auto guarded(Read read) const
    {
        try {
            return read();
        } catch (const ParameterError& error) {
            std::string       reason = error.what();
            const std::string prefix = "hypha: ";
            if (reason.starts_with(prefix)) {
                reason.erase(0, prefix.size());
            }
            this->fail(reason);
        }
    }

    uint32_t size(const char* key) const
    {
        const auto value = this->guarded([this, key] {
            return this->view_.required<uint32_t>(key);
        });
        if (value == 0) {
            this->fail(this->view_.name(key) + ": must be at least 1, found 0");
        }
        return value;
    }

    uint32_t count(const char* key) const
    {
        return this->guarded([this, key] {
            return this->view_.optional<uint32_t>(key, 0U);
        });
    }

    /// The numbers of @p key; @p cols 0 leaves the length to the file (one row).
    std::vector<double> matrix(const char* key, size_t rows, size_t cols) const
    {
        const std::string                        name = this->view_.name(key);
        const std::optional<std::vector<double>> found = this->guarded([this, key] {
            return this->view_.optionalArray<double>(key);
        });
        if (!found) {
            const bool rosLayout = this->guarded([this, key] {
                return this->view_.optionalArray<double>(std::string(key) + ".data").has_value();
            });
            this->fail(
                rosLayout ? name + ": write the numbers of its 'data' as one flat array, " + key
                                + " = [...], without rows and cols"
                          : "missing key '" + name + "'"
            );
        }
        const std::vector<double>& values = *found;
        const bool                 wrongShape = cols > 0 && values.size() != rows * cols;
        if (wrongShape) {
            this->fail(
                std::format(
                    "{}: expected {} x {} = {} elements, found {}",
                    name,
                    rows,
                    cols,
                    rows * cols,
                    values.size()
                )
            );
        }
        for (size_t index = 0; index < values.size(); ++index) {
            if (!std::isfinite(values[index])) {
                this->fail(
                    std::format("{}[{}]: {} is not a finite number", name, index, values[index])
                );
            }
        }
        return values;
    }

    const ConfigView& view() const
    {
        return this->view_;
    }

  private:
    const ConfigView& view_;
    std::string       where_;
};

inline sensor_msgs::CameraInfo
read(const ConfigView& view, const std::string& where, std::string* cameraName)
{
    const Reader            reader(view, where);
    sensor_msgs::CameraInfo info;
    info.width = reader.size("image_width");
    info.height = reader.size("image_height");
    info.distortion_model = reader.guarded([&view] {
        return view.required<std::string>("distortion_model", constraints::nonempty());
    });
    const auto copy = [](const std::vector<double>& from, auto& to) {
        std::copy(from.begin(), from.end(), to.begin());
    };
    info.D = reader.matrix("distortion_coefficients", 1, 0);
    copy(reader.matrix("camera_matrix", 3, 3), info.K);
    copy(reader.matrix("rectification_matrix", 3, 3), info.R);
    copy(reader.matrix("projection_matrix", 3, 4), info.P);
    info.binning_x = reader.count("binning_x");
    info.binning_y = reader.count("binning_y");
    info.roi.x_offset = reader.count("roi.x_offset");
    info.roi.y_offset = reader.count("roi.y_offset");
    info.roi.height = reader.count("roi.height");
    info.roi.width = reader.count("roi.width");
    info.roi.do_rectify = reader.guarded([&view] {
        return view.optional<bool>("roi.do_rectify", false);
    });
    const std::string name = reader.guarded([&view] {
        return view.optional<std::string>("camera_name", std::string{});
    });
    // Every key the calibration knows is read: what is left is a typo or a key of another
    // format, refused rather than dropped.
    const std::vector<std::string> unknown = view.unreadKeys();
    if (!unknown.empty()) {
        reader.fail("unknown key '" + unknown.front() + "'");
    }
    if (cameraName != nullptr) {
        *cameraName = name;
    }
    return info;
}

/// @p text as a TOML basic string.
inline std::string quoted(const std::string& text)
{
    std::string result = "\"";
    for (const char c : text) {
        const auto code = static_cast<unsigned char>(c);
        if (c == '"' || c == '\\') {
            result += '\\';
            result += c;
        } else if (code < 0x20 || code == 0x7F) {
            result += std::format("\\u{:04X}", code);
        } else {
            result += c;
        }
    }
    return result + "\"";
}

/// @p value as a TOML float, exact: `1.0`, `0.0015`, `1e-05`.
inline std::string number(double value)
{
    std::string text = std::format("{}", value);
    if (text.find_first_of(".e") == std::string::npos) {
        text += ".0";
    }
    return text;
}

/// The array @p key of @p values, one row of @p cols numbers on each line.
template <typename Values>
std::string array(const char* key, const Values& values, size_t cols, const char* shape)
{
    std::string text = std::string(key) + " = [";
    if (cols == 0) {
        for (size_t index = 0; index < values.size(); ++index) {
            text += (index == 0 ? "" : ", ") + number(values[index]);
        }
        return text + "]\n";
    }
    text = std::string("# ") + shape + ", row-major\n" + text + "\n";
    for (size_t index = 0; index < values.size(); ++index) {
        text += (index % cols == 0 ? "    " : " ") + number(values[index]) + ",";
        text += index % cols == cols - 1 ? "\n" : "";
    }
    return text + "]\n";
}

/// Refuses what readCameraInfo() would refuse in @p info, naming the field.
inline void checkWritable(const sensor_msgs::CameraInfo& info, const std::string& path)
{
    const auto fail = [&path](const std::string& reason) {
        throw ConfigError("hypha: camera calibration '" + path + "' not written: " + reason);
    };
    if (info.width == 0 || info.height == 0) {
        fail("width and height must be at least 1");
    }
    if (info.distortion_model.empty()) {
        fail("distortion_model must not be empty");
    }
    const auto finite = [&fail](const char* field, const auto& values) {
        for (size_t index = 0; index < values.size(); ++index) {
            if (!std::isfinite(values[index])) {
                fail(std::format("{}[{}] is not a finite number", field, index));
            }
        }
    };
    finite("D", info.D);
    finite("K", info.K);
    finite("R", info.R);
    finite("P", info.P);
}

}  // namespace detail::camera

/// @brief The calibration in the table @p table of the node's parameter file:
///        `readCameraInfo(hypha::config("camera_info"))`. `header` stays empty: the driver
///        stamps each message and names the frame.
/// @param cameraName set to `camera_name`, or to "" when the table has none.
/// @throws ConfigError naming the key, for a key that is missing or unknown, a value that
///         is not what the field needs (a size of 0, a number that is not finite, empty
///         text), or a matrix of the wrong length. `image_width`, `image_height`,
///         `distortion_model`, `distortion_coefficients`, `camera_matrix` (3 x 3),
///         `rectification_matrix` (3 x 3) and `projection_matrix` (3 x 4) are required.
inline sensor_msgs::CameraInfo
readCameraInfo(const ConfigView& table, std::string* cameraName = nullptr)
{
    return detail::camera::read(table, "[" + table.name() + "]", cameraName);
}

/// @brief The calibration file @p path, the same keys at the top level:
///        `readCameraInfo(hypha::config().path("camera_info_url"))`.
/// @throws ConfigError as the table form, naming @p path, and when the file is missing or
///         not TOML.
inline sensor_msgs::CameraInfo
readCameraInfo(const std::string& path, std::string* cameraName = nullptr)
{
    const bool present = std::filesystem::is_regular_file(path);
    if (!present) {
        throw ConfigError("hypha: camera calibration '" + path + "': cannot read the file");
    }
    std::optional<ConfigFile> file;
    try {
        file.emplace(path);
    } catch (const ConfigError&) {
        throw ConfigError(
            "hypha: camera calibration '" + path
            + "': not a TOML file the parameter loader accepts (its reason is logged above)"
        );
    }
    return detail::camera::read(file->view(), "'" + path + "'", cameraName);
}

/// @brief readCameraInfo(@p path), or no value when nothing is at @p path: the camera runs
///        uncalibrated, as with camera_info_manager (the driver says so in its log). A file
///        that is there and malformed still throws.
inline std::optional<sensor_msgs::CameraInfo>
readCameraInfoIfPresent(const std::string& path, std::string* cameraName = nullptr)
{
    std::error_code error;
    const bool      exists = std::filesystem::exists(path, error);
    if (!exists && !error) {
        return std::nullopt;
    }
    return readCameraInfo(path, cameraName);
}

/// @brief Writes @p info and @p cameraName to @p path as the calibration file that
///        readCameraInfo() reads, as `set_camera_info` does: into `<path>.tmp`, then renamed
///        over @p path, so a reader never sees half a file. Missing directories are created.
///        `header` is not written.
/// @throws ConfigError for a calibration readCameraInfo() would refuse (a size of 0, an
///         empty distortion model, a number that is not finite), naming the field;
///         std::runtime_error when the file cannot be written.
inline void writeCameraInfo(
    const std::string& path, const sensor_msgs::CameraInfo& info, const std::string& cameraName
)
{
    using namespace detail::camera;
    checkWritable(info, path);
    std::string text = "# Camera calibration (hypha::writeCameraInfo)\n";
    text += std::format("image_width = {}\nimage_height = {}\n", info.width, info.height);
    text += "camera_name = " + quoted(cameraName) + "\n";
    text += "distortion_model = " + quoted(info.distortion_model) + "\n";
    text += array("distortion_coefficients", info.D, 0, "");
    text += array("camera_matrix", info.K, 3, "3 x 3");
    text += array("rectification_matrix", info.R, 3, "3 x 3");
    text += array("projection_matrix", info.P, 4, "3 x 4");
    text += std::format("binning_x = {}\nbinning_y = {}\n", info.binning_x, info.binning_y);
    text += std::format(
        "\n[roi]\nx_offset = {}\ny_offset = {}\nheight = {}\nwidth = {}\ndo_rectify = {}\n",
        info.roi.x_offset,
        info.roi.y_offset,
        info.roi.height,
        info.roi.width,
        info.roi.do_rectify ? "true" : "false"
    );
    const std::filesystem::path target(path);
    const std::filesystem::path temporary(path + ".tmp");
    const auto                  fail = [&path](const std::string& reason) {
        throw std::runtime_error("hypha: camera calibration '" + path + "': " + reason);
    };
    std::error_code error;
    if (target.has_parent_path()) {
        std::filesystem::create_directories(target.parent_path(), error);
        if (error) {
            fail("cannot create its directory: " + error.message());
        }
    }
    {
        std::ofstream file(temporary, std::ios::binary | std::ios::trunc);
        file << text;
        file.close();
        if (!file) {
            fail("cannot write " + temporary.string());
        }
    }
    std::filesystem::rename(temporary, target, error);
    if (error) {
        std::filesystem::remove(temporary, error);
        fail("cannot replace the file: " + error.message());
    }
}

}  // namespace hypha
