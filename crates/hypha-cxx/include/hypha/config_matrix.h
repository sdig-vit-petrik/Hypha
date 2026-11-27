#pragma once

/// @file
/// `Matrix<T>` and `ConfigView::requiredMatrix`: the flat, row-major list that a ROS matrix
/// parameter is (`mixer: [1, 0, 0, 1, ...]`), read with its shape checked. config.h includes
/// this header.

#include "hypha/config.h"

#include <algorithm>
#include <cstddef>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

namespace hypha {

/// @brief A matrix read from the parameter file: `rows` x `cols`, row after row in `values`.
///        With Eigen: `Eigen::Map<const Eigen::Matrix<double, -1, -1, Eigen::RowMajor>>(
///        m.values.data(), m.rows, m.cols)`.
template <typename T>
struct Matrix
{
    size_t         rows{0};
    size_t         cols{0};
    std::vector<T> values;

    /// @throws std::out_of_range for a row or column outside the matrix.
    const T& at(size_t row, size_t col) const
    {
        if (row >= this->rows || col >= this->cols) {
            throw std::out_of_range(
                "hypha: matrix element (" + std::to_string(row) + ", " + std::to_string(col)
                + ") is outside " + std::to_string(this->rows) + " x " + std::to_string(this->cols)
            );
        }
        return this->values[row * this->cols + col];
    }
};

template <typename T>
    requires detail::ConfigValueType<T>
Matrix<T> ConfigView::requiredMatrix(const std::string& key, size_t rows, size_t cols) const
{
    if (rows == 0 && cols == 0) {
        throw std::invalid_argument(
            "hypha: requiredMatrix('" + this->name(key)
            + "') needs rows or cols; 0 for one takes "
              "it from the length"
        );
    }
    Matrix<T> matrix;
    matrix.values = this->requiredArray<T>(key);
    const size_t length = matrix.values.size();
    const bool   fits =
        rows != 0 && cols != 0 ? length == rows * cols : length % std::max(rows, cols) == 0;
    if (!fits) {
        const std::string reason =
            rows != 0 && cols != 0
                ? "expected " + std::to_string(rows) + " x " + std::to_string(cols) + " = "
                      + std::to_string(rows * cols) + " elements, found " + std::to_string(length)
                : std::to_string(length) + " elements do not make "
                      + (rows != 0 ? "columns of " + std::to_string(rows) + " rows"
                                   : "rows of " + std::to_string(cols) + " columns");
        throw ParameterError("hypha: parameter '" + this->name(key) + "': " + reason);
    }
    matrix.rows = rows != 0 ? rows : length / cols;
    matrix.cols = cols != 0 ? cols : length / rows;
    return matrix;
}

}  // namespace hypha
