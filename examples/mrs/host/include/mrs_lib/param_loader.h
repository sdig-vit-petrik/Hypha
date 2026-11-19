#pragma once

/// @file
/// mrs_lib::ParamLoader over hypha::config() (hypha port of the upstream header).
/// A parameter name maps to a TOML key: the prefix and the name, with '/' as '.', so
/// "a/b/c" reads [a.b] c. As upstream, a load with a default returns false when the default
/// was used.

#include <hypha/config.h>

#include <Eigen/Dense>
#include <ros/node_handle.h>

#include <optional>
#include <string>
#include <type_traits>
#include <vector>

namespace mrs_lib {

namespace detail {
template <class T>
struct IsVector : std::false_type
{
};

template <class T>
struct IsVector<std::vector<T>> : std::true_type
{
};
}  // namespace detail

class ParamLoader
{
  public:
    ParamLoader(const ros::NodeHandle&, const std::string& node_name = "") : node_name_(node_name)
    {
    }

    ParamLoader(const ros::NodeHandle& nh, bool, const std::string& node_name)
        : ParamLoader(nh, node_name)
    {
    }

    /// The YAML layers are merged into the TOML once; the path is only recorded.
    void addYamlFile(const std::string& path)
    {
        this->yaml_files_.push_back(path);
    }

    void addYamlFileFromParam(const std::string&)
    {
    }

    void setPrefix(const std::string& prefix)
    {
        this->prefix_ = prefix;
    }

    std::string getPrefix() const
    {
        return this->prefix_;
    }

    bool loadedSuccessfully() const
    {
        return this->loaded_;
    }

    void resetLoadedSuccessfully()
    {
        this->loaded_ = true;
    }

    /// The TOML key of @p name.
    std::string key(const std::string& name) const
    {
        return mrs::parameterKey(this->prefix_ + name);
    }

    template <class T>
    bool loadParam(const std::string& name, T& out)
    {
        return this->load<T>(name, out, nullptr);
    }

    template <class T>
    bool loadParam(const std::string& name, T& out, const T& fallback)
    {
        return this->load(name, out, &fallback);
    }

    bool loadParam(const std::string& name, std::string& out, const char* fallback)
    {
        const std::string text(fallback);
        return this->load(name, out, &text);
    }

    template <class T>
    bool loadParamReusable(const std::string& name, T& out)
    {
        return this->loadParam(name, out);
    }

    template <class T>
    bool loadParamReusable(const std::string& name, T& out, const T& fallback)
    {
        return this->loadParam(name, out, fallback);
    }

    template <class T>
    T loadParam2(const std::string& name)
    {
        T out{};
        this->loadParam(name, out);
        return out;
    }

    template <class T>
    T loadParam2(const std::string& name, const T& fallback)
    {
        T out{};
        this->loadParam(name, out, fallback);
        return out;
    }

    /// A rows x cols matrix from a flat list in row-major order.
    template <int rows, int cols>
    bool loadMatrixStatic(const std::string& name, Eigen::Matrix<double, rows, cols>& mat)
    {
        Eigen::MatrixXd dynamic;
        if (!this->loadMatrixDynamic(name, dynamic, rows, cols)) {
            return false;
        }
        mat = dynamic;
        return true;
    }

    template <int rows, int cols, class Default>
    bool loadMatrixStatic(
        const std::string& name, Eigen::Matrix<double, rows, cols>& mat, const Default& fallback
    )
    {
        if (!this->present(name)) {
            mat = fallback;
            return false;  // as upstream: false when the default was used
        }
        return this->loadMatrixStatic(name, mat);
    }

    /// A matrix from a flat list in row-major order; cols = -1 takes size / rows.
    bool loadMatrixDynamic(const std::string& name, Eigen::MatrixXd& mat, int rows, int cols)
    {
        std::vector<double> values;
        if (!this->loadParam(name, values)) {
            return false;
        }
        return this->fill(name, values, mat, rows, cols);
    }

    template <class Default>
    bool loadMatrixDynamic(
        const std::string& name, Eigen::MatrixXd& mat, const Default& fallback, int rows, int cols
    )
    {
        if (!this->present(name)) {
            mat = fallback;
            return false;  // as upstream: false when the default was used
        }
        return this->loadMatrixDynamic(name, mat, rows, cols);
    }

    /// A matrix from a flat list in row-major order; rows = -1 takes size / cols. On a
    /// failure the parameter is recorded as missing and an empty matrix is returned.
    Eigen::MatrixXd loadMatrixDynamic2(const std::string& name, int rows, int cols)
    {
        Eigen::MatrixXd mat;
        this->loadMatrixDynamic(name, mat, rows, cols);
        return mat;
    }

  private:
    /// True when the list @p name is set (a matrix is a list). Throws when it is not a list.
    bool present(const std::string& name) const
    {
        return hypha::config().optionalList(this->key(name)).has_value();
    }

    void missing(const std::string& name, const std::string& why)
    {
        this->loaded_ = false;
        mrs::recordParamError(
            "[" + this->node_name_ + "]: could not load the parameter '" + name + "' (TOML key '"
            + this->key(name) + "'): " + why
        );
    }

    bool fill(
        const std::string&         name,
        const std::vector<double>& values,
        Eigen::MatrixXd&           mat,
        int                        rows,
        int                        cols
    )
    {
        const int count = static_cast<int>(values.size());
        if (cols < 0 && rows > 0 && count % rows == 0) {
            cols = count / rows;
        }
        if (rows < 0 && cols > 0 && count % cols == 0) {
            rows = count / cols;
        }
        if (rows <= 0 || cols <= 0 || rows * cols != count) {
            this->missing(
                name,
                "expected " + std::to_string(rows) + "x" + std::to_string(cols) + " values, got "
                    + std::to_string(count)
            );
            return false;
        }
        mat.resize(rows, cols);
        for (int r = 0; r < rows; ++r) {
            for (int c = 0; c < cols; ++c) {
                mat(r, c) = values[static_cast<size_t>(r * cols + c)];
            }
        }
        return true;
    }

    template <class T>
    bool load(const std::string& name, T& out, const T* fallback)
    {
        const std::string k = this->key(name);
        try {
            if constexpr (detail::IsVector<T>::value) {
                using E = typename T::value_type;
                const std::optional<hypha::ConfigList> list = hypha::config().optionalList(k);
                if (!list) {
                    if (fallback != nullptr) {
                        out = *fallback;
                        return false;  // as upstream: false when the default was used
                    }
                    this->missing(name, "not set");
                    return false;
                }
                T values;
                for (const hypha::ConfigView element : *list) {
                    values.push_back(element.template required<E>(""));
                }
                out = std::move(values);
                return true;
            } else if constexpr (std::is_same_v<T, ros::Duration>) {
                double       seconds = 0.0;
                const double fallbackSeconds = fallback != nullptr ? fallback->toSec() : 0.0;
                const bool   loaded = this->load<double>(
                    name, seconds, fallback != nullptr ? &fallbackSeconds : nullptr
                );
                out = ros::Duration(seconds);
                return loaded;
            } else {
                const std::optional<T> value = hypha::config().optional<T>(k);
                if (value.has_value()) {
                    out = *value;
                    return true;
                }
                if (fallback != nullptr) {
                    out = *fallback;
                    return false;  // as upstream: false when the default was used
                }
                this->missing(name, "not set");
                return false;
            }
        } catch (const std::exception& error) {
            this->missing(name, error.what());
            return false;
        }
    }

    std::string              node_name_;
    std::string              prefix_;
    std::vector<std::string> yaml_files_;
    bool                     loaded_{true};
};

}  // namespace mrs_lib
