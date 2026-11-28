#pragma once

/// @file
/// The calibration file of a camera as sensor_msgs::CameraInfo: what a ROS driver loads with
/// camera_info_manager or camera_calibration_parsers, the YAML that `camera_calibration`
/// writes (ROS 1 and ROS 2 alike). A file that does not hold what a camera needs is refused
/// with the file, the line and the field, as a bad parameter is.

#include "hypha/detail/yaml_subset.h"
#include "hypha/status.h"

#include <sensor_msgs/CameraInfo.h>

#include <charconv>
#include <cmath>
#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <fstream>
#include <initializer_list>
#include <iterator>
#include <string>
#include <string_view>
#include <system_error>
#include <vector>

namespace hypha {

namespace detail::camera {

using yaml::fail;
using yaml::Node;

inline const Node& required(const Node& map, const char* key, const std::string& where)
{
    const Node* node = map.find(key);
    if (node == nullptr) {
        fail(where, 0, "missing key '" + std::string(key) + "'");
    }
    return *node;
}

/// Refuses a key of @p map that is not in @p known: a typo is not silently dropped.
inline void onlyKeys(
    const Node&                        map,
    std::initializer_list<const char*> known,
    const std::string&                 prefix,
    const std::string&                 where
)
{
    for (size_t index = 0; index < map.keys.size(); ++index) {
        bool isKnown = false;
        for (const char* key : known) {
            isKnown = isKnown || map.keys[index] == key;
        }
        if (!isKnown) {
            fail(where, map.values[index].line, "unknown key '" + prefix + map.keys[index] + "'");
        }
    }
}

inline const std::string&
scalar(const Node& node, const std::string& name, const std::string& where)
{
    if (node.kind != Node::Kind::Scalar) {
        fail(where, node.line, name + ": expected a single value");
    }
    return node.text;
}

/// An unsigned integer, at least @p minimum, in @p name.
inline uint32_t
integer(const Node& node, const std::string& name, const std::string& where, uint32_t minimum)
{
    const std::string& text = scalar(node, name, where);
    uint32_t           value = 0;
    const char*        end = text.data() + text.size();
    const auto         result = std::from_chars(text.data(), end, value);
    const bool         ok = !text.empty() && result.ec == std::errc{} && result.ptr == end;
    if (!ok || value < minimum) {
        const std::string kind = minimum > 0 ? "a positive integer" : "a non-negative integer";
        fail(where, node.line, name + ": '" + text + "' is not " + kind);
    }
    return value;
}

inline double number(const Node& item, const std::string& name, const std::string& where)
{
    std::string text = item.text;
    const bool  signedText = text.size() > 1 && text.front() == '+';
    if (signedText) {
        text.erase(0, 1);
    }
    double      value = 0.0;
    const char* end = text.data() + text.size();
    const auto  result = std::from_chars(text.data(), end, value);
    const bool  parsed = !text.empty() && result.ec == std::errc{} && result.ptr == end;
    const bool  yamlSpecial = text == ".nan" || text == ".inf" || text == "-.inf";
    if (!parsed && !yamlSpecial) {
        fail(where, item.line, name + ": '" + item.text + "' is not a number");
    }
    if (yamlSpecial || !std::isfinite(value)) {
        fail(where, item.line, name + ": '" + item.text + "' is not a finite number");
    }
    return value;
}

/// The matrix @p key (`rows`, `cols`, `data`), row-major. @p rows and @p cols fix its shape;
/// 0 and 0 leave it to the file (the distortion coefficients), which then must agree with
/// itself.
inline std::vector<double>
matrix(const Node& root, const char* key, size_t rows, size_t cols, const std::string& where)
{
    const std::string name = key;
    const Node&       node = required(root, key, where);
    if (node.kind != Node::Kind::Mapping) {
        fail(where, node.line, name + ": expected rows, cols and data on their own lines");
    }
    onlyKeys(node, {"rows", "cols", "data"}, name + ".", where);
    for (const char* part : {"rows", "cols", "data"}) {
        if (node.find(part) == nullptr) {
            fail(where, node.line, name + ": missing key '" + part + "'");
        }
    }
    const size_t fileRows = integer(*node.find("rows"), name + ".rows", where, 0);
    const size_t fileCols = integer(*node.find("cols"), name + ".cols", where, 0);
    const Node&  data = *node.find("data");
    if (data.kind != Node::Kind::Sequence) {
        fail(where, data.line, name + ".data: expected a list of numbers");
    }
    const bool fixed = rows > 0;
    if (fixed && (fileRows != rows || fileCols != cols)) {
        fail(
            where,
            node.line,
            name + ": expected " + std::to_string(rows) + " x " + std::to_string(cols) + ", found "
                + std::to_string(fileRows) + " x " + std::to_string(fileCols)
        );
    }
    const size_t held = fileRows * fileCols;
    if (fixed && data.items.size() != held) {
        fail(
            where,
            data.line,
            name + ".data: expected " + std::to_string(held) + " numbers (" + std::to_string(rows)
                + " x " + std::to_string(cols) + "), found " + std::to_string(data.items.size())
        );
    }
    if (!fixed && data.items.size() != held) {
        fail(
            where,
            node.line,
            name + ": rows " + std::to_string(fileRows) + " x cols " + std::to_string(fileCols)
                + " holds " + std::to_string(held) + " numbers, but data has "
                + std::to_string(data.items.size())
        );
    }
    std::vector<double> values;
    for (size_t index = 0; index < data.items.size(); ++index) {
        const std::string element = name + ".data[" + std::to_string(index) + "]";
        values.push_back(number(data.items[index], element, where));
    }
    return values;
}

inline void readRoi(const Node& node, sensor_msgs::CameraInfo& info, const std::string& where)
{
    if (node.kind != Node::Kind::Mapping) {
        fail(where, node.line, "roi: expected its keys on their own lines");
    }
    onlyKeys(node, {"x_offset", "y_offset", "height", "width", "do_rectify"}, "roi.", where);
    const auto read = [&node, &where](const char* key, uint32_t& field) {
        const Node* value = node.find(key);
        if (value != nullptr) {
            field = integer(*value, std::string("roi.") + key, where, 0);
        }
    };
    read("x_offset", info.roi.x_offset);
    read("y_offset", info.roi.y_offset);
    read("height", info.roi.height);
    read("width", info.roi.width);
    const Node* rectify = node.find("do_rectify");
    if (rectify != nullptr) {
        const std::string& text = scalar(*rectify, "roi.do_rectify", where);
        const bool isBool = text == "true" || text == "false" || text == "True" || text == "False";
        if (!isBool) {
            fail(where, rectify->line, "roi.do_rectify: '" + text + "' is not true or false");
        }
        info.roi.do_rectify = text == "true" || text == "True";
    }
}

}  // namespace detail::camera

/// @brief The calibration in @p text, a ROS camera calibration YAML. `header` stays empty:
///        the driver stamps each message and names the frame.
/// @param source   what the refusals call the text, usually the file name.
/// @param cameraName set to the file's `camera_name`, or to "" when it has none.
/// @throws ConfigError naming @p source, the line and the field, for a key that is missing
///         or unknown, a value that is not what the field needs, or a matrix of the wrong
///         shape. `image_width`, `image_height`, `camera_matrix` (3 x 3), `distortion_model`,
///         `distortion_coefficients`, `rectification_matrix` (3 x 3) and `projection_matrix`
///         (3 x 4) are required.
inline sensor_msgs::CameraInfo
parseCameraInfo(std::string_view text, const std::string& source, std::string* cameraName = nullptr)
{
    using namespace detail::camera;
    const std::string where = "camera calibration '" + source + "'";
    const Node        root = detail::yaml::parse(text, where);
    onlyKeys(
        root,
        {"image_width",
         "image_height",
         "camera_name",
         "camera_matrix",
         "distortion_model",
         "distortion_coefficients",
         "rectification_matrix",
         "projection_matrix",
         "binning_x",
         "binning_y",
         "roi"},
        "",
        where
    );
    sensor_msgs::CameraInfo info;
    info.width = integer(required(root, "image_width", where), "image_width", where, 1);
    info.height = integer(required(root, "image_height", where), "image_height", where, 1);
    const Node& model = required(root, "distortion_model", where);
    info.distortion_model = scalar(model, "distortion_model", where);
    if (info.distortion_model.empty()) {
        fail(where, model.line, "distortion_model: must not be empty");
    }
    const auto copy = [](const std::vector<double>& from, auto& to) {
        std::copy(from.begin(), from.end(), to.begin());
    };
    copy(matrix(root, "camera_matrix", 3, 3, where), info.K);
    info.D = matrix(root, "distortion_coefficients", 0, 0, where);
    copy(matrix(root, "rectification_matrix", 3, 3, where), info.R);
    copy(matrix(root, "projection_matrix", 3, 4, where), info.P);
    const Node* binningX = root.find("binning_x");
    const Node* binningY = root.find("binning_y");
    if (binningX != nullptr) {
        info.binning_x = integer(*binningX, "binning_x", where, 0);
    }
    if (binningY != nullptr) {
        info.binning_y = integer(*binningY, "binning_y", where, 0);
    }
    const Node* roi = root.find("roi");
    if (roi != nullptr) {
        readRoi(*roi, info, where);
    }
    if (cameraName != nullptr) {
        const Node* name = root.find("camera_name");
        *cameraName = name == nullptr ? std::string{} : scalar(*name, "camera_name", where);
    }
    return info;
}

/// @brief parseCameraInfo() of the file @p path:
/// `readCameraInfo(config().path("camera_info_url"))`.
/// @throws ConfigError as parseCameraInfo(), and when the file cannot be read.
inline sensor_msgs::CameraInfo
readCameraInfo(const std::string& path, std::string* cameraName = nullptr)
{
    std::ifstream file(path, std::ios::binary);
    const bool    readable = std::filesystem::is_regular_file(path) && file.is_open();
    if (!readable) {
        detail::yaml::fail("camera calibration '" + path + "'", 0, "cannot read the file");
    }
    const std::string text{std::istreambuf_iterator<char>(file), std::istreambuf_iterator<char>()};
    return parseCameraInfo(text, path, cameraName);
}

}  // namespace hypha
