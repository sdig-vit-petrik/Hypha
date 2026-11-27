// hypha::readCameraInfo: the ROS camera calibration file (camera_calibration, ROS 1 and
// ROS 2) to sensor_msgs::CameraInfo, and the refusals that name the file, line and field.

#include "hypha/camera_info.h"
#include "hypha/config.h"
#include "hypha/hypha.h"

#include <gtest/gtest.h>
#include <sensor_msgs/CameraInfo.h>

#include <cstdint>
#include <filesystem>
#include <fstream>
#include <optional>
#include <string>

namespace {

// What `rosrun camera_calibration cameracalibrator.py` writes: PyYAML wraps the long
// flow sequences onto continuation lines.
const std::string kCalibration =
    "image_width: 640\n"
    "image_height: 480\n"
    "camera_name: narrow_stereo\n"
    "camera_matrix:\n"
    "  rows: 3\n"
    "  cols: 3\n"
    "  data: [430.215, 0.0, 306.691, 0.0, 430.53169, 227.22489, 0.0, 0.0, 1.0]\n"
    "distortion_model: plumb_bob\n"
    "distortion_coefficients:\n"
    "  rows: 1\n"
    "  cols: 5\n"
    "  data: [-0.363528858080088, 0.16190833389336, -0.00010590,\n"
    "    0.000316, 1.5e-3]\n"
    "rectification_matrix:\n"
    "  rows: 3\n"
    "  cols: 3\n"
    "  data: [1, 0, 0, 0, 1, 0, 0, 0, 1]\n"
    "projection_matrix:\n"
    "  rows: 3\n"
    "  cols: 4\n"
    "  data: [432.0, 0.0, 306.0, 0.0, 0.0, 432.0, 227.0, 0.0, 0.0, 0.0, 1.0, 0.0]\n";

/// The text of the ConfigError that reading @p text throws, or an empty string.
std::string refusal(const std::string& text)
{
    try {
        hypha::parseCameraInfo(text, "calib.yaml");
    } catch (const hypha::ConfigError& error) {
        return error.what();
    }
    return {};
}

/// kCalibration with @p from replaced by @p to.
std::string changed(const std::string& from, const std::string& to)
{
    std::string text = kCalibration;
    const size_t at = text.find(from);
    EXPECT_NE(at, std::string::npos) << from;
    text.replace(at, from.size(), to);
    return text;
}

}  // namespace

TEST(CameraInfo, ReadsTheFileThatCameraCalibrationWrites)
{
    std::string                    name;
    const sensor_msgs::CameraInfo info = hypha::parseCameraInfo(kCalibration, "calib.yaml", &name);

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
    EXPECT_DOUBLE_EQ(info.K[8], 1.0);
    EXPECT_DOUBLE_EQ(info.R[0], 1.0);
    EXPECT_DOUBLE_EQ(info.R[1], 0.0);
    EXPECT_DOUBLE_EQ(info.P[2], 306.0);
    EXPECT_DOUBLE_EQ(info.P[10], 1.0);
    EXPECT_EQ(info.binning_x, 0U);
    EXPECT_TRUE(info.header.frame_id.empty());
}

TEST(CameraInfo, AcceptsBlockListsCommentsQuotesAndAWindowsFile)
{
    std::string text = "# calibrated 2026-11-01\n---\n" + kCalibration;
    text.replace(text.find("image_width: 640"), 16, "image_width: 640   # pixels");
    text.replace(text.find("image_height: 480"), 17, "image_height: \"480\"");
    text.replace(text.find("camera_name: narrow_stereo"), 26, "camera_name: 'cam 0'");
    const std::string flat = "  data: [1, 0, 0, 0, 1, 0, 0, 0, 1]";
    text.replace(
        text.find(flat), flat.size(), "  data:\n  - 1\n  - 0\n  - 0\n  - 0\n  - 1\n  - 0\n  - 0\n  - 0\n  - 1"
    );
    std::string windows;
    for (const char c : text) {
        if (c == '\n') {
            windows += '\r';
        }
        windows += c;
    }
    std::string                    name;
    const sensor_msgs::CameraInfo info = hypha::parseCameraInfo(windows, "calib.yaml", &name);
    EXPECT_EQ(name, "cam 0");
    EXPECT_EQ(info.width, 640U);
    EXPECT_EQ(info.height, 480U);
    EXPECT_DOUBLE_EQ(info.R[4], 1.0);
    EXPECT_DOUBLE_EQ(info.R[8], 1.0);
    EXPECT_DOUBLE_EQ(info.P[10], 1.0);
}

TEST(CameraInfo, AFlowMappingIsRefusedWithItsLine)
{
    EXPECT_EQ(
        refusal(changed(
            "camera_matrix:\n  rows: 3\n  cols: 3\n  data: [430.215, 0.0, 306.691, 0.0, "
            "430.53169, 227.22489, 0.0, 0.0, 1.0]\n",
            "camera_matrix: {rows: 3, cols: 3, data: [1, 0, 0, 0, 1, 0, 0, 0, 1]}\n"
        )),
        "hypha: camera calibration 'calib.yaml': line 4: camera_matrix: a flow mapping '{...}' is "
        "not supported; write its entries on their own lines"
    );
}

TEST(CameraInfo, ReadsOptionalBinningAndRoi)
{
    const sensor_msgs::CameraInfo info = hypha::parseCameraInfo(
        kCalibration + "binning_x: 2\nbinning_y: 2\nroi:\n  x_offset: 10\n  y_offset: 20\n"
                       "  height: 100\n  width: 200\n  do_rectify: true\n",
        "calib.yaml"
    );
    EXPECT_EQ(info.binning_x, 2U);
    EXPECT_EQ(info.binning_y, 2U);
    EXPECT_EQ(info.roi.x_offset, 10U);
    EXPECT_EQ(info.roi.y_offset, 20U);
    EXPECT_EQ(info.roi.height, 100U);
    EXPECT_EQ(info.roi.width, 200U);
    EXPECT_TRUE(info.roi.do_rectify);
}

TEST(CameraInfo, ANotRequiredCameraNameIsNotRequired)
{
    std::string text = kCalibration;
    text.erase(text.find("camera_name"), std::string("camera_name: narrow_stereo\n").size());
    std::string name = "unchanged";
    EXPECT_NO_THROW(hypha::parseCameraInfo(text, "calib.yaml", &name));
    EXPECT_TRUE(name.empty());
}

TEST(CameraInfo, ARefusalNamesTheFileTheLineAndTheField)
{
    EXPECT_EQ(
        refusal(changed("  data: [430.215", "  data: [abc")),
        "hypha: camera calibration 'calib.yaml': line 7: camera_matrix.data[0]: 'abc' is not a "
        "number"
    );
    EXPECT_EQ(
        refusal(changed("0.0, 0.0, 1.0]\ndistortion_model", "0.0, 1.0]\ndistortion_model")),
        "hypha: camera calibration 'calib.yaml': line 7: camera_matrix.data: expected 9 numbers "
        "(3 x 3), found 8"
    );
    EXPECT_EQ(
        refusal(changed("  cols: 5\n", "  cols: 4\n")),
        "hypha: camera calibration 'calib.yaml': line 9: distortion_coefficients: rows 1 x cols "
        "4 holds 4 numbers, but data has 5"
    );
    EXPECT_EQ(
        refusal(changed("  cols: 4\n", "  cols: 3\n")),
        "hypha: camera calibration 'calib.yaml': line 18: projection_matrix: expected 3 x 4, "
        "found 3 x 3"
    );
}

TEST(CameraInfo, EachRequiredKeyIsNamedWhenMissing)
{
    for (const std::string key :
         {"image_width", "image_height", "camera_matrix", "distortion_model",
          "distortion_coefficients", "rectification_matrix", "projection_matrix"}) {
        std::string text = kCalibration;
        const size_t at = text.find(key + ":");
        ASSERT_NE(at, std::string::npos) << key;
        text.replace(at, key.size(), "x_" + key);
        const std::string message = refusal(text);
        EXPECT_NE(message.find("calib.yaml"), std::string::npos) << key << ": " << message;
        EXPECT_NE(message.find("'x_" + key + "'"), std::string::npos) << key << ": " << message;
    }
    EXPECT_EQ(
        refusal(changed("distortion_model: plumb_bob\n", "")),
        "hypha: camera calibration 'calib.yaml': missing key 'distortion_model'"
    );
}

TEST(CameraInfo, AnUnknownOrRepeatedKeyIsRefused)
{
    EXPECT_EQ(
        refusal(changed("image_width: 640\n", "image_width: 640\nimage_wdith: 640\n")),
        "hypha: camera calibration 'calib.yaml': line 2: unknown key 'image_wdith'"
    );
    EXPECT_EQ(
        refusal(changed("image_height: 480\n", "image_height: 480\nimage_width: 320\n")),
        "hypha: camera calibration 'calib.yaml': line 3: key 'image_width' is set twice"
    );
}

TEST(CameraInfo, ABadSizeOrNumberIsRefused)
{
    EXPECT_EQ(
        refusal(changed("image_width: 640", "image_width: 0")),
        "hypha: camera calibration 'calib.yaml': line 1: image_width: '0' is not a positive "
        "integer"
    );
    EXPECT_EQ(
        refusal(changed("image_height: 480", "image_height: 48.5")),
        "hypha: camera calibration 'calib.yaml': line 2: image_height: '48.5' is not a positive "
        "integer"
    );
    EXPECT_EQ(
        refusal(changed("0.000316", ".nan")),
        "hypha: camera calibration 'calib.yaml': line 13: distortion_coefficients.data[3]: "
        "'.nan' is not a finite number"
    );
    EXPECT_NE(
        refusal(changed("distortion_model: plumb_bob", "distortion_model: ''")).find(
            "distortion_model"
        ),
        std::string::npos
    );
}

TEST(CameraInfo, AFileOfAnotherFormatIsRefusedWithItsLine)
{
    EXPECT_EQ(
        refusal("%YAML:1.0\nimage_width: 640\n"),
        "hypha: camera calibration 'calib.yaml': line 1: expected 'key: value'"
    );
    EXPECT_EQ(
        refusal("[narrow_stereo]\n\ncamera matrix\n430 0 306\n"),
        "hypha: camera calibration 'calib.yaml': line 1: expected 'key: value'"
    );
    EXPECT_EQ(refusal(""), "hypha: camera calibration 'calib.yaml': the file is empty");
    EXPECT_EQ(
        refusal(changed("  rows: 3\n  cols: 3\n  data: [430", "\trows: 3\n  cols: 3\n  data: [430")),
        "hypha: camera calibration 'calib.yaml': line 5: a tab indents the line"
    );
    EXPECT_EQ(
        refusal(changed("0.000316, 1.5e-3]", "0.000316, 1.5e-3")),
        "hypha: camera calibration 'calib.yaml': line 12: data: the list is not closed"
    );
}

TEST(CameraInfo, ReadsAFileAndRefusesAMissingOne)
{
    const std::filesystem::path path =
        std::filesystem::temp_directory_path() / "hypha_camera_info_test.yaml";
    {
        std::ofstream output(path);
        output << kCalibration;
    }
    std::string name;
    const auto  info = hypha::readCameraInfo(path.string(), &name);
    EXPECT_EQ(info.width, 640U);
    EXPECT_EQ(name, "narrow_stereo");
    std::filesystem::remove(path);

    try {
        hypha::readCameraInfo(path.string());
        FAIL() << "a missing file must be refused";
    } catch (const hypha::ConfigError& error) {
        EXPECT_EQ(
            std::string(error.what()),
            "hypha: camera calibration '" + path.string() + "': cannot read the file"
        );
    }
}

// The pattern of a ported camera driver: the path is a parameter, relative to the
// parameter file, and a bad file stops the node with the configuration error (exit 78).
TEST(CameraInfo, ADriverReadsItsCalibrationFromACameraInfoUrlParameter)
{
    const std::filesystem::path directory =
        std::filesystem::temp_directory_path() / "hypha_camera_info_driver";
    std::filesystem::create_directories(directory);
    {
        std::ofstream(directory / "calib.yaml") << kCalibration;
        std::ofstream(directory / "node.toml") << "camera_info_url = 'calib.yaml'\n"
                                                  "bad_url = 'missing.yaml'\n";
    }
    ASSERT_TRUE(hypha::enableSimTime(hypha::Time::fromNanos(1000000000)));
    {
        hypha::Node node("camera_info_driver_test", (directory / "node.toml").string());
        const sensor_msgs::CameraInfo info =
            hypha::readCameraInfo(hypha::config().path("camera_info_url"));
        EXPECT_EQ(info.width, 640U);
        EXPECT_THROW(
            hypha::readCameraInfo(hypha::config().path("bad_url")), hypha::ConfigError
        );
    }
    std::filesystem::remove_all(directory);
}
