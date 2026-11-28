// hypha::readCameraInfo and writeCameraInfo: the camera calibration in TOML (a file of its
// own or a table of the parameter file) to sensor_msgs::CameraInfo and back, and the
// refusals that name the file or table and the key.

#include "hypha/camera_info.h"
#include "hypha/config.h"
#include "hypha/hypha.h"

#include <gtest/gtest.h>
#include <sensor_msgs/CameraInfo.h>

#include <cmath>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <limits>
#include <optional>
#include <string>

namespace {

// The calibration `camera_calibration` computes for a 640 x 480 camera, in the TOML layout.
const std::string kCalibration =
    "image_width = 640\n"
    "image_height = 480\n"
    "camera_name = \"narrow_stereo\"\n"
    "distortion_model = \"plumb_bob\"\n"
    "distortion_coefficients = [-0.363528858080088, 0.16190833389336, -0.00010590, 0.000316, "
    "1.5e-3]\n"
    "camera_matrix = [\n"
    "    430.215, 0.0, 306.691,\n"
    "    0.0, 430.53169, 227.22489,\n"
    "    0.0, 0.0, 1.0,\n"
    "]\n"
    "rectification_matrix = [1, 0, 0, 0, 1, 0, 0, 0, 1]\n"
    "projection_matrix = [432.0, 0.0, 306.0, 0.0, 0.0, 432.0, 227.0, 0.0, 0.0, 0.0, 1.0, 0.0]\n";

std::filesystem::path directory()
{
    const std::filesystem::path path =
        std::filesystem::temp_directory_path() / "hypha_camera_info_test";
    std::filesystem::create_directories(path);
    return path;
}

/// @p text written to `calib.toml` in the test directory.
std::string written(const std::string& text)
{
    const std::filesystem::path path = directory() / "calib.toml";
    std::ofstream(path, std::ios::trunc) << text;
    return path.string();
}

/// The text of the ConfigError that reading @p text throws, the file named `calib.toml`, or
/// an empty string.
std::string refusal(const std::string& text)
{
    const std::string path = written(text);
    try {
        hypha::readCameraInfo(path);
    } catch (const hypha::ConfigError& error) {
        std::string  message = error.what();
        const size_t at = message.find(path);
        if (at != std::string::npos) {
            message.replace(at, path.size(), "calib.toml");
        }
        return message;
    }
    return {};
}

/// kCalibration with @p from replaced by @p to.
std::string changed(const std::string& from, const std::string& to)
{
    std::string  text = kCalibration;
    const size_t at = text.find(from);
    EXPECT_NE(at, std::string::npos) << from;
    text.replace(at, from.size(), to);
    return text;
}

}  // namespace

TEST(CameraInfo, ReadsACalibrationFile)
{
    std::string                   name;
    const sensor_msgs::CameraInfo info = hypha::readCameraInfo(written(kCalibration), &name);

    EXPECT_EQ(name, "narrow_stereo");
    EXPECT_EQ(info.width, 640U);
    EXPECT_EQ(info.height, 480U);
    EXPECT_EQ(info.distortion_model, "plumb_bob");
    ASSERT_EQ(info.D.size(), 5U);
    EXPECT_DOUBLE_EQ(info.D[0], -0.363528858080088);
    EXPECT_DOUBLE_EQ(info.D[2], -0.0001059);
    EXPECT_DOUBLE_EQ(info.D[4], 0.0015);
    EXPECT_DOUBLE_EQ(info.K[0], 430.215);
    EXPECT_DOUBLE_EQ(info.K[4], 430.53169);
    EXPECT_DOUBLE_EQ(info.K[5], 227.22489);
    EXPECT_DOUBLE_EQ(info.K[8], 1.0);
    EXPECT_DOUBLE_EQ(info.R[0], 1.0);
    EXPECT_DOUBLE_EQ(info.R[1], 0.0);
    EXPECT_DOUBLE_EQ(info.P[2], 306.0);
    EXPECT_DOUBLE_EQ(info.P[10], 1.0);
    EXPECT_EQ(info.binning_x, 0U);
    EXPECT_FALSE(info.roi.do_rectify);
    EXPECT_TRUE(info.header.frame_id.empty());
}

TEST(CameraInfo, ReadsOptionalBinningAndRoi)
{
    const sensor_msgs::CameraInfo info = hypha::readCameraInfo(written(
        kCalibration
        + "binning_x = 2\nbinning_y = 2\n[roi]\nx_offset = 10\ny_offset = 20\n"
          "height = 100\nwidth = 200\ndo_rectify = true\n"
    ));
    EXPECT_EQ(info.binning_x, 2U);
    EXPECT_EQ(info.binning_y, 2U);
    EXPECT_EQ(info.roi.x_offset, 10U);
    EXPECT_EQ(info.roi.y_offset, 20U);
    EXPECT_EQ(info.roi.height, 100U);
    EXPECT_EQ(info.roi.width, 200U);
    EXPECT_TRUE(info.roi.do_rectify);
}

TEST(CameraInfo, ACameraNameIsNotRequired)
{
    std::string name = "unchanged";
    EXPECT_NO_THROW(
        hypha::readCameraInfo(written(changed("camera_name = \"narrow_stereo\"\n", "")), &name)
    );
    EXPECT_TRUE(name.empty());
}

TEST(CameraInfo, AMatrixOfTheWrongLengthIsRefusedWithItsShape)
{
    EXPECT_EQ(
        refusal(changed("    0.0, 0.0, 1.0,\n", "    0.0, 1.0,\n")),
        "hypha: camera calibration 'calib.toml': camera_matrix: expected 3 x 3 = 9 elements, "
        "found 8"
    );
    EXPECT_EQ(
        refusal(changed("227.0, 0.0, 0.0, 0.0, 1.0, 0.0]", "227.0, 0.0, 0.0]")),
        "hypha: camera calibration 'calib.toml': projection_matrix: expected 3 x 4 = 12 "
        "elements, found 9"
    );
    EXPECT_EQ(
        refusal(changed("    430.215,", "    \"abc\",")),
        "hypha: camera calibration 'calib.toml': parameter 'camera_matrix[0]': expected a "
        "number, found \"abc\""
    );
}

TEST(CameraInfo, TheRosLayoutOfAMatrixIsRefusedWithTheFlatForm)
{
    EXPECT_EQ(
        refusal(
            changed("rectification_matrix = [1, 0, 0, 0, 1, 0, 0, 0, 1]\n", "")
            + "[rectification_matrix]\nrows = 3\ncols = 3\ndata = [1, 0, 0, 0, 1, 0, 0, 0, 1]\n"
        ),
        "hypha: camera calibration 'calib.toml': rectification_matrix: write the numbers of its "
        "'data' as one flat array, rectification_matrix = [...], without rows and cols"
    );
}

TEST(CameraInfo, EachRequiredKeyIsNamedWhenMissing)
{
    for (const std::string key :
         {"image_width",
          "image_height",
          "camera_matrix",
          "distortion_model",
          "distortion_coefficients",
          "rectification_matrix",
          "projection_matrix"}) {
        std::string  text = kCalibration;
        const size_t at = text.find(key + " =");
        ASSERT_NE(at, std::string::npos) << key;
        text.replace(at, key.size(), "x_" + key);
        const std::string message = refusal(text);
        EXPECT_NE(message.find("'calib.toml'"), std::string::npos) << key << ": " << message;
        EXPECT_NE(message.find(key), std::string::npos) << key << ": " << message;
    }
    EXPECT_EQ(
        refusal(changed("camera_matrix", "x_camera_matrix")),
        "hypha: camera calibration 'calib.toml': missing key 'camera_matrix'"
    );
    EXPECT_EQ(
        refusal(changed("distortion_model = \"plumb_bob\"\n", "")),
        "hypha: camera calibration 'calib.toml': required parameter not set: distortion_model"
    );
}

TEST(CameraInfo, AnUnknownKeyIsRefused)
{
    EXPECT_EQ(
        refusal(changed("image_width = 640\n", "image_width = 640\nimage_wdith = 640\n")),
        "hypha: camera calibration 'calib.toml': unknown key 'image_wdith'"
    );
    EXPECT_EQ(
        refusal(kCalibration + "[roi]\nx_ofset = 3\n"),
        "hypha: camera calibration 'calib.toml': unknown key 'roi.x_ofset'"
    );
}

TEST(CameraInfo, ABadSizeOrNumberIsRefused)
{
    EXPECT_EQ(
        refusal(changed("image_width = 640", "image_width = 0")),
        "hypha: camera calibration 'calib.toml': image_width: must be at least 1, found 0"
    );
    EXPECT_NE(
        refusal(changed("image_height = 480", "image_height = 48.5")).find("'image_height'"),
        std::string::npos
    );
    EXPECT_EQ(
        refusal(changed("0.000316", "nan")),
        "hypha: camera calibration 'calib.toml': distortion_coefficients[3]: nan is not a "
        "finite number"
    );
    EXPECT_EQ(
        refusal(changed("432.0, 0.0, 306.0", "432.0, 0.0, -inf")),
        "hypha: camera calibration 'calib.toml': projection_matrix[2]: -inf is not a finite "
        "number"
    );
    EXPECT_NE(
        refusal(changed("\"plumb_bob\"", "\"\"")).find("'distortion_model'"), std::string::npos
    );
}

TEST(CameraInfo, AFileThatIsNotTomlOrIsMissingIsRefused)
{
    EXPECT_EQ(
        refusal("image_width: 640\nimage_height: 480\n"),
        "hypha: camera calibration 'calib.toml': not a TOML file the parameter loader accepts "
        "(its reason is logged above)"
    );
    const std::string missing = (directory() / "missing.toml").string();
    try {
        hypha::readCameraInfo(missing);
        FAIL() << "a missing file must be refused";
    } catch (const hypha::ConfigError& error) {
        EXPECT_EQ(
            std::string(error.what()),
            "hypha: camera calibration '" + missing + "': cannot read the file"
        );
    }
}

TEST(CameraInfo, AnAbsentFileIsAnUncalibratedCameraAndABadOneStillThrows)
{
    EXPECT_FALSE(hypha::readCameraInfoIfPresent((directory() / "absent.toml").string()));
    std::string                                  name;
    const std::optional<sensor_msgs::CameraInfo> info =
        hypha::readCameraInfoIfPresent(written(kCalibration), &name);
    ASSERT_TRUE(info);
    EXPECT_EQ(info->width, 640U);
    EXPECT_EQ(name, "narrow_stereo");
    EXPECT_THROW(hypha::readCameraInfoIfPresent(written(changed("640", "0"))), hypha::ConfigError);
}

TEST(CameraInfo, WhatWriteCameraInfoWritesReadsBackTheSame)
{
    sensor_msgs::CameraInfo info = hypha::readCameraInfo(written(kCalibration));
    info.D = {-0.25, 0.125, 1e-05, -0.0, 3.0, 0.1, 0.2, 0.30000000000000004};
    info.distortion_model = "rational_polynomial";
    info.binning_x = 2;
    info.binning_y = 4;
    info.roi.x_offset = 8;
    info.roi.y_offset = 16;
    info.roi.width = 320;
    info.roi.height = 240;
    info.roi.do_rectify = true;
    const std::string name = "left \"wide\" \\ cam\t1";
    const std::string path = (directory() / "nested" / "left.toml").string();
    std::filesystem::remove_all(directory() / "nested");

    hypha::writeCameraInfo(path, info, name);
    std::string readName;
    EXPECT_EQ(hypha::readCameraInfo(path, &readName), info);
    EXPECT_EQ(readName, name);

    // set_camera_info rewrites the file the node read at start.
    info.K[0] = 512.5;
    hypha::writeCameraInfo(path, info, name);
    EXPECT_DOUBLE_EQ(hypha::readCameraInfo(path).K[0], 512.5);
    EXPECT_FALSE(std::filesystem::exists(path + ".tmp"));
    std::filesystem::remove_all(directory() / "nested");
}

TEST(CameraInfo, WriteCameraInfoRefusesWhatTheReaderWouldRefuse)
{
    sensor_msgs::CameraInfo info = hypha::readCameraInfo(written(kCalibration));
    const std::string       path = (directory() / "refused.toml").string();
    info.K[4] = std::numeric_limits<double>::quiet_NaN();
    try {
        hypha::writeCameraInfo(path, info, "cam");
        FAIL() << "a NaN must be refused";
    } catch (const hypha::ConfigError& error) {
        EXPECT_EQ(
            std::string(error.what()),
            "hypha: camera calibration '" + path + "' not written: K[4] is not a finite number"
        );
    }
    info.K[4] = 1.0;
    info.width = 0;
    EXPECT_THROW(hypha::writeCameraInfo(path, info, "cam"), hypha::ConfigError);
    EXPECT_FALSE(std::filesystem::exists(path));
}

// A calibration in the node's own parameter file: the same keys in a table.
TEST(CameraInfo, ReadsTheCameraInfoTableOfTheParameterFile)
{
    const std::filesystem::path path = directory() / "table.toml";
    std::ofstream(path, std::ios::trunc) << "rate_hz = 30.0\n[camera_info]\n" << kCalibration;
    {
        hypha::ConfigContext    context(path.string());
        std::string             name;
        sensor_msgs::CameraInfo info = hypha::readCameraInfo(hypha::config("camera_info"), &name);
        EXPECT_EQ(info.width, 640U);
        EXPECT_DOUBLE_EQ(info.K[2], 306.691);
        EXPECT_EQ(name, "narrow_stereo");
        EXPECT_EQ(hypha::config().unreadKeys(), (std::vector<std::string>{"rate_hz"}));
    }
    std::ofstream(path, std::ios::trunc)
        << "[camera_info]\n"
        << changed("image_width = 640\n", "image_width = 640\nimage_wdith = 640\n");
    {
        hypha::ConfigContext context(path.string());
        try {
            hypha::readCameraInfo(hypha::config("camera_info"));
            FAIL() << "an unknown key must be refused";
        } catch (const hypha::ConfigError& error) {
            EXPECT_EQ(
                std::string(error.what()),
                "hypha: camera calibration [camera_info]: unknown key 'camera_info.image_wdith'"
            );
        }
    }
    std::filesystem::remove(path);
}

// The pattern of a ported camera driver: the path is a parameter, relative to the
// parameter file and optional (uncalibrated without it), and a bad file stops the node with
// the configuration error (exit 78).
TEST(CameraInfo, ADriverReadsItsCalibrationFromACameraInfoUrlParameter)
{
    const std::filesystem::path root = directory() / "driver";
    std::filesystem::create_directories(root);
    {
        std::ofstream(root / "calib.toml") << kCalibration;
        std::ofstream(root / "node.toml") << "camera_info_url = 'calib.toml'\n"
                                             "bad_url = 'missing.toml'\n";
    }
    ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
    {
        hypha::Node node("camera_info_driver_test", (root / "node.toml").string());
        const std::optional<std::string> url = hypha::config().optionalPath("camera_info_url");
        ASSERT_TRUE(url);
        EXPECT_EQ(*url, (root / "calib.toml").string());
        EXPECT_EQ(hypha::readCameraInfo(*url).width, 640U);
        EXPECT_FALSE(hypha::config().optionalPath("no_such_url"));
        EXPECT_THROW(hypha::readCameraInfo(hypha::config().path("bad_url")), hypha::ConfigError);
    }
    std::filesystem::remove_all(root);
}
